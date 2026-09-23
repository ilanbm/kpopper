"""The page contract: the fields the reader and the renderer accept, and the names they
compute. Runs against the fixture record in tests/fixtures/page with no browser and no
network:

    python3 -m unittest discover -s tests
"""
import datetime
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "page"
RECORD = FIXTURE / "PROVENANCE.yaml"
BRIEF = FIXTURE / "PROVENANCE.view.yaml"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402
import render_page as R  # noqa: E402
import yaml  # noqa: E402

PLAIN_NEXT = ("next: pull <entry|prefix> (values with sources) · affects <entry> (what a change "
              "reaches) · check\n")
# the second tab of the fixture brief, as written - the scenario tests start without it
TAB_TWO = '''  - title: "The glazing quote"
    occasion: "opened when the glazier's quote comes in, to set it against the shortfall"
    serves: [s.2026_09_03_glazing]
    sections:
      - title: "What the quote buys"
        why: "the price, and the kilowatts it would keep in - both read against the shortfall on
              the first tab"
        pick: glaze.
        as: table
    shape: {entries: 11, judgments: 3, flagged: 0, blocked: 0}
'''
STRAY = ("  c.stray:\n    rests_on: [heat.gone]\n    verdict: \"a judgment on nothing\"\n"
         "    wrong_if: \"\"\n    seen: {}\n")
# a truth value beside the boiler, and a judgment whose falsifier matches it
FLUE = ('  heat.flue_clear:\n    v: true\n    name: "the flue is clear"\n'
        '    from: doc.boiler_sheet\n')
FLUE_HELD = ('  c.flue_held:\n    rests_on: [heat.flue_clear]\n'
             '    verdict: "the boiler is burning against an open flue"\n'
             '    wrong_if: "heat.flue_clear == false"\n'
             '    seen: {heat.flue_clear: true}\n')
# one value written with hyphens: a date is not two comparisons
DATED = ('  c.dated:\n    rests_on: [when.first_cold_night]\n'
         '    verdict: "the cold comes after the new year"\n'
         '    wrong_if: "when.first_cold_night < \'2027-01-01\'"\n'
         '    seen: {when.first_cold_night: "2027-02-01"}\n')


def run(*args, cwd=None, env=None):
    """The scripts as a session runs them: a subprocess, its exit code and both streams."""
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd,
                       capture_output=True, text=True, env=env)
    return p.returncode, p.stdout, p.stderr


def copy_fixture(into):
    """A scratch copy of the fixture, brief beside the record, for tests that edit it."""
    shutil.copy(RECORD, into / "PROVENANCE.yaml")
    shutil.copy(BRIEF, into / "PROVENANCE.view.yaml")
    return into / "PROVENANCE.yaml"


def edit(path, old, new, count=-1):
    text = path.read_text(encoding="utf-8")
    assert old in text, f"{old!r} is not in {path.name}"
    path.write_text(text.replace(old, new, count), encoding="utf-8")


def read(path):
    """The record as the reader sees it, counts and all."""
    doc = P.load([str(path)])
    ids, jud, fields = P.infer(doc)
    return doc, ids, jud, fields, P.with_builtins(doc, ids, jud, fields)


class CheckAcceptsTheContract(unittest.TestCase):
    def test_fixture_is_green(self):
        code, out, err = run(SCRIPTS / "provenance.py", "check", RECORD)
        self.assertEqual(code, 0, out + err)
        self.assertNotIn("FAIL", out)

    def test_builtin_names_resolve_as_dependencies(self):
        code, out, _ = run(SCRIPTS / "provenance.py", "check", RECORD)
        self.assertNotIn("graph.flagged, which is not an entry", out)
        self.assertNotIn("page.spill, which is not an entry", out)

    def test_graph_values_are_counted(self):
        doc = P.load([str(RECORD)])
        ids, jud, fields = P.infer(doc)
        raw = P.bodies(doc)
        values = P.counts(doc, ids, jud, fields, raw)
        self.assertEqual(values["graph.judgments"], 3)
        self.assertEqual(values["graph.entries"], 11)
        self.assertEqual(values["graph.flagged"], 0)
        self.assertEqual(values["graph.open"], 1)

    def test_only_referenced_builtins_become_entries(self):
        doc = P.load([str(RECORD)])
        ids, jud, fields = P.infer(doc)
        raw = P.bodies(doc)
        self.assertIn("graph.flagged", ids)
        self.assertIn("page.spill", ids)
        self.assertNotIn("graph.entries", ids)   # nothing rests on it, so it is not an entry here
        built = P.builtins(doc, ids, jud, fields, raw)
        self.assertEqual(sorted(built), ["graph.flagged", "page.spill", "page.unserved"])
        self.assertEqual(built["graph.flagged"]["v"], 0)
        # page.* has no value in the reader: it is counted where the brief is
        self.assertNotIn("v", built["page.spill"])

    def test_a_reference_to_nothing_fails(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, "{{heat.deficit_kw}}", "{{heat.nothing_here}}")
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1)
            self.assertIn("c.boiler_short: because references heat.nothing_here", out)

    def test_a_reference_the_judgment_does_not_rest_on_fails(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, "{{heat.deficit_kw}}", "{{when.first_cold_night}}")
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1)
            self.assertIn("does not declare as a dependency", out)

    def test_references_are_found_in_text(self):
        self.assertEqual(P.refs_in("It gives {{heat.boiler_kw}} kW. {{c.boiler_short}}"),
                         ["heat.boiler_kw", "c.boiler_short"])
        self.assertEqual(P.refs_in("no references here"), [])

    def test_a_predicate_over_graph_is_evaluated(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, 'wrong_if: "page.spill > 0"', 'wrong_if: "graph.flagged < 1"')
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("v.heating_tab: wrong_if holds", out)

    def test_a_predicate_over_page_is_left_to_the_page(self):
        _, out, _ = run(SCRIPTS / "provenance.py", "check", RECORD)
        self.assertIn("page.spill, which is counted when the page is built", out)

    def test_prose_with_references_is_not_a_predicate(self):
        # a because with references and a dash used to tie with the rule field for the
        # predicate role; a reader that guesses between them is the one thing this refuses
        code, out, err = run(SCRIPTS / "provenance.py", "check", RECORD)
        self.assertEqual(code, 0, out + err)
        self.assertNotIn("two fields fit", err)

    def test_a_compound_predicate_is_refused_rather_than_decided_false(self):
        # the comparison shape takes everything after the operator as its value, so this
        # was compared against the text "0 or graph.blocked > 0" - false in every state
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, "rests_on: [heat.boiler_kw, heat.loss_kw, heat.deficit_kw]",
                 "rests_on: [heat.boiler_kw, heat.loss_kw, heat.deficit_kw, graph.flagged, "
                 "graph.blocked]")
            edit(rec, 'wrong_if: "heat.loss_kw <= heat.boiler_kw"',
                 'wrong_if: "graph.flagged > 0 or graph.blocked > 0"')
            edit(rec, "seen: {heat.boiler_kw: 24, heat.loss_kw: 31,",
                 "seen: {graph.flagged: 0, graph.blocked: 0, heat.boiler_kw: 24, heat.loss_kw: 31,")
            _, ids, jud, fields, raw = read(rec)
            self.assertIsNone(P.evaluate(jud["c.boiler_short"]["pred"], raw, ids))
            self.assertIn("no_predicate", P.flags(ids, jud, fields, raw)["c.boiler_short"])
            self.assertIn("no_predicate", R.build([str(rec)], None)[4]["flags"]["c.boiler_short"])
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL c.boiler_short: wrong_if is not one comparison this reader "
                          "decides (a name, an operator, one value) - and nothing says why not, "
                          "so it can never be re-checked", out)
            _, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertIn("c.boiler_short: nothing evaluable would falsify it", out)

    def test_a_compound_predicate_declared_un_evaluable_is_noted(self):
        # the door a judgment already has: say the reader cannot decide it, and why
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, 'wrong_if: "heat.loss_kw <= heat.boiler_kw"',
                 'wrong_if: "heat.loss_kw <= heat.boiler_kw, or the wind turns"\n'
                 '    blocked_on: "the wind is nobody\'s number yet"')
            code, out, err = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("NOTE c.boiler_short: wrong_if is not one comparison this reader "
                          "decides", out)

    def test_a_truth_value_is_matched_in_both_of_its_states(self):
        # `str(True)` is not the `false` a record writes, so comparing them as text matched
        # in neither state: the falsifier stood green whichever way the fact went
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, "  when.first_cold_night:", FLUE + "  when.first_cold_night:")
            edit(rec, "judgments:\n", "judgments:\n" + FLUE_HELD)
            code, out, err = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out + err)          # the fact holds true: it does not
            _, ids, jud, fields, raw = read(rec)
            self.assertIs(P.evaluate("heat.flue_clear == false", raw, ids), False)
            self.assertIs(P.evaluate("heat.flue_clear != false", raw, ids), True)
            # the flue blocks: the one moment the falsifier existed for
            edit(rec, "  heat.flue_clear:\n    v: true", "  heat.flue_clear:\n    v: false")
            _, ids, jud, fields, raw = read(rec)
            self.assertIs(P.evaluate("heat.flue_clear == false", raw, ids), True)
            self.assertIn("falsified", P.flags(ids, jud, fields, raw)["c.flue_held"])
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL c.flue_held: wrong_if holds (heat.flue_clear == false) - "
                          "broken by its own condition", out)

    def test_a_truth_value_is_never_ordered_and_never_held_against_another_kind(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, "  when.first_cold_night:", "  heat.flue_clear:\n    v: true\n"
                      "    name: \"the flue is clear\"\n    from: doc.boiler_sheet\n"
                      "  when.first_cold_night:")
            _, ids, _, _, raw = read(rec)
            # ordering decides nothing about a truth value, and it is refused by shape
            self.assertIn("orders a truth value (< false), which is matched, never ordered",
                          P.why_undecided("heat.flue_clear < false"))
            self.assertIsNone(P.evaluate("heat.flue_clear < false", raw, ids))
            # a truth value and a value that is not one are two kinds of thing
            self.assertEqual(P.why_undecided("heat.flue_clear == 1"), "")
            self.assertIsNone(P.evaluate("heat.flue_clear == 1", raw, ids))
            self.assertIsNone(P.evaluate("heat.boiler_kw == true", raw, ids))

    def test_one_value_is_anything_that_carries_no_second_comparison(self):
        # a value is refused for carrying another comparison, never for what it is written
        # with: refusing one the reader compares perfectly well would silence a falsifier
        # that works, which is this same failure from the other side
        for pred in ('when.first_cold_night < "2027-01-01"',            # hyphens: a date
                     "tool.cli != 'legacy --denominator, cross-event'",  # hyphens and a comma
                     "heat.loss_kw > 1,000",                            # a thousands separator
                     'doc.note == "the "best" result"',                  # quotes inside quotes
                     "heat.deficit_kw != 7"):
            self.assertEqual(P.why_undecided(pred), "", pred)
        self.assertEqual(P.one_comparison('when.first_cold_night < "2027-01-01"'), "")
        self.assertEqual(P.one_comparison("page.spill > 1,000"), "")
        # and the comparisons those values would have been read as
        raw = {"x.n": {"v": 2000}, "x.t": {"v": 'the "best" result'}}
        self.assertIs(P.evaluate("x.n > 1,000", raw, set(raw)), True)
        self.assertIs(P.evaluate('x.t == "the "best" result"', raw, set(raw)), True)
        # two quoted values are still two comparisons
        self.assertIn("is not one comparison",
                      P.why_undecided('a.b == "x" or c.d == "y"'))

    def test_a_count_is_never_a_truth_value(self):
        # a computed name is a count whatever the record holds, so `page.spill == false`
        # would be read as None once the page counts it - and read as green by anything
        # that only asks whether the sign holds
        self.assertIn("holds a count against a truth value",
                      P.why_undecided("page.spill == false"))
        self.assertIn("holds a count against a truth value",
                      P.why_undecided("graph.flagged != true"))
        self.assertEqual(P.why_undecided("heat.flue_clear == false"), "")
        # reached through an entry it is the same sign, and a sign is decided where the
        # values are in hand - otherwise the arrangement is born green and never fires
        raw = {"page.spill": {"v": 0}, "x.flag": {"v": True}, "x.n": {"v": 3}}
        self.assertIn("holds a count against a truth value",
                      P.one_comparison("page.spill == x.flag", raw, set(raw)))
        self.assertEqual(P.one_comparison("page.spill == x.n", raw, set(raw)), "")
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, "judgments:\n", "judgments:\n" + DATED)
            code, out, err = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out + err)
            _, ids, jud, fields, raw = read(rec)
            self.assertIs(P.evaluate(jud["c.dated"]["pred"], raw, ids), False)

    def test_the_shape_is_read_in_one_place(self):
        # what an arrangement's sign is held to and what the reader will decide are the
        # same question; two readings of it drift, and one of them silently
        for pred in ("page.spill > 0 or page.unserved > 0", "graph.flagged >",
                     "page.spill < true", "the wind turns"):
            self.assertEqual(P.one_comparison(pred), P.why_undecided(pred), pred)
        # and where an arrangement asks something narrower, it says its own thing: a count
        # is never a truth value, and a sign names the value it carries
        self.assertIn("holds a count against a truth value",
                      P.one_comparison("page.spill == false"))
        self.assertIn("does not name one value a sign carries",
                      P.one_comparison("page.spill > lots"))
        self.assertEqual(P.why_undecided("page.spill > lots"), "")

    def test_pull_reads_a_computed_name(self):
        code, out, _ = run(SCRIPTS / "provenance.py", "pull", "graph.flagged", RECORD)
        self.assertEqual(code, 0, out)
        self.assertIn("graph.flagged: 0 (judgments that need a person)", out)


class ThePageAcceptsTheContract(unittest.TestCase):
    def test_verify_is_clean(self):
        code, out, err = run(SCRIPTS / "render_page.py", "--verify", RECORD)
        self.assertEqual(code, 0, out + err)
        self.assertNotIn("FAIL", out)

    def test_render_is_byte_reproducible(self):
        _, a, _ = run(SCRIPTS / "render_page.py", RECORD)
        _, b, _ = run(SCRIPTS / "render_page.py", RECORD)
        self.assertEqual(a, b)

    def test_groups_declare_the_grouping(self):
        _, page, _ = run(SCRIPTS / "render_page.py", RECORD)
        self.assertIn('<h3 dir="auto">Heating</h3>', page)
        self.assertIn('<h3 dir="auto">Calendar</h3>', page)

    def test_the_older_name_for_a_grouping_still_reads(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "groups:\n", "fronts:\n")
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            self.assertIn('<h3 dir="auto">Heating</h3>', page)
            self.assertIn('<h3 dir="auto">Calendar</h3>', page)

    def test_a_flat_grouping_is_one_scheme(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            text = brief.read_text(encoding="utf-8")
            text = text[:text.index("groups:")] + (
                "groups:\n  Heating: [heat., c.boiler_short]\n  Calendar: [when.first_cold_night]\n"
                "labels:\n  when.first_cold_night: first cold night\n")
            brief.write_text(re.sub(r"\n        by: [^\n]*", "", text), encoding="utf-8")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            self.assertIn('<h3 dir="auto">Heating</h3>', page)

    def test_a_section_draws_by_its_own_scheme(self):
        _, page, _ = run(SCRIPTS / "render_page.py", RECORD)
        dom = re.sub(r"<script>.*?</script>", "", page, flags=re.S)
        i = dom.index("By where it came from")
        section = dom[i:dom.index("<h2", i + 1)]
        for name in ("The service sheet", "Worked out in the session", "Cold-night inputs"):
            self.assertIn(f'<h3 dir="auto">{name}</h3>', section)
        # heat.loss_kw is under two groups of this scheme, so it is drawn under both
        self.assertEqual(section.count('data-id="heat.loss_kw"'), 2)
        self.assertNotIn("Heating", section)

    def test_a_section_may_read_by_the_source_an_entry_came_from(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "by: threads", "by: from")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            self.assertIn('<h3 dir="auto">the boiler&#x27;s service sheet</h3>', page)

    def test_the_older_shape_name_still_reads(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml",
                 "        as: grouped\n        by: threads", "        as: fronts\n        by: threads")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            self.assertIn('<h3 dir="auto">Heating</h3>', page)

    def test_a_section_reading_by_no_scheme_fails_verify(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "by: threads", "by: owners")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("section 'By thread' reads by 'owners', which is not a scheme the brief "
                          "declares (declared: threads, where it came from) nor a field an entry "
                          "carries", out)

    def test_a_section_may_read_by_any_field_the_entries_carry(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "by: threads", "by: unit")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            dom = re.sub(r"<script>.*?</script>", "", page, flags=re.S)
            i = dom.index("By thread")
            section = dom[i:dom.index("<h2", i + 1)]
            self.assertIn('<h3 dir="auto">kW</h3>', section)      # the two entries with a unit
            self.assertEqual(section.count('data-id="heat.'), 3)

    def test_an_empty_group_is_said(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml",
                 "Cold-night inputs: [heat.loss_kw, heat.boiler_kw]", "Cold-night inputs: [heat.nothing]")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("group 'Cold-night inputs' (scheme 'where it came from') picks nothing", out)

    def test_the_first_tab_is_drawn_under_its_own_name(self):
        _, page, _ = run(SCRIPTS / "render_page.py", RECORD)
        self.assertIn('data-tab="now" aria-selected="true">The February night', page)
        self.assertIn("opened when deciding whether to fire the old boiler", page)

    def test_text_is_checked_and_drawn(self):
        code, out, _ = run(SCRIPTS / "render_page.py", "--verify", RECORD)
        self.assertEqual(code, 0, out)
        self.assertNotIn("not yet drawn", out)
        _, page, _ = run(SCRIPTS / "render_page.py", RECORD)
        self.assertIn('<div class="txt" dir="auto">', page)

    def test_text_referencing_nothing_fails_verify(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "{{heat.loss_kw}}.", "{{heat.gone}}.")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("text references heat.gone, which is not an entry", out)

    def test_a_tab_serving_no_session_source_fails_verify(self):
        for bad in ("s.nobody", "heat.boiler_kw", "doc.boiler_sheet"):
            with tempfile.TemporaryDirectory() as d:
                rec = copy_fixture(pathlib.Path(d))
                edit(pathlib.Path(d) / "PROVENANCE.view.yaml",
                     "serves: [s.2026_09_02_heating]", f"serves: [{bad}]")
                code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
                self.assertEqual(code, 1, out)
                self.assertIn(f"serves {bad}, which is not a session source", out)

    def test_reasoning_references_are_drawn(self):
        _, page, _ = run(SCRIPTS / "render_page.py", RECORD)
        dom = re.sub(r"<script>.*?</script>", "", page, flags=re.S)
        self.assertIn('It gives <span class="fx in" data-id="heat.boiler_kw">24</span> kW', dom)
        self.assertIn('<span class="fx in" data-id="heat.deficit_kw">shortfall on the coldest night</span>', dom)
        self.assertNotIn("{{", dom)

    def test_a_page_decided_falsifier_fails_verify(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # one judgment nothing picks up, resting on nothing: it falls through, the page
            # counts it, and the arrangement's own line - page.spill > 0 - is crossed
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "pick: judgments",
                 "pick: [c.boiler_short, v.heating_tab]")
            rec.write_text(rec.read_text(encoding="utf-8") + (
                "  c.stray:\n    rests_on: [heat.gone]\n    verdict: \"a judgment on nothing\"\n"
                "    wrong_if: \"\"\n    seen: {}\n"), encoding="utf-8")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("v.heating_tab: wrong_if holds (page.spill > 0) - decided by the page", out)

    def test_a_moved_value_under_a_text_is_reported(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, "    v: 31\n", "    v: 35\n")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("its text saw heat.loss_kw = 31, now 35 - read it again", out)

    def test_every_tab_is_checked_as_drawn(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            brief.write_text(brief.read_text(encoding="utf-8").replace(
                "groups:\n",
                "  - title: \"Later\"\n    occasion: \"opened after the first frost\"\n"
                "    serves: [s.2026_09_02_heating]\n"
                "    sections:\n      - title: \"Gone\"\n        pick: heat.typo\n"
                "    shape: {entries: 99, judgments: 3, flagged: 0, blocked: 0}\n"
                "groups:\n"), encoding="utf-8")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("section 'Gone' (tab 'Later') picks nothing", out)
            self.assertIn("tab 'Later' recorded a different shape: entries: 99 -> 11", out)
            self.assertIn("tab 'Later' serves s.2026_09_02_heating and picks nothing it recorded - "
                          "serving is earned by picks", out)
            self.assertNotIn("draws the first", out)
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            self.assertIn('<button type="button" data-tab="now3" aria-selected="false">Later</button>', page)
            self.assertIn('<section id="panel-now3" hidden>', page)

class TheCountsHoldTogether(unittest.TestCase):
    def test_a_falsified_judgment_is_counted(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, 'wrong_if: "heat.loss_kw <= heat.boiler_kw"',
                 'wrong_if: "heat.loss_kw >= heat.boiler_kw"')
            doc = P.load([str(rec)])
            ids, jud, fields = P.infer(doc)
            values = P.counts(doc, ids, jud, fields, P.bodies(doc))
            self.assertEqual(values["graph.falsified"], 1)
            self.assertEqual(values["graph.flagged"], 1)

    def test_a_count_is_taken_before_any_line_over_it_is_decided(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, 'wrong_if: "page.spill > 0"', 'wrong_if: "graph.flagged < 1"')
            doc = P.load([str(rec)])
            ids, jud, fields = P.infer(doc)
            values = P.counts(doc, ids, jud, fields, P.bodies(doc))
            # "fewer than one flagged" holds while nothing is flagged, which would flag the
            # arrangement, which would unhold it. So the count is taken first and never
            # includes what reading it decided: here it stays at zero, and check - which
            # decides the line against that zero - is the one to fail the arrangement.
            self.assertEqual(values["graph.flagged"], 0)
            self.assertEqual(values["graph.falsified"], 0)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("v.heating_tab: wrong_if holds (graph.flagged < 1)", out)

    def test_page_spill_is_counted_on_the_page(self):
        _, page, _ = run(SCRIPTS / "render_page.py", RECORD)
        m = re.search(r'"page\.spill":\s*\{[^}]*"v":\s*0', page)
        self.assertIsNotNone(m, "page.spill should carry the count of what fell through")

    def test_shape_counts_exclude_computed_names(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            text = brief.read_text(encoding="utf-8")
            brief.write_text(re.sub(r"\n    shape:.*", "", text), encoding="utf-8")
            _, _, err = run(SCRIPTS / "render_page.py", rec)
            self.assertIn("entries: 11", err)
            self.assertIn("judgments: 3", err)


def dom_of(page):
    return re.sub(r"<script>.*?</script>", "", page, flags=re.S)


class TheWrittenLayer(unittest.TestCase):
    def test_section_text_is_drawn_with_its_references_resolved(self):
        _, page, _ = run(SCRIPTS / "render_page.py", RECORD)
        dom = dom_of(page)
        self.assertIn('<div class="txt" dir="auto">The boiler gives <span class="fx in" '
                      'data-id="heat.boiler_kw">24</span> kW and the coldest night takes '
                      '<span class="fx in" data-id="heat.loss_kw">31</span>. ', dom)
        self.assertNotIn("{{", dom)

    def test_a_judgment_reference_places_its_reasoning(self):
        _, page, _ = run(SCRIPTS / "render_page.py", RECORD)
        # the reasoning sits where the text asked for it, marked as a judgment and hoverable
        # as one, with its own references live inside it
        self.assertIn('<span class="fx in rsn" data-id="c.boiler_short">It gives <span class="fx in" '
                      'data-id="heat.boiler_kw">24</span> kW against a loss of', dom_of(page))

    def test_a_section_may_be_text_alone(self):
        code, out, _ = run(SCRIPTS / "render_page.py", "--verify", RECORD)
        self.assertEqual(code, 0, out)
        dom = dom_of(run(SCRIPTS / "render_page.py", RECORD)[1])
        self.assertIn('<h2 dir="auto">What comes first</h2>', dom)      # no count: nothing picked
        self.assertIn('<span class="fx in" data-id="when.first_cold_night">2027-02-01</span>; '
                      'every number above', dom)

    def test_a_text_that_never_read_a_reference_is_noted(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            edit(brief, '          c.boiler_short:\n'
                        '            verdict: "the old boiler cannot hold 12\u00b0C on the coldest '
                        'February night"\n'
                        '            because: "It gives {{heat.boiler_kw}} kW against a loss of '
                        '{{heat.loss_kw}} kW - a\n                      shortfall of '
                        '{{heat.deficit_kw}} kW before any wind."\n', "")
            edit(brief, '        seen: {when.first_cold_night: "2027-02-01"}\n', "")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("section 'What the numbers say': text references c.boiler_short, which its "
                          "seen does not carry - never read against it", out)
            self.assertIn("section 'What comes first': its text carries no seen, so nothing can tell "
                          "when it goes stale", out)

    def test_the_hover_reads_the_resolved_reasoning(self):
        _, page, _ = run(SCRIPTS / "render_page.py", RECORD)
        self.assertIn('"because": "It gives 24 kW against a loss of 31 kW - a shortfall of shortfall '
                      'on the coldest night kW before any wind."', page)

    def test_a_moved_reference_tints_the_text_and_review_clears_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35",
                                 "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out + err)
            dom = dom_of(run(SCRIPTS / "render_page.py", rec)[1])
            # the tint is in the markup: it shows wherever the page does, scripts or not
            self.assertIn('<div class="txt moved" dir="auto">', dom)
            self.assertIn('<span class="fx in mv" data-id="heat.loss_kw" title="was 31 when this '
                          'was reviewed">35</span>', dom)
            self.assertIn('<div class="mvd" dir="auto">Moved since this was read: heat loss on a '
                          '-5°C night 31 &rarr; 35</div>', dom)
            _, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertIn("its text saw heat.loss_kw = 31, now 35 - read it again", out)
            code, out, err = run(SCRIPTS / "provenance.py", "review", "What the numbers say",
                                 "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("seen heat.loss_kw: 31 -> 35", out)
            dom = dom_of(run(SCRIPTS / "render_page.py", rec)[1])
            self.assertNotIn('class="txt moved"', dom)
            self.assertNotIn('<div class="mvd"', dom)
            _, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertNotIn("read it again", out)
            brief = (pathlib.Path(d) / "PROVENANCE.view.yaml").read_text(encoding="utf-8")
            self.assertIn("heat.loss_kw: 35", brief)
            self.assertIn('reviewed: "2026-09-03"', brief)

    def test_a_rewritten_verdict_tints_the_text_that_placed_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, 'verdict: "the old boiler cannot hold 12°C on the coldest February night"',
                 'verdict: "the old boiler cannot hold 12°C on any February night"')
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("section 'What the numbers say': its text saw the verdict over the "
                          "reasoning it places of c.boiler_short = the old boiler cannot hold "
                          "12\u00b0C on the coldest February nig\u2026, now the old boiler cannot hold "
                          '12\u00b0C on any February night - read it again, then: review "What the '
                          'numbers say"', out)
            self.assertIn('<div class="txt moved" dir="auto">', dom_of(run(SCRIPTS / "render_page.py", rec)[1]))

    def test_a_moved_dependency_tints_the_card_and_review_clears_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # a move the predicate does not name is the one nothing decides
            edit(rec, 'wrong_if: "heat.loss_kw <= heat.boiler_kw"', 'wrong_if: "heat.loss_kw <= 0"')
            code, out, _ = run(SCRIPTS / "provenance.py", "set", "heat.boiler_kw", "25",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("MOVED     c.boiler_short: heat.boiler_kw moved 24 -> 25 since it was "
                          "reviewed - if it still holds: review c.boiler_short", out)
            dom = dom_of(run(SCRIPTS / "render_page.py", rec)[1])
            self.assertRegex(dom, r'<div class="card moved"[^>]*data-judgment="c.boiler_short"'
                             r'[^>]*data-review="moved"[^>]*><div class="cardtop">.*?'
                             r'<span class="judgment-label">judgment</span>.*?'
                             r'<div class="vd fx" data-id="c.boiler_short"')
            self.assertIn('<div class="mvd" dir="auto">Moved since this was reviewed: boiler output '
                          '24 &rarr; 25</div>', dom)
            # the reasoning placed in the section text carries the judgment's tint too
            self.assertIn('<span class="fx in rsn mv" data-id="c.boiler_short">', dom)
            code, out, _ = run(SCRIPTS / "provenance.py", "review", "c.boiler_short",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("heat.boiler_kw: 24 -> 25", out)
            dom = dom_of(run(SCRIPTS / "render_page.py", rec)[1])
            self.assertNotIn('class="card moved"', dom)
            self.assertNotIn("rsn mv", dom)


class ASignalTheAuthorCannotSilence(unittest.TestCase):
    """Two ways an arrangement could be written that quietly emptied its own warnings."""

    def test_a_rewritten_reasoning_tints_the_text_that_places_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # the verdict above it does not move; only the argument the text puts on the page
            edit(rec, 'because: "It gives {{heat.boiler_kw}} kW against a loss of {{heat.loss_kw}} kW - a\n'
                      '              shortfall of {{heat.deficit_kw}} kW before any wind."',
                 'because: "The glazing is the problem, not the boiler; {{heat.deficit_kw}} is\n'
                 '              beside the point."')
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("section 'What the numbers say': its text saw the reasoning it places "
                          "of c.boiler_short = It gives", out)
            self.assertIn('read it again, then: review "What the numbers say"', out)
            dom = dom_of(run(SCRIPTS / "render_page.py", rec)[1])
            self.assertIn('<div class="txt moved" dir="auto">', dom)
            self.assertIn("Moved since this was read: boiler short", dom)
            # and the review the note names clears it
            code, out, _ = run(SCRIPTS / "provenance.py", "review", "What the numbers say",
                               "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("seen c.boiler_short - the reasoning it places: It gives", out)
            self.assertNotIn("read it again", run(SCRIPTS / "render_page.py", "--verify", rec)[1])
            self.assertNotIn('class="txt moved"', dom_of(run(SCRIPTS / "render_page.py", rec)[1]))

    def test_what_rests_on_a_judgment_still_watches_only_its_verdict(self):
        # the other half of the same rule: three rewrites of the prose must not flag
        # everything downstream, or the marks stop meaning anything
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, 'because: "It gives {{heat.boiler_kw}} kW against a loss of {{heat.loss_kw}} kW - a\n'
                      '              shortfall of {{heat.deficit_kw}} kW before any wind."',
                 'because: "Put another way: {{heat.deficit_kw}} is what the night takes and the\n'
                 '              boiler does not give."')
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertNotIn("c.boiler_short", out.replace("3 judgments", ""))
            doc = P.load([str(rec)])
            ids, jud, fields = P.infer(doc)
            raw = P.with_builtins(doc, ids, jud, fields)
            # what a text is held to, and what a judgment is held to, part here: the text
            # carries both halves, so a rewritten argument moves under it; the judgment
            # carries only the conclusion, so it does not
            verdict = "the old boiler cannot hold 12\u00b0C on the coldest February night"
            self.assertEqual(P.snapshot_value("c.boiler_short", raw, ids, jud, {}), verdict)
            shown = P.shown_value("c.boiler_short", raw, ids, jud, {})
            # two named fields, never one line with a separator: prose contains every
            # separator anyone might pick
            self.assertEqual(shown["verdict"], verdict)
            self.assertIn("Put another way", shown["because"])

    def test_prose_covers_a_flagged_judgment_but_does_not_silence_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            # flagged: never checked against one of the things it rests on
            edit(rec, '    seen: {heat.boiler_kw: 24, heat.loss_kw: 31,\n'
                      '           heat.deficit_kw: "heat.loss_kw - heat.boiler_kw"}',
                 '    seen: {heat.boiler_kw: 24, heat.loss_kw: 31}')
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 1)
            # no section picks it; one section's prose places its reasoning
            edit(brief, "        pick: judgments\n", "        pick: [v.heating_tab]\n")
            page = run(SCRIPTS / "render_page.py", rec)[1]
            dom = dom_of(page)
            self.assertIn('<span class="fx in rsn"', dom)                  # still drawn in the prose
            self.assertIn("Not covered by this arrangement", dom)          # and still in the net
            self.assertRegex(page, r'"page\.spill":[^}]*"v": 1')
            # picking it is what accounts for it
            edit(brief, "        pick: [v.heating_tab]\n", "        pick: judgments\n")
            page = run(SCRIPTS / "render_page.py", rec)[1]
            self.assertNotIn("Not covered by this arrangement", dom_of(page))
            self.assertRegex(page, r'"page\.spill":[^}]*"v": 0')

    def test_a_mention_still_counts_as_covered(self):
        # the half that does not change: coverage asks whether the arrangement reached it
        _, out, _ = run(SCRIPTS / "render_page.py", "--verify", RECORD)
        self.assertIn("tab 'The February night' serves s.2026_09_02_heating: picks 3 of 3 "
                      "they recorded", out)

    def test_a_placement_with_no_reasoning_is_said(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            text = rec.read_text(encoding="utf-8")
            i = text.index('    because: "It gives')
            rec.write_text(text[:i] + text[text.index('    wrong_if: "heat.loss_kw'):],
                           encoding="utf-8")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("section 'What the numbers say': places c.boiler_short for its reasoning, "
                          "which it does not carry - its verdict is drawn in that sentence instead", out)
            # the sentence still reads: a verdict is drawn rather than a hole
            self.assertIn('<span class="fx in rsn" data-id="c.boiler_short">the old boiler cannot',
                          dom_of(run(SCRIPTS / "render_page.py", rec)[1]))

    def test_a_long_reasoning_is_cut_at_a_word_and_marked(self):
        self.assertEqual(R.clipped("short enough", 40), ("short enough", False))
        drawn, cut = R.clipped(("word " * 30).strip(), 40)
        self.assertTrue(cut)
        self.assertTrue(drawn.endswith("\u2026"), drawn)
        self.assertLessEqual(len(drawn), 40)
        self.assertNotIn("wor\u2026", drawn)              # never mid-word
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            long = ("The reasoning runs on and on with clause after clause. " * 10
                    + "And only here, at the very end, does it say the load-bearing thing.")
            edit(rec, 'because: "It gives {{heat.boiler_kw}} kW against a loss of {{heat.loss_kw}} kW - a\n'
                      '              shortfall of {{heat.deficit_kw}} kW before any wind."',
                 'because: "' + long.strip() + '"')
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("1 reasoning longer than the 400 characters a card carries; each is "
                          "drawn to its last whole word and marked. Longest first: "
                          "c.boiler_short (617)", out)
            dom = dom_of(run(SCRIPTS / "render_page.py", rec)[1])
            drawn = re.search(r'<div class="bc" dir="auto">(.*?)</div>', dom, re.S).group(1)
            self.assertTrue(drawn.endswith("\u2026"), drawn[-40:])
            self.assertLessEqual(len(drawn), R.CARD_CHARS)
            # what the cut costs is visible: the last sentence is not on the page at all
            self.assertNotIn("the load-bearing thing", dom)

    def test_a_reasoning_that_only_resolves_long_is_said_and_not_cut(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # short as written, long once the record reads into it: nothing to cut, and the
            # fix is the name behind the reference rather than the sentence
            edit(rec, '    name: "shortfall on the coldest night"',
                 '    name: "' + ("the shortfall measured against the boiler's rated output and "
                                  "the glazing as it stands " * 6).strip() + '"')
            text = rec.read_text(encoding="utf-8")
            i, j = text.index('    because: "It gives'), text.index('    wrong_if: "heat.loss_kw')
            rec.write_text(text[:i] + '    because: "In short: {{heat.deficit_kw}}."\n' + text[j:],
                           encoding="utf-8")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("1 reasoning within 400 characters as written and past them once their "
                          "references resolve; what each names is long, so the card is drawn "
                          "whole. Longest first: c.boiler_short (526)", out)
            self.assertNotIn("longer than the 400 characters", out)
            card = re.search(r'<div class="bc" dir="auto">(.*?)</div>',
                             dom_of(run(SCRIPTS / "render_page.py", rec)[1]), re.S).group(1)
            self.assertNotIn("\u2026", card)          # nothing was cut; it is drawn whole
            self.assertGreater(len(re.sub(r"<[^>]+>", "", card)), R.CARD_CHARS)


LONG_A = ("the glazier will cover the north wall in toughened double glazing before the "
          "first cold night")
LONG_B = ("the glazier will cover the north wall in toughened double glazing after the "
          "first frost")


def glazing(into, predicate=None, key="glaze.terms"):
    """A record whose judgment rests on one long value - the shape every surface that
    compares two readings is exercised on. The predicate decides whether a move is muted
    (it names the moved entry) or moved (it does not); `key` is the id that holds the
    value, so a long one can be given where the line's own budget is under test."""
    predicate = predicate or '    wrong_if: "glaze.quote_eur > 9000"'
    rec = into / "PROVENANCE.yaml"
    rec.write_text(
        "meta:\n  updated: 2026-09-04\n  name: Glazing\n"
        '  scope: "One long value, changed at its end."\n\n'
        'sources:\n  doc.quote: {name: "the quote", file: "q.pdf", read: "2026-09-04"}\n\n'
        'known:\n  glaze.quote_eur: {v: 4200, name: "the quote", from: doc.quote}\n'
        f"  {key}:\n"
        f'    quoted: "{LONG_A}"\n'
        '    name: "what the glazier undertook"\n    from: doc.quote\n\n'
        "judgments:\n  c.terms_hold:\n"
        f"    rests_on: [{key}, glaze.quote_eur]\n"
        '    verdict: "the wall is covered in time"\n'
        f"{predicate}\n"
        f'    seen: {{glaze.quote_eur: 4200, {key}: "{LONG_A}"}}\n', encoding="utf-8")
    return rec


class AComparisonShowsWhereItParts(unittest.TestCase):
    """Six surfaces printed two readings side by side and clipped both at the head - which
    is the half that did not change. Each showed one string twice."""

    def moved(self, rec, later="2026-09-05"):
        code, out, err = run(SCRIPTS / "provenance.py", "set", "glaze.terms", LONG_B,
                             "--as-of", later, rec)
        self.assertEqual(code, 0, out + err)
        return out

    def test_the_muted_line_says_what_moved(self):
        # the worst of them: the reader is told nothing is asked of them, and the reason
        # used to be two identical strings
        with tempfile.TemporaryDirectory() as d:
            rec = glazing(pathlib.Path(d), '    wrong_if: "glaze.terms == \'withdrawn\'"')
            out = self.moved(rec)
            self.assertIn("MUTED     c.terms_hold: glaze.terms moved …before the first cold "
                          "night -> …after the first frost, inside wrong_if", out)
            self.assertIn("nothing is asked", out)

    def test_the_reach_line_says_what_moved(self):
        with tempfile.TemporaryDirectory() as d:
            out = self.moved(glazing(pathlib.Path(d)))
            self.assertIn("MOVED     c.terms_hold: glaze.terms moved …before the first cold "
                          "night -> …after the first frost since it was reviewed", out)

    def test_check_and_open_say_what_moved(self):
        with tempfile.TemporaryDirectory() as d:
            rec = glazing(pathlib.Path(d))
            self.moved(rec)
            _, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertIn("MOVED c.terms_hold: glaze.terms differs from its snapshot "
                          "(…before the first cold night -> …after the first frost)", out)
            _, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertIn("glaze.terms differs from what it last saw: …before the first cold "
                          "night -> …after the first frost", out)

    def test_pull_keeps_the_arrow_and_the_second_reading(self):
        # the one a per-side reading does not reach: the whole line was cut, so a long
        # value ate the budget and the reading beside it never appeared at all
        with tempfile.TemporaryDirectory() as d:
            rec = glazing(pathlib.Path(d))
            self.moved(rec)
            _, out, _ = run(SCRIPTS / "provenance.py", "pull", "c.terms_hold", rec)
            line = next(l for l in out.split("\n") if "moved since review" in l)
            self.assertIn("->", line)
            self.assertIn("…after the first frost", line)
            self.assertLessEqual(len(line), 110)

    def test_pull_keeps_the_comparison_whole_under_a_long_id(self):
        # the sides are cut to what the line has left, and the line is never cut across
        # the comparison - so a dependency long enough to eat the budget costs its own
        # readings room, never the arrow or the second of them
        with tempfile.TemporaryDirectory() as d:
            long_id = "glaze.terms_as_the_glazier_set_them_out_in_the_quote_of_september"
            rec = glazing(pathlib.Path(d), f'    wrong_if: "{long_id} == \'withdrawn\'"',
                          key=long_id)
            code, out, err = run(SCRIPTS / "provenance.py", "set", long_id, LONG_B,
                                 "--as-of", "2026-09-05", rec)
            self.assertEqual(code, 0, out + err)
            _, out, _ = run(SCRIPTS / "provenance.py", "pull", "c.terms_hold", rec)
            line = next(l for l in out.split("\n") if "moved since review" in l)
            self.assertIn(" -> ", line)
            self.assertIn("within wrong_if", line)          # the state survives too
            self.assertIn("…before the first cold", line)
            self.assertIn("…after the first frost", line)

    def test_a_value_with_no_word_boundary_still_shows_its_difference(self):
        # urls, hashes, long numbers: there is no word to open the window on, so it opens
        # inside the token rather than falling back to the head both sides share
        a = "https://example.org/quotes/north-wall-2026-09-04-v1.pdf"
        b = "https://example.org/quotes/north-wall-2026-09-04-v2.pdf"
        wa, wb = P.apart(a, b)
        self.assertNotEqual(wa, wb)
        self.assertTrue(wa.endswith("v1.pdf") and wb.endswith("v2.pdf"), (wa, wb))
        h = "c3f1a9e2b7d4" * 4
        ha, hb = P.apart(h, h[:38] + "ZZ" + h[40:])
        self.assertNotEqual(ha, hb)

    def test_the_fork_refusal_says_how_the_two_readings_differ(self):
        # it asks a person to choose between two readings of one day; it may not show them
        # the same words twice
        with tempfile.TemporaryDirectory() as d:
            rec = glazing(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "set", "glaze.terms", LONG_B,
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 1)
            said = out + err
            self.assertIn("glaze.terms holds …before the first cold night as of 2026-09-04, "
                          "and a reading of the same day says …after the first frost", said)

    def test_review_says_what_moved_under_the_judgment(self):
        with tempfile.TemporaryDirectory() as d:
            rec = glazing(pathlib.Path(d))
            self.moved(rec)
            code, out, _ = run(SCRIPTS / "provenance.py", "review", "c.terms_hold",
                               "--as-of", "2026-09-05", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("  glaze.terms: …before the first cold night -> …after the "
                          "first frost", out)

    def test_the_head_is_kept_where_the_two_part_inside_it(self):
        # nothing is bought by dropping a head that already shows the difference
        self.assertEqual(P.apart("4200", "9100"), ("4200", "9100"))
        self.assertEqual(P.apart("yes", "no"), ("yes", "no"))
        a, b = P.apart("a wholly different opening" + " x" * 40, "another opening entirely" + " x" * 40)
        self.assertTrue(a.startswith("a wholly"), a)
        self.assertTrue(b.startswith("another"), b)

    def test_the_window_opens_on_a_whole_word(self):
        a, b = P.apart(LONG_A, LONG_B)
        self.assertEqual((a, b), ("…before the first cold night", "…after the first frost"))
        for s in (a, b):
            self.assertNotIn("…g", s)          # never mid-word, as "…g before" would be

    def test_a_long_tail_past_the_window_is_still_clipped(self):
        # the window opens at the word the two part on, and what runs past the budget from
        # there is clipped at the end the way anything else is
        a, b = P.apart("same head " * 8 + "and then a tail that runs on well past what a line here holds",
                       "same head " * 8 + "and then a different tail that also runs on and on and on")
        self.assertTrue(a.startswith("…tail that"), a)
        self.assertTrue(b.startswith("…different tail"), b)
        self.assertTrue(a.endswith("…") and b.endswith("…"), (a, b))
        self.assertLessEqual(max(len(a), len(b)), 40)


class TheReasoningReachesAgents(unittest.TestCase):
    def test_pull_prints_the_reasoning_resolved(self):
        code, out, _ = run(SCRIPTS / "provenance.py", "pull", "c.boiler_short", RECORD)
        self.assertEqual(code, 0, out)
        self.assertIn("    holds\n    because: It gives 24 kW against a loss of 31 kW - a shortfall of "
                      "shortfall on the coldest night kW", out)
        self.assertNotIn("{{", out)

    def test_open_prints_the_reasoning_under_what_needs_a_person(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, 'wrong_if: "heat.loss_kw <= heat.boiler_kw"', 'wrong_if: "heat.loss_kw <= 0"')
            run(SCRIPTS / "provenance.py", "set", "heat.boiler_kw", "25", "--as-of", "2026-09-03", rec)
            code, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertEqual(code, 0, out)
            # resolved to what the record holds now, not to what the judgment saw
            self.assertIn("  c.boiler_short: heat.boiler_kw differs from what it last saw: 24 -> 25\n"
                          "      because: It gives 25 kW against a loss of 31 kW", out)


class TheWritePath(unittest.TestCase):
    def test_set_changes_one_value_and_answers_with_the_reach(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35",
                                 "--as-of", "2026-09-03", "--why", "re-measured", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("set heat.loss_kw: 31 -> 35 (as of 2026-09-03)", out)
            self.assertIn("worked out from it: heat.deficit_kw", out)
            self.assertIn("MUTED     c.boiler_short: heat.loss_kw moved 31 -> 35, inside wrong_if "
                          "(heat.loss_kw <= heat.boiler_kw) - nothing is asked", out)
            self.assertIn("'What the numbers say' saw heat.loss_kw = 31 - read it again, then: "
                          'review "What the numbers say"', out)
            # the diff is the change: the value, its date, the reason, and meta.updated
            after = rec.read_text(encoding="utf-8")
            self.assertEqual(after, before.replace(
                "    v: 31\n", '    v: 35\n    of: "2026-09-03"\n    # set 2026-09-03: re-measured\n'
            ).replace("updated: 2026-09-02", "updated: 2026-09-03"))
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_set_refuses_what_is_not_a_value(self):
        for key, why in (("heat.deficit_kw", "worked out from a rule"),
                         ("c.boiler_short", "reviewed, not set"),
                         ("heat.nothing", "is not an entry"),
                         ("graph.flagged", "counted by the reader, never set")):
            with tempfile.TemporaryDirectory() as d:
                rec = copy_fixture(pathlib.Path(d))
                before = rec.read_text(encoding="utf-8")
                code, out, err = run(SCRIPTS / "provenance.py", "set", key, "1", rec)
                self.assertEqual(code, 1, key)
                self.assertIn(why, out + err)
                self.assertEqual(rec.read_text(encoding="utf-8"), before)

    def test_a_reason_is_one_line(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35",
                                 "--why", "re-measured\ninjected: true", rec)
            self.assertEqual(code, 1)
            self.assertIn("--why is one line", out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)

    def test_a_pointer_record_is_followed_to_the_file_that_holds_the_entry(self):
        with tempfile.TemporaryDirectory() as d:
            root, mid, leaf = (pathlib.Path(d) / n for n in ("PROVENANCE.yaml", "mid.yaml", "leaf.yaml"))
            root.write_text("record: mid.yaml\n", encoding="utf-8")
            mid.write_text("also: leaf.yaml\nknown:\n  x.one: {v: 1, name: one}\n", encoding="utf-8")
            leaf.write_text("known:\n  x.two: {v: 2, name: two}\njudgments:\n  c.two:\n"
                            "    rests_on: [x.two]\n    verdict: small\n    wrong_if: \"x.two > 5\"\n"
                            "    seen: {x.two: 2}\n", encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "set", "x.two", "3", "--as-of", "2026-09-03", root)
            self.assertEqual(code, 0, out + err)
            self.assertIn("MUTED     c.two: x.two moved 2 -> 3", out)
            self.assertIn('x.two: {v: 3, of: "2026-09-03", name: two}', leaf.read_text(encoding="utf-8"))
            self.assertEqual(root.read_text(encoding="utf-8"), "record: mid.yaml\n")

    def test_set_of_the_same_value_writes_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            code, out, _ = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "31", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("heat.loss_kw is already 31; nothing written", out)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)

    def test_add_inserts_in_id_order_and_touches_nothing_else(self):
        with tempfile.TemporaryDirectory() as d:
            rec = pathlib.Path(d) / "PROVENANCE.yaml"
            # This tests insertion, not the clock: the source record can have been
            # updated in a timezone whose calendar day is still ahead of CI's.
            updated = "2026-09-01"
            before = re.sub(r"^  updated: \S+", "  updated: " + updated,
                            (ROOT / "GROUNDING.yaml").read_text(encoding="utf-8"), count=1, flags=re.M)
            rec.write_text(before, encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "add", "p.reference_test", "v=1",
                                 "name=a value added in id order", "from=scripts/provenance.py",
                                 "--as-of", updated, rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("add p.reference_test into known, before p.script_copies", out)
            after = rec.read_text(encoding="utf-8")
            block = ("  p.reference_test:\n    v: 1\n    name: \"a value added in id order\"\n"
                     "    from: \"scripts/provenance.py\"\n")
            self.assertEqual(after.replace(block, "", 1), before)
            self.assertLess(after.index("  p.gate_bounces:"), after.index(block))
            self.assertLess(after.index(block), after.index("  p.script_copies:"))
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_add_fills_seen_and_refuses_an_unknown_dependency(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.wind", "rests_on=[heat.nothing]",
                                 "verdict=x", "wrong_if=heat.nothing > 1", rec)
            self.assertEqual(code, 1)
            self.assertIn("rests on heat.nothing, which is not an entry - add it first", out + err)
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "heat.wind_kw", "v=3", "unit=kW",
                               "name=loss to wind", "from=doc.boiler_sheet", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("add heat.wind_kw into known, after heat.deficit_kw", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "c.wind",
                               "rests_on=[heat.wind_kw, heat.deficit_kw, c.boiler_short]",
                               "verdict=wind widens the shortfall by {{heat.wind_kw}} kW",
                               "wrong_if=heat.wind_kw <= 0", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("add c.wind into judgments, after c.boiler_short", out)
            self.assertIn("the new judgment holds: wrong_if does not hold (heat.wind_kw <= 0)", out)
            text = rec.read_text(encoding="utf-8")
            self.assertIn('    seen:\n      heat.wind_kw: 3\n'
                          '      heat.deficit_kw: "heat.loss_kw - heat.boiler_kw"\n'
                          '      c.boiler_short: "the old boiler cannot hold 12°C on the coldest '
                          'February night"\n', text)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            _, out, _ = run(SCRIPTS / "provenance.py", "pull", "c.wind", rec)
            self.assertIn("+ c.wind: wind widens the shortfall by {{heat.wind_kw}} kW", out)

    def test_add_refuses_what_check_would_fail(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            cases = (
                (["heat.loss_kw", "v=1"], "already an entry"),
                (["c.w", "rests_on=[heat.loss_kw]", "verdict=x", "wrong_if=heat.loss_kw > 0"],
                 "would be born broken"),
                (["c.w", "rests_on=[heat.loss_kw]", "verdict=x", "wrong_if=heat.loss_kw < 0",
                  "because={{heat.nothing}}"], "references heat.nothing, which is not an entry"),
                (["c.w", "rests_on=[heat.loss_kw]", "verdict=x", "wrong_if=heat.boiler_kw < 0"],
                 "wrong_if reads heat.boiler_kw, which the judgment does not rest on"),
                (["c.w", "rests_on=[heat.loss_kw]", "verdict=x", "wrong_if=heat.loss_kw < 0",
                  "seen={heat.loss_kw: 31}"], "seen is written by this tool"),
            )
            for args, why in cases:
                code, out, err = run(SCRIPTS / "provenance.py", "add", *args, rec)
                self.assertEqual(code, 1, args)
                self.assertIn(why, out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)

    def test_add_may_rest_on_a_count_nothing_mentioned_yet(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.small", "rests_on=[graph.entries]",
                                 "verdict=a record small enough to read whole",
                                 "wrong_if=graph.entries > 100", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("the new judgment holds: wrong_if does not hold (graph.entries > 100)", out)
            self.assertIn("seen: {graph.entries: 11}", rec.read_text(encoding="utf-8"))
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_add_takes_an_open_question(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "q.glazing",
                               "is the north wall worth glazing?", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("add q.glazing into open, before q.second_boiler", out)
            self.assertIn('  q.glazing: "is the north wall worth glazing?"\n  q.second_boiler:',
                          rec.read_text(encoding="utf-8"))

    def test_review_rewrites_seen_from_what_the_record_holds(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35", "--as-of", "2026-09-03", rec)
            code, out, _ = run(SCRIPTS / "provenance.py", "review", "c.boiler_short",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("review c.boiler_short: seen rewritten from what the record holds (2026-09-03)\n"
                          "  heat.loss_kw: 31 -> 35\n", out)
            self.assertIn("c.boiler_short holds: wrong_if does not hold", out)
            text = rec.read_text(encoding="utf-8")
            self.assertIn("heat.loss_kw: 35", text)
            self.assertNotIn("heat.loss_kw: 31", text)
            # a second review has nothing to rewrite, and says so
            code, out, _ = run(SCRIPTS / "provenance.py", "review", "c.boiler_short",
                               "--as-of", "2026-09-03", rec)
            self.assertIn("what it saw is what the record holds", out)
            self.assertEqual(rec.read_text(encoding="utf-8"), text)

    def test_review_refuses_a_judgment_written_on_one_line_and_leaves_the_record(self):
        # its fields are inside that line: a line written under it would be read as another
        # judgment of the collection (four spaces) or break the file (two)
        judgment = 'verdict: known, requires: [api.limit], fails_if: "api.limit > 100"'
        cases = ((judgment, "seen"),
                 (judgment + ", seen: {api.limit: 5}", "seen"),
                 (judgment + ', seen: {api.limit: 10}, replaced: ["the limit rose on 2025-12-01"]',
                  "reviewed"),
                 (judgment + ', seen: {api.limit: 10}, reviewed: "2025-12-01"', "reviewed"))
        for pad in ("  ", "    "):
            for body, field in cases:
                with tempfile.TemporaryDirectory() as d:
                    rec = pathlib.Path(d) / "GROUNDING.yaml"
                    text = f"known:\n{pad}api.limit: {{v: 10}}\njudgments:\n{pad}d.w: {{{body}}}\n"
                    rec.write_text(text, encoding="utf-8")
                    code, out, err = run(SCRIPTS / "provenance.py", "review", "d.w",
                                         "--as-of", "2026-01-01", rec)
                    self.assertEqual(code, 1, (pad, body, out + err))
                    self.assertIn(f"refused - d.w is written on one line, and review writes {field}: as a "
                                  "line of its own - write d.w with one field per line, then review it "
                                  "again", out + err)
                    self.assertEqual(rec.read_text(encoding="utf-8"), text)
        # with nothing to write into it, it is reviewed as before
        with tempfile.TemporaryDirectory() as d:
            rec = pathlib.Path(d) / "GROUNDING.yaml"
            text = (f"known:\n    api.limit: {{v: 10}}\njudgments:\n"
                    f"    d.w: {{{judgment}, seen: {{api.limit: 10}}}}\n")
            rec.write_text(text, encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "review", "d.w", "--as-of", "2026-01-01", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("review d.w: what it saw is what the record holds (2026-01-01)", out)
            self.assertEqual(rec.read_text(encoding="utf-8"), text)

    def test_review_of_an_arrangement_rewrites_the_tab_shape(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            edit(brief, "shape: {entries: 11,", "shape: {entries: 9,", count=1)
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            self.assertIn("changed shape since this arrangement was written", page)
            code, out, _ = run(SCRIPTS / "provenance.py", "review", "v.heating_tab",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("shape of tab 'The February night': entries: 11, judgments: 3, flagged: 0, "
                          "blocked: 0", out)
            self.assertIn("shape: {entries: 11, judgments: 3, flagged: 0, blocked: 0}",
                          brief.read_text(encoding="utf-8"))
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            self.assertNotIn("changed shape since this arrangement was written", page)

    def test_review_of_a_moved_arrangement_writes_the_settled_shape(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            # a snapshot that no longer matches puts the arrangement in front of a person -
            # and its own flag is part of the shape it stands on
            edit(rec, 'graph.flagged: 0, page.spill: 0}', 'graph.flagged: 1, page.spill: 0}')
            self.assertIn("flagged: 0 &rarr; 1", run(SCRIPTS / "render_page.py", rec)[1])
            code, out, _ = run(SCRIPTS / "provenance.py", "review", "v.heating_tab",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("graph.flagged: 1 -> 0", out)
            # the shape written is the record's shape after the review, not before it
            self.assertIn("shape of tab 'The February night': entries: 11, judgments: 3, flagged: 0, "
                          "blocked: 0", out)
            self.assertIn("shape: {entries: 11, judgments: 3, flagged: 0, blocked: 0}",
                          brief.read_text(encoding="utf-8"))
            page = run(SCRIPTS / "render_page.py", rec)[1]
            self.assertNotIn("changed shape since this arrangement was written", page)
            self.assertEqual(run(SCRIPTS / "render_page.py", "--verify", rec)[0], 0)

    def test_a_long_value_folds_and_reads_back_whole(self):
        for s in ("It gives 24 kW against a loss of 31 kW - a shortfall of 7 kW before any wind, "
                  "and a second boiler is cheaper than glazing the north wall this winter.",
                  'a "quoted" word, a \\ backslash, and  two  spaces  inside, kept whole across a fold '
                  "that would otherwise eat one of them somewhere along this long line",
                  "מה קורה כשעובדה זזה - the unified node model, written once and folded under its "
                  "opening quote at word boundaries so a line never runs past the margin"):
            lines = P._field_lines("because", s, 4)
            self.assertGreater(len(lines), 1)
            self.assertTrue(all(len(l) <= 100 for l in lines), lines)
            self.assertEqual(yaml.safe_load("k:\n" + "\n".join(lines) + "\n")["k"]["because"], s)

    def test_the_commands_carry_their_own_help(self):
        for cmd in ("set", "add", "review"):
            code, out, _ = run(SCRIPTS / "provenance.py", cmd, "--help")
            self.assertEqual(code, 0)
            self.assertIn(f"  {cmd} <", out)
        code, out, _ = run(SCRIPTS / "kpopper", "add", "--help")
        self.assertEqual(code, 0, out)
        self.assertIn("in id order", out)
        code, out, _ = run(SCRIPTS / "kpopper")
        self.assertIn("kpop review", out)


def before_second_session(d):
    """The fixture as the first session left it: one tab, and none of what the second wrote."""
    rec = copy_fixture(d)
    brief = d / "PROVENANCE.view.yaml"
    lines = rec.read_text(encoding="utf-8").split("\n")
    for nid in ("v.glazing_tab", "glaze.saving_kw", "glaze.quote_eur", "glaze.north_wall_m2",
                "s.2026_09_03_glazing"):
        _, _, s, e = P._locate(lines, nid)
        del lines[s:e]
    rec.write_text("\n".join(lines), encoding="utf-8")
    edit(brief, TAB_TWO, "")
    return rec, brief


class IntentsTabsCoverage(unittest.TestCase):
    """A session records what it was for as a source; a tab serves intents and earns the claim
    by picks; coverage is counted, printed at build and check, and never acted on."""

    def test_every_tab_is_drawn_and_says_what_it_serves(self):
        dom = dom_of(run(SCRIPTS / "render_page.py", RECORD)[1])
        self.assertIn('data-tab="now" aria-selected="true">The February night', dom)
        self.assertIn('<button type="button" data-tab="now2" aria-selected="false">The glazing quote '
                      '<span class="n">3</span></button>', dom)
        self.assertIn('<section id="panel-now2" hidden>', dom)
        self.assertIn('This tab is for one occasion &mdash; <b dir="auto">opened when the glazier&#x27;s '
                      'quote comes in, to set it against the shortfall</b>', dom)
        self.assertIn('It serves: <span class="fx" data-id="s.2026_09_03_glazing">What would glazing the '
                      'north wall cost, and how much of the shortfall would it close?</span>', dom)

    def test_the_hover_carries_what_a_session_asked(self):
        _, page, _ = run(SCRIPTS / "render_page.py", RECORD)
        self.assertIn('"asked": "Can we keep the greenhouse above 12°C through February on the old '
                      'boiler?"', page)

    def test_intents_are_dated_and_ordered_newest_first(self):
        self.assertEqual(R.intent_date("s.2026_09_03_x", {}), datetime.date(2026, 9, 3))
        self.assertEqual(R.intent_date("s.x", {"read": "2026-08-30"}), datetime.date(2026, 8, 30))
        self.assertIsNone(R.intent_date("s.x", {"of": "this session"}))
        doc = P.load([str(RECORD)])
        ids, jud, fields = P.infer(doc)
        raw = P.bodies(doc)
        self.assertEqual([k for k, _, _ in R.intents_of(ids, jud, raw)],
                         ["s.2026_09_03_glazing", "s.2026_09_02_heating"])
        # what a session recorded: what comes from it, and the judgment that rests on it
        self.assertEqual(sorted(R.recorded_by("s.2026_09_03_glazing", ids, jud, raw)),
                         ["glaze.north_wall_m2", "glaze.quote_eur", "glaze.saving_kw", "v.glazing_tab"])

    def test_coverage_is_reported_at_verify(self):
        code, out, _ = run(SCRIPTS / "render_page.py", "--verify", RECORD)
        self.assertEqual(code, 0, out)
        self.assertIn("NOTE coverage: 10 covered · spill 0 · 0 intents no tab serves · 0 recent in a "
                      "row · drift 0.0 since 2026-09-03", out)
        self.assertIn("NOTE tab 'The February night' serves s.2026_09_02_heating: picks 3 of 3 they "
                      "recorded", out)
        self.assertIn("NOTE tab 'The glazing quote' serves s.2026_09_03_glazing: picks 3 of 4 they "
                      "recorded", out)
        self.assertIn("NOTE no section picks: doc, q, s", out)
        self.assertNotIn("served by no tab", out)

    def test_a_second_session_plays_out(self):
        with tempfile.TemporaryDirectory() as d:
            rec, brief = before_second_session(pathlib.Path(d))
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("0 intents no tab serves", out)
            # a second session records its intent and writes under a new prefix, through add
            for args in (["s.2026_09_03_glazing", "asked=What would glazing the north wall cost, and "
                          "how much of the shortfall would it close?", "name=the glazing question",
                          "of=this session", "read=2026-09-03"],
                         ["glaze.north_wall_m2", "v=18", "unit=m2", "name=north wall",
                          "from=s.2026_09_03_glazing"],
                         ["glaze.quote_eur", "v=2400", "unit=EUR", "name=the quote",
                          "from=s.2026_09_03_glazing"],
                         ["glaze.saving_kw", "v=4", "unit=kW", "name=loss stopped",
                          "from=s.2026_09_03_glazing"]):
                code, out, err = run(SCRIPTS / "provenance.py", "add", *args, "--as-of", "2026-09-03", rec)
                self.assertEqual(code, 0, out + err)
            # Explicit page verification says the intent is served by no tab, and where what it wrote falls
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("NOTE s.2026_09_03_glazing is served by no tab - asked: What would glazing the "
                          "north wall cost, and how much of the shortfall would it close?\n"
                          "NOTE   hint: it wrote glaze. (3) - 0 of 3 inside 'The February night'\n", out)
            # the page already shows the entries, in the section the brief cannot switch off
            dom = dom_of(run(SCRIPTS / "render_page.py", rec)[1])
            self.assertIn('<h2 class="spill" dir="ltr">Not covered by this arrangement <span class="n">3'
                          '</span></h2>', dom)
            self.assertIn('Written for <span class="fx" data-id="s.2026_09_03_glazing">What would glazing '
                          'the north wall cost, and how much of the shortfall would it close?</span>, '
                          'which no tab serves.', dom)
            self.assertIn('<td class="kl" dir="auto">the quote</td><td class="v" dir="auto">'
                          '<span class="fx" data-id="glaze.quote_eur">2,400</span></td>', dom)
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("coverage: 6 covered · spill 0 · 1 intents no tab serves · 1 recent in a row · "
                          "drift 0.5 since 2026-09-02", out)
            self.assertIn("no section picks: doc, glaze, q, s", out)
            # a tab that declares it serves the intent and picks nothing it wrote fails
            edit(brief, "groups:\n", TAB_TWO.replace("pick: glaze.", "pick: heat.") + "groups:\n")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL tab 'The glazing quote' serves s.2026_09_03_glazing and picks nothing it "
                          "recorded - serving is earned by picks", out)
            self.assertIn("hint: it wrote glaze. (3) - 0 of 3 inside 'The February night'; 0 of 3 inside "
                          "'The glazing quote'", out)
            # a section that picks them earns it
            edit(brief, "pick: heat.\n        as: table\n    shape:", "pick: glaze.\n        as: table\n    shape:")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("0 intents no tab serves · 0 recent in a row · drift 0.0", out)
            self.assertIn("tab 'The glazing quote' serves s.2026_09_03_glazing: picks 3 of 3 they recorded", out)
            self.assertNotIn('class="spill"', dom_of(run(SCRIPTS / "render_page.py", rec)[1]))

    def test_an_intent_that_recorded_nothing_is_outside_coverage(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "s.2026_09_04_idle",
                               "asked=is the greenhouse worth heating at all?",
                               "name=a question, and nothing written for it", "read=2026-09-04",
                               "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out)
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("NOTE s.2026_09_04_idle recorded nothing, so no tab can serve it and none needs to", out)
            self.assertIn("0 intents no tab serves · 0 recent in a row", out)
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "serves: [s.2026_09_03_glazing]",
                 "serves: [s.2026_09_03_glazing, s.2026_09_04_idle]")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL tab 'The glazing quote' serves s.2026_09_04_idle and picks nothing it "
                          "recorded - serving is earned by picks - it recorded nothing", out)

    def test_a_falsifier_over_unserved_is_decided_by_the_page(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "    serves: [s.2026_09_03_glazing]\n", "")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL v.glazing_tab: wrong_if holds (page.unserved > 0) - decided by the page", out)
            self.assertIn("hint: it wrote glaze. (3), v. (1) - 1 of 4 inside 'The February night'; "
                          "3 of 4 inside 'The glazing quote'", out)
            # Core checking preserves the unavailable page condition without rendering.
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("wrong_if reads page.unserved, which is counted when the page is built", out)
            self.assertIn("page layout not checked", out)
            self.assertNotIn("wrong_if holds", out)

    def test_page_counts_are_snapshotted_by_add(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.both_tabs",
                                 "rests_on=[page.unserved, page.recent_unserved, page.drift, page.covered]",
                                 "verdict=two tabs, each earning what it serves",
                                 "wrong_if=page.unserved > 0", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out + err)
            # settled against the record as written: the new judgment is itself picked
            self.assertIn("seen: {page.unserved: 0, page.recent_unserved: 0, page.drift: 0.0, "
                          "page.covered: 11}", rec.read_text(encoding="utf-8"))
            self.assertEqual(run(SCRIPTS / "render_page.py", "--verify", rec)[0], 0)

    def test_drift_has_no_value_without_a_born(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, '    born: "2026-09-02"\n', "")
            edit(rec, '    born: "2026-09-03"\n', "")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("· drift -", out)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.drifty", "rests_on=[page.drift]",
                                 "verdict=x", "wrong_if=page.drift > 0.5", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 1)
            self.assertIn("page.drift has no value to snapshot: the page could not count it - no "
                          "arrangement carries born", out + err)
            # named anyway, the page says so where the value would stand
            edit(rec, "rests_on: [s.2026_09_03_glazing, page.unserved]",
                 "rests_on: [s.2026_09_03_glazing, page.unserved, page.drift]")
            dom = dom_of(run(SCRIPTS / "render_page.py", rec)[1])
            self.assertRegex(dom, r'<td class="kl" dir="auto">[^<]+</td><td class="v" dir="auto">'
                             r'<span class="fx" data-id="page.drift"><span class="derived">'
                             r'not counted yet</span></span></td>')
            self.assertNotIn('>page.drift<', dom)

    def test_spill_is_drawn_on_every_tab(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "pick: judgments",
                 "pick: [c.boiler_short, v.heating_tab, v.glazing_tab]")
            rec.write_text(rec.read_text(encoding="utf-8") + STRAY, encoding="utf-8")
            dom = dom_of(run(SCRIPTS / "render_page.py", rec)[1])
            self.assertEqual(dom.count('<h2 class="spill" dir="ltr">Not covered by this arrangement '
                                       '<span class="n">1</span></h2>'), 2)
            for key in ("now", "now2"):
                i = dom.index(f'<section id="panel-{key}"')
                self.assertIn('data-id="c.stray"', dom[i:dom.index("</section>", i)])

    def test_the_opener_does_not_require_page_coverage(self):
        with tempfile.TemporaryDirectory() as d:
            rec, brief = before_second_session(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_03_glazing",
                "asked=What would glazing the north wall cost?", "name=the glazing question",
                "read=2026-09-03", "--as-of", "2026-09-03", rec)
            run(SCRIPTS / "provenance.py", "add", "glaze.quote_eur", "v=2400", "name=the quote",
                "from=s.2026_09_03_glazing", "--as-of", "2026-09-03", rec)
            code, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertEqual(code, 0, out)
            self.assertTrue(out.endswith(PLAIN_NEXT), out)
            self.assertNotIn("served by no tab", out)
        # every intent served, or no brief at all: the line is what it always was
        _, out, _ = run(SCRIPTS / "provenance.py", "open", RECORD)
        self.assertTrue(out.endswith(PLAIN_NEXT), out)
        with tempfile.TemporaryDirectory() as d:
            rec = pathlib.Path(d) / "PROVENANCE.yaml"
            shutil.copy(RECORD, rec)
            _, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertTrue(out.endswith(PLAIN_NEXT), out)

    def test_the_gate_reminds_about_what_the_session_left(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            state = pathlib.Path(d) / "state"
            self.assertEqual(run(SCRIPTS / "provenance.py", "mark", state, rec)[0], 0)
            self.assertEqual(run(SCRIPTS / "provenance.py", "gate", state, rec)[0], 0)
            # entries written, no intent recorded
            run(SCRIPTS / "provenance.py", "add", "heat.wind_kw", "v=3", "unit=kW", "name=loss to wind",
                "from=doc.boiler_sheet", "--as-of", "2026-09-04", rec)
            code, out, _ = run(SCRIPTS / "provenance.py", "gate", state, rec)
            self.assertEqual(code, 2, out)
            self.assertEqual(out, 'this session wrote 1 entry (heat.wind_kw) and recorded no intent: add '
                                  's.<date>_<slug> asked="..." name="...", and from: it on what it wrote\n')
            # the intent recorded, with what it wrote from it: now it is the intent no tab serves
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_04_wind", "asked=how much does the wind add?",
                "name=the wind question", "read=2026-09-04", "--as-of", "2026-09-04", rec)
            run(SCRIPTS / "provenance.py", "add", "heat.gust_kw", "v=2", "unit=kW", "name=loss in a gust",
                "from=s.2026_09_04_wind", "--as-of", "2026-09-04", rec)
            code, out, _ = run(SCRIPTS / "provenance.py", "gate", state, rec)
            self.assertEqual(code, 0, out)
            self.assertEqual(out, "")
            # served by a tab whose sections pick what it wrote, the gate has nothing to say
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "    serves: [s.2026_09_02_heating]\n",
                 "    serves: [s.2026_09_02_heating, s.2026_09_04_wind]\n")
            self.assertEqual(run(SCRIPTS / "provenance.py", "gate", state, rec)[0], 0)

    def test_the_gate_holds_the_record_against_its_mark(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            state = pathlib.Path(d) / "state"
            # a record already red at session start never bounces the session for it
            rec.write_text(rec.read_text(encoding="utf-8") + STRAY, encoding="utf-8")
            run(SCRIPTS / "provenance.py", "mark", state, rec)
            self.assertEqual(run(SCRIPTS / "provenance.py", "gate", state, rec)[0], 0)
            rec.write_text(rec.read_text(encoding="utf-8")
                           + STRAY.replace("c.stray", "c.stray2").replace("heat.gone", "heat.gone2"),
                           encoding="utf-8")
            code, out, _ = run(SCRIPTS / "provenance.py", "gate", state, rec)
            self.assertEqual(code, 2, out)
            self.assertIn(f"{rec} fails check with 4 problems (2 at session start).\n"
                          "Fix the record - or declare the hole with blocked_on - before finishing:\n", out)
            self.assertIn("FAIL c.stray2: rests on heat.gone2, which is not an entry\n", out)
            self.assertIn("this session wrote 1 entry (c.stray2) and recorded no intent", out)
            # a mark that holds only the count, as an older opener wrote it, is still read
            state.write_text("2\n", encoding="utf-8")
            code, out, _ = run(SCRIPTS / "provenance.py", "gate", state, rec)
            self.assertEqual(code, 2, out)
            self.assertIn("fails check with 4 problems (2 at session start)", out)
            self.assertNotIn("recorded no intent", out)

    def test_legacy_stop_never_continues_the_turn(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            env = dict(os.environ, TMPDIR=d, KPOPPER_AGENT_SESSION="t1",
                       PATH=str(pathlib.Path(sys.executable).parent) + os.pathsep + os.environ["PATH"],
                       KPOPPER_RUNTIME_HOME=str(pathlib.Path(d) / "no-private-runtime"))

            def hook(name, payload):
                return subprocess.run(["sh", str(SCRIPTS / name)], cwd=d, input=json.dumps(payload),
                                      capture_output=True, text=True, env=env)
            p = hook("session_open.sh", {"session_id": "t1"})
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertIn("nothing needs a person right now.", p.stdout)
            self.assertTrue((pathlib.Path(d) / "kpopper-base-t1").exists())
            self.assertEqual(hook("session_gate.sh", {"session_id": "t1"}).returncode, 0)
            run(SCRIPTS / "provenance.py", "add", "heat.storm_kw", "v=5", "unit=kW", "name=loss in a storm",
                "from=doc.boiler_sheet", "--as-of", "2026-09-04", rec, env=env)
            p = hook("session_gate.sh", {"session_id": "t1"})
            self.assertEqual((p.returncode, p.stdout, p.stderr), (0, "", ""))
            # once: the second stop goes through
            p = hook("session_gate.sh", {"session_id": "t1"})
            self.assertEqual(p.returncode, 0)
            self.assertEqual(p.stderr, "")

    def test_a_reference_to_a_page_count_reads_the_same_on_both_surfaces(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, '              numbers."', '              numbers. Today {{page.unserved}} intents are unserved."')
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            # the hover payload and the card both read the count, not the count's name
            self.assertIn("Today 0 intents are unserved.", page)
            self.assertIn('Today <span class="fx in" data-id="page.unserved">0</span> intents are unserved.',
                          dom_of(page))
            self.assertNotIn("Today intents no tab", page)

    def test_spill_shows_each_id_once(self):
        with tempfile.TemporaryDirectory() as d:
            rec, brief = before_second_session(pathlib.Path(d))
            edit(brief, "pick: judgments", "pick: [c.boiler_short, v.heating_tab]")
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_03_glazing",
                "asked=What would glazing the north wall cost?", "name=the glazing question",
                "read=2026-09-03", "--as-of", "2026-09-03", rec)
            run(SCRIPTS / "provenance.py", "add", "glaze.quote_eur", "v=2400", "name=the quote",
                "from=s.2026_09_03_glazing", "--as-of", "2026-09-03", rec)
            # a judgment the unserved session wrote that is also flagged, and nothing picks
            rec.write_text(rec.read_text(encoding="utf-8") + (
                "  c.glaze_hope:\n    rests_on: [s.2026_09_03_glazing, glaze.gone]\n"
                "    verdict: \"glazing closes the gap\"\n    wrong_if: \"\"\n    seen: {}\n"),
                encoding="utf-8")
            dom = dom_of(run(SCRIPTS / "render_page.py", rec)[1])
            i = dom.index('<section id="panel-now"')
            panel = dom[i:dom.index("</section>", i)]
            self.assertIn('<h2 class="spill" dir="ltr">Not covered by this arrangement <span class="n">2'
                          '</span></h2>', panel)
            self.assertEqual(panel.count('data-id="c.glaze_hope"'), 1)
            self.assertEqual(panel.count('data-id="glaze.quote_eur"'), 1)

    def test_the_recent_streak_counts_by_day(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_03_frost", "asked=when is the first frost due?",
                "name=the frost question", "read=2026-09-03", "--as-of", "2026-09-03", rec)
            run(SCRIPTS / "provenance.py", "add", "when.first_frost", "v=2026-11-02", "name=first frost",
                "from=s.2026_09_03_frost", "--as-of", "2026-09-03", rec)
            # the same day as a served intent: the page noticed that day, so the run is 0
            _, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertIn("1 intents no tab serves · 0 recent in a row", out)
            # nothing of that day served: both count, and the day before ends the run
            edit(brief, "    serves: [s.2026_09_03_glazing]\n", "")
            _, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertIn("2 intents no tab serves · 2 recent in a row", out)


if __name__ == "__main__":
    unittest.main()
