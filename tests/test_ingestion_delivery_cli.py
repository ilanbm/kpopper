"""The primary hands a bound job to a native worker without duplicating hook delivery."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import ingestion as I
import ingestion_delivery as D
import ingestion_hooks as H


@unittest.skipIf(os.name == "nt", "background ingestion requires POSIX locking")
class NativeDeliveryCLI(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / "PROVENANCE.yaml"
        self.state = self.root / "private state"
        self.input = self.root / "report.json"
        self.recipient = "originating-task"
        self.env = dict(os.environ, CODEX_SESSION_ID=self.recipient,
                        CODEX_THREAD_ID="different-child-id")
        self.record.write_text(yaml.safe_dump({
            "sources": {"s.initial": {"name": "Initial report", "read": "2026-09-08"}},
            "known": {"fact.ready": {"v": True, "from": "s.initial", "at": "report", "of": "2026-09-08"}},
            "judgments": {"c.ready": {"rests_on": ["fact.ready"], "seen": {"fact.ready": True},
                                      "verdict": "The checklist permits progress.", "wrong_if": "fact.ready == false"}}
        }, sort_keys=False))
        self.input.write_text(json.dumps({"source_quote": "The checklist is not ready.",
                                          "target": "fact.ready", "value": False,
                                          "date": "2026-09-09", "session_id": "untrusted-other-task"}))

    def cli(self, *args, env=None):
        return subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), "ingest", *args],
                              cwd=self.root, env=env or self.env, text=True, capture_output=True)

    def capture(self, recipient=None):
        return self.cli("capture", "--file", str(self.input), "--record", str(self.record),
                        "--state-dir", str(self.state), "--no-start", "--notify-task",
                        self.recipient if recipient is None else recipient)

    def hook(self, session=None, source=None):
        payload = {"session_id": session or self.recipient,
                   "hook_event_name": "SessionStart" if source else "PostToolUse"}
        if source:
            payload["source"] = source
        return H.handle(payload, "codex", "start" if source else "wait",
                        self.record, self.state, wait_seconds=0)

    def run_printed(self, command, *args):
        result = subprocess.run(shlex.split(command) + list(args), cwd=self.root,
                                env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_host_identity_is_checked_before_any_capture(self):
        before = self.record.read_bytes()
        for recipient in ("untrusted-other-task", ""):
            result = self.capture(recipient)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("must match this host", result.stderr)
        self.assertFalse(self.state.exists())
        self.assertEqual(before, self.record.read_bytes())

    def test_no_host_identity_cannot_reserve_message_delivery(self):
        env = {k: v for k, v in self.env.items() if k not in ("CODEX_THREAD_ID", "CODEX_SESSION_ID")}
        result = self.cli("capture", "--file", str(self.input), "--record", str(self.record),
                          "--state-dir", str(self.state), "--notify-task", self.recipient, env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.state.exists())

    def test_printed_handoff_keeps_one_recipient_and_confirms_delivery_only(self):
        captured = self.capture()
        self.assertEqual(captured.returncode, 0, captured.stderr)
        job = json.loads(captured.stdout)["delivery_job"]
        self.assertTrue(job["dispatch_required"])
        self.assertEqual(job["recipient"], self.recipient)
        I.process(self.record, self.state)
        notice = I.pending(self.record, self.state)[0]
        self.assertEqual(self.hook(), ("", "", 0))
        self.assertIn(notice["id"], self.hook("another-reader")[0])

        attention = self.run_printed(job["wait_command"])
        self.assertEqual(attention["status"], "attention")
        self.assertEqual(attention["recipient"], self.recipient)
        self.assertIn(notice["id"], attention["message"])
        self.assertIn("c.ready", attention["message"])
        self.assertIn(str(self.record), attention["message"])
        self.assertEqual(self.hook(), ("", "", 0))
        before = self.record.read_bytes()
        self.run_printed(job["complete_command"], "--claim-token", attention["claim_token"], "--outcome", "sent")
        self.assertEqual(before, self.record.read_bytes())
        self.assertEqual(self.hook(), ("", "", 0))
        self.assertFalse((self.state / "handled.json").exists())
        self.assertEqual(I.pending(self.record, self.state)[0]["id"], notice["id"])
        self.assertIn(notice["id"], self.hook(source="resume")[0])

    def test_failed_native_send_restores_existing_hook_delivery(self):
        captured = self.capture()
        self.assertEqual(captured.returncode, 0, captured.stderr)
        job = json.loads(captured.stdout)["delivery_job"]
        I.process(self.record, self.state)
        attention = self.run_printed(job["wait_command"])
        self.run_printed(job["complete_command"], "--claim-token", attention["claim_token"], "--outcome", "failed")
        self.assertIn(attention["signal_ids"][0], self.hook()[0])

    def test_bounded_message_contains_every_signal_it_will_confirm(self):
        notices = [{"id": format(i, "032x"), "category": "question", "target": "fact.ready",
                    "affected_judgments": ["c.ready"], "reason": "r" * 900,
                    "source_quote": "q" * 500} for i in range(8)]
        message = D._message(notices, self.record)
        self.assertIn(str(self.record), message)
        for notice in notices:
            self.assertIn(notice["id"], message)
        self.assertLess(len(message), 20000)


if __name__ == "__main__":
    unittest.main()
