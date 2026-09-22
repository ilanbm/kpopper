"""The first write creates the record; recording reminders never interrupt a reply."""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

import yaml

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "page"


def run(*args, cwd, env=None, stdin=None):
    p = subprocess.run([str(a) for a in args], cwd=str(cwd), input=stdin, capture_output=True,
                       text=True, env=env)
    return p.returncode, p.stdout, p.stderr


class Scratch(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = pathlib.Path(self.tmp.name).resolve() / "project"
        self.dir.mkdir()
        # the hooks' state lives outside the project, as a host's temp directory does
        state = pathlib.Path(self.tmp.name).resolve() / "state"
        state.mkdir()
        self.env = dict(os.environ, TMPDIR=str(state), GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t",
                        GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@t",
                        PATH=str(pathlib.Path(sys.executable).parent) + os.pathsep + os.environ["PATH"],
                        KPOPPER_RUNTIME_HOME=str(state / "no-private-runtime"))

    def git(self, *args, cwd=None):
        code, out, err = run("git", *args, cwd=cwd or self.dir, env=self.env)
        self.assertEqual(code, 0, err)
        return out

    def cli(self, *args, cwd=None):
        return run(sys.executable, SCRIPTS / "provenance.py", *args, cwd=cwd or self.dir, env=self.env)

    def hook(self, name, payload, *args, cwd=None):
        return run("sh", SCRIPTS / name, *args, cwd=cwd or self.dir, env=self.env, stdin=json.dumps(payload))


class FirstWrite(Scratch):
    def test_the_first_add_creates_the_record_and_the_next_extends_it(self):
        code, out, err = self.cli("add", "pricing", "name=Acme's pricing page", "url=https://acme.example/pricing",
                                  "read=2026-08-30", "--as-of", "2026-08-30")
        self.assertEqual(code, 0, err)
        self.assertIn("created", out)
        self.assertIn("born with its first entry", out)
        record = self.dir / "GROUNDING.yaml"
        self.assertTrue(record.exists())
        text = record.read_text(encoding="utf-8")
        document = yaml.safe_load(text)
        self.assertEqual(document["meta"]["updated"], "2026-08-30")
        self.assertEqual(document["meta"]["reasoning"]["profile"], "core/v1")
        self.assertIn("pricing", document["sources"])
        code, out, err = self.cli("add", "acme.seat_price", "v=42", "unit=USD/mo", "from=pricing",
                                  "at=Enterprise tier", "name=seat price")
        self.assertEqual(code, 0, err)
        self.assertNotIn("created", out)
        text = record.read_text(encoding="utf-8")
        self.assertIn("known:", text)
        self.assertIn("  acme.seat_price:", text)
        self.assertEqual(self.cli("check")[0], 0)
        code, out, _ = self.cli("add", "which_tier", "does the enterprise tier include support?")
        self.assertEqual(code, 0, out)
        self.assertIn("open:", record.read_text(encoding="utf-8"))

    def test_in_a_checkout_the_record_is_born_at_the_root(self):
        self.git("init", "-q")
        sub = self.dir / "notes" / "deep"
        sub.mkdir(parents=True)
        code, out, err = self.cli("add", "brief", "name=the brief", "file=notes/brief.md", "read=2026-09-01", cwd=sub)
        self.assertEqual(code, 0, err)
        self.assertTrue((self.dir / "GROUNDING.yaml").exists())
        self.assertFalse((sub / "GROUNDING.yaml").exists())
        self.assertFalse((self.dir / "PROVENANCE.yaml").exists())

    def test_no_other_command_creates_a_record_and_a_dangling_registration_is_refused(self):
        code, _, err = self.cli("set", "acme.seat_price", "43")
        self.assertNotEqual(code, 0)
        self.assertFalse((self.dir / "GROUNDING.yaml").exists())
        self.git("init", "-q")
        (self.dir / ".git" / "kpopper-record").write_text(str(self.dir / "gone.yaml"), encoding="utf-8")
        code, _, err = self.cli("add", "pricing", "name=x", "url=https://x", "read=2026-08-30")
        self.assertNotEqual(code, 0)
        self.assertIn("unavailable", err + _)
        self.assertFalse((self.dir / "GROUNDING.yaml").exists())


class Gate(Scratch):
    def record(self, commit=True):
        for name in ("PROVENANCE.yaml", "PROVENANCE.view.yaml", "PROVENANCE.measure.yaml"):
            shutil.copy(FIXTURE / name, self.dir / name)
        if commit:
            self.git("init", "-q")
            self.git("add", "-A")
            self.git("commit", "-q", "-m", "record")

    def test_file_changes_without_record_writes_never_block_stop(self):
        self.record()
        payload = {"session_id": "n1", "cwd": str(self.dir)}
        self.assertEqual(self.hook("session_open.sh", payload, "--host", "claude")[0], 0)
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0, "nothing happened yet")
        (self.dir / "notes.md").write_text("a finding\n", encoding="utf-8")
        code, _, err = self.hook("session_gate.sh", payload, "--host", "claude")
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0)
        (self.dir / "more.md").write_text("more\n", encoding="utf-8")
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0, "and not again")

    def test_record_writes_keep_attribution_checks(self):
        self.record()
        payload = {"session_id": "n2", "cwd": str(self.dir)}
        _, opened, _ = self.hook("session_open.sh", payload)
        context = next(line.removeprefix("KPOPPER_AGENT_CONTEXT ")
                       for line in opened.splitlines() if line.startswith("KPOPPER_AGENT_CONTEXT "))
        self.env.update(json.loads(context)["environment"])
        (self.dir / "notes.md").write_text("a finding\n", encoding="utf-8")
        code, out, err = self.cli("add", "heat.storm_kw", "v=5", "unit=kW", "name=loss in a storm",
                                  "from=doc.boiler_sheet", "--as-of", "2026-09-04")
        self.assertEqual(code, 0, err)
        code, _, err = self.hook("session_gate.sh", payload)
        self.assertEqual(code, 2)
        self.assertIn("recorded no intent", err, "the record's own reminders come first")
        self.assertNotIn("the record untouched", err)
        payload = {"session_id": "n3", "cwd": str(self.dir)}
        self.hook("session_open.sh", payload)
        (self.dir / "other.md").write_text("x\n", encoding="utf-8")
        code, _, err = self.hook("session_gate.sh", payload)
        self.assertEqual((code, err), (0, ""))

    def test_outside_git_the_prompts_count_and_the_soft_line_rides_the_grounding_hook(self):
        self.record(commit=False)
        payload = {"session_id": "n4", "cwd": str(self.dir)}
        self.hook("session_open.sh", payload, "--host", "codex")
        ground = SCRIPTS / "ground_hook.py"

        def prompt(text):
            code, out, err = run(sys.executable, ground, "codex", "prompt", cwd=self.dir, env=self.env,
                                 stdin=json.dumps({**payload, "hook_event_name": "UserPromptSubmit", "prompt": text}))
            self.assertEqual(code, 0, err)
            self.assertEqual(err, "")
            if not out.strip():
                return ""
            response = json.loads(out)
            self.assertEqual(set(response), {"hookSpecificOutput"})
            specific = response["hookSpecificOutput"]
            self.assertEqual(set(specific), {"hookEventName", "additionalContext"})
            self.assertEqual(specific["hookEventName"], "UserPromptSubmit")
            return specific["additionalContext"]
        for i in range(7):
            self.assertNotIn("the record untouched", prompt("unrelated work, turn %d" % i))
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0, "seven prompts are not real work yet")
        line = prompt("unrelated work, turn 8")
        self.assertIn("8 prompts in, the record untouched", line)
        self.assertIn("$record", line)
        self.assertIn("Complete the user's current request", line)
        self.assertIn("authorized", line)
        self.assertIn("Do not answer or mention this reminder", line)
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0,
                         "not on the turn whose prompt already carried the question")
        self.assertEqual(prompt("לא הבנתי, איזו גרסה שבדקת עבדה יותר טוב?"), "")
        for active in (False, True, False):
            code, out, err = self.hook("session_gate.sh", {**payload, "stop_hook_active": active}, "--host", "codex")
            self.assertEqual((code, out, err), (0, "", ""), "a reminder must not become a new user prompt")
        asked = [i for i in range(10, 30) if "the record untouched" in prompt("unrelated work, turn %d" % i)]
        self.assertEqual(asked, [18, 28], "only prompt context repeats after its cooldown")
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0, "and the gate stays quiet")

    def test_changed_tree_reminder_names_each_host_without_blocking(self):
        self.record()
        for host, skill in (("codex", "$record"), ("claude", "/kpopper:record")):
            with self.subTest(host=host):
                payload = {"session_id": "changed-" + host, "cwd": str(self.dir)}
                self.hook("session_open.sh", payload, "--host", host)
                (self.dir / (host + ".txt")).write_text("a finding\n")
                code, out, err = run(sys.executable, SCRIPTS / "ground_hook.py", host, "prompt",
                    cwd=self.dir, env=self.env, stdin=json.dumps({**payload, "prompt": "Continue the answer"}))
                self.assertEqual((code, err), (0, ""))
                text = json.loads(out)["hookSpecificOutput"]["additionalContext"]
                self.assertIn("1 file of the tree changed", text)
                self.assertIn(skill, text)
                self.assertEqual(self.hook("session_gate.sh", payload, "--host", host), (0, "", ""))


if __name__ == "__main__":
    unittest.main()
