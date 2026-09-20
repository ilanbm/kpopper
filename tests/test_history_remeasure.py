"""History-backed remeasure stays prospective and uses admitted history mutations."""
import copy
import datetime
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts import history_contract as C
from scripts import history_hypotheses as HH
from scripts import history_store as H
from scripts import history_transaction as T
from scripts import remeasure
from tests.test_history_snapshot_capture import TEMPLATE, claim


ROOT = Path(__file__).resolve().parents[1]
PYTHON = sys.executable


class HistoryRemeasure(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.entry = self.root / "GROUNDING.yaml"
        self.store = H.Store(self.entry)
        self.marker = C.authority(record_id="remeasure-fixture", authority="history", generation=1)
        marker = Path(self.store.layout["history_authority"])
        marker.parent.mkdir(parents=True)
        marker.write_bytes(C.encode_document(self.marker))
        template = copy.deepcopy(TEMPLATE)
        template["meta"]["history"] = H.baseline(self.marker, {}, H.reduce({}))
        self.entry.write_bytes(C.encode_document(template))

    def publish(self, objects, operation):
        captured = self.store.capture()
        pairs = [(obj, C.encode_document(obj)) for obj in objects]
        receipt = T.semantic_receipt(profile="core/v1", capabilities={}, before={}, after={})
        arguments = dict(marker=self.marker, operation=operation,
                         parents=C.commit_frontier(captured.commits), baseline=captured.baseline,
                         objects=pairs, receipt=receipt,
                         view_template=C.document_template(captured.document))
        draft = C.make_commit(**arguments, view=b"")
        selected = {**captured.objects, **{obj["id"]: obj for obj in objects}}
        commits = {**captured.commits, operation: C.encode_document(draft)}
        rendered = self.store.render(captured, objects=selected, commits=commits)
        manifest = C.make_commit(**arguments, view=rendered)
        files = [{"path": self.entry.name, "role": "record", "before": captured.entry_bytes,
                  "after": rendered}]
        files += [{"path": (Path(self.store.layout["history"]) / obj["subject"] /
                            (obj["id"] + ".yaml")).relative_to(self.root).as_posix(),
                   "role": "history_object", "before": None, "after": raw}
                  for obj, raw in pairs]
        files.append({"path": (Path(self.store.layout["history_commits"]) /
                               (operation + ".yaml")).relative_to(self.root).as_posix(),
                      "role": "history_commit", "before": None,
                      "after": C.encode_document(manifest)})
        mutation = T.PreparedMutation(operation=operation, authority=self.marker,
                                     baseline=captured.baseline, files=files,
                                     receipt=receipt, entry=self.entry.name)
        self.store.commit(mutation, verify=lambda _data: None)

    def fixture(self, value=10, recipe="30", *, limit=20, bad=False, judgment=True):
        earlier = (datetime.datetime.now(datetime.timezone.utc).date()
                   - datetime.timedelta(days=2)).isoformat()
        reading = claim(body={"v": value, "of": earlier, "measure": "base_value",
                              "at": "fixture source", "applies": {"environment": "test"}})
        objects = [reading]
        if judgment:
            condition = ({"expr": "p.input / 0 > 1"} if bad else
                         {"expr": f"p.input > {limit}"})
            judgment = claim("d.limit", operation="limit", kind="judgment",
                             pins={"p.input": reading["id"]},
                             body={"verdict": "okay", "rests_on": ["p.input"],
                                   "seen": {"p.input": value}, "wrong_if": condition})
            objects.append(judgment)
        self.publish(objects, "initial")
        measure = Path(self.store.layout["measure"])
        measure.parent.mkdir(parents=True, exist_ok=True)
        measure.write_text("base_value: [" + PYTHON + ", -I, -c, \"print(" + recipe + ")\"]\n")
        return reading

    def bytes(self):
        return {path.relative_to(self.root).as_posix(): path.read_bytes()
                for path in sorted(self.root.rglob("*")) if path.is_file()}

    def test_plan_runs_nothing_and_writes_nothing(self):
        self.fixture()
        before = self.bytes()
        with mock.patch.object(remeasure, "run_recipe", side_effect=AssertionError("recipe ran")):
            lines, code = remeasure.measure([str(self.entry)])
        self.assertEqual(code, 0, "\n".join(lines))
        self.assertIn("nothing ran - add --run", "\n".join(lines))
        self.assertEqual(self.bytes(), before)

    def test_changed_value_is_final_prospective_state_and_trips_falsifier(self):
        original = self.fixture()
        before = self.bytes()
        lines, code = remeasure.measure([str(self.entry)], run=True)
        output = "\n".join(lines)
        self.assertEqual(code, 1, output)
        self.assertIn("FALSIFIED d.limit", output)
        self.assertIn("prospective history candidate only", output)
        self.assertEqual(self.bytes(), before)
        captured = self.store.capture()
        self.assertEqual(captured.state["subjects"]["p.input"]["body"]["v"], 10)
        self.assertEqual(captured.objects[original["id"]]["body"]["at"], "fixture source")

    def test_unchanged_and_scalar_values_are_clean_and_use_real_utc_day(self):
        self.fixture(value=True, recipe="'true'", judgment=False)
        lines, code = remeasure.measure([str(self.entry)], run=True)
        output = "\n".join(lines)
        self.assertEqual(code, 0, output)
        self.assertIn("p.input: true - as recorded", output)
        self.assertIn("measured on " + datetime.datetime.now(datetime.timezone.utc).date().isoformat()
                      + " (UTC)", output)

    def test_named_fold_and_measurement_share_final_scope_and_recheck_head(self):
        self.fixture(limit=100)
        proposal = HH.prepare(self.entry, "candidate",
                              {"kind": "set", "id": "p.input", "value": 15,
                               "as_of": (datetime.datetime.now(datetime.timezone.utc).date()
                                         - datetime.timedelta(days=1)).isoformat()},
                              head={"claim": "candidate", "wrong_if": {"expr": "p.input > 25"}},
                              by="writer", operation="candidate-proposal")
        HH.commit(self.entry, proposal, verify=lambda _data: None)
        lines, code = remeasure.measure([str(self.entry)], run=True)
        output = "\n".join(lines)
        self.assertEqual(code, 1, output)
        self.assertIn("FALSIFIED hypothesis candidate: wrong_if holds on the measured candidate", output)
        self.assertEqual(self.store.capture().state["subjects"]["p.input"]["body"]["v"], 10)

    def test_unevaluated_named_head_is_incomplete_not_clear(self):
        self.fixture(limit=100)
        proposal = HH.prepare(self.entry, "legacy-head",
                              {"kind": "set", "id": "p.input", "value": 15,
                               "as_of": (datetime.datetime.now(datetime.timezone.utc).date()
                                         - datetime.timedelta(days=1)).isoformat()},
                              head={"claim": "legacy condition", "wrong_if": "p.input > 25"},
                              by="writer", operation="legacy-head-proposal")
        HH.commit(self.entry, proposal, verify=lambda _data: None)
        lines, code = remeasure.measure([str(self.entry)], run=True)
        output = "\n".join(lines)
        self.assertEqual(code, 1, output)
        self.assertIn("HOLE hypothesis legacy-head: wrong_if unavailable", output)

    def test_never_fold_and_same_day_admission_refuse(self):
        self.fixture(limit=100)
        today = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
        proposal = HH.prepare(self.entry, "what-if",
                              {"kind": "set", "id": "p.input", "value": 15,
                               "as_of": today}, head={"claim": "what if", "folds": "never"},
                              by="writer", operation="never-proposal")
        HH.commit(self.entry, proposal, verify=lambda _data: None)
        with self.assertRaisesRegex(BaseException, "hypothesis_never_folds"):
            remeasure.measure([str(self.entry)], run=True)

    def test_same_day_measurement_refuses_instead_of_forcing_acceptance(self):
        self.fixture(limit=100)
        from scripts import history_authoring as authoring
        today = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
        mutation = authoring.prepare_batch(
            self.entry, [{"kind": "set", "id": "p.input", "value": 15, "as_of": today}],
            by="writer", operation="same-day-base")
        authoring.commit(self.entry, mutation, verify=lambda _data: None)
        lines, code = remeasure.measure([str(self.entry)], run=True)
        self.assertEqual(code, 1)
        self.assertIn("same day", "\n".join(lines))

    def test_physical_hypothesis_is_explicitly_incomplete(self):
        self.fixture(limit=100)
        directory = Path(self.store.layout["hypotheses"])
        directory.mkdir(parents=True, exist_ok=True)
        directory.joinpath("physical.yaml").write_text(
            "hypothesis: {claim: physical}\nreadings:\n  p.input: {v: 12}\n")
        with self.assertRaisesRegex(BaseException, "physical hypotheses.*no faithful admitted history path"):
            remeasure.measure([str(self.entry)], run=True)

    def test_pending_target_context_is_retained_in_final_candidate(self):
        self.fixture(limit=100)
        operations = remeasure._operations()
        base = operations.load([str(self.entry)], allow_history=True)
        state = remeasure.HR.prepare([str(self.entry)], base)
        final = remeasure.HR.measured(
            [str(self.entry)], state,
            {"p.input": ("base_value", operations.world(state["candidate"]).raw["p.input"], 30)},
            lambda name: "measured by " + name)
        before = operations.snapshot_for(base).to_data()["context"]
        after = operations.snapshot_for(final["document"]).to_data()["context"]
        self.assertEqual(after["pending"], before["pending"])
        self.assertEqual(after["target"], before["target"])
        self.assertEqual(after["project"], before["project"])

    def test_unknown_is_not_clean_and_source_mutation_refuses_before_results(self):
        self.fixture(bad=True)
        lines, code = remeasure.measure([str(self.entry)], run=True)
        self.assertEqual(code, 1, "\n".join(lines))
        self.assertIn("HOLE", "\n".join(lines))

        original = self.entry.read_bytes()
        def mutating(*_args, **_kwargs):
            self.entry.write_bytes(original + b"\n")
            return "30\n", None
        with mock.patch.object(remeasure, "run_recipe", side_effect=mutating):
            lines, code = remeasure.measure([str(self.entry)], run=True)
        self.assertEqual(code, 1)
        self.assertIn("snapshot_changed", "\n".join(lines))

    def test_real_cli_routes_history(self):
        self.fixture()
        package_temp = tempfile.TemporaryDirectory()
        self.addCleanup(package_temp.cleanup)
        package_root = Path(package_temp.name)
        package_root.joinpath("kpopper").symlink_to(ROOT / "scripts", target_is_directory=True)
        environment = dict(os.environ, PYTHONPATH=str(package_root))
        result = subprocess.run([PYTHON, "-m", "kpopper.cli", "remeasure", "--run",
                                 str(self.entry)], cwd=self.root, env=environment,
                                text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("prospective history candidate only", result.stdout)


if __name__ == "__main__":
    unittest.main()
