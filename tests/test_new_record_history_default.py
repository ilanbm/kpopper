"""Acceptance boundary for new history defaults and legacy preservation."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from scripts import history_contract as C, history_store as H

ROOT = Path(__file__).resolve().parents[1]


class NewRecordHistoryDefault(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.workspace = self.root / "fresh workspace"
        self.workspace.mkdir()
        self.env = dict(os.environ)
        self.env.update(XDG_STATE_HOME=str(self.root / "state"),
                        XDG_CONFIG_HOME=str(self.root / "config"),
                        KPOPPER_SESSION_DISABLE="1")

    def cli(self, *args, cwd=None):
        return subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), *args],
                              cwd=cwd or self.workspace, env=self.env,
                              capture_output=True, text=True)

    def add_first(self):
        result = self.cli("add", "p.value", "v=1", "--as-of", "2026-09-01")
        self.assertEqual(result.returncode, 0, result.stderr)
        return self.workspace / "GROUNDING.yaml"

    def test_fresh_first_write_defaults_to_core_history_and_reopens(self):
        record = self.add_first()
        self.assertTrue(record.is_file())
        import yaml
        document = yaml.safe_load(record.read_text())
        self.assertEqual(document["meta"]["reasoning"]["profile"], "core/v1")
        marker_path = self.workspace / ".kpopper" / "history.yaml"
        marker = C.decode_document(marker_path.read_bytes())
        self.assertEqual(marker["authority"], "history")
        self.assertEqual(marker["record_id"], document["meta"]["history"]["record_id"])
        self.assertEqual(marker["generation"], document["meta"]["history"]["authority_generation"])
        captured = H.Store(record).capture()
        self.assertEqual(captured.state["subjects"]["p.value"]["body"]["v"], 1)

        reopened = self.cli("open", "--json")
        self.assertEqual(reopened.returncode, 0, reopened.stderr)
        payload = json.loads(reopened.stdout)
        self.assertEqual(payload["assessment_profile"], "core/v1")
        self.assertEqual(Path(payload["record"]).resolve(), record.resolve())
        checked = self.cli("check")
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertIn("core/v1 snapshot", checked.stdout)
        pulled = self.cli("pull", "p.value")
        self.assertEqual(pulled.returncode, 0, pulled.stderr)
        self.assertEqual(json.loads(pulled.stdout)["profile"], "core/v1-consumer/v1")
        affected = self.cli("affects", "p.value")
        self.assertEqual(affected.returncode, 0, affected.stderr)
        self.assertIn("core/v1 snapshot", affected.stdout)

    def test_existing_undeclared_grounding_stays_legacy(self):
        record = self.workspace / "GROUNDING.yaml"
        record.write_text("known:\n  p.value: {v: 1}\n", encoding="utf-8")
        reopened = self.cli("open", "--json")
        self.assertEqual(reopened.returncode, 0, reopened.stderr)
        self.assertEqual(Path(json.loads(reopened.stdout)["record"]).resolve(), record.resolve())
        import yaml
        document = yaml.safe_load(record.read_text())
        self.assertNotIn("reasoning", document.get("meta", {}))
        self.assertFalse((self.workspace / ".kpopper" / "history.yaml").exists())

    def test_other_public_readers_auto_select_declared_core(self):
        self.add_first()
        assessed = self.cli("assess", "p.value")
        self.assertEqual(assessed.returncode, 0, assessed.stderr)
        self.assertEqual(json.loads(assessed.stdout)["assessment_profile"], "core/v1")
        exported = self.cli("export", "p.value")
        self.assertEqual(exported.returncode, 0, exported.stderr)
        self.assertIn("Shared assessment", exported.stdout)
        searched = self.cli("search", "value")
        self.assertEqual(searched.returncode, 0, searched.stderr)
        self.assertIn("findings_revision", json.loads(searched.stdout))
        page = self.root / "record.html"
        rendered = self.cli("page", "--out", str(page))
        self.assertEqual(rendered.returncode, 0, rendered.stderr)
        self.assertTrue(page.is_file())

    def test_session_mark_and_gate_auto_select_declared_core(self):
        record = self.add_first()
        state = self.root / "session-mark.json"
        mark = subprocess.run([sys.executable, str(ROOT / "scripts/provenance.py"),
            "mark", str(state), str(record)], cwd=self.workspace, env=self.env,
            capture_output=True, text=True)
        self.assertEqual(mark.returncode, 0, mark.stderr)
        self.assertEqual(json.loads(state.read_text())["profile"], "core/v1")
        gate = subprocess.run([sys.executable, str(ROOT / "scripts/provenance.py"),
            "gate", str(state), str(record)], cwd=self.workspace, env=self.env,
            capture_output=True, text=True)
        self.assertEqual(gate.returncode, 0, gate.stderr + gate.stdout)

    def test_expected_history_refusal_is_a_clean_cli_error(self):
        self.add_first()
        refused = self.cli("add", "p.other", "v=2", "--profile", "ordinary-reader/v1")
        self.assertNotEqual(refused.returncode, 0)
        self.assertNotIn("Traceback", refused.stderr)
        self.assertIn("history_profile_migration_required", refused.stderr)

    def test_core_open_does_not_silently_drop_legacy_options(self):
        self.add_first()
        for option in (("--chars", "10"), ("--budget", "2"), ("--host", "codex")):
            with self.subTest(option=option):
                result = self.cli("open", *option)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("core_profile_option_unsupported: " + option[0],
                              result.stderr + result.stdout)

    def test_existing_undeclared_legacy_name_stays_legacy(self):
        record = self.workspace / "PROVENANCE.yaml"
        record.write_text("known:\n  p.value: {v: 1}\n", encoding="utf-8")
        reopened = self.cli("open", "--json")
        self.assertEqual(reopened.returncode, 0, reopened.stderr)
        self.assertEqual(Path(json.loads(reopened.stdout)["record"]).resolve(), record.resolve())
        import yaml
        document = yaml.safe_load(record.read_text())
        self.assertNotIn("reasoning", document.get("meta", {}))
        self.assertFalse((self.workspace / "PROVENANCE.history.yaml").exists())

    def test_concurrent_first_writers_produce_one_history_generation(self):
        commands = [[sys.executable, str(ROOT / "scripts/cli.py"), "add", subject,
                     "v=" + value, "--as-of", "2026-09-01"]
                    for subject, value in (("p.first", "1"), ("p.second", "2"))]
        processes = [subprocess.Popen(command, cwd=self.workspace, env=self.env,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                     for command in commands]
        results = [process.communicate(timeout=20) + (process.returncode,) for process in processes]
        self.assertTrue(all(code == 0 for stdout, stderr, code in results), results)
        captured = H.Store(self.workspace / "GROUNDING.yaml").capture()
        self.assertEqual(set(captured.state["subjects"]), {"p.first", "p.second"})
        self.assertEqual(captured.marker["generation"], 1)
        self.assertEqual(len(captured.commits), 2)

    def test_configured_missing_legacy_name_is_not_created(self):
        legacy = self.workspace / "PROVENANCE.yaml"
        config = self.workspace / ".kpopper" / "project.json"
        config.parent.mkdir()
        config.write_text(json.dumps({"version": 1, "mode": "simple", "record": str(legacy),
                                      "publication": None, "generation": 1}))
        result = self.cli("add", "p.value", "v=1", "--as-of", "2026-09-01")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("new records must use GROUNDING.yaml", result.stderr + result.stdout)
        self.assertFalse(legacy.exists())
        self.assertFalse((self.workspace / "PROVENANCE.history.yaml").exists())
        self.assertFalse((self.workspace / "PROVENANCE.history").exists())


if __name__ == "__main__":
    unittest.main()
