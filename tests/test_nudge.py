"""The first write creates the record; a session that did real work and never wrote is asked
once - by the gate as a stop, by the grounding hook as a line - whether there was nothing to keep."""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

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
                        GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@t")

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
        record = self.dir / "PROVENANCE.yaml"
        self.assertTrue(record.exists())
        text = record.read_text(encoding="utf-8")
        self.assertIn("meta:\n  updated: 2026-08-30", text)
        self.assertIn("sources:", text)
        self.assertIn("  pricing:", text)
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
        self.assertTrue((self.dir / "PROVENANCE.yaml").exists())
        self.assertFalse((sub / "PROVENANCE.yaml").exists())

    def test_no_other_command_creates_a_record_and_a_dangling_registration_is_refused(self):
        code, _, err = self.cli("set", "acme.seat_price", "43")
        self.assertNotEqual(code, 0)
        self.assertFalse((self.dir / "PROVENANCE.yaml").exists())
        self.git("init", "-q")
        (self.dir / ".git" / "kpopper-record").write_text(str(self.dir / "gone.yaml"), encoding="utf-8")
        code, _, err = self.cli("add", "pricing", "name=x", "url=https://x", "read=2026-08-30")
        self.assertNotEqual(code, 0)
        self.assertIn("unavailable", err + _)
        self.assertFalse((self.dir / "PROVENANCE.yaml").exists())


class Gate(Scratch):
    def record(self, commit=True):
        for name in ("PROVENANCE.yaml", "PROVENANCE.view.yaml", "PROVENANCE.measure.yaml"):
            shutil.copy(FIXTURE / name, self.dir / name)
        if commit:
            self.git("init", "-q")
            self.git("add", "-A")
            self.git("commit", "-q", "-m", "record")

    def test_real_work_with_the_record_untouched_is_asked_once_and_names_the_host_skill(self):
        self.record()
        payload = {"session_id": "n1", "cwd": str(self.dir)}
        self.assertEqual(self.hook("session_open.sh", payload, "--host", "claude")[0], 0)
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0, "nothing happened yet")
        (self.dir / "notes.md").write_text("a finding\n", encoding="utf-8")
        code, _, err = self.hook("session_gate.sh", payload, "--host", "claude")
        self.assertEqual(code, 2)
        self.assertIn("1 file of the tree changed, the record untouched", err)
        self.assertIn("/kpopper:record", err)
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0, "asked once")
        (self.dir / "more.md").write_text("more\n", encoding="utf-8")
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0, "and not again")

    def test_a_session_that_wrote_is_not_asked_and_a_plain_host_hears_the_command(self):
        self.record()
        payload = {"session_id": "n2", "cwd": str(self.dir)}
        self.hook("session_open.sh", payload)
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
        self.assertEqual(code, 2)
        self.assertIn("`kpopper add`", err)

    def test_outside_git_the_prompts_count_and_the_soft_line_rides_the_grounding_hook(self):
        self.record(commit=False)
        payload = {"session_id": "n4", "cwd": str(self.dir)}
        self.hook("session_open.sh", payload, "--host", "claude")
        ground = SCRIPTS / "ground_hook.py"

        def prompt(text):
            code, out, err = run(sys.executable, ground, "claude", "prompt", cwd=self.dir, env=self.env,
                                 stdin=json.dumps({**payload, "hook_event_name": "UserPromptSubmit", "prompt": text}))
            self.assertEqual(code, 0, err)
            return json.loads(out)["hookSpecificOutput"]["additionalContext"] if out.strip() else ""
        for i in range(7):
            self.assertNotIn("the record untouched", prompt("unrelated work, turn %d" % i))
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0, "seven prompts are not real work yet")
        line = prompt("unrelated work, turn 8")
        self.assertIn("8 prompts in, the record untouched", line)
        self.assertIn("/kpopper:record", line)
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0,
                         "not on the turn whose prompt already carried the question")
        self.assertEqual(prompt("unrelated work, turn 9"), "", "the soft line waits out its cooldown")
        code, _, err = self.hook("session_gate.sh", payload, "--host", "claude")
        self.assertEqual(code, 2, "a line that went unanswered for a turn earns one stop")
        self.assertIn("prompts in, the record untouched", err)
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0, "once")
        asked = [i for i in range(10, 30) if "the record untouched" in prompt("unrelated work, turn %d" % i)]
        self.assertEqual(asked, [19, 29], "the line returns every ten prompts while nothing is written")
        self.assertEqual(self.hook("session_gate.sh", payload, "--host", "claude")[0], 0, "and the gate stays quiet")


if __name__ == "__main__":
    unittest.main()
