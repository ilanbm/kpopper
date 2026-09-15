"""A verdict laid over a standing judgment is a reversal: folded when the base's own
condition has broken the judgment, or when a person names the id, and red until one does -
in one tree and across the branch line alike. Runs against the consolidate fixture in a
throwaway git repository, no browser, no network:

    python3 -m unittest tests.test_reversals
"""
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "consolidate"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402

VERDICT = 'verdict: "the old boiler cannot hold 12°C on the coldest February night"'
FLIPPED = 'verdict: "the old boiler holds after all"'
CONDITION = 'wrong_if: "heat.loss_kw <= heat.boiler_kw"'
REGROUNDED = 'wrong_if: "heat.loss_kw <= 30"'
OPPOSITE = ("rests_on=[heat.boiler_kw, heat.loss_kw]", "verdict=the old boiler holds on the coldest night",
            "wrong_if=heat.loss_kw > heat.boiler_kw", "--drop", "heat.deficit_kw: worked out from the two")


def run(*args, cwd=None):
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def kp(*args):
    return run(SCRIPTS / "kpopper", *args)


def git(d, *args):
    p = subprocess.run(["git", "-c", "user.email=t@example.test", "-c", "user.name=t",
                        "-c", "commit.gpgsign=false"] + list(args), cwd=str(d), capture_output=True, text=True)
    if p.returncode:
        raise AssertionError(" ".join(args) + "\n" + p.stdout + p.stderr)
    return p.stdout


def repo(d):
    """The fixture base, committed on main, with no hypotheses beside it."""
    rec = pathlib.Path(d) / "PROVENANCE.yaml"
    shutil.copy(FIXTURE / "PROVENANCE.yaml", rec)
    shutil.copy(FIXTURE / "PROVENANCE.view.yaml", pathlib.Path(d) / "PROVENANCE.view.yaml")
    git(d, "init", "-q", "-b", "main")
    git(d, "add", "-A")
    git(d, "commit", "-qm", "base")
    return rec


def edit(rec, old, new):
    text = rec.read_text(encoding="utf-8")
    assert text.count(old) == 1, old
    rec.write_text(text.replace(old, new), encoding="utf-8")


def branch(d, rec, name, edits=(), commands=()):
    """A branch off the current commit that edits the record by hand, or through the
    reader, and commits; the checkout returns to main."""
    git(d, "switch", "-qc", name)
    for old, new in edits:
        edit(rec, old, new)
    for args in commands:
        code, out, err = run(SCRIPTS / "provenance.py", *args, rec)
        assert code == 0, out + err
    git(d, "add", "-A")
    git(d, "commit", "-qm", name)
    git(d, "switch", "-q", "main")


class AcrossTheBranchLine(unittest.TestCase):

    def test_a_verdict_flipped_on_a_branch_without_evidence_waits_for_a_name(self):
        with tempfile.TemporaryDirectory() as d:
            rec = repo(d)
            branch(d, rec, "flip", edits=[(VERDICT, FLIPPED)])
            before = rec.read_text(encoding="utf-8")
            code, out, err = kp("consolidate", "--dry-run", "--from", "flip", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("reversed (1): a verdict, or other grounds, laid over a standing judgment - by its "
                          "own condition, by a person's name, or waiting for one\n"
                          "  c.boiler_short: the old boiler cannot hold 12°C on … -> the old boiler holds "
                          "after all, from flip\n"
                          "    the standing judgment holds, and its wrong_if has not fired - take it by "
                          "name: consolidate flip --take c.boiler_short\n", out)
            self.assertIn("    base: because: It gives 24 kW against a loss of 31 kW", out)
            self.assertIn("    base: rests_on: [heat.boiler_kw, heat.loss_kw, heat.deficit_kw]\n"
                          "    base: wrong_if: heat.loss_kw <= heat.boiler_kw\n"
                          "    flip: because: It gives 24 kW against a loss of 31 kW", out)
            self.assertIn("    flip: rests_on: [heat.boiler_kw, heat.loss_kw, heat.deficit_kw]\n"
                          "    flip: wrong_if: heat.loss_kw <= heat.boiler_kw\n", out)
            self.assertNotIn("\nclean", out)
            self.assertIn("not clean: 1 reversal to take by name", out)
            code, out, err = kp("consolidate", "--from", "flip", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("refused - a verdict the base's own condition has not broken folds only when a "
                          "person names it: consolidate flip --take c.boiler_short", out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            # named, it folds - and the trail says a person took it
            code, out, err = kp("consolidate", "--from", "flip", "--take", "c.boiler_short",
                                "--as-of", "2026-09-05", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("    taken by name - the standing judgment holds, and a person takes this over it "
                          "by name\n", out)
            self.assertIn("replace c.boiler_short with what flip holds, where it stands - what it replaced "
                          "kept\n  kept: the replaced verdict, because, in PROVENANCE.replaced.yaml\n", out)
            self.assertIn("files to commit: PROVENANCE.yaml, PROVENANCE.replaced.yaml\n"
                          "  nothing to delete for flip: another branch keeps its own record\n"
                          "next: git add PROVENANCE.yaml PROVENANCE.replaced.yaml && git commit\n"
                          "  then merge flip as you would - its record is folded here, and the merge "
                          "carries only its code\n", out)
            body = P.bodies(P.load([str(rec)]))["c.boiler_short"]
            self.assertEqual(body["verdict"], "the old boiler holds after all")
            self.assertEqual(body["replaced"], ["the standing judgment holds, and a person takes this over "
                                                "it by name on 2026-09-05"])
            v = P.read_replaced([str(rec)])["c.boiler_short"][0]
            self.assertEqual(v["verdict"], "the old boiler cannot hold 12°C on the coldest February night")
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_a_branch_that_brings_the_reading_and_the_verdict_has_not_fired_the_base(self):
        with tempfile.TemporaryDirectory() as d:
            rec = repo(d)
            base = git(d, "rev-parse", "HEAD").strip()
            branch(d, rec, "contra-a", commands=[
                ["set", "heat.loss_kw", "20", "--as-of", "2026-09-04", "--why", "read again"],
                ["add", "c.boiler_short", *OPPOSITE, "--as-of", "2026-09-04"]])
            code, out, err = kp("consolidate", "--dry-run", "--from", "contra-a", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("updates (1): what the base holds that a hypothesis replaces, and what rests on each\n"
                          "  heat.loss_kw: 31 -> 20, from contra-a\n"
                          "    a reading from 2026-09-04 that is newer than the base's\n", out)
            self.assertIn("    the standing judgment holds, and its wrong_if has not fired - it would hold "
                          "with contra-a's readings (heat.loss_kw <= heat.boiler_kw) - take it by name: "
                          "consolidate contra-a --take c.boiler_short\n", out)
            code, out, err = kp("consolidate", "--from", "contra-a", "--take", "c.boiler_short",
                                "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            git(d, "add", "-A")
            git(d, "commit", "-qm", "fold contra-a")
            # the other branch kept the verdict and read something else: over the base that
            # took contra-a, its old reading is contested and its verdict is a reversal that
            # its own readings would fire - both said, nothing folded
            git(d, "switch", "-qc", "contra-b", base)
            for args in (["add", "heat.job", "v=failing", "name=the brief job", "from=s.2026_09_02_heating",
                          "at=the log", "of=2026-09-04"],
                         ["review", "c.boiler_short", "--as-of", "2026-09-04"]):
                code, out, err = run(SCRIPTS / "provenance.py", *args, rec)
                self.assertEqual(code, 0, out + err)
            git(d, "add", "-A")
            git(d, "commit", "-qm", "contra-b")
            git(d, "switch", "-q", "main")
            code, out, err = kp("consolidate", "--dry-run", "--from", "contra-b", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("arrived (1): what the fold would add\n  heat.job: failing (the brief job)", out)
            self.assertIn("  c.boiler_short: the old boiler holds on the coldest… -> the old boiler cannot "
                          "hold 12°C on …, from contra-b\n"
                          "    the standing judgment holds, and its wrong_if has not fired - it would hold "
                          "with contra-b's readings (heat.loss_kw > heat.boiler_kw) - take it by name: "
                          "consolidate contra-b --take c.boiler_short\n", out)
            self.assertIn("contested (1): the door refuses the reading, so the base keeps what it holds\n"
                          "  heat.loss_kw: a reading from 2026-09-02 that is older than the base's\n", out)
            self.assertIn("not clean: a contested reading, 1 reversal to take by name", out)

    def test_the_same_verdict_on_other_grounds_travels_as_a_reversal(self):
        with tempfile.TemporaryDirectory() as d:
            rec = repo(d)
            branch(d, rec, "regrounded", edits=[(CONDITION, REGROUNDED)])
            code, out, err = kp("consolidate", "--dry-run", "--from", "regrounded", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("  c.boiler_short: the same verdict on other grounds, from regrounded\n"
                          "    the standing judgment holds, and its wrong_if has not fired - take it by "
                          "name: consolidate regrounded --take c.boiler_short\n", out)
            self.assertIn("    base: wrong_if: heat.loss_kw <= heat.boiler_kw\n", out)
            self.assertIn("    regrounded: wrong_if: heat.loss_kw <= 30\n", out)
            code, out, err = kp("consolidate", "--from", "regrounded", "--take", "c.boiler_short",
                                "--as-of", "2026-09-05", rec)
            self.assertEqual(code, 0, out + err)
            body = P.bodies(P.load([str(rec)]))["c.boiler_short"]
            self.assertEqual(P.predicate_text(body["wrong_if"]), "heat.loss_kw <= 30")
            self.assertEqual(P.read_replaced([str(rec)])["c.boiler_short"][0]["wrong_if"],
                             "heat.loss_kw <= heat.boiler_kw")

    def test_a_review_does_not_travel(self):
        with tempfile.TemporaryDirectory() as d:
            rec = repo(d)
            branch(d, rec, "reviewed", commands=[["review", "c.boiler_short", "--as-of", "2026-09-04"]])
            code, out, err = kp("consolidate", "--dry-run", "--from", "reviewed", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("reversed (0)", out)
            self.assertIn("clean - and nothing to write", out)

    def test_a_branch_folds_onto_a_committed_base_only(self):
        with tempfile.TemporaryDirectory() as d:
            rec = repo(d)
            branch(d, rec, "flip", edits=[(VERDICT, FLIPPED)])
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.note", "v=1", "name=a note",
                                 "from=doc.boiler_sheet", "at=its margin", "of=2026-09-05", rec)
            self.assertEqual(code, 0, out + err)
            code, out, err = kp("consolidate", "--from", "flip", "--take", "c.boiler_short", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("refused - PROVENANCE.yaml carries uncommitted changes: a branch's record folds "
                          "onto a committed base, so the fold is a commit of its own - commit first, then "
                          "consolidate again", out + err)
            self.assertNotIn("reversed (", out)
            self.assertIn("  heat.note:\n", rec.read_text(encoding="utf-8"))
            # a local hypothesis folds on a dirty record: it is this checkout's own work
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "28",
                                 "--as-of", "2026-09-06", "--hypothesis", "recount", rec)
            self.assertEqual(code, 0, out + err)
            code, out, err = kp("consolidate", "recount", "--as-of", "2026-09-06", rec)
            self.assertEqual(code, 0, out + err)

    def test_take_names_only_what_a_hypothesis_lays_over_a_judgment(self):
        with tempfile.TemporaryDirectory() as d:
            rec = repo(d)
            branch(d, rec, "flip", edits=[(VERDICT, FLIPPED)])
            code, out, err = kp("consolidate", "--from", "flip", "--take", "heat.loss_kw", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("refused - --take names heat.loss_kw, which no hypothesis here lays over a "
                          "standing judgment", out + err)


class InOneTree(unittest.TestCase):

    def test_the_same_verdict_on_other_grounds_is_refused_into_a_hypothesis(self):
        with tempfile.TemporaryDirectory() as d:
            rec = pathlib.Path(d) / "PROVENANCE.yaml"
            shutil.copytree(FIXTURE, d, dirs_exist_ok=True)
            before = rec.read_text(encoding="utf-8")
            args = ["add", "c.boiler_short", "rests_on=[heat.boiler_kw, heat.loss_kw, heat.deficit_kw]",
                    "verdict=the old boiler cannot hold 12°C on the coldest February night",
                    "because=the wind alone takes the margin", "wrong_if=heat.loss_kw <= heat.boiler_kw",
                    "--as-of", "2026-09-04"]
            code, out, err = run(SCRIPTS / "provenance.py", *args, rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("c.boiler_short is already a judgment concluding the same, on other grounds - the "
                          "standing judgment holds, and its wrong_if has not fired, so the same verdict on "
                          "other grounds is a decision written again, and a hypothesis holds it until a "
                          "person takes it: add c.boiler_short", out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            name = (out + err).strip().rsplit("--hypothesis ", 1)[1].strip()
            code, out, err = run(SCRIPTS / "provenance.py", *args, "--hypothesis", name, rec)
            self.assertEqual(code, 0, out + err)
            code, out, err = run(SCRIPTS / "consolidate.py", "--dry-run", name, rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn(f"  c.boiler_short: the same verdict on other grounds, from {name}\n", out)
            code, out, err = run(SCRIPTS / "consolidate.py", name, "--take", "c.boiler_short",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            body = P.bodies(P.load([str(rec)]))["c.boiler_short"]
            self.assertEqual(body["because"], "the wind alone takes the margin")
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_once_the_condition_fired_the_same_verdict_on_other_grounds_replaces_in_place(self):
        with tempfile.TemporaryDirectory() as d:
            rec = pathlib.Path(d) / "PROVENANCE.yaml"
            shutil.copytree(FIXTURE, d, dirs_exist_ok=True)
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", "2026-09-04", rec)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short",
                                 "rests_on=[heat.boiler_kw, heat.loss_kw, heat.deficit_kw]",
                                 "verdict=the old boiler cannot hold 12°C on the coldest February night",
                                 "because=the wind alone takes the margin", "wrong_if=heat.loss_kw <= 10",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("supersede c.boiler_short: the same verdict on other grounds - its wrong_if holds "
                          "(heat.loss_kw <= heat.boiler_kw)\n"
                          "  kept: the replaced verdict, because, in PROVENANCE.replaced.yaml (version 1)\n", out)


class TheHelp(unittest.TestCase):
    def test_the_help_names_take_and_says_a_review_does_not_travel(self):
        code, out, err = kp("consolidate", "--help")
        self.assertEqual(code, 0, out + err)
        self.assertIn("[--take <id>]", out)
        self.assertIn("reversed", out)
        self.assertIn("does not travel", out)
        self.assertIn("folds onto a committed base only", out)


if __name__ == "__main__":
    unittest.main()


class AReadingFromAnotherSource(unittest.TestCase):
    """Two sources for one id that disagree are two instruments: no day orders them across
    the branch line, and a person names which the id follows."""

    def test_a_branch_reading_from_another_source_waits_for_a_name(self):
        with tempfile.TemporaryDirectory() as d:
            rec = repo(d)
            branch(d, rec, "meter", commands=[
                ["add", "doc.meter", "name=the heat meter", "url=https://example.test/meter",
                 "read=2026-09-04"],
                ["set", "heat.loss_kw", "29", "--source", "doc.meter", "--at", "the night of the 3rd",
                 "--as-of", "2026-09-04"]])
            code, out, err = kp("consolidate", "--dry-run", "--from", "meter", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("  heat.loss_kw: 31 -> 29, from meter\n"
                          "    a reading from doc.meter where the base reads from s.2026_09_02_heating - one "
                          "id follows one source; take it by name: consolidate meter --take heat.loss_kw - "
                          "the base keeps what it holds\n", out)
            self.assertIn("contested (1): the door refuses the reading, so the base keeps what it holds\n"
                          "  heat.loss_kw: a reading from doc.meter where the base reads from "
                          "s.2026_09_02_heating - one id follows one source; take it by name: consolidate "
                          "meter --take heat.loss_kw\n", out)
            self.assertNotIn("read again on a later day", out)
            code, out, err = kp("consolidate", "--from", "meter", "--take", "heat.loss_kw",
                                "--as-of", "2026-09-05", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("    a reading from doc.meter where the base reads from s.2026_09_02_heating - "
                          "taken by name\n", out)
            body = P.bodies(P.load([str(rec)]))["heat.loss_kw"]
            self.assertEqual((body["v"], body["from"], body["at"]), (29, "doc.meter", "the night of the 3rd"))

    def test_the_same_value_from_another_source_is_a_citation_change(self):
        with tempfile.TemporaryDirectory() as d:
            rec = repo(d)
            branch(d, rec, "meter", commands=[
                ["add", "doc.meter", "name=the heat meter", "url=https://example.test/meter",
                 "read=2026-09-04"],
                ["set", "heat.loss_kw", "31", "--source", "doc.meter", "--at", "the night of the 3rd",
                 "--as-of", "2026-09-04"]])
            code, out, err = kp("consolidate", "--dry-run", "--from", "meter", rec)
            self.assertEqual(code, 0, out + err)
            self.assertNotIn("one id follows one source", out)


class AReadingFromTheFuture(unittest.TestCase):

    def test_a_day_ahead_of_today_is_refused_wherever_a_reading_is_dated(self):
        with tempfile.TemporaryDirectory() as d:
            rec = pathlib.Path(d) / "PROVENANCE.yaml"
            shutil.copytree(FIXTURE, d, dirs_exist_ok=True)
            before = rec.read_text(encoding="utf-8")
            import datetime
            ahead = (datetime.date.today() + datetime.timedelta(days=2)).isoformat()
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", ahead, rec)
            self.assertEqual(code, 1)
            self.assertIn(f"--as-of {ahead} is after today", out + err)
            self.assertIn("date it the day it was read", out + err)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.note", "v=1", "name=a note",
                                 "from=doc.boiler_sheet", "at=its margin", f"of={ahead}", rec)
            self.assertEqual(code, 1)
            self.assertIn(f"of: {ahead} is after today", out + err)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "doc.later", "name=a document",
                                 "url=https://example.test/later", f"read={ahead}", rec)
            self.assertEqual(code, 1)
            self.assertIn(f"read: {ahead} is after today", out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            today = datetime.date.today().isoformat()
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", today, rec)
            self.assertEqual(code, 0, out + err)
