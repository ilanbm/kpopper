"""The grounding line: the entries a prompt touches, named until read, silent otherwise."""
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
sys.path.insert(0, str(SCRIPTS))
import ground_hook as G  # noqa: E402

UNRELATED = [
    "hello",
    "thanks, that looks right",
    "rename the release script and bump the version",
    "fix the failing unit test in the parser module",
    "what time is the meeting tomorrow morning",
    "translate this paragraph into French please",
]


class Fixture(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = pathlib.Path(self.tmp.name).resolve()
        for name in ("PROVENANCE.yaml", "PROVENANCE.view.yaml", "PROVENANCE.measure.yaml"):
            shutil.copy(FIXTURE / name, self.dir / name)
        self.env = dict(os.environ, TMPDIR=str(self.dir))

    def hook(self, script, *args, **payload):
        payload.setdefault("session_id", "g1")
        payload.setdefault("cwd", str(self.dir))
        p = subprocess.run([sys.executable, str(SCRIPTS / script), *args], cwd=str(self.dir),
                           input=json.dumps(payload), capture_output=True, text=True, env=self.env)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertNotIn("Traceback", p.stderr)
        if not p.stdout.strip():
            return ""
        return json.loads(p.stdout)["hookSpecificOutput"]["additionalContext"]

    def prompt(self, text, host="claude"):
        return self.hook("ground_hook.py", host, "prompt", hook_event_name="UserPromptSubmit", prompt=text)


class Words(unittest.TestCase):
    def test_function_words_drop_and_hebrew_loses_one_prefix_letter(self):
        self.assertEqual(G.words("what does the boiler put out"), {"boiler"})
        self.assertEqual(G.words("המשכנתא והריבית"), {"המשכנתא", "משכנתא", "והריבית", "הריבית"})

    def test_a_prompt_names_the_entries_it_touches_best_first(self):
        index = G.entries(str(FIXTURE / "PROVENANCE.yaml"))
        found = [k for _, k, _ in G.hits("the heat loss on the coldest night versus the boiler output", index)]
        self.assertIn("heat.loss_kw", found)
        self.assertIn("heat.boiler_kw", found)
        self.assertEqual([k for _, k, _ in G.hits("the heat loss on a -5 night", index)][0], "heat.loss_kw")
        self.assertEqual(G.hits("hello", index), [])
        self.assertEqual(G.hits("what does c.boiler_short rest on", index)[0][1], "c.boiler_short")


class Line(Fixture):
    def test_named_on_a_matching_prompt_and_silent_on_the_rest(self):
        line = self.prompt("the heat loss on a -5 night")
        self.assertIn("kpopper: the record holds", line)
        self.assertIn("heat.loss_kw", line)
        self.assertIn("/kpopper:ground", line)
        self.assertNotIn("kW", line)               # a pointer, never a value
        for text in UNRELATED:
            with self.subTest(prompt=text):
                self.assertEqual(self.prompt(text), "")

    def test_the_host_names_its_own_skill_and_a_plain_host_the_reader(self):
        self.assertIn("$ground", self.prompt("the boiler output on the coldest night", "codex"))
        self.hook("ground_hook.py", "claude", "start", hook_event_name="SessionStart", source="startup")
        self.assertIn("kpop pull", self.prompt("the boiler output on the coldest night", "plain"))

    def test_named_until_read_then_silent_until_its_body_changes(self):
        ask = "the heat loss on a -5 night"
        first = self.prompt(ask)
        self.assertIn("heat.loss_kw", first)
        self.assertEqual(self.prompt(ask), "", "within the cooldown an unread entry is not repeated")
        for _ in range(G.COOLDOWN):
            self.prompt("something unrelated entirely")
        self.assertIn("heat.loss_kw", self.prompt(ask), "after the cooldown an unread entry is named again")
        # read: the ids pass through a tool's output
        self.hook("ground_hook.py", "claude", "read", hook_event_name="PostToolUse", tool_name="Bash",
                  tool_response={"stdout": "heat.loss_kw: 9.5 (heat loss on a -5°C night)\nheat.boiler_kw: 7"})
        for _ in range(G.COOLDOWN + 1):
            self.prompt("something unrelated entirely")
        self.assertNotIn("heat.loss_kw", self.prompt(ask), "a read entry is not named again")
        # its body changes: the session saw an older reading
        subprocess.run([sys.executable, str(SCRIPTS / "provenance.py"), "set", "heat.loss_kw", "11",
                        "--as-of", "2026-09-05", "PROVENANCE.yaml"], cwd=str(self.dir), capture_output=True)
        self.assertIn("heat.loss_kw", self.prompt(ask), "a changed body is unread again")

    def test_compaction_forgets_what_was_named_and_read(self):
        ask = "the heat loss on a -5 night"
        self.assertIn("heat.loss_kw", self.prompt(ask))
        self.hook("ground_hook.py", "claude", "start", hook_event_name="SessionStart", source="compact")
        self.assertIn("heat.loss_kw", self.prompt(ask))

    def test_a_subagent_and_a_workspace_without_a_record_get_nothing(self):
        self.assertEqual(self.hook("ground_hook.py", "claude", "prompt", prompt="the boiler output", agent_id="sub"), "")
        os.remove(self.dir / "PROVENANCE.yaml")
        self.assertEqual(self.prompt("the boiler output on the coldest night"), "")


class Citation(Fixture):
    def edit(self, file, tool="Edit", **more):
        return self.hook("edit_hook.py", "claude", hook_event_name="PreToolUse", tool_name=tool,
                         tool_input={"file_path": file}, **more)

    def test_a_cited_file_is_said_once_with_the_entries_that_rest_on_it(self):
        line = self.edit("boiler/service-2025.pdf")
        self.assertIn("doc.boiler_sheet", line)
        self.assertIn("/kpopper:ground doc.boiler_sheet", line)
        self.assertEqual(self.edit("boiler/service-2025.pdf"), "", "said once per file")
        self.assertEqual(self.edit("notes/unrelated.md"), "")
        self.assertEqual(self.edit("PROVENANCE.yaml"), "", "the record itself is the writer's business")
        self.assertEqual(self.edit("boiler/service-2025.pdf", tool="Read", session_id="g2"), "")

    def test_an_absolute_path_inside_the_project_resolves_the_same(self):
        self.assertIn("doc.boiler_sheet", self.edit(str(self.dir / "boiler" / "service-2025.pdf"), session_id="g3"))


class Wiring(unittest.TestCase):
    def test_the_plugin_registers_every_leg_on_both_hosts(self):
        claude = json.load(open(ROOT / "hooks" / "hooks.json", encoding="utf-8"))["hooks"]
        text = json.dumps(claude)
        for leg in ("ground_hook.py\\\" claude start", "ground_hook.py\\\" claude prompt", "ground_hook.py\\\" claude read",
                    "edit_hook.py\\\" claude", "session_open.sh\\\" --host claude", "session_gate.sh\\\" --host claude"):
            self.assertIn(leg, text)
        # the read leg listens to every tool: a shell, a file read, the checked-session tools
        read = [h for h in claude["PostToolUse"] if "ground_hook.py" in json.dumps(h)]
        self.assertEqual(len(read), 1)
        self.assertNotIn("matcher", read[0])
        self.assertTrue(any("Edit" in h.get("matcher", "") for h in claude["PreToolUse"]))
        for name in ("plugin-hooks.json", "hooks.json"):
            codex = json.dumps(json.load(open(ROOT / "adapters" / "codex" / name, encoding="utf-8")))
            for leg in ("ground_hook.py\\\" codex start", "ground_hook.py\\\" codex prompt", "ground_hook.py\\\" codex read",
                        "session_open.sh\\\" --host codex", "session_gate.sh\\\" --host codex"):
                self.assertIn(leg, codex, name)

if __name__ == "__main__":
    unittest.main()
