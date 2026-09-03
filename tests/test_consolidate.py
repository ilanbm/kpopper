"""Consolidation is a test: the record with its hypotheses laid over it, checked with the
reader's own check; the fold when the check is clean; a negative finding on refutation;
another branch's committed record read as one more hypothesis. Runs against the fixture in
tests/fixtures/consolidate - a base and two hypotheses that disagree about the loss - with no
browser and no network; the branch scenarios build a throwaway git repository:

    python3 -m unittest discover -s tests
"""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "consolidate"
RECORD = FIXTURE / "PROVENANCE.yaml"
PAGE_FIXTURE = ROOT / "tests" / "fixtures" / "page"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402
import consolidate as C  # noqa: E402

RECOUNT_BLOCK = ('  s.2026_09_03_recount:\n'
                 '    asked: "Recount the loss with the north wall as it is, not as the plan drew it."\n'
                 '    name: "the recount, as this session read it: the same wall at its real U-value"\n'
                 '    of: "this session"\n'
                 '    read: "2026-09-03"\n')
LOSS_BEFORE = ('  heat.loss_kw:\n    v: 31\n    unit: kW\n    name: "heat loss on a -5°C night"\n'
               '    from: s.2026_09_02_heating\n'
               '    at: "worked out from the glazing area during the session"\n    of: "2026-09-02"\n')
LOSS_AFTER = ('  heat.loss_kw:\n    v: 28\n    unit: kW\n    name: "heat loss on a -5°C night"\n'
              '    from: s.2026_09_03_recount\n    at: "the north wall at its measured U-value"\n'
              '    of: "2026-09-03"\n')
HEADINGS = ("arrived (", "updates (", "moved / falsified (", "contested (", "candidates (",
            "new subjects (")


def run(*args, cwd=None):
    """The scripts as a session runs them: a subprocess, its exit code and both streams."""
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd,
                       capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def kp(*args, cwd=None):
    return run(SCRIPTS / "kpopper", *args, cwd=cwd)


def copy_fixture(into):
    """A scratch copy of the fixture - the base, its brief and the hypotheses beside it."""
    shutil.copytree(FIXTURE, into, dirs_exist_ok=True)
    return into / "PROVENANCE.yaml"


def git(d, *args):
    p = subprocess.run(["git", "-c", "user.email=t@example.test", "-c", "user.name=t",
                        "-c", "commit.gpgsign=false"] + list(args), cwd=str(d),
                       capture_output=True, text=True)
    if p.returncode:
        raise AssertionError(" ".join(args) + "\n" + p.stdout + p.stderr)
    return p.stdout


def order_of(out):
    """Where each heading of the report stands, in the order printed."""
    return [out.index(h) for h in HEADINGS if h in out]


class TheDryRunTests(unittest.TestCase):
    """The base with the hypotheses laid over it, checked; a report in fixed order; nothing
    written."""

    def test_a_contested_id_stops_the_run_and_shows_both_sources(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = {p: p.read_text(encoding="utf-8") for p in pathlib.Path(d).rglob("*.yaml")}
            code, out, err = kp("consolidate", "--dry-run", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("the base with bigger_boiler, glazing_redo laid over it, in name order\n", out)
            self.assertIn("  bigger_boiler (born 2026-09-03, ", out)
            self.assertIn(", never folds): a 36 kW boiler holds the greenhouse even with the wind", out)
            self.assertIn("contested (1): an id two hypotheses hold with different claims - the union has no "
                          "value to lay, so the run stops here\n  heat.loss_kw:\n"
                          "    the base holds heat.loss_kw: 31 (heat loss on a -5°C night) <- s.2026_09_02_heating, "
                          "at worked out from the glazing area duri ...\n"
                          "    bigger_boiler says heat.loss_kw: 33 (heat loss on a -5°C night) <- s.2026_09_02_heating, "
                          "at the session's figure plus 2 kW for th ...\n"
                          "    glazing_redo says heat.loss_kw: 28 (heat loss on a -5°C night) <- s.2026_09_03_recount, "
                          "at the north wall at its measured U-valu ...\n", out)
            self.assertIn("re-read against the merged tree", out)
            # the run stopped: no other section was printed, and the fold is refused too
            self.assertNotIn("arrived (", out)
            code, out, err = kp("consolidate", rec)
            self.assertEqual(code, 1)
            self.assertIn("refused - a contested id stops the fold: heat.loss_kw", out + err)
            self.assertEqual({p: p.read_text(encoding="utf-8") for p in pathlib.Path(d).rglob("*.yaml")},
                             before)

    def test_a_consistent_hypothesis_is_clean_and_the_report_keeps_its_order(self):
        code, out, err = kp("consolidate", "--dry-run", "glazing_redo", RECORD)
        self.assertEqual(code, 0, out + err)
        self.assertIn("the base with glazing_redo laid over it\n"
                      "  glazing_redo (born 2026-09-03, ", out)
        self.assertIn("arrived (1): what the fold would add\n"
                      "  s.2026_09_03_recount:  (the recount, as this session read it: the same wall at its "
                      "real U- ... - from glazing_redo\n"
                      "updates (1): what the base holds that a hypothesis replaces, and what rests on each\n"
                      "  heat.loss_kw: 31 -> 28, from glazing_redo\n"
                      "    a reading from 2026-09-03 that is newer than the base's\n"
                      "    worked out from it: heat.deficit_kw\n"
                      "    MUTED     c.boiler_short: heat.loss_kw moved 31 -> 28, inside wrong_if "
                      "(heat.loss_kw <= heat.boiler_kw) - nothing is asked\n"
                      "moved / falsified (0): what the union moves or breaks\n"
                      "contested (0)\n"
                      "candidates (0): pairs for a person to judge as the same subject or distinct\n"
                      "new subjects (0): prefixes the base does not hold\n"
                      "\nclean: glazing_redo may fold - consolidate glazing_redo\n", out)
        self.assertEqual(order_of(out), sorted(order_of(out)))
        self.assertEqual(len(order_of(out)), len(HEADINGS))

    def test_a_falsifier_that_holds_is_red(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = kp("consolidate", "--dry-run", "bigger_boiler", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("arrived (2): what the fold would add\n"
                          "  + c.boiler_enough: a 36 kW boiler holds 12°C on the coldest night, wind included - "
                          "from bigger_boiler\n"
                          "    HOLDS     c.boiler_enough: wrong_if does not hold (heat.loss_kw > heat.boiler_kw)\n"
                          "  doc.boiler_catalogue:  (the supplier's catalogue, the 36 kW model) as of 2026-09-03 - "
                          "from bigger_boiler\n", out)
            self.assertIn("  heat.boiler_kw: 24 -> 36, from bigger_boiler\n", out)
            self.assertIn("    FIRED     c.boiler_short: wrong_if holds (heat.loss_kw <= heat.boiler_kw) - broken "
                          "by its own condition\n", out)
            self.assertIn("moved / falsified (1): what the union moves or breaks\n"
                          "  FALSIFIED c.boiler_short: wrong_if holds (heat.loss_kw <= heat.boiler_kw) - broken by "
                          "its own condition\n", out)
            self.assertIn("\nnot clean: a falsifier holds - nothing folds until it is read again\n", out)
            # a what-if never folds, red or not; and the fold refuses before it looks
            code, out, err = kp("consolidate", "bigger_boiler", rec)
            self.assertEqual(code, 1)
            self.assertIn("refused - the dry run is not clean; nothing folds until it is", out + err)
            self.assertTrue((pathlib.Path(d) / "PROVENANCE.d" / "bigger_boiler.yaml").exists())

    def test_a_hypothesis_own_falsifier_is_evaluated(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            h = pathlib.Path(d) / "PROVENANCE.d" / "glazing_redo.yaml"
            # the head says the hypothesis is wrong if the loss is still 31 or more: under the union
            # it is 28, so the head holds; move the line and it falls
            h.write_text(h.read_text(encoding="utf-8").replace('wrong_if: "heat.loss_kw >= 31"',
                                                                'wrong_if: "heat.loss_kw < 30"'),
                         encoding="utf-8")
            code, out, err = kp("consolidate", "--dry-run", "glazing_redo", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("  FALSIFIED glazing_redo: its own wrong_if holds (heat.loss_kw < 30)\n", out)

    def test_a_same_day_reading_is_contested_until_someone_reads_again(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35", "--as-of",
                                 "2026-09-02", "--hypothesis", "heat_loss_kw", rec)
            self.assertEqual(code, 0, out + err)
            before = rec.read_text(encoding="utf-8")
            code, out, err = kp("consolidate", "--dry-run", "heat_loss_kw", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("  heat.loss_kw: 31 -> 35, from heat_loss_kw\n"
                          "    a reading of the same day - the base keeps what it holds\n", out)
            self.assertIn("contested (1): the door refuses the reading, so the base keeps what it holds\n"
                          "  heat.loss_kw: a reading of the same day\n"
                          "    the base holds heat.loss_kw: 31 (heat loss on a -5°C night) <- ", out)
            self.assertIn("    heat_loss_kw says heat.loss_kw: 35 (heat loss on a -5°C night) <- ", out)
            self.assertIn("  read again on a later day - set it in the base or in the hypothesis with --as-of - "
                          "or refute the hypothesis\n", out)
            self.assertIn("\nnot clean: a contested reading - nothing folds until it is read again\n", out)
            code, out, err = kp("consolidate", "heat_loss_kw", rec)
            self.assertEqual(code, 1)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            # read again on a later day, in the hypothesis: the door opens
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35", "--as-of", "2026-09-03",
                "--hypothesis", "heat_loss_kw", rec)
            code, out, err = kp("consolidate", "--dry-run", "heat_loss_kw", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("    a reading from 2026-09-03 that is newer than the base's\n", out)
            self.assertIn("\nclean: heat_loss_kw may fold - consolidate heat_loss_kw\n", out)
            code, out, err = kp("consolidate", "heat_loss_kw", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            text = rec.read_text(encoding="utf-8")
            self.assertIn("  heat.loss_kw:\n    v: 35\n", text)
            self.assertFalse((pathlib.Path(d) / "PROVENANCE.d" / "heat_loss_kw.yaml").exists())
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_a_premise_that_moved_blocks_the_fold_and_not_the_build(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.margin",
                                 "rests_on=[heat.boiler_kw, heat.loss_kw]", "verdict=there is a margin",
                                 "wrong_if=heat.loss_kw > 40", "--as-of", "2026-09-03",
                                 "--hypothesis", "glazing_redo", rec)
            self.assertEqual(code, 0, out + err)
            # the base moves the premise the hypothesis's judgment snapshotted at 24
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.boiler_kw", "26", "--as-of",
                                 "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            before = rec.read_text(encoding="utf-8")
            code, out, err = kp("consolidate", "--dry-run", "glazing_redo", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("  + c.margin: there is a margin - from glazing_redo\n"
                          "    MOVED     c.margin: heat.boiler_kw moved 24 -> 26 since it was reviewed - if it "
                          "still holds: review c.margin\n", out)
            self.assertIn("moved / falsified (1): what the union moves or breaks\n"
                          "  MOVED c.margin: heat.boiler_kw differs from its snapshot (24 -> 26) - re-review, or "
                          "refresh seen\n", out)
            self.assertIn("\nmoved: 1 judgment to re-review before the fold - a premise moved under it\n", out)
            code, out, err = kp("consolidate", "glazing_redo", rec)
            self.assertEqual(code, 1)
            self.assertIn("refused - the dry run is not clean; nothing folds until it is", out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            # reviewed in the hypothesis, against the record as it stands under it, it folds
            code, out, err = run(SCRIPTS / "provenance.py", "review", "c.margin", "--as-of", "2026-09-04",
                                 "--hypothesis", "glazing_redo", rec)
            self.assertEqual(code, 0, out + err)
            code, out, err = kp("consolidate", "glazing_redo", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("carry c.margin from glazing_redo into judgments, after c.boiler_short\n", out)
            self.assertIn("    seen: {heat.boiler_kw: 26, heat.loss_kw: 28}\n", rec.read_text(encoding="utf-8"))
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_a_hole_names_the_hypothesis_that_holds_the_entry(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            (pathlib.Path(d) / "PROVENANCE.d" / "catalogue.yaml").write_text(
                'hypothesis: {claim: "the catalogue can be trusted", born: "2026-09-03"}\n'
                "judgments:\n  c.catalogue:\n    rests_on: [doc.boiler_catalogue, heat.boiler_kw]\n"
                '    verdict: "the catalogue figure stands"\n    wrong_if: "heat.boiler_kw < 10"\n'
                '    seen: {doc.boiler_catalogue: "read 2026-09-03", heat.boiler_kw: 36}\n', encoding="utf-8")
            code, out, err = kp("consolidate", "--dry-run", "catalogue", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("    BROKEN    c.catalogue: rests on doc.boiler_catalogue, which is not an entry\n", out)
            self.assertIn("  FAIL c.catalogue: rests on doc.boiler_catalogue, which is not an entry - held by "
                          "hypothesis bigger_boiler: consolidate them together\n", out)
            self.assertIn("\nnot clean: a hole - nothing folds until it is read again\n", out)

    def test_a_judgment_replaces_the_standing_one_only_through_the_door(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short",
                                 "rests_on=[heat.boiler_kw, heat.loss_kw]",
                                 "verdict=the old boiler holds after all", "wrong_if=heat.loss_kw > 40",
                                 "--as-of", "2026-09-03", "--hypothesis", "c_boiler_short", rec)
            self.assertEqual(code, 0, out + err)
            code, out, err = kp("consolidate", "--dry-run", "c_boiler_short", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("  c.boiler_short: the old boiler cannot hold 12°C on … -> the old boiler holds "
                          "after all, from c_boiler_short\n"
                          "    the standing judgment holds, and no request: names a person's asking for the "
                          "change - the base keeps what it holds\n", out)
            self.assertIn("    the base holds + c.boiler_short: the old boiler cannot hold 12°C on the coldest "
                          "February night\n    c_boiler_short says + c.boiler_short: the old boiler holds after "
                          "all\n", out)
            # the standing judgment breaks on a newer reading: its wrong_if holds, the door opens
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", "2026-09-04", rec)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 1)
            code, out, err = kp("consolidate", "c_boiler_short", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("    its wrong_if holds (heat.loss_kw <= heat.boiler_kw)\n", out)
            self.assertIn("replace c.boiler_short with what c_boiler_short holds, where it stands\n"
                          "folded c_boiler_short: 0 entries and 1 judgment - 0 added, 1 replaced\n", out)
            text = rec.read_text(encoding="utf-8")
            self.assertEqual(text.count("c.boiler_short:"), 1)
            self.assertIn("  c.boiler_short:\n    rests_on: [heat.boiler_kw, heat.loss_kw]\n"
                          '    verdict: "the old boiler holds after all"\n    wrong_if: "heat.loss_kw > 40"\n'
                          "    seen: {heat.boiler_kw: 24, heat.loss_kw: 31}\n", text)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_new_subjects_are_prefixes_the_base_lacks(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "add", "glaze.saving_kw", "v=4", "unit=kW", "name=loss stopped",
                "from=s.2026_09_03_recount", "--as-of", "2026-09-03", "--hypothesis", "glazing_redo", rec)
            code, out, err = kp("consolidate", "--dry-run", "glazing_redo", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("  glaze.saving_kw: 4 (loss stopped) <- s.2026_09_03_recount - from glazing_redo\n", out)
            self.assertIn("new subjects (1): prefixes the base does not hold\n  glaze: glaze.saving_kw\n", out)

    def test_a_name_nothing_holds_is_refused(self):
        code, out, err = kp("consolidate", "--dry-run", "nosuch", RECORD)
        self.assertEqual(code, 1)
        self.assertIn("refused - no hypothesis named nosuch beside the record (there: bigger_boiler, "
                      "glazing_redo)", out + err)
        code, out, err = kp("consolidate", "--dry-run", PAGE_FIXTURE / "PROVENANCE.yaml")
        self.assertEqual(code, 0, out + err)
        self.assertEqual(out, "no hypotheses beside the record - nothing to consolidate\n")

    def test_a_hypothesis_the_reader_cannot_read_is_refused_before_the_test(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            (pathlib.Path(d) / "PROVENANCE.d" / "broken.yaml").write_text("hypothesis: [1, 2\n", encoding="utf-8")
            code, out, err = kp("consolidate", "--dry-run", "glazing_redo", rec)
            self.assertEqual(code, 1)
            self.assertIn("refused - hypothesis broken could not be read: while parsing", out + err)


class TheFold(unittest.TestCase):
    """The union written into the base with the reader's own edits: blocks carried over whole,
    in id order, the folded file deleted, the files to commit printed."""

    def test_a_consistent_hypothesis_folds_and_the_diff_is_its_blocks(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            git(d, "init", "-q", "-b", "main")
            git(d, "add", "-A")
            git(d, "commit", "-qm", "base")
            before = rec.read_text(encoding="utf-8")
            code, out, err = kp("consolidate", "glazing_redo", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("\nclean: glazing_redo may fold - consolidate glazing_redo\n\n"
                          "replace heat.loss_kw with what glazing_redo holds, where it stands\n"
                          "carry s.2026_09_03_recount from glazing_redo into sources, after s.2026_09_02_heating\n"
                          "folded glazing_redo: 2 entries and 0 judgments - 1 added, 1 replaced\n"
                          "files to commit: PROVENANCE.yaml, PROVENANCE.d/glazing_redo.yaml (deleted)\n\n"
                          "the record needs a person on 0 judgments - check says the rest\n", out)
            expected = before.replace("  updated: 2026-09-03\n", "  updated: 2026-09-04\n", 1)
            expected = expected.replace(LOSS_BEFORE, LOSS_AFTER, 1)
            expected = expected.replace('    read: "2026-09-02"\n  doc.boiler_sheet:',
                                        '    read: "2026-09-02"\n' + RECOUNT_BLOCK + "  doc.boiler_sheet:", 1)
            self.assertEqual(rec.read_text(encoding="utf-8"), expected)
            self.assertFalse((pathlib.Path(d) / "PROVENANCE.d" / "glazing_redo.yaml").exists())
            self.assertTrue((pathlib.Path(d) / "PROVENANCE.d" / "bigger_boiler.yaml").exists())
            self.assertEqual(sorted(git(d, "status", "--short").split("\n")),
                             ["", " D PROVENANCE.d/glazing_redo.yaml", " M PROVENANCE.yaml"])
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("1 judgments, 9 entries, 0 problems", out)
            # what was folded reads as the base now: pull shows the recount as the source
            code, out, _ = run(SCRIPTS / "provenance.py", "pull", "heat.loss_kw", rec)
            self.assertIn("heat.loss_kw: 28 (heat loss on a -5°C night) <- s.2026_09_03_recount, at the north "
                          "wall at its measured U-valu ...\n", out)
            self.assertIn("    proposes 28 -> 33, from bigger_boiler\n", out)

    def test_the_folded_directory_goes_when_it_is_empty(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            os.remove(pathlib.Path(d) / "PROVENANCE.d" / "bigger_boiler.yaml")
            code, out, err = kp("consolidate", rec)
            self.assertEqual(code, 0, out + err)
            self.assertFalse((pathlib.Path(d) / "PROVENANCE.d").exists())
            self.assertEqual(kp("consolidate", "--dry-run", rec)[1],
                             "no hypotheses beside the record - nothing to consolidate\n")

    def test_a_what_if_is_evaluated_and_never_written(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            h = pathlib.Path(d) / "PROVENANCE.d" / "glazing_redo.yaml"
            h.write_text(h.read_text(encoding="utf-8").replace('born: "2026-09-03"\n',
                                                                'born: "2026-09-03"\n  folds: never\n'),
                         encoding="utf-8")
            code, out, err = kp("consolidate", "--dry-run", "glazing_redo", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("\nclean - and nothing here folds: glazing_redo is a what-if, evaluated and never "
                          "written\n", out)
            code, out, err = kp("consolidate", "glazing_redo", rec)
            self.assertEqual(code, 1)
            self.assertIn("refused - glazing_redo never folds: a what-if is evaluated and never written; "
                          "consolidate the others by name", out + err)
            self.assertTrue(h.exists())

    def test_nothing_to_fold_is_said(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            (pathlib.Path(d) / "PROVENANCE.d" / "same.yaml").write_text(
                "known:\n  heat.boiler_kw:\n    v: 24\n    unit: kW\n    name: \"boiler output\"\n"
                "    from: doc.boiler_sheet\n", encoding="utf-8")
            code, out, err = kp("consolidate", "same", rec)
            self.assertEqual(code, 1)
            self.assertIn("refused - nothing to fold: the base already holds everything same proposes", out + err)


class TheRefutation(unittest.TestCase):
    """One negative finding in the base, the file gone, nothing else of the hypothesis."""

    def test_refute_writes_exactly_one_finding_and_deletes_the_file(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            code, out, err = kp("consolidate", "--refute", "glazing_redo",
                                "the recount used the plan's U-value, not the wall's", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("add hyp.glazing_redo into known, before when.first_cold_night\n", out)
            self.assertIn("refuted glazing_redo: hyp.glazing_redo holds its claim as a negative finding, from "
                          "s.2026_09_02_heating; PROVENANCE.d/glazing_redo.yaml deleted, and nothing else of it "
                          "enters\nfiles to commit: PROVENANCE.yaml, PROVENANCE.d/glazing_redo.yaml (deleted)\n", out)
            finding = ('  hyp.glazing_redo:\n    v: refuted\n'
                       '    name: "the loss was overcounted - the north wall is double glazed already"\n'
                       '    from: s.2026_09_02_heating\n'
                       '    at: "the recount used the plan\'s U-value, not the wall\'s"\n'
                       '    of: "2026-09-04"\n')
            expected = before.replace("  updated: 2026-09-03\n", "  updated: 2026-09-04\n", 1)
            expected = expected.replace("  when.first_cold_night:", finding + "  when.first_cold_night:", 1)
            self.assertEqual(rec.read_text(encoding="utf-8"), expected)
            self.assertFalse((pathlib.Path(d) / "PROVENANCE.d" / "glazing_redo.yaml").exists())
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
            code, out, _ = run(SCRIPTS / "provenance.py", "pull", "hyp", rec)
            self.assertIn("hyp.glazing_redo: refuted (the loss was overcounted - the north wall is double glazed "
                          "already) <- s.2026_09_02 ...\n", out)
            # the finding is countable by its value, from the reader's own view
            doc = P.load([str(rec)])
            self.assertEqual([k for k, b in P.bodies(doc).items()
                              if isinstance(b, dict) and b.get("v") == "refuted"], ["hyp.glazing_redo"])
            # a second refutation of a hypothesis reborn under that name is a second finding
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "29", "--as-of", "2026-09-02",
                "--hypothesis", "glazing_redo", rec)
            code, out, err = kp("consolidate", "--refute", "glazing_redo", "still the plan's figure",
                                "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("  hyp.glazing_redo_2:\n    v: refuted\n    name: \"hypothesis glazing_redo\"\n",
                          rec.read_text(encoding="utf-8"))

    def test_the_source_is_the_newest_session_unless_named(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_04_merge", "asked=Fold what holds.",
                "name=the merge", "read=2026-09-04", "--as-of", "2026-09-04", rec)
            code, out, err = kp("consolidate", "--refute", "bigger_boiler", "the catalogue is a sales sheet",
                                "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("from s.2026_09_04_merge;", out)
            code, out, err = kp("consolidate", "--refute", "glazing_redo", "why", "--as", "doc.boiler_sheet", rec)
            self.assertEqual(code, 1)
            self.assertIn("refused - doc.boiler_sheet is not a session source carrying what it was asked", out + err)
            code, out, err = kp("consolidate", "--refute", "glazing_redo", "why", "--as", "s.2026_09_02_heating",
                                "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("from s.2026_09_02_heating;", out)

    def test_refute_needs_a_why_and_a_hypothesis(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = {p: p.read_text(encoding="utf-8") for p in pathlib.Path(d).rglob("*.yaml")}
            for args, why in ((["--refute", "glazing_redo"], "a refutation says why"),
                              (["--refute", "nosuch", "why"], "no hypothesis named nosuch beside the record"),
                              (["--refute", "glazing_redo", "why", "--dry-run"],
                               "--refute takes one hypothesis and its why, and nothing else"),
                              (["--dry-run", "--as", "s.2026_09_02_heating"], "--as names the session source of a "
                               "refutation: it goes with --refute"),
                              (["--nope"], "--nope is not an option of consolidate")):
                code, out, err = kp("consolidate", *args, rec)
                self.assertEqual(code, 1, args)
                self.assertIn(why, out + err, args)
            self.assertEqual({p: p.read_text(encoding="utf-8") for p in pathlib.Path(d).rglob("*.yaml")}, before)

    def test_the_help_defines_the_finding_and_the_dispatcher_names_the_command(self):
        code, out, _ = kp("consolidate", "--help")
        self.assertEqual(code, 0)
        self.assertIn("  consolidate [--dry-run] [<hypothesis> ...]", out)
        self.assertIn("    v: refuted              the value every negative finding of this kind carries", out)
        self.assertIn("--from <ref>", out)
        code, out, _ = kp()
        self.assertIn("kpopper consolidate", out)
        self.assertIn("pull <seed> --from REF", out)


def branch_with_a_recount(d):
    """A throwaway repository: the fixture base on main, and a branch that read the loss
    again - the same writes glazing_redo holds, committed as a branch's own record."""
    rec = pathlib.Path(d) / "PROVENANCE.yaml"
    shutil.copy(FIXTURE / "PROVENANCE.yaml", rec)
    shutil.copy(FIXTURE / "PROVENANCE.view.yaml", pathlib.Path(d) / "PROVENANCE.view.yaml")
    git(d, "init", "-q", "-b", "main")
    git(d, "add", "-A")
    git(d, "commit", "-qm", "base")
    git(d, "switch", "-qc", "recount")
    for args in (["add", "s.2026_09_03_recount",
                  "asked=Recount the loss with the north wall as it is, not as the plan drew it.",
                  "name=the recount, as this session read it: the same wall at its real U-value",
                  "of=this session", "read=2026-09-03", "--as-of", "2026-09-03"],
                 ["set", "heat.loss_kw", "28", "--as-of", "2026-09-03", "--why", "the north wall at its "
                  "measured U-value"],
                 ["add", "c.margin", "rests_on=[heat.boiler_kw, heat.loss_kw]", "verdict=there is a margin",
                  "wrong_if=heat.loss_kw > 40", "--as-of", "2026-09-03"],
                 ["review", "c.boiler_short", "--as-of", "2026-09-03"]):
        code, out, err = run(SCRIPTS / "provenance.py", *args, rec)
        assert code == 0, out + err
    git(d, "add", "-A")
    git(d, "commit", "-qm", "recount")
    git(d, "switch", "-q", "main")
    return rec


class AnotherBranch(unittest.TestCase):
    """Another branch's committed record, read as hypotheses named after the ref - the same
    union, the same report, the same refusal; a pull, never a push."""

    def test_a_branch_is_read_as_a_hypothesis_named_after_the_ref(self):
        with tempfile.TemporaryDirectory() as d:
            rec = branch_with_a_recount(d)
            before = rec.read_text(encoding="utf-8")
            code, out, err = kp("consolidate", "--dry-run", "--from", "recount", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("the base with recount laid over it\n  recount (born ", out)
            self.assertIn("): what recount committed (", out)
            self.assertIn("), read as a hypothesis\n", out)
            self.assertIn("arrived (2): what the fold would add\n"
                          "  + c.margin: there is a margin - from recount\n"
                          "    HOLDS     c.margin: wrong_if does not hold (heat.loss_kw > 40)\n"
                          "  s.2026_09_03_recount:  (the recount, as this session read it: the same wall at its "
                          "real U-value ... - from recount\n"
                          "updates (1): what the base holds that a hypothesis replaces, and what rests on each\n"
                          "  heat.loss_kw: 31 -> 28, from recount\n"
                          "    a reading from 2026-09-03 that is newer than the base's\n", out)
            # the branch's review of c.boiler_short - the same verdict, a refreshed seen - is
            # the branch's own business: it does not arrive
            self.assertNotIn("c.boiler_short:", out.split("updates (1)")[0])
            self.assertIn("\nclean: recount may fold - consolidate recount\n", out)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            code, out, err = kp("consolidate", "--from", "recount", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("folded recount: 2 entries and 1 judgment - 2 added, 1 replaced\n"
                          "files to commit: PROVENANCE.yaml\n"
                          "  nothing to delete for recount: another branch keeps its own record\n", out)
            text = rec.read_text(encoding="utf-8")
            self.assertIn(RECOUNT_BLOCK, text)
            self.assertIn("  heat.loss_kw:\n    v: 28\n    # set 2026-09-03: the north wall at its measured "
                          "U-value\n", text)
            self.assertIn("  c.margin:\n", text)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
            self.assertEqual(git(d, "status", "--short").strip(), "M PROVENANCE.yaml")

    def test_pull_from_shows_what_the_branch_proposes(self):
        with tempfile.TemporaryDirectory() as d:
            rec = branch_with_a_recount(d)
            code, out, err = kp("pull", "heat.loss_kw", "--from", "recount", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("heat.loss_kw: 31 (heat loss on a -5°C night) <- s.2026_09_02_heating, at worked out "
                          "from the glazing area duri ...\n    proposes 31 -> 28, from recount\n", out)
            self.assertIn("+ c.margin (in hypothesis recount): there is a margin\n", out)
            # without --from, pull is the reader's own
            code, out, err = kp("pull", "heat.loss_kw", rec)
            self.assertEqual(code, 0, out + err)
            self.assertNotIn("recount", out)
            code, out, err = kp("pull", "--from", "recount", rec)
            self.assertEqual(code, 1)
            self.assertIn("pull needs a seed", out + err)

    def test_the_branch_hypothesis_files_come_along_unless_this_record_holds_them(self):
        with tempfile.TemporaryDirectory() as d:
            rec = branch_with_a_recount(d)
            git(d, "switch", "-q", "recount")
            shutil.copytree(FIXTURE / "PROVENANCE.d", pathlib.Path(d) / "PROVENANCE.d")
            git(d, "add", "-A")
            git(d, "commit", "-qm", "hypotheses")
            git(d, "switch", "-q", "main")
            code, out, err = kp("consolidate", "--dry-run", "--from", "recount", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("the base with recount, recount:bigger_boiler, recount:glazing_redo laid over it, in "
                          "name order\n", out)
            self.assertIn("  recount:bigger_boiler says heat.loss_kw: 33", out)
            # the same file beside this record is the same hypothesis, not a second holder
            shutil.copytree(FIXTURE / "PROVENANCE.d", pathlib.Path(d) / "PROVENANCE.d")
            code, out, err = kp("consolidate", "--dry-run", "--from", "recount", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("the base with bigger_boiler, glazing_redo, recount laid over it, in name order\n", out)
            self.assertNotIn("recount:", out)
            # named, the ref's hypotheses are chosen like any other
            code, out, err = kp("consolidate", "--dry-run", "--from", "recount", "recount", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("the base with recount laid over it\n", out)

    def test_a_ref_nothing_knows_and_a_record_outside_a_checkout_are_refused(self):
        with tempfile.TemporaryDirectory() as d:
            rec = branch_with_a_recount(d)
            code, out, err = kp("consolidate", "--dry-run", "--from", "nosuch", rec)
            self.assertEqual(code, 1)
            self.assertIn("refused - nosuch is not a commit this checkout knows", out + err)
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = kp("consolidate", "--dry-run", "--from", "main", rec)
            self.assertEqual(code, 1)
            self.assertIn("is not in a git checkout", out + err)


class TheCallable(unittest.TestCase):
    """The union-and-re-check as a function: a what-if held in memory asks it and reads the
    answer, and nothing touches the disk."""

    def test_a_what_if_in_memory_is_evaluated(self):
        doc = P.load([str(RECORD)])
        h = C.hypothesis("colder", {"known": {"heat.loss_kw": {"v": 20, "unit": "kW",
                                                              "name": "heat loss on a -5°C night",
                                                              "of": "2026-09-04"}}},
                         head={"claim": "what if the loss were 20", "folds": "never"})
        c = C.union_of(doc, [h])
        self.assertEqual([k for k, _ in c.arrived], [])
        self.assertEqual([(k, old, new, may) for k, _, old, new, may, _ in c.updates],
                         [("heat.loss_kw", 31, 20, True)])
        self.assertEqual(c.falsified, ["c.boiler_short: wrong_if holds (heat.loss_kw <= heat.boiler_kw) - "
                                       "broken by its own condition"])
        self.assertTrue(c.red)
        self.assertEqual(P.value_of(c.raw, c.ids, "heat.loss_kw"), 20)
        lines = C.report(c)
        self.assertEqual(lines[0], "the base with colder laid over it")
        self.assertEqual(lines[1], "  colder (never folds): what if the loss were 20")
        self.assertIn("  FALSIFIED c.boiler_short: wrong_if holds (heat.loss_kw <= heat.boiler_kw) - broken by "
                      "its own condition", lines)
        self.assertEqual(lines[-1], "not clean: a falsifier holds - nothing folds until it is read again")
        with self.assertRaises(SystemExit):
            C.fold([str(RECORD)], c)
        self.assertEqual(P.load([str(RECORD)]).hypotheses.keys(), doc.hypotheses.keys())

    def test_the_union_is_read_the_way_the_reader_reads(self):
        doc, hyps = C.read([str(RECORD)], ["glazing_redo"])
        self.assertEqual([h["name"] for h in hyps], ["glazing_redo"])
        c = C.union_of(doc, hyps)
        self.assertFalse(c.blocked)
        self.assertIn("s.2026_09_03_recount", c.ids)
        self.assertEqual(P.value_of(c.raw, c.ids, "heat.loss_kw"), 28)
        self.assertEqual(sorted(c.jud), ["c.boiler_short"])
        self.assertEqual(c.moved, [])
        self.assertEqual(c.new_subjects, [])
        # the whole set is contested before anything is laid
        doc, hyps = C.read([str(RECORD)])
        c = C.union_of(doc, hyps)
        self.assertEqual(list(c.contested), ["heat.loss_kw"])
        self.assertIsNone(c.doc)
        self.assertTrue(c.red)


if __name__ == "__main__":
    unittest.main()
