"""Behavioral checks for the production selective-ingestion queue."""
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

import yaml


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("kpopper_ingestion", ROOT / "scripts" / "ingestion.py")
I = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(I)


class IngestionQueue(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / "PROVENANCE.yaml"
        self.state = self.root / "private-state"
        (self.root / "old.txt").write_text("old fixture\n")
        self.write_record()

    def write_record(self, *, fired=False, moved=False):
        wrong_if = "facts.count < 0"
        if fired:
            wrong_if = "facts.count == false"
        judgment = {
            "rests_on": ["facts.count"], "verdict": "The count remains acceptable.",
            "seen": {"facts.count": True if fired else 3},
        }
        if not moved:
            judgment["wrong_if"] = wrong_if
        doc = {
            "meta": {"name": "Queue fixture", "updated": "2026-09-07"},
            "schema": {"deps": "rests_on", "snapshot": "seen", "predicate": "wrong_if"},
            "sources": {"s.old": {"name": "Old fixture", "file": "old.txt", "read": "2026-09-07"}},
            "known": {
                "facts.count": {"name": "Count", "v": True if fired else 3,
                                "from": "s.old", "at": "line 1", "of": "2026-09-07"},
                "facts.other": {"name": "Other", "v": 10, "from": "s.old", "at": "line 1",
                                "of": "2026-09-07"},
            },
            "judgments": {"c.acceptable": judgment},
        }
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))

    def envelope(self, **changes):
        value = changes.pop("value", 4)
        out = {"event_id": "evt-1", "session_id": "session-1",
               "source_quote": "There are four packages now.", "target": "facts.count",
               "value": value, "date": "2026-09-08", "kind": "report"}
        out.update(changes)
        return out

    def capture(self, envelope=None):
        return I.capture(envelope or self.envelope(), self.record, self.state, start=False)

    def read(self):
        return yaml.safe_load(self.record.read_text())

    def test_default_storage_is_outside_the_record_and_read_status_does_not_create_it(self):
        state_home = tempfile.TemporaryDirectory()
        self.addCleanup(state_home.cleanup)
        xdg = Path(state_home.name)
        with mock.patch.dict(os.environ, {"XDG_STATE_HOME": str(xdg)}):
            expected = I.state_path(self.record)
            self.assertFalse(expected.exists())
            self.assertEqual(I.status(record=self.record), [])
            self.assertEqual(I.pending(record=self.record), [])
            self.assertFalse(expected.exists())
            got = I.capture(self.envelope(), self.record, start=False)
            self.assertEqual(got["state"], "captured")
            self.assertTrue(expected.is_dir())
            self.assertFalse(str(expected).startswith(str(self.record.parent)))
            self.assertEqual(stat.S_IMODE(expected.stat().st_mode), 0o700)
            source = Path(I.status(got["event_id"], self.record)["source_file"])
            self.assertEqual(source.read_text(), self.envelope()["source_quote"])
            self.assertEqual(stat.S_IMODE(source.stat().st_mode), 0o600)

    def test_relative_xdg_state_home_is_ignored(self):
        fallback = tempfile.TemporaryDirectory()
        self.addCleanup(fallback.cleanup)
        with mock.patch.dict(os.environ, {"XDG_STATE_HOME": "relative-state"}), \
                mock.patch.object(I.Path, "home", return_value=Path(fallback.name)):
            path = I.state_path(self.record)
        self.assertTrue(path.is_absolute())
        expected = (Path(fallback.name) / ".local" / "state").resolve()
        self.assertTrue(str(path).startswith(str(expected)))
        self.assertFalse(str(path).startswith(str(self.root)))

    def test_quiet_muted_move_applies_without_a_signal(self):
        before = self.read()
        event = self.capture()
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "applied")
        self.assertEqual(I.pending(self.record, self.state), [])
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 4)
        self.assertEqual(receipt["reach"]["judgments"], ["c.acceptable"])
        after = self.read()
        self.assertEqual(after["judgments"], before["judgments"])
        self.assertEqual(after["known"]["facts.other"], before["known"]["facts.other"])
        self.assertEqual(after["sources"]["s.old"], before["sources"]["s.old"])

    def test_newly_fired_judgment_is_a_durable_contradiction(self):
        self.write_record(fired=True)
        event = self.capture(self.envelope(value=False))
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["newly_fired_judgments"], ["c.acceptable"])
        signals = I.pending(self.record, self.state)
        self.assertEqual([x["category"] for x in signals], ["contradiction"])
        self.assertEqual(signals[0]["source_quote"], self.envelope()["source_quote"])
        self.assertEqual(signals[0]["affected_judgments"], ["c.acceptable"])
        I.acknowledge(signals[0]["id"], self.record, self.state)
        self.assertEqual(I.pending(self.record, self.state), [])
        self.assertEqual(len(I.pending(self.record, self.state, include_handled=True)), 1)
        self.assertEqual(receipt, I.status(event["event_id"], self.record, self.state))

    def test_repaired_contradiction_stops_delivery_but_keeps_its_receipt(self):
        self.write_record(fired=True)
        event = self.capture(self.envelope(value=False))
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(len(I.pending(self.record, self.state)), 1)
        doc = self.read()
        doc["known"]["facts.count"].update(v=True, **{"from": "s.old", "at": "line 1",
                                                       "of": "2026-09-09"})
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        self.assertEqual(I.pending(self.record, self.state), [])
        self.assertEqual(I.status(event["event_id"], self.record, self.state), receipt)
        stored = list((self.state / "signals").glob("*.json"))
        self.assertEqual(len(stored), 1)

    def test_actionable_moved_judgment_asks_a_question(self):
        self.write_record(moved=True)
        event = self.capture()
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["actionable_judgments"], ["c.acceptable"])
        signal = I.pending(self.record, self.state)[0]
        self.assertEqual(signal["category"], "question")
        self.assertIn("requires review", signal["question"])

    def test_pending_recomputes_partially_resolved_judgments(self):
        self.write_record(moved=True)
        doc = self.read()
        doc["judgments"]["c.second"] = {
            "rests_on": ["facts.count"], "verdict": "Second remains acceptable.",
            "seen": {"facts.count": 3},
        }
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        event = self.capture()
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["actionable_judgments"], ["c.acceptable", "c.second"])
        fixed = self.read()
        fixed["judgments"]["c.acceptable"]["seen"]["facts.count"] = 4
        self.record.write_text(yaml.safe_dump(fixed, sort_keys=False))
        signal = I.pending(self.record, self.state)[0]
        self.assertEqual(signal["actionable_judgments"], ["c.second"])
        self.assertEqual(signal["affected_judgments"], ["c.second"])
        self.assertNotIn("c.acceptable", signal["reason"])
        stored = json.loads(next((self.state / "signals").glob("*.json")).read_text())
        self.assertEqual(stored["actionable_judgments"], ["c.acceptable", "c.second"])

    def test_ambiguous_input_is_retained_and_never_guessed(self):
        envelope = self.envelope()
        envelope.pop("target")
        envelope["question"] = "Which count did you mean?"
        event = self.capture(envelope)
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 3)
        signal = I.pending(self.record, self.state)[0]
        self.assertEqual(signal["question"], envelope["question"])
        self.assertEqual(Path(receipt["source_file"]).read_text(), envelope["source_quote"])
        self.assertIsNone(receipt["source"])

    def test_superseded_receipt_does_not_claim_a_record_source(self):
        captured = self.capture(self.envelope(event_id="legacy-superseded"))
        event = I._load(I._event_file(self.state, captured["event_id"]))
        receipt = I._finish(self.state, event, self.envelope(event_id="legacy-superseded"),
                            "superseded", "superseded by an already accepted report")
        self.assertIsNone(receipt["source"])
        self.assertTrue(Path(receipt["source_file"]).is_file())
        self.assertNotIn("s.ingest_" + captured["event_id"], self.read()["sources"])

    def test_wrong_type_keeps_source_and_does_not_write(self):
        event = self.capture(self.envelope(value="four"))
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertIn("scalar type", receipt["reason"])
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 3)
        self.assertTrue(Path(receipt["source_file"]).is_file())

    def test_tampered_source_text_never_reaches_the_record(self):
        event = self.capture()
        source = Path(I.status(event["event_id"], self.record, self.state)["source_file"])
        source.write_text("There are nine packages now.")
        before = self.record.read_bytes()
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertIn("source text changed", receipt["reason"])
        self.assertIsNone(receipt["source"])
        self.assertEqual(self.record.read_bytes(), before)

    def test_tampered_envelope_never_changes_value_or_date(self):
        event = self.capture()
        envelope_path = self.state / "envelopes" / (event["event_id"] + ".json")
        envelope_path.write_text(json.dumps(self.envelope(value=9, date="2026-09-09")))
        before = self.record.read_bytes()
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertIn("envelope changed", receipt["reason"])
        self.assertIsNone(receipt["source"])
        self.assertEqual(self.record.read_bytes(), before)

    def test_source_integrity_is_checked_again_at_commit(self):
        event = self.capture()
        original = I._prepare

        def prepare_then_tamper(*args):
            prepared = original(*args)
            source = Path(I.status(event["event_id"], self.record, self.state)["source_file"])
            source.write_text("tampered")
            return prepared

        before = self.record.read_bytes()
        with mock.patch.object(I, "_prepare", side_effect=prepare_then_tamper):
            receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertIn("source text changed", receipt["reason"])
        self.assertEqual(self.record.read_bytes(), before)

    def test_inline_value_formula_is_retained_as_a_question_without_rewriting_it(self):
        doc = self.read()
        doc["known"]["facts.total"] = {
            "name": "Total", "v": "facts.count + facts.other", "from": "s.old",
            "at": "line 1", "of": "2026-09-07",
        }
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        before = self.record.read_bytes()
        envelope = self.envelope(event_id="formula", target="facts.total", value="15",
                                 source_quote="The total is fifteen.")
        event = self.capture(envelope)
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertIn("inline expression", receipt["reason"])
        self.assertEqual(self.record.read_bytes(), before)
        self.assertEqual(Path(receipt["source_file"]).read_text(), envelope["source_quote"])

    def test_ordinary_text_value_is_not_treated_as_a_formula(self):
        doc = self.read()
        doc["known"]["facts.status"] = {
            "name": "Status", "v": "not selected", "from": "s.old",
            "at": "line 1", "of": "2026-09-07",
        }
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        event = self.capture(self.envelope(event_id="text", target="facts.status",
                                           value="selected", source_quote="It is selected."))
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "applied")
        self.assertEqual(self.read()["known"]["facts.status"]["v"], "selected")

    def test_source_is_added_to_the_record_existing_source_collection(self):
        doc = self.read()
        doc["evidence"] = doc.pop("sources")
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        event = self.capture()
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "applied")
        updated = self.read()
        self.assertNotIn("sources", updated)
        self.assertIn(receipt["source"], updated["evidence"])

    def test_empty_unproven_source_collection_is_not_guessed_as_a_value_collection(self):
        doc = self.read()
        doc["sources"] = {}
        doc["notes"] = {"n.context": {"name": "Context note", "of": "this session"}}
        for body in doc["known"].values():
            body.pop("from", None)
            body.pop("at", None)
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        before = self.record.read_bytes()
        event = self.capture()
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertIn("source collection", receipt["reason"])
        self.assertIsNone(receipt["source"])
        self.assertEqual(self.record.read_bytes(), before)

    def test_existing_from_identifies_even_a_minimal_source_collection(self):
        doc = self.read()
        doc["evidence"] = {"s.old": {"name": "Existing source"}}
        del doc["sources"]
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        event = self.capture()
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "applied")
        self.assertIn(receipt["source"], self.read()["evidence"])

    def test_native_yaml_date_metadata_can_be_snapshotted(self):
        text = self.record.read_text().replace("of: '2026-09-07'", "of: 2026-09-07", 1)
        self.record.write_text(text)
        self.assertIsInstance(self.read()["known"]["facts.count"]["of"], __import__("datetime").date)
        event = self.capture()
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "applied")
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 4)

    def test_non_contract_iso_date_forms_are_retained_as_questions(self):
        before = self.record.read_bytes()
        for number, value in enumerate(("20260909", "2026-W37-3"), 1):
            with self.subTest(value=value):
                event = self.capture(self.envelope(event_id="bad-date-" + str(number), date=value))
                receipt = I.process(self.record, self.state, event["event_id"])[0]
                self.assertEqual(receipt["state"], "needs_primary")
                self.assertIn("YYYY-MM-DD", receipt["reason"])
                self.assertIsNone(receipt["source"])
        self.assertEqual(self.record.read_bytes(), before)

    def test_external_target_change_is_never_overwritten(self):
        event = self.capture()
        doc = self.read()
        doc["known"]["facts.count"]["v"] = 9
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertIn("changed after capture", receipt["reason"])
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 9)

    def test_repeated_unrelated_cas_changes_become_an_explicit_question(self):
        event = self.capture()
        original = I._prepare
        attempts = []

        def prepare_then_change(*args):
            prepared = original(*args)
            doc = self.read()
            attempts.append(len(attempts) + 1)
            doc["meta"]["concurrent"] = attempts[-1]
            self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
            return prepared

        with mock.patch.object(I, "_prepare", side_effect=prepare_then_change):
            receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(len(attempts), I.MAX_REPREPARES + 1)
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertIn("changed repeatedly", receipt["reason"])
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 3)
        self.assertEqual(I.pending(self.record, self.state)[0]["category"], "question")

    def test_independent_targets_do_not_stale_each_other(self):
        first = self.capture(self.envelope(event_id="first", target="facts.count", value=4,
                                           source_quote="Count is four."))
        second = self.capture(self.envelope(event_id="second", target="facts.other", value=11,
                                            source_quote="Other is eleven."))
        receipts = I.process(self.record, self.state)
        self.assertEqual([x["state"] for x in receipts], ["applied", "applied"])
        doc = self.read()
        self.assertEqual(doc["known"]["facts.count"]["v"], 4)
        self.assertEqual(doc["known"]["facts.other"]["v"], 11)
        self.assertEqual({first["event_id"], second["event_id"]},
                         {x["event_id"] for x in receipts})

    def test_newer_dated_ingestion_can_follow_its_exact_owned_predecessor(self):
        older = self.capture(self.envelope(event_id="older", value=4, date="2026-09-08",
                                           source_quote="Count is four."))
        newer = self.capture(self.envelope(event_id="newer", value=5, date="2026-09-09",
                                           source_quote="Count is five."))
        receipts = I.process(self.record, self.state)
        by_id = {x["event_id"]: x for x in receipts}
        self.assertEqual(by_id[older["event_id"]]["state"], "applied")
        self.assertEqual(by_id[newer["event_id"]]["state"], "applied")
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 5)
        self.assertTrue(Path(by_id[older["event_id"]]["source_file"]).is_file())

    def test_later_arriving_older_and_same_day_reports_never_supersede(self):
        newest = self.capture(self.envelope(event_id="newest", value=5, date="2026-09-10",
                                            source_quote="Count is five."))
        older = self.capture(self.envelope(event_id="older-arrival", value=4, date="2026-09-09",
                                           source_quote="Count was four yesterday."))
        same_day = self.capture(self.envelope(event_id="same-day", value=6, date="2026-09-10",
                                              source_quote="Count is six."))
        by_id = {x["event_id"]: x for x in I.process(self.record, self.state)}
        self.assertEqual(by_id[newest["event_id"]]["state"], "applied")
        self.assertEqual(by_id[older["event_id"]]["state"], "needs_primary")
        self.assertEqual(by_id[same_day["event_id"]]["state"], "needs_primary")
        self.assertIsNone(by_id[older["event_id"]]["source"])
        self.assertIsNone(by_id[same_day["event_id"]]["source"])
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 5)
        for receipt in by_id.values():
            self.assertTrue(Path(receipt["source_file"]).is_file())

    def test_detached_capture_worker_finishes_after_the_cli_exits(self):
        envelope_file = self.root / "event.json"
        envelope_file.write_text(json.dumps(self.envelope(event_id="detached")))
        run = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "ingestion.py"), "capture",
             "--file", str(envelope_file), "--record", str(self.record),
             "--state-dir", str(self.state)],
            text=True, capture_output=True, check=True,
        )
        event = json.loads(run.stdout)
        lease = self.state / "worker.lease"
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            current = I.status(event["event_id"], self.record, self.state)
            # The receipt is terminal before the worker's final queue scan and
            # lease release. Keep its directory alive until that work is done.
            if current.get("state") in I.TERMINAL and not lease.exists():
                break
            time.sleep(0.02)
        self.assertFalse(lease.exists(), "The detached worker did not finish before cleanup")
        self.assertEqual(current["state"], "applied")
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 4)

    def test_stable_event_is_idempotent(self):
        first = self.capture()
        second = self.capture()
        self.assertEqual(first["event_id"], second["event_id"])
        one = I.process(self.record, self.state, first["event_id"])[0]
        self.assertEqual(I.process(self.record, self.state, first["event_id"]), [])
        self.assertEqual(one, I.status(first["event_id"], self.record, self.state))
        sources = [k for k in self.read()["sources"] if k.startswith("s.ingest_")]
        self.assertEqual(len(sources), 1)

    def test_stable_capture_retry_restarts_missing_worker(self):
        event = self.capture()
        with mock.patch.object(I, "_start_worker_locked") as start:
            retried = I.capture(self.envelope(), self.record, self.state, start=True)
        self.assertEqual(retried["event_id"], event["event_id"])
        start.assert_called_once()

    def test_spawn_failure_leaves_capture_retryable(self):
        envelope = self.envelope(event_id="spawn-failure")
        with mock.patch.object(I, "_start_worker_locked", side_effect=OSError("cannot spawn")):
            with self.assertRaises(OSError):
                I.capture(envelope, self.record, self.state, start=True)
        captured = I.status(I._event_id(self.record.resolve(), envelope), self.record, self.state)
        self.assertEqual(captured["state"], "captured")
        with mock.patch.object(I, "_start_worker_locked") as start:
            I.capture(envelope, self.record, self.state, start=True)
        start.assert_called_once()

    def test_unsafe_existing_state_directories_are_refused_before_mutation(self):
        occupied = self.root / "occupied-state"
        occupied.mkdir(mode=0o755)
        sentinel = occupied / "keep.txt"
        sentinel.write_text("keep")
        mode = stat.S_IMODE(occupied.stat().st_mode)
        with self.assertRaisesRegex(ValueError, "not owned"):
            I.capture(self.envelope(event_id="occupied"), self.record, occupied, start=False)
        self.assertEqual(sentinel.read_text(), "keep")
        self.assertEqual(stat.S_IMODE(occupied.stat().st_mode), mode)
        self.assertEqual(list(occupied.iterdir()), [sentinel])
        record_before = self.record.read_bytes()
        with self.assertRaisesRegex(ValueError, "cannot contain"):
            I.capture(self.envelope(event_id="repository-root"), self.record, self.root, start=False)
        self.assertEqual(self.record.read_bytes(), record_before)

    def test_recovery_after_record_commit_does_not_write_twice(self):
        event = self.capture()
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.record, self.state, event["event_id"], _crash_after_commit=True)
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 4)
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertTrue(receipt["recovered"])
        sources = [k for k in self.read()["sources"] if k.startswith("s.ingest_")]
        self.assertEqual(len(sources), 1)

    def test_recovery_reports_committed_truth_when_source_was_later_tampered(self):
        event = self.capture(self.envelope(event_id="recovery-tamper"))
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.record, self.state, event["event_id"], _crash_after_commit=True)
        source = Path(I.status(event["event_id"], self.record, self.state)["source_file"])
        source.write_text("tampered after commit")
        receipt = I.process(self.record, self.state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertIsNone(receipt["source"])
        self.assertTrue(receipt["record_committed"])
        self.assertEqual(receipt["record_source"], "s.ingest_" + event["event_id"])
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 4)

    def test_pointer_and_hypothesis_context_become_questions(self):
        pointer = self.root / "pointer.yaml"
        pointer.write_text("record: PROVENANCE.yaml\n")
        state = self.root / "pointer-state"
        envelope = self.envelope(event_id="pointer")
        event = I.capture(envelope, pointer, state, start=False)
        receipt = I.process(pointer, state, event["event_id"])[0]
        self.assertEqual(receipt["state"], "needs_primary")
        self.assertIn("pointer", receipt["reason"])
        self.assertEqual(self.read()["known"]["facts.count"]["v"], 3)


if __name__ == "__main__":
    unittest.main()
