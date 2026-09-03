"""The page contract: the fields the reader and the renderer accept, and the names they
compute. Runs against the fixture record in tests/fixtures/page with no browser and no
network:

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
FIXTURE = ROOT / "tests" / "fixtures" / "page"
RECORD = FIXTURE / "PROVENANCE.yaml"
BRIEF = FIXTURE / "PROVENANCE.view.yaml"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402
import yaml  # noqa: E402


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


def edit(path, old, new):
    text = path.read_text(encoding="utf-8")
    assert old in text, f"{old!r} is not in {path.name}"
    path.write_text(text.replace(old, new), encoding="utf-8")


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
        self.assertEqual(values["graph.judgments"], 2)
        self.assertEqual(values["graph.entries"], 7)
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
        self.assertEqual(sorted(built), ["graph.flagged", "page.spill"])
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

    def test_a_waiting_tab_is_checked_as_if_drawn(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            brief.write_text(brief.read_text(encoding="utf-8").replace(
                "groups:\n",
                "  - title: \"Later\"\n    occasion: \"opened after the first frost\"\n"
                "    serves: [s.2026_09_02_heating]\n"
                "    sections:\n      - title: \"Gone\"\n        pick: heat.typo\n"
                "    shape: {entries: 99, judgments: 2, flagged: 0, blocked: 0}\n"
                "groups:\n"), encoding="utf-8")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("section 'Gone' (tab 'Later') picks nothing", out)
            self.assertIn("tab 'Later' recorded a different shape: entries: 99 -> 7", out)
            self.assertIn("2 tabs declared; the page draws the first and keeps the rest", out)


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
            self.assertIn("entries: 7", err)
            self.assertIn("judgments: 2", err)


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
            edit(brief, "shape: {entries: 7,", "shape: {entries: 9,")
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            self.assertIn("changed shape since this arrangement was written", page)
            code, out, _ = run(SCRIPTS / "provenance.py", "review", "v.heating_tab",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("shape of tab 'The February night': entries: 7, judgments: 2, flagged: 0, "
                          "blocked: 0", out)
            self.assertIn("shape: {entries: 7, judgments: 2, flagged: 0, blocked: 0}",
                          brief.read_text(encoding="utf-8"))
            _, page, _ = run(SCRIPTS / "render_page.py", rec)
            self.assertNotIn("changed shape since this arrangement was written", page)

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


if __name__ == "__main__":
    unittest.main()
