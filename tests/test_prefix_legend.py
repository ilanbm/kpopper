"""The legend: a record kept under letters says in its head what they stand for, and every
surface that shows a prefix to a person shows the word - the opener's head, inside the part no
budget cuts, and the page's namespace bar and headings. Only for prefixes the record holds,
never for a record that declares nothing, and what the opener cannot print, check says."""
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"

RECORD = '''meta:
  updated: 2026-09-11
  name: Legend
  scope: "A record kept under letters."
{legend}
sources:
  doc.quote: {{name: "the quote", file: "q.pdf", read: "2026-09-11"}}

known:
  m.quote_eur: {{v: 4200, name: "the quote, in euros", from: doc.quote}}
  m.area_sqm: {{v: 12, name: "the wall, in square metres", from: doc.quote}}
  p.budget_eur: {{v: 9000, name: "what the wall may cost", from: doc.quote}}

judgments:
  d.within_budget:
    rests_on: [m.quote_eur, p.budget_eur]
    verdict: "the quote is within budget"
    wrong_if: "m.quote_eur > p.budget_eur"
    seen: {{m.quote_eur: 4200, p.budget_eur: 9000}}
'''


def run(script, legend, *args, extra_files=()):
    """`script` on a record whose head carries `legend` - the exit code and both streams."""
    with tempfile.TemporaryDirectory() as d:
        rec = pathlib.Path(d) / "PROVENANCE.yaml"
        rec.write_text(RECORD.format(legend=legend), encoding="utf-8")
        for name, text in extra_files:
            (pathlib.Path(d) / name).write_text(text, encoding="utf-8")
        r = subprocess.run([sys.executable, str(SCRIPTS / script), *args, str(rec)],
                           capture_output=True, text=True, cwd=d)
        return r.returncode, r.stdout + r.stderr


def opened(legend, *args):
    return run("provenance.py", legend, "open", *args)


class TheLegendIsPrintedBesideTheNamespace(unittest.TestCase):

    def test_a_declared_letter_is_spelled_out_in_the_head(self):
        code, out = opened("  prefixes: {d: decision, m: measurement, p: parameter}")
        self.assertEqual(code, 0, out)
        self.assertIn("holds: m (2)", out)
        self.assertIn("prefixes: d=decision · m=measurement · p=parameter", out)

    def test_a_record_that_declares_nothing_prints_no_line(self):
        code, out = opened("")
        self.assertEqual(code, 0, out)
        self.assertNotIn("prefixes:", out)

    def test_a_letter_the_record_does_not_hold_is_not_repeated(self):
        # z. is held by nothing, and q's meaning is not a word
        code, out = opened("  prefixes: {m: measurement, z: nothing here, q: 7}")
        self.assertEqual(code, 0, out)
        self.assertIn("prefixes: m=measurement\n", out)

    def test_the_legend_survives_a_tight_budget(self):
        code, out = opened("  prefixes: {d: decision}", "--chars", "200")
        self.assertEqual(code, 0, out)
        self.assertIn("prefixes: d=decision", out)

    def test_a_sentence_is_cut_to_a_word_and_the_body_survives(self):
        code, out = opened("  prefixes: {m: '" + "measurement " * 12 + "'}", "--chars", "400")
        self.assertEqual(code, 0, out)
        line = next(l for l in out.splitlines() if l.startswith("prefixes:"))
        self.assertLess(len(line), 60)
        self.assertTrue(line.endswith("…"), line)


class WhatTheOpenerCannotPrintCheckSays(unittest.TestCase):

    def test_a_key_yaml_reads_as_a_boolean_is_named(self):
        code, out = run("provenance.py", "  prefixes: {on: onboarding, m: measurement}", "check")
        self.assertEqual(code, 0, out)
        self.assertIn("meta.prefixes: the key True is not a word", out)
        self.assertIn("unless quoted", out)

    def test_a_prefix_nothing_holds_and_a_value_that_is_no_word_are_named(self):
        code, out = run("provenance.py", "  prefixes: {z: nothing, m: 7}", "check")
        self.assertEqual(code, 0, out)
        self.assertIn("meta.prefixes: z is held by nothing in the record", out)
        self.assertIn("meta.prefixes: m stands for 7, which is not a word", out)

    def test_a_clean_legend_says_nothing(self):
        code, out = run("provenance.py", "  prefixes: {m: measurement}", "check")
        self.assertEqual(code, 0, out)
        self.assertNotIn("meta.prefixes", out)


class ARecordSplitAcrossFilesKeepsEveryLegend(unittest.TestCase):

    def test_the_second_file_adds_its_letters_and_replaces_none(self):
        rooms = ('meta:\n  prefixes: {r: room}\n'
                 'known:\n  r.hall: {v: 40, name: "the hall, seats", from: doc.quote}\n')
        code, out = run("provenance.py", "  prefixes: {m: measurement}\nalso: rooms.yaml", "open",
                        extra_files=[("rooms.yaml", rooms)])
        self.assertEqual(code, 0, out)
        self.assertIn("prefixes: m=measurement · r=room", out)


class ThePageShowsTheWord(unittest.TestCase):

    def test_the_namespace_bar_and_heading_carry_the_word_and_keep_the_letter(self):
        code, out = run("render_page.py", "  prefixes: {m: measurement, d: decision}")
        self.assertEqual(code, 0, out[-2000:])
        page = re.sub(r"<script>.*?</script>", "", out, flags=re.S)
        self.assertIn('<a href="#g-m" title="m.">measurement (2)</a>', page)
        self.assertIn('<h2 id="g-m" title="m.">measurement</h2>', page)
        self.assertIn('<a href="#g-p">p (1)</a>', page)      # undeclared: the prefix itself

    def test_a_record_that_declares_nothing_renders_as_before(self):
        code, out = run("render_page.py", "")
        self.assertEqual(code, 0, out[-2000:])
        self.assertIn('<a href="#g-m">m (2)</a>', out)
        self.assertNotIn('title="m."', out)


if __name__ == "__main__":
    unittest.main()
