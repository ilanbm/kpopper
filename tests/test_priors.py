"""A session's prior as a source, and the judgment decided on it: the prior.* claim whose
value is the confidence, and the reopened_by field the reader accepts beside blocked_on -
a decided judgment that names the sign a person reads, never a hole and never waiting.
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
        self.assertRegex(out, r"\n2 judgments, 7 entries, 0 problems, 1 declared\n")

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
        self.assertNotIn("wrong_if", out)

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
            self.assertIn(f"add prior.names_are_stable into known, after {PRIOR}", out)
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
            self.assertRegex(out, r"\n3 judgments, 9 entries, 0 problems, 2 declared\n")

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
