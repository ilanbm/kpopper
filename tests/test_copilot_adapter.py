"""Copilot's hook boundary carries context and blocks once using native JSON."""
import json
import os
import shlex
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
HOOK = ROOT / "adapters/copilot/cli/hook.py"


@unittest.skipIf(os.name == "nt", "the stop gate requires a POSIX shell")
class CopilotHooks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.work = self.root / "Project with spaces"
        self.work.mkdir()
        self.env = {**os.environ, "TMPDIR": str(self.root),
                    "XDG_CONFIG_HOME": str(self.root / "config"),
                    "XDG_STATE_HOME": str(self.root / "state"),
                    "KPOPPER_SESSION_DISABLE": "1", "KPOPPER_READ_MODE": "frozen"}
        self.payload = {"sessionId": "copilot-fixture", "cwd": str(self.work),
                        "source": "startup"}

    def call(self, mode, payload=None):
        result = subprocess.run([sys.executable, str(HOOK), mode],
                                input=json.dumps(self.payload if payload is None else payload),
                                cwd=self.root, env=self.env, text=True, capture_output=True,
                                timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def record(self, broken=False):
        text = '''meta: {name: Copilot fixture, scope: Copilot fixture opening}
schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}
sources:
  source.request: {name: Current request}
known:
  deadline.days: {v: 7, from: source.request, name: Days remaining}
'''
        if broken:
            text += '''judgments:
  decision.ready:
    verdict: There is time to submit
    rests_on: [deadline.days]
    seen: {}
    wrong_if: deadline.days < 1
'''
        (self.work / "GROUNDING.yaml").write_text(text, encoding="utf-8")

    def test_context_uses_payload_workspace_and_preserves_session_identity(self):
        self.record()
        output = self.call("start")
        self.assertIn("Copilot fixture", output["additionalContext"])
        self.assertIn('"KPOPPER_AGENT_SESSION": "copilot-fixture"', output["additionalContext"])
        self.assertTrue((self.root / "kpopper-base-copilot-fixture").exists())
        self.assertEqual(self.call("stop"), {})

    def test_legacy_stop_stays_silent_and_checks_still_report_failures(self):
        self.record()
        self.call("start")
        self.record(broken=True)
        result = self.call("stop")
        self.assertEqual(result, {})
        self.assertEqual(self.call("stop", {**self.payload, "stop_hook_active": True}), {})
        # The same finding stays quiet across later turns, independently of the flag.
        self.assertEqual(self.call("stop", {**self.payload, "stop_hook_active": False}), {})
        record = self.work / "GROUNDING.yaml"
        record.write_text(record.read_text(encoding="utf-8") + '''  decision.other:
    verdict: Another unchecked decision
    rests_on: [deadline.days]
    seen: {}
    wrong_if: deadline.days < 1
''', encoding="utf-8")
        new = self.call("stop", {**self.payload, "stop_hook_active": False})
        self.assertEqual(new, {})
        checked = subprocess.run([sys.executable, str(ROOT / "scripts/provenance.py"), "check"],
                                 cwd=self.work, env=self.env, text=True, capture_output=True)
        self.assertNotEqual(checked.returncode, 0)
        self.assertIn("decision.ready", checked.stdout)
        self.assertIn("decision.other", checked.stdout)

    def test_resume_does_not_reset_baseline_after_a_new_failure(self):
        self.record()
        self.call("start")
        self.record(broken=True)
        self.call("start", {**self.payload, "source": "resume"})
        self.assertEqual(self.call("stop"), {})

    def test_first_use_does_not_create_a_record(self):
        self.assertIn("KPOPPER_START", self.call("start")["additionalContext"])
        self.assertEqual(list(self.work.iterdir()), [])

    def test_config_uses_native_events_and_absolute_executable_arguments(self):
        config = self.call("config")
        self.assertEqual(config["version"], 1)
        self.assertEqual(set(config["hooks"]), {"sessionStart"})
        for event, mode in (("sessionStart", "start"),):
            item = config["hooks"][event][0]
            self.assertEqual(item["exec"], sys.executable)
            self.assertEqual(item["args"], [str(HOOK), mode])
            self.assertNotIn("command", item)

    def test_non_object_payload_fails_open(self):
        self.assertEqual(self.call("start", ["invalid"]), {})

    def test_non_ascii_context_ignores_inherited_ascii_output_encoding(self):
        self.record()
        path = self.work / "GROUNDING.yaml"
        path.write_text(path.read_text().replace("Copilot fixture", "תאימות — Copilot"),
                        encoding="utf-8")
        self.env["PYTHONIOENCODING"] = "ascii"
        self.assertIn("תאימות — Copilot", self.call("start")["additionalContext"])

    def test_closed_stdin_fails_open_without_json_crash(self):
        command = shlex.join([sys.executable, str(HOOK), "start"]) + " <&-"
        result = subprocess.run(["sh", "-c", command], cwd=self.work, env=self.env,
                                capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {})


if __name__ == "__main__":
    unittest.main()
