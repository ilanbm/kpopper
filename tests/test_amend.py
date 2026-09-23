"""`answer` closes an open question with what answered it; `correct` rewrites an entry only
while nothing landed rests on it. Both run on ordinary and history-backed records, through the
command line as a session runs it, with no network:

    python3 -m unittest tests.test_amend
"""
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
CLI = ROOT / "scripts" / "cli.py"

RECORD = """meta:
  updated: '2026-09-20'
sources:
  src.inspection:
    name: The boiler inspection report
    file: inspection.pdf
    read: '2026-09-20'
known:
  m.boiler_age:
    name: Age of the boiler
    v: 14
    unit: years
    from: src.inspection
    at: p.2
open:
  q.second_boiler: Do we need a second boiler for the annex?
  q.annex_floor: Will the annex add a second floor?
judgments:
  d.one_boiler:
    name: One boiler is enough for now
    rests_on: [m.boiler_age]
    verdict: one boiler serves the annex until the boiler is 20 years old
    because: The inspection puts the boiler at {{m.boiler_age}} years
    wrong_if: "m.boiler_age >= 20"
    seen: {m.boiler_age: 14}
"""
LOAD = ("add", "m.annex_load", "v=40", "unit=kW", "name=Annex heat load", "from=src.inspection", "at=p.3")
VERDICT = "    verdict: one boiler serves the annex until the boiler is 20 years old"


@unittest.skipUnless(os.name == "posix", "record writers require POSIX locks")
class Rewrites(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = pathlib.Path(temporary.name).resolve()
        self.root = self.home / "work"
        self.root.mkdir()
        (self.home / "tmp").mkdir()
        self.env = {k: v for k, v in os.environ.items()
                    if k not in ("KPOPPER_AGENT_SESSION", "CODEX_THREAD_ID", "KPOPPER_READ_MODE")}
        # a private temporary directory holds the session receipts; git looks no higher than
        # the test's own directory, so a record outside a repository is outside git
        self.env.update(TMPDIR=str(self.home / "tmp"), XDG_STATE_HOME=str(self.home / "state"),
                        XDG_CONFIG_HOME=str(self.home / "config"),
                        KPOPPER_PRIVATE_HOME=str(self.home / "private"),
                        GIT_CEILING_DIRECTORIES=str(self.home), KPOPPER_SESSION_DISABLE="1")

    def run_cli(self, *args, session=None):
        env = dict(self.env, **({"KPOPPER_AGENT_SESSION": session} if session else {}))
        return subprocess.run([sys.executable, str(CLI), *args], cwd=self.root, env=env,
                              capture_output=True, text=True, timeout=120)

    def ok(self, *args, session=None):
        done = self.run_cli(*args, session=session)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        return done.stdout

    def refused(self, expect, *args, session=None):
        done = self.run_cli(*args, session=session)
        said = done.stdout + done.stderr
        self.assertNotEqual(done.returncode, 0, "unexpected success: " + said)
        self.assertIn(expect, said)
        return said

    def git(self, *args):
        done = subprocess.run(["git", "-C", str(self.root), *args], env=self.env,
                              capture_output=True, text=True)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)

    def repository(self):
        self.git("init", "-q")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.test")

    def commit(self):
        self.git("add", "-A")
        self.git("-c", "commit.gpgsign=false", "commit", "-qm", "fixture")

    @property
    def path(self):
        return self.root / "GROUNDING.yaml"

    def record(self):
        return self.path.read_text(encoding="utf-8")

    def write(self, text):
        self.path.write_text(text, encoding="utf-8")

    def edit(self, old, new):
        text = self.record()
        self.assertIn(old, text)
        self.write(text.replace(old, new))

    def entry(self, nid):
        document = yaml.safe_load(self.record())
        return next(members[nid] for members in document.values()
                    if isinstance(members, dict) and nid in members)

    def born(self, session=None):
        """A history-backed record born through add, as a new record is."""
        for args in (("add", "src.inspection", "name=The boiler inspection report", "file=inspection.pdf",
                      "read=2026-09-20"),
                     ("add", "m.boiler_age", "v=14", "unit=years", "name=Age of the boiler",
                      "from=src.inspection", "at=p.2"),
                     ("add", "q.second_boiler", "Do we need a second boiler for the annex?"),
                     ("add", "d.one_boiler", "rests_on=[m.boiler_age]",
                      "verdict=one boiler serves the annex until the boiler is 20 years old",
                      "because=The inspection puts the boiler at {{m.boiler_age}} years",
                      "wrong_if=m.boiler_age >= 20", "name=One boiler is enough for now")):
            self.ok(*args, session=session)
        self.assertIn("  history:", self.record())

    def objects(self):
        """Every history object beside the record, read."""
        return [yaml.safe_load(path.read_text(encoding="utf-8"))
                for path in sorted((self.root / ".kpopper" / "history").rglob("*.yaml"))]

    # ── answer ───────────────────────────────────────────────────────────────
    def test_answer_closes_a_question_where_it_stands(self):
        self.write(RECORD)
        self.assertIn("2 open questions", self.ok("open"))
        said = self.ok("answer", "q.second_boiler", "d.one_boiler", "--why", "the inspection settles it")
        self.assertIn("answer q.second_boiler", said)
        text = self.record()
        # it stays in open:, where it was: what was asked, then what closed it under one key
        self.assertIn('open:\n  q.second_boiler:\n    question: "Do we need a second boiler for the annex?"\n'
                      '    answered:\n      by: d.one_boiler\n'
                      '      said: "one boiler serves the annex until the boiler is 20 years old"\n', text)
        self.assertIn('      because: "the inspection settles it"\n', text)
        self.assertIn("  q.annex_floor: Will the annex add a second floor?\n", text)
        question = self.entry("q.second_boiler")
        self.assertEqual(list(question), ["question", "answered"])
        answered = question["answered"]
        self.assertEqual(list(answered), ["by", "said", "of", "because"])
        self.assertEqual(answered["said"], "one boiler serves the annex until the boiler is 20 years old")
        self.assertRegex(answered["of"], r"^\d{4}-\d{2}-\d{2}$")
        self.assertEqual(answered["because"], "the inspection settles it")
        opened = self.ok("open")
        self.assertIn("1 open questions", opened)
        self.assertNotIn("? q.second_boiler", opened)
        self.assertIn("? q.annex_floor", opened)
        pulled = self.ok("pull", "q.second_boiler")
        self.assertIn("q.second_boiler: Do we need a second boiler for the annex? - answered by "
                      "d.one_boiler on " + answered["of"] + " - the insp", pulled)
        self.assertIn("0 problems", self.ok("check"))

    def test_answering_keeps_a_questions_own_fields(self):
        self.write(RECORD.replace("  q.annex_floor: Will the annex add a second floor?\n",
                                  "  q.annex_floor:\n    name: Annex floor\n    v: Will the annex add a second floor?\n"
                                  "    from: src.inspection\n    at: p.9\n"))
        self.ok("answer", "q.annex_floor", "d.one_boiler", "--why", "one floor keeps one boiler")
        text = self.record()
        self.assertIn("    from: src.inspection\n    at: p.9\n    answered:\n      by: d.one_boiler\n", text)
        self.assertIn('      because: "one floor keeps one boiler"\n', text)
        question = self.entry("q.annex_floor")
        self.assertEqual(list(question), ["name", "v", "from", "at", "answered"])
        self.assertEqual((question["name"], question["v"]), ("Annex floor", "Will the annex add a second floor?"))
        # what was asked reads first from question:, then v:, then name:
        self.assertIn("q.annex_floor: Will the annex add a second floor? - answered by d.one_boiler",
                      self.ok("pull", "q.annex_floor"))
        untouched = self.record()
        self.refused("--dropped carries the reason; --why goes with an answer",
                     "answer", "q.second_boiler", "--dropped", "no", "--why", "no")
        self.assertEqual(self.record(), untouched)
        # an entry that is one value answers with that value, and is watched for it
        self.edit("known:\n", "known:\n  m.floors: 1\n")
        self.ok("answer", "q.second_boiler", "m.floors")
        self.assertEqual(self.entry("q.second_boiler")["answered"]["said"], 1)
        self.edit("  m.floors: 1\n", "  m.floors: 2\n")
        self.assertIn("q.second_boiler: its answer moved - m.floors now says 2, it said 1", self.ok("open"))

    def test_a_question_is_dropped_with_its_reason(self):
        self.write(RECORD)
        self.ok("answer", "q.annex_floor", "--dropped", "the extension was cancelled", "--as-of", "2026-09-22")
        text = self.record()
        # a mapping this short is written on one line, as the writer writes every mapping that fits
        self.assertIn('  q.annex_floor:\n    question: "Will the annex add a second floor?"\n'
                      '    dropped: {of: "2026-09-22", because: "the extension was cancelled"}\n', text)
        self.assertEqual(self.entry("q.annex_floor"), {"question": "Will the annex add a second floor?",
                                                        "dropped": {"of": "2026-09-22",
                                                                    "because": "the extension was cancelled"}})
        self.assertNotIn("answered:", text)
        opened = self.ok("open")
        self.assertNotIn("? q.annex_floor", opened)
        self.assertIn("1 open questions", opened)
        self.assertIn("q.annex_floor: Will the annex add a second floor? - dropped on 2026-09-22 - "
                      "the extension was cancelled", self.ok("pull", "q.annex_floor"))

    def test_answer_refuses_what_it_cannot_close(self):
        self.write(RECORD)
        untouched = self.record()
        self.refused("d.one_boiler is not an open question", "answer", "d.one_boiler", "m.boiler_age")
        self.refused("m.missing is not an entry - record the answer first",
                     "answer", "q.second_boiler", "m.missing")
        self.refused("q.annex_floor is a question", "answer", "q.second_boiler", "q.annex_floor")
        self.refused("q.second_boiler is a question", "answer", "q.second_boiler", "q.second_boiler")
        self.refused("--dropped", "answer", "q.second_boiler")
        self.refused("or --dropped", "answer", "q.second_boiler", "d.one_boiler", "--dropped", "cancelled")
        self.refused("--dropped needs the reason", "answer", "q.second_boiler", "--dropped", "  ")
        self.refused("q.missing is not an entry", "answer", "q.missing", "d.one_boiler")
        self.assertEqual(self.record(), untouched)
        self.ok("answer", "q.second_boiler", "d.one_boiler")
        self.refused("already answered", "answer", "q.second_boiler", "m.boiler_age")
        self.ok("answer", "q.annex_floor", "--dropped", "the extension was cancelled")
        self.refused("q.annex_floor is already answered", "answer", "q.annex_floor", "d.one_boiler")
        self.edit("open:\n", "open:\n  q.floors: 2\n")
        self.refused("q.floors is not a question this reader can close", "answer", "q.floors", "d.one_boiler")

    def test_a_moved_answer_is_flagged_for_a_person_and_stays_answered(self):
        self.write(RECORD)
        self.ok("answer", "q.second_boiler", "d.one_boiler")
        self.edit(VERDICT, VERDICT.replace("20 years", "25 years"))
        self.assertIn("q.second_boiler: its answer moved - d.one_boiler now says", self.ok("open"))
        checked = self.ok("check")             # the flag is for a person; it fails nothing
        self.assertIn("NOTE q.second_boiler: its answer moved - d.one_boiler now says", checked)
        self.assertIn("0 problems", checked)
        self.assertIn("    answered:\n      by: d.one_boiler\n", self.record())
        self.edit("    v: 14\n", "    v: 22\n")    # the judgment's own condition now holds
        self.assertIn("q.second_boiler: its answer d.one_boiler is broken by its own condition",
                      self.ok("open"))
        self.edit("    v: 22\n", "    v: 14\n")
        self.edit("  d.one_boiler:", "  d.two_boilers:")
        self.assertIn("q.second_boiler: its answer d.one_boiler is no longer an entry", self.ok("open"))
        self.assertIn("NOTE q.second_boiler: its answer d.one_boiler is no longer an entry", self.ok("check"))

    # ── correct ──────────────────────────────────────────────────────────────
    def test_correct_rewrites_what_no_commit_holds_and_flags_what_rests_on_it(self):
        self.repository()
        self.write(RECORD)
        self.commit()
        self.ok(*LOAD)
        # its condition reads another dependency, so a move of the load is a person's to judge
        self.ok("add", "d.annex_heating", "rests_on=[m.annex_load, m.boiler_age]",
                "verdict=the annex needs its own heating circuit",
                "because=the load is {{m.annex_load}} kW on a boiler {{m.boiler_age}} years old",
                "wrong_if=m.boiler_age >= 30", "name=Annex heating circuit")
        said = self.ok("correct", "m.annex_load", "v=45", "--why", "p.3 says 45")
        self.assertIn("correct m.annex_load", said)
        self.assertIn("d.annex_heating", said)
        self.assertNotIn("nearest existing", said)     # the entry it rewrites is already there
        text = self.record()
        # rewritten where it stands, its fields in their order
        self.assertIn("known:\n  m.annex_load:\n    v: 45\n    unit: kW\n", text)
        self.assertNotIn("    v: 40", text)
        # what rests on it is flagged, never rewritten
        self.assertEqual(self.entry("d.annex_heating")["seen"], {"m.annex_load": 40, "m.boiler_age": 14})
        self.assertIn("MOVED d.annex_heating: m.annex_load differs from its snapshot (40 -> 45)", self.ok("check"))
        untouched = self.record()
        self.refused("m.boiler_age is already in a commit", "correct", "m.boiler_age", "v=15")
        self.assertEqual(self.record(), untouched)
        self.commit()
        said = self.refused("m.annex_load is already in a commit", "correct", "m.annex_load", "v=46")
        self.assertIn("(add m.annex_load ... --hypothesis NAME)", said)
        self.assertEqual(self.record(), untouched)

    def test_correct_refuses_an_unlanded_entry_that_landed_work_rests_on(self):
        self.repository()
        self.write(RECORD)
        self.commit()
        self.ok("add", "m.flue", "v=true", "name=Flue is clear", "from=src.inspection", "at=p.4")
        self.edit("    rests_on: [m.boiler_age]", "    rests_on: [m.boiler_age, m.flue]")
        self.edit("    seen: {m.boiler_age: 14}", "    seen: {m.boiler_age: 14, m.flue: true}")
        untouched = self.record()
        self.refused("d.one_boiler rests on m.flue and is already in a commit", "correct", "m.flue", "v=false")
        self.assertEqual(self.record(), untouched)
        # a landed question answered by it rests on it too
        self.ok("answer", "q.second_boiler", "m.flue")
        self.refused("d.one_boiler and q.second_boiler rest on m.flue and are already in a commit",
                     "correct", "m.flue", "v=false")

    def test_outside_git_only_the_session_that_wrote_it_corrects_it(self):
        self.write(RECORD)
        self.ok(*LOAD, session="session-one")
        self.refused("no session identity", "correct", "m.annex_load", "v=45")
        self.refused("m.annex_load was not written by this session", "correct", "m.annex_load", "v=45",
                     session="session-two")
        self.ok("correct", "m.annex_load", "v=45", session="session-one")
        self.assertEqual(self.entry("m.annex_load")["v"], 45)
        # the correction is this session's writing too
        self.ok("correct", "m.annex_load", "v=46", session="session-one")
        self.assertEqual(self.entry("m.annex_load")["v"], 46)
        self.refused("d.one_boiler and m.boiler_age were not written by this session",
                     "correct", "m.boiler_age", "v=15", session="session-one")

    def test_correct_keeps_what_an_entry_is_and_what_the_tool_writes(self):
        self.repository()
        self.write("meta:\n  updated: '2026-09-20'\n")
        self.commit()
        self.write(RECORD)
        untouched = self.record()
        self.refused("m.boiler_age is not a judgment", "correct", "m.boiler_age", "rests_on=[src.inspection]")
        self.refused("rests on something", "correct", "d.one_boiler", "--unset", "rests_on")
        self.refused("seen is written by this tool", "correct", "d.one_boiler", "seen={m.boiler_age: 15}")
        self.refused("reviewed is written by this tool", "correct", "d.one_boiler", "reviewed=2026-09-21")
        self.refused("born broken", "correct", "d.one_boiler", "wrong_if=m.boiler_age >= 10")
        self.refused("nothing to correct", "correct", "m.boiler_age", "v=14")
        self.refused("m.missing is not an entry", "correct", "m.missing", "v=1")
        self.refused("m.boiler_age has no field color", "correct", "m.boiler_age", "--unset", "color")
        self.refused("fields are written field=value", "correct", "m.boiler_age", "v=15", "years")
        self.refused("m.boiler_age holds fields", "correct", "m.boiler_age", "15")
        self.refused("q.second_boiler is one value", "correct", "q.second_boiler", "question=Why?", "name=Boiler")
        self.refused("is not an option of correct", "correct", "m.boiler_age", "v=15", "--as-of", "2026-09-21")
        self.assertEqual(self.record(), untouched)
        # an entry that is one value takes one value, an equals sign and all
        self.ok("correct", "q.annex_floor", "Will the annex = two floors?")
        self.assertIn('  q.annex_floor: "Will the annex = two floors?"\n', self.record())
        self.ok("correct", "d.one_boiler", "wrong_if=m.boiler_age >= 25")
        judgment = self.entry("d.one_boiler")
        self.assertIn("m.boiler_age >= 25", self.record())
        self.assertEqual(judgment["seen"], {"m.boiler_age": 14})
        self.assertEqual(list(judgment), ["name", "rests_on", "verdict", "because", "wrong_if", "seen"])
        self.ok("correct", "m.boiler_age", "--unset", "unit")
        self.assertNotIn("unit", self.entry("m.boiler_age"))
        # how a question was settled is written by answer, and a correction leaves it
        self.ok("answer", "q.second_boiler", "d.one_boiler")
        self.refused("q.second_boiler records how a question was settled", "correct", "q.second_boiler",
                     "answered={by: m.boiler_age, said: 14}")
        self.ok("correct", "q.annex_floor", "Will the annex add a second or third floor?")
        self.assertIn("  q.annex_floor: \"Will the annex add a second or third floor?\"\n", self.record())
        # a trail this tool left is the judgment's, and a correction keeps it as it stands
        self.edit("    seen: {m.boiler_age: 14}\n",
                  "    seen: {m.boiler_age: 14}\n    replaced: [its wrong_if held on 2026-09-01]\n")
        self.ok("correct", "d.one_boiler", "because=The report puts the boiler at {{m.boiler_age}} years")
        judgment = self.entry("d.one_boiler")
        self.assertEqual(judgment["replaced"], ["its wrong_if held on 2026-09-01"])
        # the snapshot is taken again where it stood, not moved to the end
        self.assertEqual(list(judgment)[-2:], ["seen", "replaced"])

    def test_correct_leaves_a_hypothesis_to_the_fold(self):
        self.write(RECORD)
        self.ok(*LOAD, session="session-one")
        home = self.root / ".kpopper" / "hypotheses"
        home.mkdir(parents=True)
        (home / "annex_load.yaml").write_text(
            "meta:\n  born: '2026-09-21'\nknown:\n  m.annex_load:\n    v: 48\n    unit: kW\n"
            "    from: src.inspection\n    at: p.3\n    of: '2026-09-21'\n", encoding="utf-8")
        untouched = self.record()
        self.refused("hypothesis annex_load holds m.annex_load or rests on it - a correction there is a "
                     "decision for the fold", "correct", "m.annex_load", "v=45", session="session-one")
        self.assertEqual(self.record(), untouched)

    # ── history-backed records ───────────────────────────────────────────────
    def test_history_records_mark_the_corrected_version_and_pin_the_answer(self):
        self.repository()
        self.born()
        said = self.ok("answer", "q.second_boiler", "d.one_boiler", "--why", "the rating settles it")
        self.assertRegex(said, r"history committed: \S+ \(answer q.second_boiler\)")
        heads = yaml.safe_load(self.record())["meta"]["history"]["heads"]
        acts = [o["body"] for o in self.objects() if o.get("kind") == "act" and o["subject"] == "q.second_boiler"]
        pinned = [act for act in acts if act["act"] == "accept" and "read" in act]
        self.assertEqual(len(pinned), 1, acts)
        self.assertEqual(pinned[0]["read"], {"d.one_boiler": heads["d.one_boiler"][0]})
        self.assertEqual(pinned[0]["because"], "the rating settles it")
        self.assertIn("answered", self.ok("--json", "pull", "q.second_boiler"))
        self.assertEqual(self.entry("q.second_boiler")["answered"]["by"], "d.one_boiler")
        old = heads["m.boiler_age"][0]
        said = self.ok("correct", "m.boiler_age", "v=15", "--why", "p.2 says 15")
        self.assertRegex(said, r"history committed: \S+ \(correct m.boiler_age\)")
        corrections = [o["body"] for o in self.objects()
                       if o.get("kind") == "act" and o["subject"] == "m.boiler_age" and o["body"]["act"] == "correct"]
        self.assertEqual([act["over"] for act in corrections], [[old]])
        self.assertIn("m.boiler_age", self.ok("history", "status", "--json"))
        self.assertIn("corrected", self.ok("--json", "pull", "m.boiler_age"))
        self.assertEqual(self.entry("m.boiler_age")["v"], 15)
        self.assertIn("problems", self.ok("check"))
        self.commit()
        self.refused("already in a commit", "correct", "m.boiler_age", "v=16")

    def test_history_records_outside_git_are_corrected_by_the_session_that_wrote_them(self):
        self.born(session="session-one")
        self.refused("no session identity", "correct", "m.boiler_age", "v=15")
        self.refused("d.one_boiler and m.boiler_age were not written by this session",
                     "correct", "m.boiler_age", "v=15", session="session-two")
        said = self.ok("correct", "m.boiler_age", "v=15", session="session-one")
        self.assertRegex(said, r"history committed: \S+ \(correct m.boiler_age\)")
        self.assertEqual(self.entry("m.boiler_age")["v"], 15)

    def test_history_readers_flag_a_moved_answer(self):
        self.repository()
        self.born()
        self.ok("answer", "q.second_boiler", "d.one_boiler")
        acts = [o["body"] for o in self.objects() if o.get("kind") == "act" and o["subject"] == "q.second_boiler"]
        self.assertEqual([act["because"] for act in acts if "read" in act], ["explicit answer"])
        self.ok("add", "q.annex_floor", "Will the annex add a second floor?")
        self.ok("answer", "q.annex_floor", "--dropped", "the extension was cancelled")
        acts = [o["body"] for o in self.objects() if o.get("kind") == "act" and o["subject"] == "q.annex_floor"]
        self.assertIn("the extension was cancelled", [act["because"] for act in acts])
        self.assertEqual(self.entry("q.annex_floor")["dropped"]["because"], "the extension was cancelled")
        self.ok("correct", "d.one_boiler", "verdict=one boiler serves the annex until the boiler is 25 years old")
        self.assertIn("q.second_boiler: its answer moved - d.one_boiler now says", self.ok("open"))
        checked = self.ok("check")
        self.assertIn("NOTE q.second_boiler: its answer moved - d.one_boiler now says", checked)
        self.assertIn("0 problems", checked)


if __name__ == "__main__":
    unittest.main()
