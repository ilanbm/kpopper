"""The tree measured against the record: every entry that names a recipe is taken again by the
allowlist beside the record, and what differs is one more hypothesis through the dry run. Runs
against the fixture in tests/fixtures/remeasure - a base with three measured readings, the
files they are taken from, and one hypothesis - on scratch copies, with no network:

    python3 -m unittest discover -s tests
"""
import datetime
import io
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "remeasure"
CONSOLIDATE_FIXTURE = ROOT / "tests" / "fixtures" / "consolidate"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402
import remeasure as R   # noqa: E402

TODAY = datetime.datetime.now(datetime.timezone.utc).date().isoformat()


def run(*args, cwd=None):
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd,
                       capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def kp(*args, cwd=None):
    return run(SCRIPTS / "kpopper", *args, cwd=cwd)


def copy_fixture(into, hypothesis=True):
    """A scratch copy of the fixture - the base, the allowlist, the files the recipes read and,
    unless told otherwise, the hypothesis beside it."""
    shutil.copytree(FIXTURE, into, dirs_exist_ok=True)
    if not hypothesis:
        shutil.rmtree(into / "PROVENANCE.d")
    return into / "PROVENANCE.yaml"


def edit(path, old, new):
    text = io.open(path, encoding="utf-8").read()
    assert old in text, old
    io.open(path, "w", encoding="utf-8").write(text.replace(old, new, 1))


def allowlist(d, text):
    io.open(d / "PROVENANCE.measure.yaml", "w", encoding="utf-8").write(text)


class ThePlanTests(unittest.TestCase):
    """Without --run nothing runs; the plan says what would, and where from."""

    def test_the_plan_runs_nothing_and_says_what_it_would_run(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            allowlist(pathlib.Path(d), "boiler_kw: [python3, -I, -c, \"open('ran', 'w').write('x'); print(24)\"]\n"
                                       "loss_kw: [python3, -I, -c, \"print(28)\"]\n"
                                       "cold_nights: [python3, -I, -c, \"print(3)\"]\n"
                                       "spare: [python3, -c, 'print(1)']\n")
            code, out, err = kp("remeasure", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("3 recipes named by 3 entries, from PROVENANCE.measure.yaml, run from", out)
            self.assertIn("heat.boiler_kw <- boiler_kw: ", out)
            self.assertIn("python3 -I -c ", out)
            self.assertIn("named by no entry, never run: spare", out)
            self.assertIn("nothing ran - add --run to measure this tree", out)
            self.assertFalse((pathlib.Path(d) / "ran").exists(), "a recipe ran without --run")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertTrue((pathlib.Path(d) / "ran").exists(), "the recipe did not run under --run")

    def test_a_record_with_no_measures_and_no_allowlist_has_nothing_to_do(self):
        code, out, err = kp("remeasure", "--run", CONSOLIDATE_FIXTURE / "PROVENANCE.yaml")
        self.assertEqual(code, 0, out + err)
        self.assertEqual(out.strip(), "no measures beside the record - nothing to re-measure")

    def test_the_command_takes_run_and_a_file_and_nothing_else(self):
        code, out, err = kp("remeasure", "--bogus", FIXTURE / "PROVENANCE.yaml")
        self.assertNotEqual(code, 0)
        self.assertIn("--bogus is not an option of remeasure", out + err)
        code, out, err = kp("remeasure", "--help")
        self.assertEqual(code, 0)
        self.assertIn("remeasure [--run] [file]", out)


class TheMeasurementTests(unittest.TestCase):
    """The tree agrees, or it does not: the readings that differ are one hypothesis through the
    dry run, and the exit code is the dry run's."""

    def test_the_tree_agreeing_with_the_union_is_clean(self):
        # the hypothesis proposes 28 for the loss and repeats the entry without measure:; the
        # base's recipe still applies, and reads the union's value - the hypothesis's. A scratch
        # copy: recipes run from the checkout's root, and the fixture sits inside this one
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = kp("remeasure", "--run", rec)
        self.assertEqual(code, 0, out + err)
        self.assertIn(f"measured on {TODAY} (UTC)", out)
        self.assertIn("3 entries by 3 recipes", out)
        self.assertIn("  heat.boiler_kw: 24 - as recorded (boiler_kw)\n", out)
        self.assertIn("  heat.loss_kw: 28 - as recorded (loss_kw)\n", out)
        self.assertIn("  log.cold_nights: 3 - as recorded (cold_nights)\n", out)
        self.assertTrue(out.rstrip().endswith("the record holds what this tree measures"), out)
        self.assertNotIn("laid over it", out)

    def test_an_older_reading_that_moved_is_green_with_the_refresh_command(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d), hypothesis=False)
            before = rec.read_text(encoding="utf-8")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("the base with tree/here laid over it\n"
                          f"  tree/here (born {TODAY}, today, never folds): what the tree measures\n", out)
            self.assertIn("updates (1): what the base holds that a hypothesis replaces, and what rests on each\n"
                          "  heat.loss_kw: 31 -> 28, from tree/here\n"
                          f"    a reading from {TODAY} that is newer than the base's\n"
                          "    worked out from it: heat.deficit_kw\n"
                          "    MUTED     c.boiler_short: heat.loss_kw moved 31 -> 28, inside wrong_if", out)
            self.assertIn("  heat.loss_kw: 31 recorded (2026-09-02, by its own of:) -> 28 measured by loss_kw\n"
                          f"    refresh: kpop set heat.loss_kw 28 --why 'measured by loss_kw' --as-of {TODAY} "
                          + str(rec) + "\n", out)
            self.assertTrue(out.rstrip().endswith("the tree reads 1 entry differently, none across a line - refresh them"), out)
            # the generic advice of the dry run - fold it, refute it, --as-of it - is not given
            self.assertNotIn("may fold", out)
            self.assertNotIn("read again on a later day", out)
            self.assertNotIn("refute", out)
            self.assertEqual(rec.read_text(encoding="utf-8"), before, "the runner wrote the record")
            # the command it printed is one the write path takes
            code, out, err = kp("set", "heat.loss_kw", "28", "--why", "measured by loss_kw",
                                "--as-of", TODAY, rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("set heat.loss_kw: 31 -> 28", out)
            self.assertIn('at: "worked out from the glazing area during the session"', rec.read_text(encoding="utf-8"))

    def test_a_falsifier_that_holds_on_the_measured_value_is_red_and_named(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            with io.open(pathlib.Path(d) / "readings.txt", "a", encoding="utf-8") as f:
                f.write("2026-02-03\n2026-02-04\n2026-02-05\n2026-02-06\n")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("  FALSIFIED c.short_snap: wrong_if holds (log.cold_nights > 5) - broken by its own condition\n", out)
            # the judgment whose sign does not name the count moved, and is a flag, not a failure
            self.assertIn("  MOVED c.season_plan: log.cold_nights differs from its snapshot (3 -> 7)", out)
            self.assertIn("  log.cold_nights: 3 recorded (2026-09-02, by the read: of run.night_log) -> 7 measured by cold_nights\n"
                          "    refresh: kpop set log.cold_nights 7 --why 'measured by cold_nights' ", out)
            self.assertIn("not clean: a falsifier that holds on what the tree measures - red until the record and the tree agree", out)

    def test_a_reading_of_the_same_day_is_contested_through_the_door(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d), hypothesis=False)
            edit(rec, '    of: "2026-09-02"\n    measure: loss_kw', f'    of: "{TODAY}"\n    measure: loss_kw')
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("contested (1): the door refuses the reading, so the base keeps what it holds\n"
                          "  heat.loss_kw: a reading of the same day\n", out)
            self.assertIn(f"  heat.loss_kw: 31 recorded ({TODAY}, by its own of:) -> 28 measured by loss_kw\n"
                          "    correct the recorded claim in this pull request, or run the measurement again on a "
                          "later day; do not future-date this result\n", out)
            self.assertNotIn("refresh:", out)
            self.assertIn("not clean: a reading the tree contests", out)

    def test_a_hypothesis_the_tree_contradicts_is_contested_and_the_run_stops(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            (pathlib.Path(d) / "loss.txt").write_text("30\n", encoding="utf-8")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("the base with glazing_redo, tree/here laid over it, in name order\n", out)
            self.assertIn("contested (1): an id two hypotheses hold with different claims", out)
            self.assertIn("    glazing_redo says heat.loss_kw: 28 ", out)
            self.assertIn("    tree/here says heat.loss_kw: 30 ", out)
            self.assertIn("  heat.loss_kw: 28 recorded (2026-09-03, by its own of:) -> 30 measured by loss_kw\n"
                          "    correct the recorded claim in this pull request", out)
            self.assertNotIn("re-read against the merged tree", out)

    def test_a_hypothesis_that_drops_the_recipe_is_refused_before_anything_runs(self):
        # the fold carries a hypothesis's block over whole, so a replacement that says nothing
        # about the recipe would drop it and nothing would take that reading again
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.d" / "glazing_redo.yaml", "    measure: loss_kw\n", "")
            allowlist(pathlib.Path(d), "boiler_kw: [python3, -I, -c, \"open('ran', 'w').write('x'); print(24)\"]\n"
                                       "loss_kw: [python3, -I, -c, 'print(28)']\n"
                                       "cold_nights: [python3, -I, -c, 'print(3)']\n")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("heat.loss_kw: glazing_redo replaces it without measure: loss_kw - the fold carries "
                          "the block over whole, so the recipe would be dropped and nothing would take this "
                          "reading again; carry measure: loss_kw into glazing_redo, or drop it from the base "
                          "first", out)
            self.assertFalse((pathlib.Path(d) / "ran").exists())

    def test_the_tree_is_read_beside_the_hypothesis_that_holds_the_same_entry(self):
        # both carry the recipe; the tree measures a third value, and the two claims contest
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            (pathlib.Path(d) / "loss.txt").write_text("33\n", encoding="utf-8")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("heat.loss_kw <- loss_kw:", out)
            self.assertIn("tree/here says heat.loss_kw: 33", out)

    def test_two_recipes_named_for_one_entry_is_a_contested_recipe(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.d" / "glazing_redo.yaml",
                 "    measure: loss_kw\n", "    measure: loss_by_wall\n")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("heat.loss_kw: two recipes named for one entry - loss_by_wall by glazing_redo; "
                          "loss_kw by the base - a contested recipe; one of them, or neither", out)

    def test_the_tree_is_named_so_that_no_file_can_share_the_name(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d), hypothesis=False)
            (pathlib.Path(d) / "PROVENANCE.d").mkdir()
            shutil.copy(FIXTURE / "PROVENANCE.d" / "glazing_redo.yaml", pathlib.Path(d) / "PROVENANCE.d" / "tree.yaml")
            (pathlib.Path(d) / "loss.txt").write_text("30\n", encoding="utf-8")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("tree, tree/here laid over it", out)
            self.assertIn("tree says heat.loss_kw: 28", out)
            self.assertIn("tree/here says heat.loss_kw: 30", out)
            self.assertIn("/", R.TREE + "/here")


class TheHolesTests(unittest.TestCase):
    """A measurement nothing takes is a hole, and a hole is red - before anything runs where
    the record or the allowlist cannot be read whole."""

    def test_a_recipe_the_allowlist_lacks_is_red_before_anything_runs(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            allowlist(pathlib.Path(d), "boiler_kw: [python3, -I, -c, \"open('ran', 'w').write('x'); print(24)\"]\n"
                                       "loss_kw: [python3, -I, -c, \"print(28)\"]\n")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("refused - the record names recipe PROVENANCE.measure.yaml does not hold: cold_nights - "
                          "a measurement nothing takes is a hole, and a falsifier reading it tests nothing", out)
            self.assertFalse((pathlib.Path(d) / "ran").exists())

    def test_measures_named_with_no_allowlist_beside_the_record_is_red(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            os.remove(pathlib.Path(d) / "PROVENANCE.measure.yaml")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("refused - 3 recipes named and no PROVENANCE.measure.yaml beside the record to hold them: "
                          "boiler_kw, cold_nights, loss_kw", out)

    def test_the_allowlist_is_refused_whole_on_a_shape_it_cannot_take(self):
        cases = [
            ("boiler_kw: [python3, -c, 'print(24)']\nboiler_kw: [python3, -c, 'print(25)']\n",
             "the key 'boiler_kw' appears twice"),
            ("on: [python3, -c, 'print(1)']\n", "True is not a recipe name"),
            ("boiler_kw: []\n", "boiler_kw: a recipe is a non-empty list of non-empty strings"),
            ("boiler_kw: python3 -c 'print(24)'\n", "boiler_kw: a recipe is a non-empty list of non-empty strings"),
            ("boiler_kw: [python3, 3]\n", "boiler_kw: a recipe is a non-empty list of non-empty strings"),
            ("- python3\n", "is not a mapping of recipe names to argument lists"),
            ("base: &b {loss_kw: [python3, -c, 'print(1)']}\n<<: *b\nloss_kw: [python3, -c, 'print(2)']\n",
             "the key 'loss_kw' appears twice"),
        ]
        for text, said in cases:
            with tempfile.TemporaryDirectory() as d:
                rec = copy_fixture(pathlib.Path(d))
                allowlist(pathlib.Path(d), text)
                code, out, err = kp("remeasure", "--run", rec)
                self.assertNotEqual(code, 0, text)
                self.assertIn("PROVENANCE.measure.yaml", out + err)
                self.assertIn(said, out + err, text)

    def test_what_a_recipe_prints_must_be_the_one_value_of_the_recorded_kind(self):
        cases = [
            ("print('11 lines')", "FAIL cold_nights (log.cold_nights): printed '11 lines' where the record holds a number"),
            ("print('nan')", "printed 'nan' where the record holds a number"),
            ("print(11); print(0)", "printed 2 lines - the value is the one line a recipe prints; diagnostics go to stderr"),
            ("pass", "printed nothing - the value is the one line a recipe prints"),
            ("import sys; sys.stderr.write('no logger here'); sys.exit(3)", "exited 3 - stderr: no logger here"),
            ("print('x' * 200000)", "printed more than 64 KiB on stdout and was stopped"),
        ]
        for code_, said in cases:
            with tempfile.TemporaryDirectory() as d:
                rec = copy_fixture(pathlib.Path(d))
                allowlist(pathlib.Path(d), "boiler_kw: [python3, -I, -c, 'print(24)']\n"
                                           "loss_kw: [python3, -I, -c, 'print(28)']\n"
                                           f"cold_nights: [python3, -I, -c, {code_!r}]\n")
                code, out, err = kp("remeasure", "--run", rec)
                self.assertEqual(code, 1, code_ + "\n" + out + err)
                self.assertIn(said, out, code_)
                self.assertIn("not clean: a hole", out)

    def test_a_recipe_that_does_not_finish_is_stopped_and_red(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            allowlist(pathlib.Path(d), "boiler_kw: [python3, -I, -c, 'print(24)']\n"
                                       "loss_kw: [python3, -I, -c, 'print(28)']\n"
                                       "cold_nights: [python3, -I, -c, 'import time; time.sleep(30)']\n")
            t0 = time.time()
            lines, code = R.measure([str(rec)], run=True, timeout=1)
            self.assertLess(time.time() - t0, 10)
            self.assertEqual(code, 1, "\n".join(lines))
            self.assertIn("  FAIL cold_nights (log.cold_nights): did not finish in 1 s and was stopped", lines)

    @unittest.skipIf(os.name == "nt", "the process group is a POSIX one")
    def test_nothing_a_recipe_left_running_outlives_it(self):
        # a recipe that starts something and exits would leave it holding the pipes and
        # running past the bound this promises, so the group goes when the recipe does
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d), hypothesis=False)
            child = ("import os, subprocess, sys, tempfile\n"
                     "p = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(45)'])\n"
                     "open('child.pid', 'w').write(str(p.pid))\n"
                     "print(3)\n")
            allowlist(pathlib.Path(d), "boiler_kw: [python3, -I, -c, 'print(24)']\n"
                                       "loss_kw: [python3, -I, -c, 'print(31)']\n"
                                       f"cold_nights: [python3, -I, -c, {child!r}]\n")
            t0 = time.time()
            lines, code = R.measure([str(rec)], run=True)
            self.assertLess(time.time() - t0, 20, "\n".join(lines))
            pid = int((pathlib.Path(d) / "child.pid").read_text(encoding="utf-8"))
            for _ in range(40):
                try:
                    os.kill(pid, 0)
                except OSError:
                    break
                time.sleep(0.1)
            else:
                os.kill(pid, 9)
                self.fail(f"the recipe's child {pid} outlived the run")

    def test_an_executable_nothing_has_is_red_without_running(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            allowlist(pathlib.Path(d), "boiler_kw: [no-such-tool-here, '24']\n"
                                       "loss_kw: [python3, -I, -c, 'print(28)']\n"
                                       "cold_nights: [python3, -I, -c, 'print(3)']\n")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("no-such-tool-here (not found)", out)
            self.assertIn("FAIL boiler_kw (heat.boiler_kw): no executable 'no-such-tool-here' on the path", out)


class TheFieldTests(unittest.TestCase):
    """`measure:` stands on a stored scalar reading, as a bare name - held to by the reader at
    check and at add, and by the runner before anything runs."""

    def test_check_fails_a_measure_that_cannot_stand(self):
        cases = [
            ('    wrong_if: "log.cold_nights > 5"\n', '    wrong_if: "log.cold_nights > 5"\n    measure: snap\n',
             "c.short_snap: measure: snap on a judgment - a judgment is not measured; the entries it rests on are"),
            ('    rule: "heat.loss_kw - heat.boiler_kw"\n', '    rule: "heat.loss_kw - heat.boiler_kw"\n    measure: deficit\n',
             "heat.deficit_kw: measure: deficit on an entry worked out from a rule - measure what it is worked out from"),
            ('    measure: boiler_kw\n', '    measure: "rm -rf /"\n',
             "heat.boiler_kw: measure: 'rm -rf /' is not a recipe name - letters, digits, underscores and dashes, "
             "opening with a letter; what runs lives in the allowlist beside the record, never here"),
            ('    measure: boiler_kw\n', '    measure: boiler.kw\n',
             "heat.boiler_kw: measure: 'boiler.kw' is not a recipe name"),
            ('    read: "2026-09-02"\n  run.night_log:', '    read: "2026-09-02"\n    measure: sheet\n  run.night_log:',
             "doc.boiler_sheet: measure: sheet on an entry with no value of its own"),
        ]
        for old, new, said in cases:
            with tempfile.TemporaryDirectory() as d:
                rec = copy_fixture(pathlib.Path(d))
                edit(rec, old, new)
                code, out, err = kp("check", rec)
                self.assertEqual(code, 1, out + err)
                self.assertIn("FAIL " + said, out)
                # and the runner refuses before running anything
                allowlist(pathlib.Path(d), "boiler_kw: [python3, -I, -c, \"open('ran', 'w').write('x'); print(24)\"]\n"
                                           "loss_kw: [python3, -I, -c, 'print(28)']\ncold_nights: [python3, -I, -c, 'print(3)']\n")
                code, out, err = kp("remeasure", "--run", rec)
                self.assertEqual(code, 1, out + err)
                self.assertIn("refused - the record names recipes it cannot", out)
                self.assertIn(said, out)
                self.assertFalse((pathlib.Path(d) / "ran").exists())

    def test_add_refuses_a_measure_that_cannot_stand_and_writes_one_that_can(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.pump_kw", "v=5", "from=doc.boiler_sheet",
                                 "measure=rm -rf /", rec)
            self.assertNotEqual(code, 0)
            self.assertIn("measure: 'rm -rf /' is not a recipe name", out + err)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.pump_kw", "v=5", "from=doc.boiler_sheet",
                                 "measure=pump_kw", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("    measure: pump_kw\n", rec.read_text(encoding="utf-8"))
            code, out, err = kp("pull", "heat.pump_kw", rec)
            self.assertIn("heat.pump_kw: 5 measured by pump_kw <- doc.boiler_sheet", out)

    def test_a_judgment_the_page_decides_is_refused_against_the_tree(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            with io.open(rec, "a", encoding="utf-8") as f:
                f.write('  c.page_bound:\n    rests_on: [log.cold_nights, page.covered]\n'
                        '    verdict: "the page covers more than the snap ran"\n'
                        '    wrong_if: "page.covered < log.cold_nights"\n'
                        '    seen: {log.cold_nights: 3, page.covered: 0}\n')
            code, out, err = kp("check", rec)
            self.assertEqual(code, 0, out + err)
            (pathlib.Path(d) / "readings.txt").write_text("a\nb\nc\nd\n", encoding="utf-8")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("  FAIL c.page_bound: wrong_if reads page.covered beside log.cold_nights, which the tree "
                          "measured differently - the page decides it, and the page does not see the tree", out)
            self.assertIn("a sign the page decides", out)

    def test_the_refresh_command_is_quoted_for_a_shell_or_withheld(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d), hypothesis=False)
            edit(rec, "  when.first_cold_night:", '  note.label:\n    v: "old"\n    name: "the label"\n'
                      '    from: s.2026_09_02_heating\n    measure: label\n  when.first_cold_night:')
            allowlist(pathlib.Path(d), "boiler_kw: [python3, -I, -c, 'print(24)']\n"
                                       "loss_kw: [python3, -I, -c, 'print(31)']\ncold_nights: [python3, -I, -c, 'print(3)']\n"
                                       "label: [python3, -I, -c, \"print('$(printf injected)')\"]\n")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("    refresh: kpop set note.label '$(printf injected)' --why 'measured by label' ", out)
            allowlist(pathlib.Path(d), "boiler_kw: [python3, -I, -c, 'print(24)']\n"
                                       "loss_kw: [python3, -I, -c, 'print(31)']\ncold_nights: [python3, -I, -c, 'print(3)']\n"
                                       "label: [python3, -I, -c, \"print('--help')\"]\n")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("    edit it by hand in this pull request - note.label: v: '--help'", out)
            self.assertNotIn("refresh:", out)


class TheReadingTests(unittest.TestCase):
    """What a recipe printed is the reading the record holds, or it is not: a number by its
    value, text exactly - and the refresh carries it or says a hand must."""

    def with_label(self, d, prints, recorded='"old"'):
        rec = copy_fixture(pathlib.Path(d), hypothesis=False)
        edit(rec, "  when.first_cold_night:", f'  note.label:\n    v: {recorded}\n    name: "the label"\n'
             '    from: s.2026_09_02_heating\n    measure: label\n  when.first_cold_night:')
        allowlist(pathlib.Path(d), "boiler_kw: [python3, -I, -c, 'print(24)']\n"
                                   "loss_kw: [python3, -I, -c, 'print(31)']\n"
                                   "cold_nights: [python3, -I, -c, 'print(3)']\n"
                                   f"label: [python3, -I, -c, {prints!r}]\n")
        return rec

    def test_a_text_reading_is_compared_exactly(self):
        # "001" is not 1, and two spaces are not one: a number's tolerance is not text's
        for recorded, prints, agrees in (('"001"', "print('1')", False),
                                         ('"a  b"', "print('a b')", False),
                                         ('"001"', "print('001')", True),
                                         ("28", "print('28.0')", True)):
            with tempfile.TemporaryDirectory() as d:
                rec = self.with_label(d, prints, recorded)
                code, out, err = kp("remeasure", "--run", rec)
                self.assertEqual(code, 0, out + err)
                if agrees:
                    self.assertIn(" - as recorded (label)", out, (recorded, prints))
                    self.assertIn("the record holds what this tree measures", out)
                else:
                    self.assertIn("note.label: ", out, (recorded, prints))
                    self.assertIn("the tree reads 1 entry differently", out)

    def test_a_value_the_write_path_would_read_differently_is_left_to_a_hand(self):
        with tempfile.TemporaryDirectory() as d:
            rec = self.with_label(d, "print('002')")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("    edit it by hand in this pull request - note.label: v: '002' - set would read "
                          "'002' as 2, which is not what was measured", out)
            self.assertNotIn("refresh:", out)

    def test_what_holds_the_id_now_must_be_an_entry_with_a_reading(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d), hypothesis=False)
            (pathlib.Path(d) / "PROVENANCE.d").mkdir()
            io.open(pathlib.Path(d) / "PROVENANCE.d" / "flat.yaml", "w", encoding="utf-8").write(
                'hypothesis:\n  claim: "the loss is a bare number"\n  born: "2026-09-03"\n'
                "known:\n  heat.loss_kw: 28\n")
            code, out, err = kp("remeasure", "--run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("FAIL loss_kw (heat.loss_kw): what holds this id now is not an entry with a "
                          "reading of its own, so there is nothing to measure against", out)

    def test_a_working_tree_with_changes_says_so_in_the_reason(self):
        # the fixture's own directory is inside this checkout, so a scratch file makes it dirty
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d), hypothesis=False)
            (pathlib.Path(d) / "loss.txt").write_text("28\n", encoding="utf-8")
            lines, code = R.measure([str(rec)], run=True)
            said = [l for l in lines if "measured by loss_kw" in l]
            self.assertTrue(said, "\n".join(lines))
            # the scratch copy is in no checkout of its own, so the reason names no commit
            self.assertNotIn(" at ", said[0].split("->")[-1])
            commit, label = R._commit_of(str(ROOT))
            self.assertTrue(label.startswith(commit), (commit, label))


class TheRepositoryTests(unittest.TestCase):
    """This repository's own record names recipes for its tree-facts, and its tree measures what
    it says - the step the pull request runs, run here."""

    def test_the_tree_measures_what_the_record_says(self):
        code, out, err = kp("remeasure", "--run", cwd=str(ROOT))
        self.assertEqual(code, 0, out + err)
        for line in ("  m.page_string_lines: 0 - as recorded (page_string_lines)",
                     "  p.recipe_runners: 1 - as recorded (recipe_runners)",
                     "  p.hook_slot: 2000 - as recorded (hook_slot)"):
            self.assertIn(line, out)
        self.assertTrue(out.rstrip().endswith("the record holds what this tree measures"), out)

    def test_nothing_that_reads_the_record_imports_the_runner(self):
        for name in ("provenance.py", "consolidate.py", "sameness.py", "render_page.py", "cli.py"):
            text = (SCRIPTS / name).read_text(encoding="utf-8")
            self.assertNotIn("import remeasure", text, name)
            self.assertNotIn("run_recipe", text, name)        # nothing but the runner runs one
            self.assertNotIn("allowlist_path", text, name)    # or resolves the list it runs from


if __name__ == "__main__":
    unittest.main()
