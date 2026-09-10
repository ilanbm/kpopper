"""Recovery and host boundaries for the followup coordinator."""
import datetime as dt
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from scripts import followups as F, followups_hook as H, onboarding as O, followup_daily as D

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipIf(os.name == "nt", "Persistent followup writes require POSIX locking")
class Integration(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.work = self.root / "work"
        self.work.mkdir()
        self.record = self.work / "PROVENANCE.yaml"
        self.record.write_text("known:\n  facts.count: {v: 1}\njudgments:\n  c.ok:\n    rests_on: [facts.count]\n    seen: {facts.count: 1}\n    wrong_if: facts.count > 3\n    verdict: OK\n")
        self.now = dt.datetime.now(dt.timezone.utc)
        environment = patch.dict(os.environ, {"XDG_STATE_HOME": str(self.root / "state")})
        environment.start()
        self.addCleanup(environment.stop)
        self.store = F.Store(self.work, now=lambda: self.now)
        self.store.setup(timezone="Asia/Jerusalem")
        self.spec = {"id": "read", "title": "Read the new report", "why": "Inform a decision",
                     "how": "Read the source", "related": ["facts.count"], "when": {"at": "2020-01-01"},
                     "scope": "Read and report"}

    def test_backup_restore_parks_uncertain_work_and_preserves_corruption(self):
        self.store.add(self.spec)
        row = self.store.scan()["items"][0]
        self.store.claim("read", row["occurrence"], "test")
        backup = self.store.root / "followups.previous.yaml"
        self.assertTrue(backup.is_file())
        self.store.path.write_bytes(b"broken: [")
        with self.assertRaises(F.Refused):
            self.store.scan()
        result = self.store.restore(backup, "Inspected last valid backup; effects need reconciliation")
        self.assertEqual(Path(result["quarantine"]).read_bytes(), b"broken: [")
        self.assertEqual(self.store.scan()["items"][0]["state"], "needs_user")
        with self.assertRaises(F.Refused):
            self.store.claim("read", row["occurrence"], "another")

    def test_restore_live_claim_marks_it_interrupted(self):
        self.store.add(self.spec)
        row = self.store.scan()["items"][0]
        claim = self.store.claim("read", row["occurrence"], "test")["claim"]
        saved = self.root / "snapshot.yaml"
        saved.write_bytes(self.store.path.read_bytes())
        self.store.restore(saved, "Reconciled backup age; prior work uncertain")
        self.assertEqual(self.store.scan()["items"][0]["state"], "interrupted")
        with self.assertRaises(F.Refused):
            self.store.finish("read", claim["token"], "done", "Late worker")

    def test_external_completion_unblocks_prerequisite_without_claiming_owner_work(self):
        self.store.add({**self.spec, "executor": "external:existing-routine"})
        self.store.add({**self.spec, "id": "next", "when": {"completed": "read"}})
        self.store.resolve("read", "done", "Existing owner returned completion receipt")
        rows = {row["id"]: row for row in self.store.scan()["items"]}
        self.assertEqual(rows["next"]["state"], "ready")
        self.assertEqual(rows["read"]["state"], "done")

    def test_hook_deduplicates_unchanged_attention_and_never_claims(self):
        self.store.add(self.spec)
        before = self.record.read_bytes()
        payload = {"session_id": "session", "cwd": str(self.work)}
        self.assertIn("KPOPPER_FOLLOWUPS", H.handle(payload))
        ledger = self.store.path.read_bytes()
        self.assertEqual(H.handle(payload), "")
        self.assertEqual(self.store.path.read_bytes(), ledger)
        self.assertEqual(self.record.read_bytes(), before)
        self.assertIsNone(self.store.load()["items"]["read"]["claim"])
        self.assertEqual(H.handle({**payload, "agent_id": "child"}), "")

    def test_opening_counts_only_and_event_context_bounded(self):
        self.store.add({**self.spec, "title": "private title " * 400, "scope": "scope " * 1000})
        opening = F.summary(self.store.location, counts_only=True)
        self.assertNotIn("private title", opening)
        self.assertLess(len(F.summary(self.store.location)), 4000)
        result = subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), "--workspace", str(self.work), "open", "--json"],
                                text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("counts", json.loads(result.stdout)["followups"])

    def test_recommendation_is_per_workspace_and_respects_guidance(self):
        self.store.add(self.spec)
        before = O.status(self.store.location)["pending_tips"]
        args = [sys.executable, str(ROOT / "scripts/cli.py"), "--workspace", str(self.work), "_agent", "shown", "followups"]
        self.assertEqual(subprocess.run(args, capture_output=True).returncode, 0)
        self.assertEqual(O.status(self.store.location)["pending_tips"], before)
        self.assertNotIn("strongly recommend", O.context(self.store.location))
        other = self.root / "other"
        other.mkdir()
        location = F.W.locate(other)
        self.assertNotIn("strongly recommend", O.context(location))
        (other / "PROVENANCE.yaml").write_bytes(self.record.read_bytes())
        second = F.Store(other)
        second.setup()
        second.add(self.spec)
        location = F.W.locate(other)
        self.assertIn("strongly recommend", O.context(location))
        O._write(O.state_dir() / "guidance.json", {"enabled": False})
        self.assertNotIn("strongly recommend", O.context(location))

    def test_new_external_event_can_reopen_before_next_scheduled_check(self):
        self.store.observe({"ref": "release", "value": "pending", "observed_at": F.stamp(self.now), "evidence": "Release API"})
        self.store.add({**self.spec, "when": {"any": [{"at": "2020-01-01"}, {"external": {"ref": "release", "equals": "ready"}}]}})
        row = self.store.scan()["items"][0]
        claim = self.store.claim("read", row["occurrence"], "test")["claim"]
        self.store.finish("read", claim["token"], "checked", "Await release", F.stamp(self.now + dt.timedelta(days=1)))
        self.assertEqual(self.store.scan()["items"][0]["state"], "waiting")
        self.now += dt.timedelta(minutes=1)
        self.store.observe({"ref": "release", "value": "ready", "observed_at": F.stamp(self.now), "evidence": "Release published"})
        self.assertEqual(self.store.scan()["items"][0]["state"], "ready")

    def test_prerequisite_diamond_walk_is_memoized_and_long_chains_are_iterative(self):
        data = self.store.load()
        for index in range(50):
            children = [{"completed": "f" + str(before)} for before in (index - 1, index - 2) if before >= 0]
            data["items"]["f" + str(index)] = {"spec": {"when": {"all": children} if children else {"at": "2020-01-01"}}}
        candidate = {**self.spec, "when": {"completed": "f49"}}
        with patch.object(F.T, "referenced_tasks", wraps=F.T.referenced_tasks) as references:
            self.store._validate(candidate, data, "read")
            self.assertLess(references.call_count, 60)
        for index in range(50, 1200):
            data["items"]["f" + str(index)] = {"spec": {"when": {"completed": "f" + str(index - 1)}}}
        self.store._validate({**self.spec, "when": {"completed": "f1199"}}, data, "read")

    def test_delivery_does_not_overwrite_the_last_product_backup(self):
        self.store.add(self.spec)
        payload = {"cwd": str(self.work), "session_id": "backup-test"}
        H.handle(payload)
        row = self.store.scan()["items"][0]
        claim = self.store.claim("read", row["occurrence"], "test")["claim"]
        self.store.finish("read", claim["token"], "done", "Completed")
        backup = (self.store.root / "followups.previous.yaml").read_bytes()
        H.handle(payload)
        self.assertEqual((self.store.root / "followups.previous.yaml").read_bytes(), backup)

    def test_resume_restored_work_preserves_retry_baseline_and_generation(self):
        self.store.add({**self.spec, "when": {"changed": "facts.count"}})
        with self.store.transaction() as data:
            item = data["items"]["read"]
            item["next_at"] = F.stamp(self.now + dt.timedelta(days=2))
        snapshot = self.root / "resume-backup.yaml"
        snapshot.write_bytes(self.store.path.read_bytes())
        before = self.store.load()["items"]["read"]
        self.store.restore(snapshot, "Inspected backup and reconciled canonical work")
        self.store.resume("read", "No external effects are outstanding")
        after = self.store.load()["items"]["read"]
        for key in ("next_at", "baseline", "baseline_events", "generation"):
            self.assertEqual(after[key], before[key])
        self.record.write_text(self.record.read_text().replace("{v: 1}", "{v: 2}"))
        self.assertEqual(self.store.scan()["items"][0]["state"], "ready")

    def test_daily_same_value_observation_refresh_is_quiet(self):
        report = {"ref": "merge", "value": "pending", "observed_at": F.stamp(self.now), "evidence": "Read API"}
        self.store.observe(report)
        self.store.add({**self.spec, "when": {"external": {"ref": "merge", "equals": "merged"}}})
        first = D.start(self.store, "day-one")
        D.finish(self.store, first["claim"]["token"], "Still waiting")
        self.now += dt.timedelta(days=1)
        self.store.observe({**report, "observed_at": F.stamp(self.now), "evidence": "Same state from new read"})
        self.assertFalse(D.start(self.store, "day-two")["new_attention"])

    def test_daily_schedule_can_be_replaced_only_after_fresh_deletion_evidence(self):
        D.binding(self.store, {"host": "host", "id": "first", "state": "active", "evidence": "Read back"})
        replacement = {"host": "host", "id": "second", "state": "active", "evidence": "Created and read back"}
        with self.assertRaises(F.Refused):
            D.binding(self.store, replacement)
        D.binding(self.store, {"host": "host", "id": "first", "state": "missing", "evidence": "Confirmed deletion"})
        D.binding(self.store, replacement)
        self.assertEqual(D.status(self.store)["binding"]["id"], "second")
        self.assertEqual(self.store.load()["daily"]["binding_history"][0]["id"], "first")

    def test_observation_payloads_are_bounded_without_changing_state(self):
        before = self.store.path.read_bytes()
        with self.assertRaisesRegex(F.Refused, "64 KiB"):
            self.store.observe({"ref": "large", "value": "x" * 65537, "observed_at": F.stamp(self.now), "evidence": "API"})
        self.assertEqual(self.store.path.read_bytes(), before)

    def test_fractional_seconds_and_complete_claim_scope(self):
        self.assertEqual(F.T.parse_time("2026-09-10T12:00:00.5+03:00").microsecond, 500000)
        scope = "Read and report. " * 100 + "Do not contact anyone."
        self.store.add({**self.spec, "scope": scope})
        row = self.store.scan()["items"][0]
        claim = self.store.claim("read", row["occurrence"], "test")
        self.assertEqual(claim["spec"]["scope"], scope)


if __name__ == "__main__":
    unittest.main()
