"""The record's entry file and the directory beside it. GROUNDING.yaml is born by the first
add and found first; PROVENANCE.yaml - the name records were born under before - is still
found wherever it is. Each keeps its files under its own layout, .kpopper/ beside the new
name and the PROVENANCE.* names beside the old, and the reader never looks in the other's.
Runs against the fixtures under tests/fixtures, with no browser and no network:

    python3 -m unittest discover -s tests
"""
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
FIXTURES = ROOT / "tests" / "fixtures"

sys.path.insert(0, str(SCRIPTS))
import cli as C  # noqa: E402
import provenance as P  # noqa: E402
import render_page as R  # noqa: E402
import workspace as W  # noqa: E402


def run(*args, cwd=None, env=None, stdin=None):
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd, input=stdin,
                       capture_output=True, text=True, env=env)
    return p.returncode, p.stdout, p.stderr


def git(d, *args):
    p = subprocess.run(["git", "-c", "user.email=t@example.test", "-c", "user.name=t",
                        "-c", "commit.gpgsign=false"] + list(args), cwd=str(d),
                       capture_output=True, text=True)
    if p.returncode:
        raise AssertionError(" ".join(args) + "\n" + p.stdout + p.stderr)
    return p.stdout


def legacy(into, fixture="hypotheses"):
    """A scratch copy of a fixture as it is: a record under the old name and its files."""
    shutil.copytree(FIXTURES / fixture, into, dirs_exist_ok=True)
    return into / "PROVENANCE.yaml"


def brought_over(into, fixture="hypotheses"):
    """The same fixture under the new name and layout: GROUNDING.yaml beside .kpopper/."""
    shutil.copytree(FIXTURES / fixture, into, dirs_exist_ok=True)
    home = into / ".kpopper"
    home.mkdir()
    (into / "PROVENANCE.yaml").rename(into / "GROUNDING.yaml")
    for old, new in (("PROVENANCE.view.yaml", "view.yaml"), ("PROVENANCE.measure.yaml", "measure.yaml"),
                     ("PROVENANCE.session.json", "session.json")):
        if (into / old).exists():
            (into / old).rename(home / new)
    if (into / "PROVENANCE.d").exists():
        (into / "PROVENANCE.d").rename(home / "hypotheses")
    return into / "GROUNDING.yaml"


class Scratch(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = pathlib.Path(self.tmp.name).resolve()

    def cli(self, *args, cwd=None):
        return run(SCRIPTS / "cli.py", *args, cwd=str(cwd or self.dir))


class TheLocator(Scratch):
    def test_the_new_name_is_found_first_and_the_old_after_it(self):
        found = W.locate(self.dir)
        self.assertEqual(found["status"], "missing")
        self.assertEqual(pathlib.Path(found["record"]).name, "GROUNDING.yaml", "what a birth would create")
        (self.dir / "PROVENANCE.yaml").write_text("meta: {updated: 2026-09-01}\n", encoding="utf-8")
        found = W.locate(self.dir)
        self.assertEqual((found["status"], pathlib.Path(found["record"]).name), ("found", "PROVENANCE.yaml"))
        (self.dir / "GROUNDING.yaml").write_text("meta: {updated: 2026-09-12}\n", encoding="utf-8")
        found = W.locate(self.dir)
        self.assertEqual(pathlib.Path(found["record"]).name, "GROUNDING.yaml")

    def test_either_name_is_found_from_a_subdirectory_of_a_checkout(self):
        git(self.dir, "init", "-q")
        sub = self.dir / "notes" / "deep"
        sub.mkdir(parents=True)
        for name in ("PROVENANCE.yaml", "GROUNDING.yaml"):
            (self.dir / name).write_text("meta: {updated: 2026-09-01}\n", encoding="utf-8")
            code, out, err = self.cli("where", cwd=sub)
            self.assertEqual(code, 0, err)
            self.assertEqual(out.strip(), str(self.dir / name))
            (self.dir / name).unlink()

    def test_a_record_under_the_old_name_is_written_where_it_is_and_never_replaced(self):
        rec = legacy(self.dir)
        code, out, err = self.cli("add", "brief", "name=the brief", "file=notes/brief.md",
                                  "read=2026-09-12", "--as-of", "2026-09-12")
        self.assertEqual(code, 0, err)
        self.assertNotIn("created", out)
        self.assertFalse((self.dir / "GROUNDING.yaml").exists())
        self.assertIn("  brief:", rec.read_text(encoding="utf-8"))


class TheBirth(Scratch):
    def test_a_refused_first_add_leaves_no_record_behind(self):
        code, out, err = self.cli("add", "d.first", "verdict=a conclusion", "because=nothing",
                                  "rests_on=[s.nowhere]", "reopened_by=anything", "--as-of", "2026-09-12")
        self.assertNotEqual(code, 0)
        self.assertIn("refused", out + err)
        self.assertFalse((self.dir / "GROUNDING.yaml").exists(), "the newborn is longer than it was, and still goes")
        self.assertFalse((self.dir / "PROVENANCE.yaml").exists())


class OneHomePerRecord(Scratch):
    """The layout follows the entry file's name, and a file left under the other layout is
    not read - the guard against a second home creeping in."""

    def test_hypotheses_are_read_under_the_home_and_nowhere_else(self):
        rec = brought_over(self.dir)
        code, out, err = self.cli("open")
        self.assertEqual(code, 0, err)
        self.assertIn("2 hypotheses", out + err)
        stray = self.dir / "PROVENANCE.d"
        stray.mkdir()
        shutil.copy(self.dir / ".kpopper" / "hypotheses" / "glazing_redo.yaml", stray / "stray.yaml")
        code, out, err = self.cli("open")
        self.assertNotIn("stray", out + err, "the old layout's directory is not the new one's home")
        self.assertIn("2 hypotheses", out + err)

    def test_the_old_layout_does_not_read_the_new_ones_directory(self):
        rec = legacy(self.dir)
        code, out, err = self.cli("open")
        self.assertIn("2 hypotheses", out + err)
        stray = self.dir / ".kpopper" / "hypotheses"
        stray.mkdir(parents=True)
        shutil.copy(self.dir / "PROVENANCE.d" / "glazing_redo.yaml", stray / "stray.yaml")
        code, out, err = self.cli("open")
        self.assertNotIn("stray", out + err)
        self.assertIn("2 hypotheses", out + err)

    def test_a_write_into_a_hypothesis_lands_under_the_entry_files_layout(self):
        for make, expected in ((brought_over, ".kpopper/hypotheses/third.yaml"), (legacy, "PROVENANCE.d/third.yaml")):
            with tempfile.TemporaryDirectory() as d:
                rec = make(pathlib.Path(d))
                code, out, err = run(SCRIPTS / "cli.py", "add", "heat.note", "v=1", "from=heat.survey",
                                     "at=p.3", "name=a note", "--hypothesis", "third", cwd=d)
                self.assertEqual(code, 0, out + err)
                self.assertTrue((pathlib.Path(d) / expected).is_file(), expected)

    def test_the_brief_and_the_allowlist_are_found_under_the_entry_files_layout(self):
        rec = brought_over(self.dir, "remeasure")
        self.assertEqual(R.find_brief([str(rec)]), str(self.dir / ".kpopper" / "view.yaml"))
        (self.dir / "PROVENANCE.view.yaml").write_text("title: stray\n", encoding="utf-8")
        self.assertEqual(R.find_brief([str(rec)]), str(self.dir / ".kpopper" / "view.yaml"),
                         "a brief under the old name beside a record under the new is not the brief")
        code, out, err = self.cli("remeasure")
        self.assertEqual(code, 0, out + err)
        self.assertIn("from .kpopper/measure.yaml", out)
        with tempfile.TemporaryDirectory() as d:
            old = legacy(pathlib.Path(d), "remeasure")
            self.assertEqual(R.find_brief([str(old)]), str(pathlib.Path(d) / "PROVENANCE.view.yaml"))
            code, out, err = run(SCRIPTS / "cli.py", "remeasure", cwd=d)
            self.assertEqual(code, 0, out + err)
            self.assertIn("from PROVENANCE.measure.yaml", out)

    def test_the_layout_names_every_file_a_record_keeps_beside_itself(self):
        new, old = P.layout("/p/GROUNDING.yaml"), P.layout("/p/PROVENANCE.yaml")
        self.assertEqual({k: os.path.relpath(v, "/p") for k, v in new.items() if k in ("hypotheses", "view", "measure", "session", "build", "page")},
                         {"hypotheses": ".kpopper/hypotheses", "view": ".kpopper/view.yaml", "measure": ".kpopper/measure.yaml",
                          "session": ".kpopper/session.json", "build": ".kpopper/build", "page": ".kpopper/build/page.html"})
        self.assertEqual({k: os.path.relpath(v, "/p") for k, v in old.items() if k in ("hypotheses", "view", "measure", "session")},
                         {"hypotheses": "PROVENANCE.d", "view": "PROVENANCE.view.yaml",
                          "measure": "PROVENANCE.measure.yaml", "session": "PROVENANCE.session.json"})
        self.assertEqual((P.hypotheses_rel("a/GROUNDING.yaml"), P.hypotheses_rel("a/PROVENANCE.yaml")),
                         (".kpopper/hypotheses", "PROVENANCE.d"))


class AHalfMove(Scratch):
    """A record renamed with its files left under the earlier names, or a .kpopper/ opened
    beside a record still under the earlier name: nothing reads what was left, so check fails
    on it and the opener names it - the silence the first hand migration would otherwise meet."""

    def test_files_left_under_the_earlier_name_fail_check_and_are_named_by_the_opener(self):
        legacy(self.dir, "remeasure")                       # a hypothesis, a brief and an allowlist
        (self.dir / "PROVENANCE.yaml").rename(self.dir / "GROUNDING.yaml")
        code, out, err = self.cli("check")
        self.assertEqual(code, 1, out + err)
        for what, where in (("PROVENANCE.d/", ".kpopper/hypotheses/"), ("PROVENANCE.view.yaml", ".kpopper/view.yaml"),
                            ("PROVENANCE.measure.yaml", ".kpopper/measure.yaml")):
            self.assertIn(f"FAIL {what} is not read - left under the earlier name; move it to {where}", out)
        code, out, err = self.cli("open")
        self.assertIn("left under the earlier name, not read: PROVENANCE.d/, PROVENANCE.view.yaml, "
                      "PROVENANCE.measure.yaml - move into .kpopper/", out)
        self.assertNotIn("hypothesis waits", out, "not read, so not counted")
        home = self.dir / ".kpopper"
        home.mkdir()
        (self.dir / "PROVENANCE.d").rename(home / "hypotheses")
        (self.dir / "PROVENANCE.view.yaml").rename(home / "view.yaml")
        (self.dir / "PROVENANCE.measure.yaml").rename(home / "measure.yaml")
        code, out, err = self.cli("check")
        self.assertNotIn("not read", out)
        code, out, err = self.cli("open")
        self.assertIn("1 hypothesis waits", out)
        self.assertNotIn("left under", out)

    def test_a_home_opened_beside_a_record_under_the_earlier_name_is_named_too(self):
        legacy(self.dir, "page")
        (self.dir / ".kpopper").mkdir()
        (self.dir / ".kpopper" / "view.yaml").write_text("title: moved first\n", encoding="utf-8")
        code, out, err = self.cli("check")
        self.assertEqual(code, 1, out + err)
        self.assertIn("FAIL .kpopper/view.yaml is not read beside PROVENANCE.yaml - rename the record to "
                      "GROUNDING.yaml and move its files into .kpopper/, or move this to PROVENANCE.view.yaml", out)
        code, out, err = self.cli("open")
        self.assertIn(".kpopper/view.yaml not read beside PROVENANCE.yaml - rename the record to GROUNDING.yaml", out)

    def test_an_empty_hypotheses_directory_left_behind_loses_nothing_and_says_nothing(self):
        brought_over(self.dir, "page")
        (self.dir / "PROVENANCE.d").mkdir()
        code, out, err = self.cli("check")
        self.assertEqual(code, 0, out + err)
        self.assertNotIn("not read", out)


class ThePage(Scratch):
    def test_the_page_lands_in_the_build_directory_which_ignores_itself(self):
        brought_over(self.dir, "page")
        code, out, err = self.cli("page")
        self.assertEqual(code, 0, out + err)
        page, ignore = self.dir / ".kpopper" / "build" / "page.html", self.dir / ".kpopper" / "build" / ".gitignore"
        self.assertTrue(page.is_file())
        self.assertEqual(ignore.read_text(encoding="utf-8"), "*\n")
        self.assertFalse((self.dir / "record.html").exists())
        code, out, err = self.cli("page", "--verify")
        self.assertEqual(code, 0, out + err)

    def test_a_brief_named_on_the_command_line_is_not_taken_for_the_record(self):
        brought_over(self.dir, "page")
        code, out, err = self.cli("page", "--brief", ".kpopper/view.yaml")
        self.assertEqual(code, 0, out + err)
        self.assertTrue((self.dir / ".kpopper" / "build" / "page.html").is_file())
        self.assertFalse((self.dir / ".kpopper" / ".kpopper").exists(), "the brief is not the record")
        here = os.getcwd()
        os.chdir(self.dir)
        try:
            self.assertEqual(C.page_of(["--brief", ".kpopper/view.yaml"])["page"],
                             str(self.dir / ".kpopper" / "build" / "page.html"))
            self.assertEqual(C.page_of(["--brief", "PROVENANCE.view.yaml", "GROUNDING.yaml"])["page"],
                             str(self.dir / ".kpopper" / "build" / "page.html"), "a legacy brief beside a new record")
            with tempfile.TemporaryDirectory() as d:
                old = legacy(pathlib.Path(d), "page")
                self.assertEqual(C.page_of([str(old)])["page"], "record.html")
        finally:
            os.chdir(here)

    def test_a_record_under_the_old_name_still_writes_record_html_here(self):
        legacy(self.dir, "page")
        code, out, err = self.cli("page")
        self.assertEqual(code, 0, out + err)
        self.assertTrue((self.dir / "record.html").is_file())
        self.assertFalse((self.dir / ".kpopper").exists())


class TheEditHook(Scratch):
    def edit(self, file):
        env = dict(os.environ, TMPDIR=str(self.dir))
        payload = {"session_id": "l1", "cwd": str(self.dir), "hook_event_name": "PreToolUse",
                   "tool_name": "Edit", "tool_input": {"file_path": file}}
        code, out, err = run(SCRIPTS / "edit_hook.py", "claude", cwd=str(self.dir), env=env,
                             stdin=json.dumps(payload))
        self.assertEqual(code, 0, err)
        return out.strip()

    def test_the_entry_file_and_everything_under_the_home_are_the_writers_business(self):
        brought_over(self.dir, "remeasure")
        for file in ("GROUNDING.yaml", "PROVENANCE.yaml", ".kpopper/view.yaml", ".kpopper/hypotheses/x.yaml"):
            self.assertEqual(self.edit(file), "", file)


class AcrossBranches(Scratch):
    """A branch that brought its record over reads the base branch's record under the old
    name, and the base branch reads the renamed one - the window in which a rename lands."""

    def setUp(self):
        super().setUp()
        legacy(self.dir)
        (self.dir / "PROVENANCE.d" / "bigger_boiler.yaml").unlink()     # one hypothesis, not two contesting
        git(self.dir, "init", "-q", "-b", "main")
        git(self.dir, "add", "-A")
        git(self.dir, "commit", "-q", "-m", "record under the old name")
        git(self.dir, "checkout", "-q", "-b", "renamed")
        (self.dir / ".kpopper").mkdir()
        git(self.dir, "mv", "PROVENANCE.yaml", "GROUNDING.yaml")
        git(self.dir, "mv", "PROVENANCE.view.yaml", ".kpopper/view.yaml")
        git(self.dir, "mv", "PROVENANCE.d", ".kpopper/hypotheses")
        git(self.dir, "commit", "-q", "-m", "brought over")

    def test_each_side_reads_the_other_under_its_own_name(self):
        base_code, base_out, _ = self.cli("consolidate", "--dry-run")
        code, out, err = self.cli("consolidate", "--dry-run", "--from", "main")
        self.assertNotIn("holds no", out + err)
        self.assertIn("main", out)
        self.assertEqual(code, base_code, out + err)
        git(self.dir, "checkout", "-q", "main")
        base_code, base_out, _ = self.cli("consolidate", "--dry-run")
        code, out, err = self.cli("consolidate", "--dry-run", "--from", "renamed")
        self.assertNotIn("holds no", out + err)
        self.assertIn("renamed", out)
        self.assertEqual(code, base_code, out + err)


if __name__ == "__main__":
    unittest.main()
