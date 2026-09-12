"""A source report changes one coherent graph, never an intermediate one."""
import copy
import json
import importlib.util
from pathlib import Path
import tempfile
import subprocess
import sys
import time
import unittest
from unittest.mock import patch

import yaml

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("batch_ingestion", ROOT / "scripts/ingestion.py")
I = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(I)


class BatchIngestion(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.rec = self.root / "PROVENANCE.yaml"
        self.state = self.root / "state"
        self.doc = {
            "schema": {"deps": "rests_on", "snapshot": "seen", "predicate": "wrong_if"},
            "sources": {"s.old": {"file": "old.md", "read": "2026-09-09"}},
            "known": {
                "order.price": {"v": 10, "from": "s.old", "of": "2026-09-09"},
                "order.quantity": {"v": 10, "from": "s.old", "of": "2026-09-09"},
                "order.limit": {"v": 15, "from": "s.old", "of": "2026-09-09"},
                "order.total": {"rule": "order.price * order.quantity"},
            },
            "judgments": {"c.affordable": {
                "rests_on": ["order.price", "order.limit"], "verdict": "Within budget",
                "wrong_if": "order.price > order.limit", "seen": {"order.price": 10, "order.limit": 15},
            }},
        }
        self.rec.write_text(yaml.safe_dump(self.doc, sort_keys=False), encoding="utf-8")

    def report(self, updates=None, **extra):
        return {"event_id": "batch-1", "source_quote": "המחיר 20, הכמות 5, תקרת המחיר 30.",
                "record_sha256": I._sha(self.rec.read_bytes()),
                "date": "2026-09-10", "updates": updates or [
                    {"kind": "set", "id": "order.price", "value": 20},
                    {"kind": "set", "id": "order.quantity", "value": 5},
                    {"kind": "set", "id": "order.limit", "value": 30},
                ], **extra}

    def run_report(self, report=None):
        capture = I.capture(report or self.report(), self.rec, self.state, start=False)
        return I.process(self.rec, self.state, event_id=capture["event_id"])[0]

    def test_final_state_only_and_one_source_for_all_updates(self):
        result = self.run_report()
        self.assertEqual(result["state"], "applied", result)
        doc = yaml.safe_load(self.rec.read_text())
        self.assertEqual(doc["known"]["order.price"]["v"], 20)
        self.assertEqual(doc["known"]["order.quantity"]["v"], 5)
        self.assertEqual(doc["judgments"], self.doc["judgments"])
        self.assertEqual(doc["known"]["order.price"]["from"], result["source"])
        self.assertEqual(doc["known"]["order.quantity"]["from"], result["source"])
        self.assertEqual(Path(result["source_file"]).read_text(), self.report()["source_quote"])
        self.assertEqual(result["newly_fired_judgments"], [])
        self.assertEqual(I.pending(self.rec, self.state), [])

    def test_new_fact_rule_and_judgment_have_real_dependencies(self):
        updates = self.report()["updates"] + [
            {"kind": "add", "id": "order.shipping", "body": {"v": 8, "name": "משלוח"}},
            {"kind": "add", "id": "order.delivered", "body": {"rule": "order.total + order.shipping"}},
            {"kind": "add", "id": "c.delivered", "body": {
                "rests_on": ["order.delivered", "order.price", "order.shipping"], "verdict": "Delivery fits the budget",
                "wrong_if": "order.shipping > 20"}},
        ]
        result = self.run_report(self.report(updates, source_quote="מחיר 20, כמות 5, משלוח 8."))
        self.assertEqual(result["state"], "applied", result)
        doc = yaml.safe_load(self.rec.read_text())
        self.assertEqual(doc["known"]["order.shipping"]["from"], result["source"])
        seen = doc["judgments"]["c.delivered"]["seen"]
        self.assertEqual(seen["order.price"], 20)
        self.assertEqual(seen["order.shipping"], 8)
        self.assertEqual(seen["order.delivered"], "order.total + order.shipping")
        self.assertIn("c.delivered", result["reach"]["judgments"])
        self.assertEqual(I.P.check_lines([str(self.rec)])[0], [])

    def test_one_bad_operation_preserves_entire_record_and_source(self):
        before = self.rec.read_bytes()
        updates = self.report()["updates"] + [{"kind": "add", "id": "c.bad", "body": {
            "rests_on": ["missing.value"], "verdict": "Unsupported", "wrong_if": "missing.value > 0"}}]
        result = self.run_report(self.report(updates))
        self.assertEqual(result["state"], "needs_primary", result)
        self.assertEqual(self.rec.read_bytes(), before)
        self.assertTrue(Path(result["source_file"]).is_file())

    def test_real_falsification_survives_with_one_signal(self):
        updates = self.report()["updates"]
        updates[2]["value"] = 15
        result = self.run_report(self.report(updates))
        self.assertEqual(result["state"], "applied", result)
        self.assertEqual(result["newly_fired_judgments"], ["c.affordable"])
        self.assertEqual(len(I.pending(self.rec, self.state)), 1)
        self.assertEqual(yaml.safe_load(self.rec.read_text())["judgments"], self.doc["judgments"])

    def test_repeated_capture_and_crash_do_not_apply_twice(self):
        report = self.report()
        event = I.capture(report, self.rec, self.state, start=False)
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.rec, self.state, _crash_after_commit=True)
        after = self.rec.read_bytes()
        result = I.process(self.rec, self.state)[0]
        self.assertTrue(result["recovered"], result)
        self.assertEqual(self.rec.read_bytes(), after)
        self.assertEqual(I.capture(report, self.rec, self.state, start=False)["event_id"], event["event_id"])
        self.assertEqual(I.process(self.rec, self.state), [])

    def test_concurrent_target_edit_is_not_overwritten(self):
        I.capture(self.report(), self.rec, self.state, start=False)
        other = copy.deepcopy(self.doc)
        other["known"]["order.price"]["v"] = 99
        self.rec.write_text(yaml.safe_dump(other, sort_keys=False))
        before = self.rec.read_bytes()
        result = I.process(self.rec, self.state)[0]
        self.assertEqual(result["state"], "needs_primary")
        self.assertEqual(self.rec.read_bytes(), before)

    def test_batch_cannot_replace_an_existing_judgment(self):
        before = self.rec.read_bytes()
        result = self.run_report(self.report([{"kind": "add", "id": "c.affordable",
                                             "body": self.doc["judgments"]["c.affordable"]}]))
        self.assertEqual(result["state"], "needs_primary")
        self.assertEqual(self.rec.read_bytes(), before)

    def test_older_reading_is_retained_without_partial_changes(self):
        before = self.rec.read_bytes()
        result = self.run_report(self.report(date="2026-09-09"))
        self.assertEqual(result["state"], "needs_primary")
        self.assertEqual(self.rec.read_bytes(), before)

    def test_concurrent_edit_during_prepare_is_preserved(self):
        event = I.capture(self.report(), self.rec, self.state, start=False)
        prepare = I._prepare
        def intervening_edit(*args):
            result = prepare(*args)
            text = self.rec.read_text().replace("v: 10", "v: 99", 1)
            self.rec.write_text(text)
            return result
        with patch.object(I, "_prepare", side_effect=intervening_edit):
            result = I.process(self.rec, self.state, event["event_id"])[0]
        self.assertEqual(result["state"], "needs_primary")
        doc = yaml.safe_load(self.rec.read_text())
        self.assertEqual(doc["known"]["order.price"]["v"], 99)
        self.assertEqual(doc["known"]["order.quantity"]["v"], 10)

    def test_commit_recovery_does_not_reapply_an_explicit_revert(self):
        before = self.rec.read_bytes()
        I.capture(self.report(), self.rec, self.state, start=False)
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.rec, self.state, _crash_after_commit=True)
        self.rec.write_bytes(before)
        result = I.process(self.rec, self.state)[0]
        self.assertEqual(result["state"], "needs_primary")
        self.assertEqual(self.rec.read_bytes(), before)

    def test_new_judgment_cannot_silently_adopt_changed_premises(self):
        report = self.report([{"kind": "add", "id": "c.buy", "body": {
            "rests_on": ["order.price"], "verdict": "Buy now",
            "reopened_by": "The purchasing policy changes"}}],
            source_quote="At price 10 I recommend buying.")
        I.capture(report, self.rec, self.state, start=False)
        self.doc["known"]["order.price"]["v"] = 100
        self.rec.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        before = self.rec.read_bytes()
        result = I.process(self.rec, self.state)[0]
        self.assertEqual(result["state"], "needs_primary", result)
        self.assertEqual(self.rec.read_bytes(), before)

    def test_stale_primary_read_is_rejected_even_before_capture(self):
        report = self.report([{"kind": "add", "id": "order.shipping", "body": {"v": 8}}])
        self.doc["known"]["order.price"]["v"] = 100
        self.rec.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        before = self.rec.read_bytes()
        result = self.run_report(report)
        self.assertEqual(result["state"], "needs_primary")
        self.assertIn("primary read", result["reason"])
        self.assertEqual(self.rec.read_bytes(), before)

    def test_add_without_primary_read_is_retained_as_a_question(self):
        report = self.report([{"kind": "add", "id": "order.shipping", "body": {"v": 8}}])
        del report["record_sha256"]
        before = self.rec.read_bytes()
        result = self.run_report(report)
        self.assertEqual(result["state"], "needs_primary")
        self.assertEqual(self.rec.read_bytes(), before)

    def test_reader_count_judgment_never_snapshots_an_intermediate_batch(self):
        before = self.rec.read_bytes()
        updates = [
            {"kind": "add", "id": "c.size", "body": {"rests_on": ["graph.entries"],
                "verdict": "The corpus is small", "wrong_if": "graph.entries > 100"}},
            {"kind": "add", "id": "order.shipping", "body": {"v": 8}},
        ]
        result = self.run_report(self.report(updates))
        self.assertEqual(result["state"], "needs_primary")
        self.assertIn("reader/page counts", result["reason"])
        self.assertEqual(self.rec.read_bytes(), before)

    def test_public_capture_runs_batch_in_native_background_processor(self):
        payload = self.root / "report.json"
        payload.write_text(json.dumps(self.report(), ensure_ascii=False), encoding="utf-8")
        completed = subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), "ingest", "capture",
                                    "--record", str(self.rec), "--state-dir", str(self.state),
                                    "--file", str(payload)], capture_output=True, text=True)
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        event = json.loads(completed.stdout)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            state = I.status(event["event_id"], self.rec, self.state)
            if state["state"] in I.TERMINAL:
                break
            time.sleep(0.03)
        self.assertEqual(state["state"], "applied", state)
        self.assertEqual(yaml.safe_load(self.rec.read_text())["known"]["order.price"]["v"], 20)

    def test_grounding_batch_stages_the_hidden_brief_in_its_own_layout(self):
        modern = self.root / "GROUNDING.yaml"
        self.rec.rename(modern)
        self.rec = modern
        brief = self.root / ".kpopper" / "view.yaml"
        brief.parent.mkdir()
        # The existing page gate requires a tab to serve newly captured source intents.
        source_id = "s.ingest_" + I._event_id(self.rec.resolve(), self.report())
        brief.write_text(yaml.safe_dump({"title": "Order", "tabs": [{"title": "Budget", "serves": [source_id],
            "sections": [{"title": "Readings", "pick": ["order.price", "order.quantity", "order.limit", "c.affordable"]}]}]}))
        original = brief.read_bytes()
        result = self.run_report()
        self.assertEqual(result["state"], "applied", result)
        self.assertEqual(brief.read_bytes(), original)
        drafts = list((self.state / "drafts").glob("*/.kpopper/view.yaml"))
        self.assertEqual(len(drafts), 1)
        self.assertEqual(drafts[0].read_bytes(), original)
        self.assertFalse((self.root / "view.yaml").exists())


if __name__ == "__main__":
    unittest.main()
