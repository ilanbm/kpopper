"""A session's prior as a source, and the judgment decided on it: the prior.* claim whose
value is the confidence, the reopened_by field the reader accepts beside blocked_on - a
decided judgment that names the sign a person reads, never a hole and never waiting - and
the count of how much of a record stands on a session's own confidence.
Runs against the fixture record in tests/fixtures/priors with no browser and no network:

    python3 -m unittest discover -s tests
"""
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "priors"
RECORD = FIXTURE / "PROVENANCE.yaml"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402
import render_page as R  # noqa: E402

DECIDED = "c.order_is_the_contract"
PRIOR = "prior.macros_bind_by_position"
# the fixture's other prior-resting judgment: a local claim below the line, tried rather
# than decided, so the count has one judgment on each side of it
TRIED = "c.monthly_export_regenerated"
LOW = "prior.regeneration_is_deterministic"
OWN = ROOT / "PROVENANCE.yaml"                      # this project's own record
NO_PRIORS = ROOT / "tests" / "fixtures" / "hypotheses" / "PROVENANCE.yaml"
COUNT = "{} judgments rest on prior.* claims, {} of them on a prior at 0.8 or above"


def run(*args, cwd=None):
    """The scripts as a session runs them: a subprocess, its exit code and both streams."""
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd,
                       capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def copy_fixture(into):
    """A scratch copy of the fixture, for tests that edit it."""
    shutil.copy(RECORD, into / "PROVENANCE.yaml")
    return into / "PROVENANCE.yaml"


def edit(path, old, new):
    text = path.read_text(encoding="utf-8")
    assert old in text, f"{old!r} is not in {path.name}"
    path.write_text(text.replace(old, new), encoding="utf-8")


def read(path):
    doc = P.load([str(path)])
    ids, jud, fields = P.infer(doc)
    return doc, ids, jud, fields, P.with_builtins(doc, ids, jud, fields)


class TheReaderAcceptsAReopener(unittest.TestCase):
    def test_fixture_is_green_and_the_reopener_is_declared(self):
        code, out, err = run(SCRIPTS / "provenance.py", "check", RECORD)
        self.assertEqual(code, 0, out + err)
        self.assertNotIn("FAIL", out)
        self.assertIn(f"NOTE {DECIDED}: no predicate at all - decided; reopened by: a macro "
                      f"that breaks on an export whose column order did not change", out)
        self.assertNotIn("blocked", out.lower())
        self.assertRegex(out, r"\n3 judgments, 9 entries, 0 problems, 2 declared\n")

    def test_a_decided_judgment_needs_no_person(self):
        doc, ids, jud, fields, raw = read(RECORD)
        self.assertEqual(P.flags(ids, jud, fields, raw)[DECIDED], set())
        counts = P.counts(doc, ids, jud, fields, P.bodies(doc))
        self.assertEqual((counts["graph.flagged"], counts["graph.blocked"],
                          counts["graph.no_predicate"]), (0, 0, 0))
        _, out, _ = run(SCRIPTS / "provenance.py", "open", "--chars", "2000", RECORD)
        self.assertIn("nothing needs a person right now.", out)
        self.assertIn(f"  = {DECIDED}: the column order of the export is a contract", out)

    def test_pull_reads_the_reopener_under_a_judgment_that_holds(self):
        _, out, _ = run(SCRIPTS / "provenance.py", "pull", DECIDED, RECORD)
        self.assertIn(f"+ {DECIDED}: the column order of the export is a contract", out)
        self.assertIn("\n    holds\n", out)
        self.assertIn("    because: Macros bind by position - 0.9 confident - and the last three "
                      "exports changed their header 0", out)
        self.assertIn("    reopened by: a macro that breaks on an export whose column order did "
                      "not change - the binding would then", out)
        self.assertNotIn("blocked", out)
        under = out.split(f"+ {DECIDED}:")[1].split("\n+ ")[0]
        self.assertNotIn("wrong_if", under)

    def test_without_the_reopener_the_same_judgment_fails(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            text = rec.read_text(encoding="utf-8")
            text = re.sub(r"    reopened_by: \"[^\"]*\"\n", "", text, flags=re.S)
            rec.write_text(text, encoding="utf-8")
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn(f"FAIL {DECIDED}: no predicate at all - and nothing says why not", out)

    def test_a_reopener_does_not_excuse_a_missing_dependency(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, f"rests_on: [{PRIOR}, export.header_changes]",
                 f"rests_on: [{PRIOR}, export.header_changes, export.nothing]")
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn(f"FAIL {DECIDED}: rests on export.nothing, which is not an entry", out)
            _, _, jud, fields, raw = read(rec)
            self.assertIn("broken", P.flags(set(raw) - {"export.nothing"}, jud, fields, raw)[DECIDED])
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.by_name",
                                 "rests_on=[export.nothing]", "verdict=x",
                                 "reopened_by=a macro that binds by name", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("rests on export.nothing, which is not an entry - add it first, or "
                          "declare it missing with blocked_on", out + err)

    def test_a_reopener_does_not_excuse_prose_in_the_predicate_field(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, "    reopened_by: \"a macro that breaks",
                 "    wrong_if: \"if the macros ever bind by name\"\n    reopened_by: \"a macro that breaks")
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn(f"FAIL {DECIDED}: prose, not an evaluable predicate - a re-opener does not "
                          f"stand in for it: a predicate is evaluated, or declared un-evaluable with "
                          f"blocked_on", out)
            _, ids, jud, fields, raw = read(rec)
            self.assertIn("no_predicate", P.flags(ids, jud, fields, raw)[DECIDED])
            self.assertIn("no_predicate", R.build([str(rec)], None)[4]["flags"][DECIDED])
            _, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertIn(f"{DECIDED}: nothing evaluable would falsify it", out)

    def test_a_reopener_written_as_a_comparison_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.by_position",
                                 "rests_on=[export.header_changes]", "verdict=x",
                                 "reopened_by=export.header_changes > 0", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("reopened_by reads as a comparison (export.header_changes > 0) - a "
                          "predicate belongs in wrong_if, where it is evaluated", out + err)
            text = rec.read_text(encoding="utf-8")
            text = re.sub(r"    reopened_by: \"[^\"]*\"\n",
                          "    reopened_by: \"export.header_changes > 0\"\n", text, flags=re.S)
            rec.write_text(text, encoding="utf-8")
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn(f"FAIL {DECIDED}: reopened_by reads as a comparison (export.header_changes "
                          f"> 0) - a predicate belongs in wrong_if, where it is evaluated; a "
                          f"re-opener is the sign a person reads", out)
            _, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertIn(f"{DECIDED}: reopened_by reads as a comparison - a predicate belongs in "
                          f"wrong_if", out)

    def test_the_reopener_never_votes_for_the_predicate(self):
        # The fixture's re-opener names an entry and carries a dash - the shape of a
        # predicate. Against the record's one wrong_if it would tie, and the reader would
        # refuse to guess; a field the method names as prose casts no vote.
        _, _, _, fields, _ = read(RECORD)
        self.assertEqual(fields["predicate"], "wrong_if")
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            text = rec.read_text(encoding="utf-8")
            start, end = text.index("  c.fourteen_fit:"), text.index(f"  {DECIDED}:")
            rec.write_text(text[:start] + text[end:], encoding="utf-8")
            _, _, _, fields, _ = read(rec)
            self.assertIsNone(fields["predicate"])
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn(f"NOTE {DECIDED}: no predicate at all - decided; reopened by:", out)


class TheReaderCountsWhatRestsOnPriors(unittest.TestCase):
    """How much of a record stands on a session's own confidence, said at every check and at
    every open - the count the demotion rule is read against."""

    def test_check_says_the_count_and_never_fails_on_it(self):
        code, out, err = run(SCRIPTS / "provenance.py", "check", RECORD)
        self.assertEqual(code, 0, out + err)
        self.assertIn("NOTE " + COUNT.format(2, 1), out)
        # the two it counts are the two the fixture draws on a prior, one on each side of
        # the line; the third judgment rests on a measured column count and is not among them
        _, ids, jud, _, raw = read(RECORD)
        self.assertEqual(sorted(n for n, j in jud.items()
                                if any(d.startswith("prior.") for d in j["deps"])),
                         [TRIED, DECIDED])
        self.assertEqual((P.value_of(raw, ids, PRIOR), P.value_of(raw, ids, LOW)), (0.9, 0.6))

    def test_the_opener_carries_it_under_the_judgments_it_counts(self):
        _, out, _ = run(SCRIPTS / "provenance.py", "open", RECORD)
        self.assertIn("9 entries, 3 judgments, updated 2026-09-03\n" + COUNT.format(2, 1) + "\n",
                      out)

    def test_a_record_with_no_priors_is_told_nothing_about_them(self):
        _, ids, jud, _, raw = read(NO_PRIORS)
        self.assertEqual(P.priors_line(ids, jud, raw), "")
        for cmd in ("check", "open"):
            code, out, err = run(SCRIPTS / "provenance.py", cmd, NO_PRIORS)
            self.assertEqual(code, 0, out + err)
            self.assertNotIn("rest on prior.* claims", out, cmd)
            self.assertNotIn("or above", out, cmd)

    def test_a_confidence_no_float_can_hold_does_not_stop_the_count(self):
        # a number the record can carry but a float cannot: the count reads it the way a
        # falsifier over it would, and says its line rather than raising
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, "    v: 0.9\n", "    v: " + "9" * 4000 + "\n")
            code, out, err = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out + err)
            self.assertNotIn("Traceback", out + err)
            self.assertIn("NOTE " + COUNT.format(2, 1), out)

    def test_the_count_reads_the_value_the_record_holds_now(self):
        # a confidence restated below the line moves the judgment out of the high count at
        # the next read: nothing is stored, so nothing has to be refreshed
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "set", PRIOR, "0.5",
                                 "--why", "calibrated lower", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            _, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertIn("NOTE " + COUNT.format(2, 0), out)
            _, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertIn(COUNT.format(2, 0), out)

    def test_a_prior_the_record_does_not_hold_is_counted_but_never_above_the_line(self):
        # the judgment says it rests on a prior, so it is one of them; what that prior is
        # worth is unknown, and an unknown is not a confidence at 0.8
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.by_hand",
                                 "rests_on=[prior.unwritten, export.header_changes]",
                                 "verdict=the ledger is reconciled by hand before each export",
                                 "blocked_on=observational: the session never wrote the claim "
                                 "down, so nothing holds its confidence",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("NOTE " + COUNT.format(3, 1), out)

    def test_this_record_says_what_it_stands_on(self):
        # the count on the record this method keeps about itself, taken twice: once by the
        # reader, and once here from the file, so the line cannot drift from what it counts
        _, ids, jud, _, raw = read(OWN)

        def above(name):
            for d in jud[name]["deps"]:
                v = P.value_of(raw, ids, d) if d.startswith("prior.") else None
                if isinstance(v, (int, float)) and v >= 0.8:
                    return True
            return False
        rest = sorted(n for n, j in jud.items()
                      if any(d.startswith("prior.") for d in j["deps"]))
        high = [n for n in rest if above(n)]
        self.assertEqual((len(rest), len(high)), (6, 4), rest)
        code, out, err = run(SCRIPTS / "provenance.py", "check", OWN, cwd=ROOT)
        self.assertEqual(code, 0, out + err)
        self.assertIn("NOTE " + COUNT.format(len(rest), len(high)), out)


class ThePageShowsTheReopener(unittest.TestCase):
    def test_verify_is_clean(self):
        # from the record's own directory, as a session runs it: no brief sits beside this one
        code, out, err = run(SCRIPTS / "render_page.py", "--verify", RECORD, cwd=FIXTURE)
        self.assertEqual(code, 0, out + err)
        self.assertIn("1 tab, 0 problems", out)

    def test_the_card_carries_the_row_and_the_hover_reads_it(self):
        page, E, J, ids, info = R.build([str(RECORD)], None)
        self.assertEqual(info["flags"][DECIDED], set())
        self.assertIn('<div class="rb" dir="auto"><span class="lbl">reopened by</span>a macro that '
                      'breaks on an export whose column order did not change - the binding would '
                      'then be by name, and <span class="fx in" data-id="export.header_changes">'
                      'export.header_changes</span> the wrong thing to watch</div>', page)
        self.assertNotIn('class="dep wait"', page)
        self.assertTrue(J[DECIDED]["reopened"].startswith("a macro that breaks on an export"))
        self.assertEqual(J[DECIDED]["blocked"], "")
        self.assertIn("row('reopened by',esc(j.reopened))", page)
        # calm on the tree: no warning, no stop, the same blossom as any judgment that holds
        self.assertRegex(page, rf'<g class="tn crown" data-id="{re.escape(DECIDED)}">')
        self.assertEqual(J["c.fourteen_fit"]["reopened"], "")
        self.assertEqual(page.count('class="rb"'), 1)


class TheWritePathTakesAPrior(unittest.TestCase):
    def test_add_writes_a_prior_and_a_judgment_decided_on_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "prior.names_are_stable",
                                 "v=0.85", "unit=confidence", "reach=general",
                                 "name=a column keeps its header once macros depend on it",
                                 "from=s.2026_09_03_export", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("add prior.names_are_stable into known, before "
                          "prior.regeneration_is_deterministic", out)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.rename_is_safe",
                                 "rests_on=[prior.names_are_stable, export.header_changes]",
                                 "verdict=a column may be renamed in place",
                                 "because=Headers held {{export.header_changes}} times; the name is "
                                 "not what a macro reads.",
                                 "reopened_by=a macro that breaks on a rename alone",
                                 "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn(f"add c.rename_is_safe into judgments, after {DECIDED}", out)
            self.assertIn("the new judgment holds: decided; reopened by a macro that breaks on a "
                          "rename alone", out)
            text = rec.read_text(encoding="utf-8")
            self.assertIn('    reopened_by: "a macro that breaks on a rename alone"\n'
                          '    seen: {prior.names_are_stable: 0.85, export.header_changes: 0}\n', text)
            self.assertIn("    reach: general\n", text)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("NOTE c.rename_is_safe: no predicate at all - decided; reopened by: a "
                          "macro that breaks on a rename alone", out)
            self.assertRegex(out, r"\n4 judgments, 11 entries, 0 problems, 3 declared\n")

    def test_a_prior_restated_below_the_line_fires_the_judgment_drawn_against_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.rename_is_safe",
                                 f"rests_on=[{PRIOR}, export.header_changes]",
                                 "verdict=a column may be renamed in place",
                                 f"wrong_if={PRIOR} < 0.8",
                                 "reopened_by=a macro that breaks on a rename alone",
                                 "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn(f"the new judgment holds: wrong_if does not hold ({PRIOR} < 0.8)", out)
            code, out, err = run(SCRIPTS / "provenance.py", "set", PRIOR, "0.5",
                                 "--why", "calibrated lower", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn(f"FIRED     c.rename_is_safe: wrong_if holds ({PRIOR} < 0.8) - broken by "
                          f"its own condition", out)
            # the judgment decided on the prior alone is put in front of a person, not failed
            self.assertIn(f"MOVED     {DECIDED}: {PRIOR} moved 0.9 -> 0.5 since it was reviewed - "
                          f"if it still holds: review {DECIDED}", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn(f"FAIL c.rename_is_safe: wrong_if holds ({PRIOR} < 0.8)", out)
            self.assertIn(f"MOVED {DECIDED}: {PRIOR} differs from its snapshot (0.9 -> 0.5)", out)
            self.assertNotIn(f"FAIL {DECIDED}", out)


if __name__ == "__main__":
    unittest.main()
