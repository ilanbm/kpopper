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
                          "declares (declared: threads, where it came from)", out)

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

    def test_text_is_checked_and_reported(self):
        code, out, _ = run(SCRIPTS / "render_page.py", "--verify", RECORD)
        self.assertEqual(code, 0, out)
        self.assertIn("text on 1 section is checked and not yet drawn", out)

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


if __name__ == "__main__":
    unittest.main()
