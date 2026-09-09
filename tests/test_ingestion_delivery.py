"""Bounded native delivery reserves exactly one sender without consuming findings."""
from concurrent.futures import ThreadPoolExecutor
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest import mock

import yaml


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location(
    "kpopper_ingestion_delivery", ROOT / "scripts" / "ingestion_delivery.py")
D = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(D)
I = D.I
import ingestion_hooks as H


class NativeDelivery(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / "PROVENANCE.yaml"
        self.state = self.root / "private"
        self.recipient = "thread-root-123"
        self.record.write_text(yaml.safe_dump({
            "sources": {"s.initial": {"name": "Initial report", "read": "2026-09-08"}},
            "known": {"facts.ready": {"name": "Reported state", "v": True,
                                        "from": "s.initial", "at": "report", "of": "2026-09-08"}},
            "judgments": {"c.ready": {"verdict": "Ready permits progress.",
                                         "rests_on": ["facts.ready"],
                                         "seen": {"facts.ready": True},
                                         "wrong_if": "facts.ready == false"}},
        }, sort_keys=False))

    def envelope(self, **changes):
        out = {"event_id": "delivery-event", "session_id": "untrusted-attacker-thread",
               "source_quote": "The checklist is not ready.", "target": "facts.ready",
               "value": False, "date": "2026-09-09", "kind": "report"}
        out.update(changes)
        return out

    def capture(self, recipient=None, envelope=None):
        return D.capture(envelope or self.envelope(), recipient or self.recipient,
                         self.record, self.state, start=False)

    def process(self):
        return I.process(self.record, self.state)

    def notices(self):
        return I.pending(self.record, self.state)

    def job(self, capture):
        return I._load(D._job_path(self.state, capture["delivery_job"]["id"]))

    def test_quiet_result_has_no_message(self):
        doc = yaml.safe_load(self.record.read_text())
        doc["known"]["facts.ready"]["v"] = 3
        doc["judgments"]["c.ready"]["seen"]["facts.ready"] = 3
        doc["judgments"]["c.ready"]["wrong_if"] = "facts.ready < 0"
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        captured = self.capture(envelope=self.envelope(value=4, source_quote="There are four."))
        self.process()
        result = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        self.assertEqual(result["status"], "quiet")
        self.assertNotIn("message", result)

    def test_attention_binds_trusted_recipient_and_labels_source_untrusted(self):
        captured = self.capture()
        self.process()
        result = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        notice = self.notices()[0]
        self.assertEqual(result["status"], "attention")
        self.assertEqual(result["recipient"], self.recipient)
        self.assertNotEqual(result["recipient"], self.envelope()["session_id"])
        self.assertEqual(result["signal_ids"], [notice["id"]])
        self.assertIn(notice["id"], result["message"])
        self.assertIn(self.envelope()["source_quote"], result["message"])
        self.assertIn("untrusted data", result["message"])
        self.assertTrue(result["claim_token"])

    def test_recipient_rejects_controls_and_cannot_come_from_envelope(self):
        for value in ("", "  ", "bad\nthread", "bad\x00thread", "x" * 513):
            with self.subTest(value=repr(value)), self.assertRaises(ValueError):
                D.capture(self.envelope(), value, self.record, self.state, start=False)
        captured = self.capture(recipient="trusted-thread")
        self.assertEqual(captured["delivery_job"]["recipient"], "trusted-thread")

    def test_same_job_has_only_one_concurrent_sender(self):
        captured = self.capture()
        self.process()
        job_id = captured["delivery_job"]["id"]
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(
                lambda _: D.wait(job_id, self.record, self.state, timeout=0), range(2)))
        self.assertEqual(sorted(result["status"] for result in results), ["attention", "busy"])
        self.assertEqual(sum("claim_token" in result for result in results), 1)

    def test_failed_unknown_and_expired_jobs_restore_hook_fallback(self):
        captured = self.capture()
        self.process()
        notices = self.notices()
        epoch = captured["delivery_job"]["epoch"]
        sending = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        path = D._job_path(self.state, captured["delivery_job"]["id"])
        before_filter = path.read_bytes()
        self.assertEqual(D.hook_visible(self.state, self.recipient, notices, epoch), [])
        self.assertEqual(path.read_bytes(), before_filter)
        D.complete(captured["delivery_job"]["id"], sending["claim_token"], "failed",
                   self.record, self.state)
        self.assertEqual(D.hook_visible(self.state, self.recipient, notices, epoch), notices)
        retry = self.capture()
        self.assertFalse(retry["delivery_job"]["dispatch_required"])

        other = D.capture(self.envelope(), "other-thread", self.record, self.state, start=False)
        sending = D.wait(other["delivery_job"]["id"], self.record, self.state, timeout=0)
        D.complete(other["delivery_job"]["id"], sending["claim_token"], "unknown",
                   self.record, self.state)
        self.assertEqual(D.hook_visible(self.state, "other-thread", notices,
                                        other["delivery_job"]["epoch"]), [])
        next_epoch = "e" * 32
        I._save(D._session_path(self.state, "other-thread"),
                {"epoch": next_epoch, "offered": []})
        self.assertEqual(D.hook_visible(self.state, "other-thread", notices, next_epoch), notices)

        third = D.capture(self.envelope(), "expired-thread", self.record, self.state, start=False)
        path = D._job_path(self.state, third["delivery_job"]["id"])
        job = I._load(path)
        job["reservation_expires_at"] = time.time() - 1
        I._save(path, job)
        self.assertEqual(D.hook_visible(self.state, "expired-thread", notices,
                                        third["delivery_job"]["epoch"]), notices)

    def test_success_hides_same_epoch_but_new_epoch_reoffers_without_ack(self):
        captured = self.capture()
        self.process()
        notices = self.notices()
        sending = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        done = D.complete(captured["delivery_job"]["id"], sending["claim_token"], "sent",
                          self.record, self.state)
        epoch = captured["delivery_job"]["epoch"]
        self.assertEqual(done["status"], "sent")
        self.assertEqual(D.hook_visible(self.state, self.recipient, notices, epoch), [])
        later = [{**notices[0], "id": "later-signal"}]
        self.assertEqual(D.hook_visible(self.state, self.recipient, later, epoch), later)
        self.assertEqual(self.notices(), notices)
        session_path = D._session_path(self.state, self.recipient)
        session = I._load(session_path)
        self.assertEqual(set(session["offered"]), {notices[0]["id"]})
        new_epoch = "f" * 32
        I._save(session_path, {"epoch": new_epoch, "offered": []})
        self.assertEqual(D.hook_visible(self.state, self.recipient, notices, new_epoch), notices)

    def test_late_native_wait_recognizes_hook_offer_from_a_new_epoch(self):
        captured = self.capture()
        self.process()
        notice = self.notices()[0]
        path = D._job_path(self.state, captured["delivery_job"]["id"])
        job = I._load(path)
        job["reservation_expires_at"] = time.time() - 1
        I._save(path, job)
        payload = {"session_id": self.recipient, "hook_event_name": "SessionStart",
                   "source": "resume"}
        output, error, code = H.handle(payload, "codex", "start", self.record, self.state,
                                       wait_seconds=0)
        self.assertEqual((error, code), ("", 0))
        self.assertIn(notice["id"], output)
        session = I._load(D._session_path(self.state, self.recipient))
        self.assertNotEqual(session["epoch"], captured["delivery_job"]["epoch"])
        late = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        self.assertEqual(late["status"], "sent")
        self.assertNotIn("message", late)
        self.assertEqual(I._load(path)["delivered_epoch"], session["epoch"])

    def test_confirmed_send_after_resume_settles_in_the_current_epoch(self):
        captured = self.capture()
        self.process()
        notice = self.notices()[0]
        attention = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        original_epoch = captured["delivery_job"]["epoch"]
        payload = {"session_id": self.recipient, "hook_event_name": "SessionStart",
                   "source": "resume"}
        self.assertEqual(H.handle(payload, "codex", "start", self.record, self.state,
                                  wait_seconds=0), ("", "", 0))
        current = I._load(D._session_path(self.state, self.recipient))
        self.assertNotEqual(current["epoch"], original_epoch)
        done = D.complete(captured["delivery_job"]["id"], attention["claim_token"], "sent",
                          self.record, self.state)
        self.assertEqual(done["status"], "sent")
        current = I._load(D._session_path(self.state, self.recipient))
        self.assertIn(notice["id"], current["offered"])
        self.assertEqual(I._load(D._job_path(self.state, done["id"]))["delivered_epoch"],
                         current["epoch"])
        compact = {**payload, "source": "compact"}
        self.assertEqual(H.handle(compact, "codex", "start", self.record, self.state,
                                  wait_seconds=0), ("", "", 0))
        later = {**payload, "source": "resume"}
        self.assertIn(notice["id"], H.handle(later, "codex", "start", self.record,
                                             self.state, wait_seconds=0)[0])

    def test_resolved_notice_returns_quiet(self):
        captured = self.capture()
        self.process()
        doc = yaml.safe_load(self.record.read_text())
        doc["known"]["facts.ready"].update(v=True, **{"from": "s.initial", "at": "report",
                                                       "of": "2026-09-10"})
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        self.assertEqual(self.notices(), [])
        result = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        self.assertEqual(result["status"], "quiet")

    def test_terminal_status_rechecks_notices_before_declaring_quiet(self):
        captured = self.capture()
        notice = {"id": "a" * 32, "event_id": captured["event_id"],
                  "category": "question", "target": "facts.ready",
                  "source_quote": "The checklist is not ready.", "reason": "Review readiness."}
        terminal = {"event_id": captured["event_id"], "state": "needs_primary"}
        with mock.patch.object(I, "pending", side_effect=[[], [notice], [notice]]), \
                mock.patch.object(I, "status", return_value=terminal):
            result = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        self.assertEqual(result["status"], "attention")
        self.assertEqual(result["signal_ids"], [notice["id"]])

    def test_terminal_processing_error_releases_fallback_instead_of_claiming_quiet(self):
        captured = self.capture()
        terminal = {"event_id": captured["event_id"], "state": "error"}
        with mock.patch.object(I, "pending", side_effect=[[], []]), \
                mock.patch.object(I, "status", return_value=terminal):
            result = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        self.assertEqual(result["status"], "waiting")
        self.assertIn("processing ended in error", result["reason"])
        job = self.job(captured)
        self.assertEqual(job["status"], "queued")
        self.assertLessEqual(job["reservation_expires_at"], time.time())

    def test_corrupt_jobs_cannot_redirect_and_hooks_fail_open(self):
        captured = self.capture()
        self.process()
        notice = self.notices()[0]
        job_id = captured["delivery_job"]["id"]
        path = D._job_path(self.state, job_id)
        original = I._load(path)
        corruptions = (
            {"id": "0" * 64},
            {"recipient": "redirect-thread"},
            {"event_id": "0" * 32},
            {"status": "dispatching"},
            {"signal_ids": "not-a-list"},
            {"status": "sent", "outcome": "failed"},
            {"delivered_epoch": "not-an-epoch"},
        )
        for change in corruptions:
            with self.subTest(change=change):
                I._save(path, {**original, **change})
                self.assertEqual(D.hook_visible(self.state, self.recipient, [notice],
                                                original["epoch"]), [notice])
                with self.assertRaises(ValueError):
                    D.wait(job_id, self.record, self.state, timeout=0)
        I._save(path, original)
        attention = D.wait(job_id, self.record, self.state, timeout=0)
        claimed = I._load(path)
        I._save(path, {**claimed, "recipient": "redirect-thread"})
        with self.assertRaises(ValueError):
            D.complete(job_id, attention["claim_token"], "sent", self.record, self.state)
        self.assertFalse(D._session_path(self.state, "redirect-thread").exists())

    def test_corrupt_reservation_expiry_fails_open_through_the_hook(self):
        captured = self.capture()
        self.process()
        notice = self.notices()[0]
        path = D._job_path(self.state, captured["delivery_job"]["id"])
        job = I._load(path)
        job["reservation_expires_at"] = "bad"
        I._save(path, job)
        payload = {"session_id": self.recipient, "hook_event_name": "PostToolUse"}
        output, error, code = H.handle(payload, "codex", "wait", self.record, self.state,
                                       wait_seconds=0)
        self.assertEqual((error, code), ("", 0))
        self.assertIn(notice["id"], output)

    def test_corrupt_session_is_validated_before_wait_or_complete_set_operations(self):
        captured = self.capture()
        self.process()
        session_path = D._session_path(self.state, self.recipient)
        valid_epoch = captured["delivery_job"]["epoch"]
        I._save(session_path, {"epoch": valid_epoch, "offered": [{}]})
        waiting = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        self.assertEqual(waiting["status"], "waiting")
        self.assertIn("offered-signal state", waiting["reason"])
        self.assertEqual(self.job(captured)["status"], "queued")

        I._save(session_path, {"epoch": valid_epoch, "offered": []})
        attention = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        I._save(session_path, {"epoch": "invalid", "offered": []})
        with self.assertRaises(ValueError):
            D.complete(captured["delivery_job"]["id"], attention["claim_token"], "sent",
                       self.record, self.state)

    def test_completion_never_changes_record_source_or_global_pending(self):
        captured = self.capture()
        self.process()
        record_before = self.record.read_bytes()
        source = Path(I.status(captured["event_id"], self.record, self.state)["source_file"])
        source_before = source.read_bytes()
        notices = self.notices()
        sending = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        first = D.complete(captured["delivery_job"]["id"], sending["claim_token"], "sent",
                           self.record, self.state)
        again = D.complete(captured["delivery_job"]["id"], sending["claim_token"], "sent",
                           self.record, self.state)
        self.assertEqual(first, again)
        self.assertEqual(self.record.read_bytes(), record_before)
        self.assertEqual(source.read_bytes(), source_before)
        self.assertEqual(self.notices(), notices)

    def test_capture_is_idempotent_and_valid_claim_suppresses_redispatch(self):
        first = self.capture()
        second = self.capture()
        self.assertEqual(first["delivery_job"]["id"], second["delivery_job"]["id"])
        self.assertTrue(first["delivery_job"]["dispatch_required"])
        self.assertTrue(second["delivery_job"]["dispatch_required"])
        self.process()
        sending = D.wait(first["delivery_job"]["id"], self.record, self.state, timeout=0)
        third = self.capture()
        self.assertFalse(third["delivery_job"]["dispatch_required"])
        self.assertEqual(self.job(third)["claim_token"], sending["claim_token"])

    def test_start_happens_after_reservation_and_requests_one_dispatch(self):
        original = I.capture
        calls = []

        def capture_with_observation(envelope, record, state_dir, start):
            calls.append(start)
            result = original(envelope, record, state_dir, start=False)
            if start:
                job_id = D._job_id(self.recipient, result["event_id"])
                self.assertTrue(D._job_path(self.state, job_id).is_file())
            return result

        with mock.patch.object(I, "capture", side_effect=capture_with_observation):
            result = D.capture(self.envelope(), self.recipient, self.record, self.state, start=True)
        self.assertEqual(calls, [False, True])
        self.assertTrue(result["delivery_job"]["dispatch_required"])

    def test_wait_timeout_releases_hook_suppression_and_claim(self):
        captured = self.capture()
        result = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        self.assertEqual(result["status"], "waiting")
        job = self.job(captured)
        self.assertEqual(job["status"], "queued")
        self.assertIsNone(job["claim_token"])
        fake = [{"id": "notice", "event_id": captured["event_id"]}]
        self.assertEqual(D.hook_visible(self.state, self.recipient, fake,
                                        captured["delivery_job"]["epoch"]), fake)

    def test_non_finite_or_boolean_timeout_is_rejected_before_claim(self):
        captured = self.capture()
        for timeout in (float("nan"), float("inf"), float("-inf"), True):
            with self.subTest(timeout=timeout), self.assertRaises(ValueError):
                D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=timeout)
        job = self.job(captured)
        self.assertEqual(job["status"], "queued")
        self.assertIsNone(job["claim_token"])

    def test_complete_requires_live_matching_token_and_consistent_repeat(self):
        captured = self.capture()
        self.process()
        sending = D.wait(captured["delivery_job"]["id"], self.record, self.state, timeout=0)
        with self.assertRaises(ValueError):
            D.complete(captured["delivery_job"]["id"], "wrong", "sent",
                       self.record, self.state)
        D.complete(captured["delivery_job"]["id"], sending["claim_token"], "failed",
                   self.record, self.state)
        with self.assertRaises(ValueError):
            D.complete(captured["delivery_job"]["id"], sending["claim_token"], "sent",
                       self.record, self.state)


if __name__ == "__main__":
    unittest.main()
