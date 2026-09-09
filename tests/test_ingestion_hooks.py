"""Host delivery is selective and never turns its own notification into new input."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

import yaml

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location("kpopper_ingestion_hooks", ROOT / "scripts/ingestion_hooks.py")
H = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(H)
I = H.I


@unittest.skipIf(os.name == "nt", "background ingestion requires POSIX locking")
class IngestionHooks(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / "PROVENANCE.yaml"
        self.state = self.root / "private"
        self.payload = {"session_id": "session-one", "hook_event_name": "PostToolUse"}

    def prepare(self, fired=False):
        initial = True if fired else 3
        doc = {"sources": {"s.initial": {"name": "Initial report", "read": "2026-09-08"}},
               "known": {"facts.ready": {"name": "Reported state", "v": initial, "from": "s.initial", "at": "report", "of": "2026-09-08"}},
               "judgments": {"c.ready": {"verdict": "The reported state permits progress.", "rests_on": ["facts.ready"], "seen": {"facts.ready": initial},
                                          "wrong_if": "facts.ready == false" if fired else "facts.ready < 0"}}}
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False))
        self.envelope = {"source_quote": "The checklist is not ready." if fired else "There are four completed items.",
                         "target": "facts.ready", "value": False if fired else 4, "date": "2026-09-09", "kind": "report"}
        return I.capture(self.envelope, self.record, self.state, start=False)

    def handle(self, host, mode="wait", payload=None):
        return H.handle(payload or self.payload, host, mode, self.record, self.state, wait_seconds=0)

    def test_absent_queue_is_silent_and_does_not_create_storage(self):
        self.assertEqual(self.handle("codex"), ("", "", 0))
        self.assertEqual(self.handle("claude", "start"), ("", "", 0))
        self.assertFalse(self.state.exists())

    def test_muted_dependency_is_silent_for_both_hosts(self):
        self.prepare(); I.process(self.record, self.state)
        self.assertEqual(self.handle("codex"), ("", "", 0))
        self.assertEqual(self.handle("claude"), ("", "", 0))

    def test_codex_context_and_claude_rewake_carry_the_same_important_finding(self):
        self.prepare(True); I.process(self.record, self.state)
        notice = I.pending(self.record, self.state)[0]
        out, err, code = self.handle("codex")
        self.assertEqual((err, code), ("", 0))
        self.assertEqual(json.loads(out)["hookSpecificOutput"]["hookEventName"], "PostToolUse")
        self.assertIn(notice["id"], out)
        out, err, code = self.handle("claude")
        self.assertEqual((out, code), ("", 2))
        self.assertIn(notice["id"], err)
        self.assertIn("contradiction", err)

    def test_task_notification_never_recaptures_or_rewakes_itself(self):
        self.prepare(True); I.process(self.record, self.state)
        self.assertEqual(self.handle("claude")[2], 2)
        before = I.status(record=self.record, state_dir=self.state)
        notification = {**self.payload, "hook_event_name": "UserPromptSubmit",
                        "prompt": "<task-notification>" + self.envelope["source_quote"] + "</task-notification>"}
        with mock.patch.object(I, "capture", side_effect=AssertionError("notification cannot capture")):
            self.assertEqual(self.handle("claude", payload=notification), ("", "", 0))
        self.assertEqual(I.status(record=self.record, state_dir=self.state), before)

    def test_compaction_dedupes_but_resume_reoffers_unresolved(self):
        self.prepare(True); I.process(self.record, self.state)
        start = {**self.payload, "hook_event_name": "SessionStart", "source": "startup"}
        self.assertIn("KPOPPER_ATTENTION", self.handle("codex", "start", start)[0])
        self.assertEqual(self.handle("codex", "start", {**start, "source": "compact"}), ("", "", 0))
        self.assertIn("KPOPPER_ATTENTION", self.handle("codex", "start", {**start, "source": "resume"})[0])
        self.assertEqual(len(I.pending(self.record, self.state)), 1)

    def test_capped_batch_marks_only_notices_actually_in_the_output(self):
        self.prepare()
        notices = [{"id": "notice-%s" % i, "event_id": "e", "category": "question", "reason": "Review %s" % i} for i in range(9)]
        with mock.patch.object(I, "pending", return_value=notices):
            first = self.handle("claude")[1]
            second = self.handle("claude")[1]
        self.assertIn("notice-7", first)
        self.assertNotIn("notice-8", first)
        self.assertIn("notice-8", second)
        self.assertNotIn("notice-0", second)


class PlatformConfiguration(unittest.TestCase):
    def test_codex_manifest_selects_only_codex_hooks(self):
        manifest = json.loads((ROOT / ".codex-plugin/plugin.json").read_text())
        self.assertEqual(manifest["hooks"], "./adapters/codex/plugin-hooks.json")
        config = json.loads((ROOT / manifest["hooks"]).read_text())["hooks"]
        for event in ("PostToolUse", "UserPromptSubmit"):
            hook = config[event][0]["hooks"][0]
            self.assertTrue(hook["async"])
            self.assertNotIn("asyncRewake", hook)
            self.assertIn("codex wait", hook["command"])

    def test_claude_has_rewake_and_both_configs_keep_existing_open_and_stop(self):
        for path, host in ((ROOT / "hooks/hooks.json", "claude"), (ROOT / "adapters/codex/plugin-hooks.json", "codex")):
            config = json.loads(path.read_text())["hooks"]
            self.assertIn("session_open.sh", config["SessionStart"][0]["hooks"][0]["command"])
            self.assertIn("session_gate.sh", config["Stop"][0]["hooks"][0]["command"])
            if host == "claude":
                self.assertTrue(config["PostToolUse"][0]["hooks"][0]["asyncRewake"])


if __name__ == "__main__":
    unittest.main()
