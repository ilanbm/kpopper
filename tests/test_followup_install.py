"""The plugin installer must reach a verified host operation, not stop at a plan."""
import copy
import datetime as dt
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from scripts import followups as F, followup_install as I, followup_daily as D, onboarding as O

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipIf(os.name == "nt", "Persistent followups require POSIX locking")
class Install(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        self.work = self.root / "work"
        self.work.mkdir()
        (self.work / "PROVENANCE.yaml").write_text("known:\n  facts.count: {v: 1}\njudgments:\n  c.ok:\n    rests_on: [facts.count]\n    seen: {facts.count: 1}\n    wrong_if: facts.count > 3\n    verdict: OK\n")
        environment = patch.dict(os.environ, {"XDG_STATE_HOME": str(self.root / "state")})
        environment.start()
        self.addCleanup(environment.stop)
        self.clock = dt.datetime(2026, 9, 10, 12, tzinfo=dt.timezone.utc)
        self.store = F.Store(self.work, now=lambda: self.clock)

    def begin(self, **kwargs):
        return I.begin(self.store, owner="host-session", timezone="Asia/Jerusalem", private=True, **kwargs)

    def inventory(self, schedules=None, **changes):
        result = {"host": "fake-host", "observed_at": F.stamp(self.clock), "complete": True,
                  "schedules": schedules or [], "evidence": "Inspected complete fake scheduler inventory"}
        return {**result, **changes}

    def schedule(self, packet, **changes):
        result = {"id": "schedule-1", "state": "active", "workspace_key": self.store.location["key"],
                  "cadence": "daily", "time": "09:00", "timezone": "Asia/Jerusalem", "access_verified": True,
                  "prompt": packet["desired"]["prompt"]}
        return {**result, **changes}

    def receipt(self, schedule, **changes):
        return {"host": "fake-host", "observed_at": F.stamp(self.clock), "schedule": schedule,
                "evidence": "Read back from fake host after the operation", **changes}

    def test_command_initializes_then_creates_once_and_reads_back(self):
        packet = self.begin()
        self.assertEqual(packet["state"], "needs_host")
        self.assertIsNone(self.store.load()["daily"]["binding"])
        token = packet["installation"]["token"]
        action = I.inspect_host(self.store, token, self.inventory())
        self.assertEqual(action["action"], "create")
        created = self.schedule(packet)  # A fake host's create + independent read.
        receipt = self.receipt(created)
        result = I.finish(self.store, token, receipt)
        self.assertEqual(result["state"], "installed")
        self.assertFalse(result["first_run_verified"])
        self.assertEqual(I.finish(self.store, token, receipt)["state"], "already_recorded")
        second = self.begin()
        reused = I.inspect_host(self.store, second["installation"]["token"], self.inventory([created]))
        self.assertEqual(reused["state"], "already_installed")
        self.assertEqual(reused["action"], "none")

    def test_check_only_never_initializes_or_reserves(self):
        self.assertEqual(I.begin(self.store, check=True)["state"], "not_configured")
        self.assertFalse(self.store.root.exists())
        self.begin()
        before = self.store.path.read_bytes()
        self.assertIsNotNone(I.begin(self.store, check=True)["installation"])
        self.assertEqual(self.store.path.read_bytes(), before)

    def test_missing_timezone_and_record_are_reported_before_installation(self):
        with self.assertRaisesRegex(F.Refused, "timezone"):
            I.begin(self.store, owner="session")
        self.assertFalse(self.store.root.exists())
        (self.work / "PROVENANCE.yaml").unlink()
        with self.assertRaisesRegex(F.Refused, "record"):
            self.begin()

    def test_existing_unbound_schedule_is_adopted_and_time_preserved(self):
        packet = self.begin()
        existing = self.schedule(packet, time="17:30")
        result = I.inspect_host(self.store, packet["installation"]["token"], self.inventory([existing]))
        self.assertEqual(result["state"], "already_installed")
        self.assertEqual(result["binding"]["time"], "17:30")

    def test_requested_time_updates_existing_id_and_needs_matching_readback(self):
        packet = self.begin(time="10:30")
        token = packet["installation"]["token"]
        existing = self.schedule(packet)
        action = I.inspect_host(self.store, token, self.inventory([existing]))
        self.assertEqual((action["action"], action["id"], action["time"]), ("update", "schedule-1", "10:30"))
        with self.assertRaisesRegex(F.Refused, "readback"):
            I.finish(self.store, token, self.receipt(existing))
        self.assertEqual(I.finish(self.store, token, self.receipt({**existing, "time": "10:30"}))["state"], "installed")

    def test_pause_is_preserved_until_explicit_resume(self):
        packet = self.begin()
        paused = self.schedule(packet, state="paused")
        result = I.inspect_host(self.store, packet["installation"]["token"], self.inventory([paused]))
        self.assertEqual(result["state"], "paused")
        again = self.begin(resume=True)
        action = I.inspect_host(self.store, again["installation"]["token"], self.inventory([paused]))
        self.assertEqual(action["action"], "update")
        self.assertEqual(action["id"], paused["id"])
        self.assertEqual(action["schedule_state"], "active")

    def test_incomplete_ambiguous_and_wrong_workspace_inventory_cannot_create(self):
        packet = self.begin()
        token = packet["installation"]["token"]
        self.assertEqual(I.inspect_host(self.store, token, self.inventory(complete=False))["state"], "blocked")
        packet = self.begin()
        candidate = self.schedule(packet)
        result = I.inspect_host(self.store, packet["installation"]["token"], self.inventory([candidate, {**candidate, "id": "other"}]))
        self.assertEqual(result["action"], "none")
        packet = self.begin()
        for changes in ({"workspace_key": "another"}, {"prompt": "Unrelated task"}, {"access_verified": False}, {"cadence": "unknown"}):
            with self.subTest(changes=changes), self.assertRaises(F.Refused):
                I.inspect_host(self.store, packet["installation"]["token"], self.inventory([self.schedule(packet, **changes)]))

    def test_two_agents_cannot_issue_duplicate_creations(self):
        first = self.begin()
        second = I.begin(self.store, owner="other")
        with self.assertRaisesRegex(F.Refused, "token"):
            I.inspect_host(self.store, first["installation"]["token"], self.inventory())
        I.inspect_host(self.store, second["installation"]["token"], self.inventory())
        self.assertEqual(self.begin()["state"], "needs_reconciliation")

    def test_uncertain_creation_is_reconciled_without_repeat(self):
        packet = self.begin()
        token = packet["installation"]["token"]
        I.inspect_host(self.store, token, self.inventory())
        I.fail(self.store, token, "Fake host response lost after sending create")
        result = I.inspect_host(self.store, token, self.inventory())
        self.assertEqual((result["state"], result["action"]), ("needs_reconciliation", "none"))
        found = I.inspect_host(self.store, token, self.inventory([self.schedule(packet)]))
        self.assertEqual(found["state"], "already_installed")

    def test_failed_host_readback_never_claims_success(self):
        packet = self.begin()
        token = packet["installation"]["token"]
        I.inspect_host(self.store, token, self.inventory())
        for changes in ({"state": "paused"}, {"timezone": "UTC"}, {"prompt": "Wrong prompt"}, {"time": "12:00"}, {"cadence": "weekly"}):
            with self.subTest(changes=changes), self.assertRaises(F.Refused):
                I.finish(self.store, token, self.receipt(self.schedule(packet, **changes)))
        self.assertIsNone(self.store.load()["daily"]["binding"])

    def test_stale_inspection_is_rejected(self):
        packet = self.begin()
        report = self.inventory()
        self.clock += dt.timedelta(minutes=11)
        with self.assertRaisesRegex(F.Refused, "again"):
            I.inspect_host(self.store, packet["installation"]["token"], report)

    def test_weekly_review_is_repaired_on_the_same_id(self):
        packet = self.begin()
        result = I.inspect_host(self.store, packet["installation"]["token"], self.inventory([self.schedule(packet, cadence="weekly")]))
        self.assertEqual((result["action"], result["id"], result["cadence"]), ("update", "schedule-1", "daily"))

    def test_another_bound_host_cannot_be_silently_replaced(self):
        packet = self.begin()
        D.binding(self.store, {"host": "other-host", "id": "old", "state": "paused", "evidence": "Read existing host"})
        with self.assertRaisesRegex(F.Refused, "bound host"):
            I.inspect_host(self.store, packet["installation"]["token"], self.inventory())

    def test_cli_installer_returns_action_and_exposes_help(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), "--workspace", str(self.work),
                                 "followups", "daily", "install", "--owner", "cli", "--private", "--timezone", "Asia/Jerusalem"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        packet = json.loads(result.stdout)
        self.assertEqual(packet["action"], "inspect")
        self.assertIn("KPOPPER_DAILY_WORKSPACE=", packet["desired"]["prompt"])

    def test_private_destination_and_scheduled_runtime_are_ready(self):
        packet = self.begin()
        self.assertTrue(Path(self.store.load()["config"]["store"]).is_dir())
        payload = json.loads(packet["desired"]["prompt"].splitlines()[-1])
        environment = {key: value for key, value in os.environ.items() if key != "XDG_STATE_HOME"}
        environment.update(payload["runtime"]["environment"])
        command = payload["runtime"]["command"] + ["--workspace", payload["workspace"], "followups", "daily", "start", "--owner", "scheduled-probe"]
        result = subprocess.run(command, env=environment, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["packet"]["ledger"], str(self.store.path))

    def test_existing_missing_task_store_is_not_recreated(self):
        destination = self.root / "existing-tasks"
        destination.mkdir()
        self.store.setup(str(destination), "Asia/Jerusalem")
        destination.rmdir()
        with self.assertRaisesRegex(F.Refused, "destination"):
            I.begin(self.store, owner="installer")
        self.assertFalse(destination.exists())

    def test_definite_no_change_failure_allows_a_fresh_attempt(self):
        packet = self.begin()
        token = packet["installation"]["token"]
        I.inspect_host(self.store, token, self.inventory())
        self.assertEqual(I.fail(self.store, token, "Host permission denial: no schedule created", unchanged=True)["state"], "blocked")
        next_packet = self.begin()
        self.assertNotEqual(next_packet["installation"]["token"], token)
        self.assertEqual(I.inspect_host(self.store, next_packet["installation"]["token"], self.inventory())["action"], "create")

    def test_uncertain_operation_cannot_be_cleared_as_unchanged(self):
        packet = self.begin()
        token = packet["installation"]["token"]
        I.inspect_host(self.store, token, self.inventory())
        I.fail(self.store, token, "Timed out")
        with self.assertRaises(F.Refused):
            I.fail(self.store, token, "Empty inventory", unchanged=True)

    def test_reconciliation_retires_unknown_request_after_host_resolution(self):
        packet = self.begin(time="10:30")
        token = packet["installation"]["token"]
        I.inspect_host(self.store, token, self.inventory())
        I.fail(self.store, token, "Lost response")
        with self.assertRaises(F.Refused):
            I.reconcile(self.store, token, {**self.inventory(), "no_pending_request": False})
        result = I.reconcile(self.store, token, {**self.inventory(), "no_pending_request": True, "evidence": "Host request audit confirms no outstanding request and no schedule"})
        self.assertEqual(result["requested_options"]["time"], "10:30")
        next_packet = self.begin(time="10:30")
        self.assertNotEqual(next_packet["installation"]["token"], token)
        self.assertEqual(I.inspect_host(self.store, next_packet["installation"]["token"], self.inventory())["action"], "create")
        self.assertEqual(self.store.load()["daily"]["installation_history"][-1]["state"], "reconciled")

    def test_reconciliation_survives_record_relocation(self):
        packet = self.begin()
        token = packet["installation"]["token"]
        I.inspect_host(self.store, token, self.inventory())
        other = self.work / "moved.yaml"
        other.write_bytes((self.work / "PROVENANCE.yaml").read_bytes())
        self.store.relocate(str(other), "Moved the record")
        with self.assertRaisesRegex(F.Refused, "configuration"):
            I.finish(self.store, token, self.receipt(self.schedule(packet)))
        I.reconcile(self.store, token, {**self.inventory(), "no_pending_request": True, "evidence": "Host confirms original request ended without a schedule"})
        self.assertEqual(self.begin()["record"], str(other))

    def test_user_prompt_additions_are_preserved_during_schedule_update(self):
        packet = self.begin(time="10:30")
        customized = self.schedule(packet, prompt=packet["desired"]["prompt"] + "\nAlso include the user's existing report checklist.")
        token = packet["installation"]["token"]
        action = I.inspect_host(self.store, token, self.inventory([customized]))
        self.assertEqual(action["prompt"], customized["prompt"])
        I.finish(self.store, token, self.receipt({**customized, "time": "10:30"}))
        self.assertFalse(self.store.load()["daily"]["binding"]["managed_prompt"])

    def test_unrecognized_custom_prompt_is_not_overwritten(self):
        packet = self.begin()
        custom = self.schedule(packet, prompt=I.marker(self.store) + "\nA separately customized shared review")
        result = I.inspect_host(self.store, packet["installation"]["token"], self.inventory([custom]))
        self.assertEqual((result["state"], result["action"]), ("needs_review", "none"))

    def test_uncertain_resume_is_not_reported_as_an_intentional_pause(self):
        packet = self.begin(resume=True)
        token = packet["installation"]["token"]
        paused = self.schedule(packet, state="paused")
        I.inspect_host(self.store, token, self.inventory([paused]))
        I.fail(self.store, token, "Lost resume response")
        result = I.inspect_host(self.store, token, self.inventory([paused]))
        self.assertEqual(result["state"], "needs_reconciliation")
        self.assertNotIn("Preserved", json.dumps(result))
        self.assertEqual(self.store.load()["daily"]["installation"]["state"], "uncertain")
        I.reconcile(self.store, token, {**self.inventory([paused]), "no_pending_request": True, "evidence": "Host says request ended and schedule remains paused"})
        again = self.begin(resume=True)
        self.assertEqual(I.inspect_host(self.store, again["installation"]["token"], self.inventory([paused]))["schedule_state"], "active")

    def test_only_unmodified_managed_prompts_can_be_automatically_upgraded(self):
        packet = self.begin()
        token = packet["installation"]["token"]
        I.inspect_host(self.store, token, self.inventory())
        original = self.schedule(packet)
        I.finish(self.store, token, self.receipt(original))
        D.binding(self.store, {"host": "fake-host", "id": original["id"], "state": "active", "evidence": "Fresh host state read"})
        self.assertTrue(self.store.load()["daily"]["binding"]["managed_prompt"])
        original_plan = D.plan
        def upgraded(store):
            return {**original_plan(store), "prompt": original_plan(store)["prompt"] + "\nUpdated managed instructions."}
        with patch.object(D, "plan", side_effect=upgraded):
            next_packet = self.begin()
            action = I.inspect_host(self.store, next_packet["installation"]["token"], self.inventory([original]))
        self.assertEqual(action["action"], "update")
        self.assertTrue(action["prompt"].endswith("Updated managed instructions."))

    def test_recommendation_skips_active_schedules_and_closed_work(self):
        self.begin()
        self.store.add({"id": "later", "title": "Check", "why": "Deferred", "how": "Read", "scope": "Read only", "related": ["facts.count"], "when": {"at": "2099-01-01"}})
        self.assertIn("strongly recommend", O.context(self.store.location))
        D.binding(self.store, {"host": "host", "id": "id", "state": "active", "evidence": "Read back"})
        self.assertNotIn("strongly recommend", O.context(self.store.location))
        D.binding(self.store, {"host": "host", "id": "id", "state": "missing", "evidence": "Confirmed missing"})
        self.assertIn("strongly recommend", O.context(self.store.location))
        self.store.resolve("later", "done", "Completed elsewhere")
        self.assertNotIn("strongly recommend", O.context(self.store.location))


if __name__ == "__main__":
    unittest.main()
