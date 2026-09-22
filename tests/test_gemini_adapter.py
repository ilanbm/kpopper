"""Gemini's hook boundary must return JSON, even from a checkout path with spaces."""
import json
import os
from pathlib import Path
import shutil
import shlex
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


@unittest.skipIf(os.name == "nt", "Gemini adapter fixtures require POSIX sh and directory symlinks")
class GeminiHooks(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="kpopper-gemini-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.work = self.root / "Project with spaces"
        self.work.mkdir()
        self.plugin = self.root / "Plugin with spaces"
        self.adapter = self.plugin / "adapters" / "gemini"
        shutil.copytree(ROOT / "adapters" / "gemini", self.adapter)
        (self.plugin / "scripts").symlink_to(ROOT / "scripts", target_is_directory=True)
        self.env = {**os.environ, "XDG_STATE_HOME": str(self.root / "state"),
                    "XDG_CONFIG_HOME": str(self.root / "config"),
                    "KPOPPER_SESSION_DISABLE": "1", "TMPDIR": str(self.root),
                    "KPOPPER_READ_MODE": "frozen"}

    def record(self, broken=False):
        body = '''meta:
  name: Gemini fixture
  scope: Adapter compatibility
sources:
  source.request: {asked: "Check the deadline", name: "Current request"}
known:
  deadline.days: {v: 7, from: source.request, name: "Days remaining"}
'''
        if broken:
            body += '''schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}
judgments:
  decision.ready:
    verdict: "There is time to submit"
    rests_on: [deadline.days]
    seen: {}
    wrong_if: "deadline.days < 1"
'''
        (self.work / "GROUNDING.yaml").write_text(body, encoding="utf-8")

    def hook(self, event, raw=None):
        hooks = json.loads((self.adapter / "hooks" / "hooks.json").read_text())
        command = (hooks["hooks"][event][0]["hooks"][0]["command"] if event in hooks["hooks"]
                   else shlex.join([sys.executable, str(self.adapter / "scripts/hook.py"), event]))
        command = command.replace("${extensionPath}", str(self.adapter))
        payload = raw if raw is not None else json.dumps({
            "cwd": str(self.work), "hook_event_name": event, "source": "startup"})
        result = subprocess.run(["sh", "-c", command], cwd=self.root,
                                input=payload, text=True, capture_output=True,
                                env=self.env, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def test_start_supplies_record_as_additional_context_using_payload_cwd(self):
        self.record()
        output = self.hook("SessionStart")
        specific = output["hookSpecificOutput"]
        self.assertEqual(specific["hookEventName"], "SessionStart")
        self.assertIn("Adapter compatibility", specific["additionalContext"])

    def test_start_without_record_supplies_guidance_without_creating_record(self):
        output = self.hook("SessionStart")
        self.assertIn("KPOPPER_START", output["hookSpecificOutput"]["additionalContext"])
        self.assertFalse((self.work / "GROUNDING.yaml").exists())

    def test_end_is_advisory_even_when_record_check_fails(self):
        self.record(broken=True)
        output = self.hook("SessionEnd")
        self.assertEqual(output, {})
        self.assertNotIn("continue", output)
        self.assertNotIn("decision", output)

    def test_end_without_record_is_empty_json(self):
        self.assertEqual(self.hook("SessionEnd"), {})

    def test_bad_input_still_returns_json(self):
        self.assertEqual(self.hook("SessionStart", raw="not json"), {})

    def test_closed_stdin_still_returns_json(self):
        command = shlex.join([sys.executable, str(self.adapter / "scripts/hook.py"),
                              "SessionStart"]) + " <&-"
        result = subprocess.run(["sh", "-c", command], cwd=self.work, env=self.env,
                                capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {})


if __name__ == "__main__":
    unittest.main()
