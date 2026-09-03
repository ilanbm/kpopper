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


def run(*args, cwd=None):
    """The scripts as a session runs them: a subprocess, its exit code and both streams."""
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd,
                       capture_output=True, text=True)
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
            edit(brief, '          c.boiler_short: "the old boiler cannot hold 12°C on the coldest February night"\n', "")
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
            self.assertIn("section 'What the numbers say': its text saw c.boiler_short = the old "
                          "boiler cannot hold 12°C on the coldest February night, now the old boiler "
                          "cannot hold 12°C on any February night - read it again", out)
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
            self.assertIn('<div class="card moved"><div class="vd fx" data-id="c.boiler_short"', dom)
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
            shutil.copy(ROOT / "PROVENANCE.yaml", rec)
            before = rec.read_text(encoding="utf-8")
            updated = re.search(r"^  updated: (\S+)", before, re.M).group(1)
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
        self.assertIn("kpopper review", out)


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
            # check says the intent is served by no tab, and where what it wrote falls
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
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
            self.assertIn('<span class="fx" data-id="glaze.quote_eur">the quote</span></td>'
                          '<td class="v" dir="auto">2,400</td>', dom)
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
            # check leaves the line to the page, and still says which intent
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("NOTE s.2026_09_03_glazing is served by no tab", out)

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
            self.assertIn('<td class="k" dir="auto"><span class="fx" data-id="page.drift">page.drift</span>'
                          '</td><td class="v" dir="auto"><span class="derived">not counted yet</span></td>', dom)

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

    def test_the_opener_names_the_newest_unserved_intent(self):
        with tempfile.TemporaryDirectory() as d:
            rec, brief = before_second_session(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_03_glazing",
                "asked=What would glazing the north wall cost?", "name=the glazing question",
                "read=2026-09-03", "--as-of", "2026-09-03", rec)
            run(SCRIPTS / "provenance.py", "add", "glaze.quote_eur", "v=2400", "name=the quote",
                "from=s.2026_09_03_glazing", "--as-of", "2026-09-03", rec)
            code, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertEqual(code, 0, out)
            self.assertTrue(out.endswith("next: check - s.2026_09_03_glazing is served by no tab · pull "
                                         "<entry|prefix> (values with sources) · affects <entry> (what a "
                                         "change reaches)\n"), out)
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
            self.assertEqual(code, 2, out)
            self.assertEqual(out, "s.2026_09_04_wind is served by no tab of the page - serve it in a tab "
                                  "whose sections pick what it wrote, or leave it outside and say why\n")
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

    def test_the_hooks_mark_at_open_and_bounce_once_at_stop(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            env = dict(os.environ, TMPDIR=d)

            def hook(name, payload):
                return subprocess.run(["sh", str(SCRIPTS / name)], cwd=d, input=json.dumps(payload),
                                      capture_output=True, text=True, env=env)
            p = hook("session_open.sh", {"session_id": "t1"})
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertIn("nothing needs a person right now.", p.stdout)
            self.assertTrue((pathlib.Path(d) / "kpopper-base-t1").exists())
            self.assertEqual(hook("session_gate.sh", {"session_id": "t1"}).returncode, 0)
            run(SCRIPTS / "provenance.py", "add", "heat.storm_kw", "v=5", "unit=kW", "name=loss in a storm",
                "from=doc.boiler_sheet", "--as-of", "2026-09-04", rec)
            p = hook("session_gate.sh", {"session_id": "t1"})
            self.assertEqual(p.returncode, 2)
            self.assertIn("this session wrote 1 entry (heat.storm_kw) and recorded no intent", p.stderr)
            # once: the second stop goes through
            p = hook("session_gate.sh", {"session_id": "t1", "stop_hook_active": True})
            self.assertEqual(p.returncode, 0)
            self.assertEqual(p.stderr, "")


if __name__ == "__main__":
    unittest.main()
