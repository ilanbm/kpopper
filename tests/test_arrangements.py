"""Arrangement decisions: what the build maintains for them (born, stood), what their sign may
be, how a tab's shape move reads by it, how the brief is held against the decisions that
stand, and how one is re-decided. Runs against the fixture record in tests/fixtures/page with
no browser and no network:

    python3 -m unittest discover -s tests
"""
import pathlib
import re
import shlex
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

AS_OF = ["--as-of", "2026-09-05"]
REDECIDE = ["v.glazing_tab", "rests_on=[s.2026_09_03_glazing, s.2026_09_02_heating, page.unserved]",
            "verdict=the quote is read on the February night's tab", "wrong_if=page.unserved > 0"]
WIND_TAB = '''  - title: "The wind"
    occasion: "opened when the wind is counted"
    serves: [s.2026_09_05_wind]
    sections:
      - title: "What the wind adds"
        why: "one number"
        pick: heat.gust_kw
        as: table
'''


def run(*args):
    p = subprocess.run([sys.executable] + [str(a) for a in args], capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def copy_fixture(into):
    shutil.copy(RECORD, into / "PROVENANCE.yaml")
    shutil.copy(BRIEF, into / "PROVENANCE.view.yaml")
    return into / "PROVENANCE.yaml"


def edit(path, old, new, count=-1):
    text = path.read_text(encoding="utf-8")
    assert old in text, f"{old!r} is not in {path.name}"
    path.write_text(text.replace(old, new, count), encoding="utf-8")


def dom_of(rec):
    return re.sub(r"<script>.*?</script>", "", run(SCRIPTS / "render_page.py", rec)[1], flags=re.S)


def decided_lines(rec):
    return re.findall(r'<p class="sub"[^>]*>decided .*?</p>', dom_of(rec))


def entry(rec, nid):
    lines, out = rec.read_text(encoding="utf-8").split("\n"), []
    _, ind, s, e = P._locate(lines, nid)
    return "\n".join(lines[s:e])


def wind_lands(rec):
    """A third session records an intent, and what it wrote, that no tab serves."""
    run(SCRIPTS / "provenance.py", "add", "s.2026_09_05_wind", "asked=how much does the wind add?",
        "name=the wind question", "read=2026-09-05", *AS_OF, rec)
    run(SCRIPTS / "provenance.py", "add", "heat.gust_kw", "v=2", "unit=kW", "name=loss in a gust",
        "from=s.2026_09_05_wind", *AS_OF, rec)


class WhatTheBuildMaintains(unittest.TestCase):
    def test_the_fixture_is_green_and_carries_no_stored_stood(self):
        code, out, err = run(SCRIPTS / "provenance.py", "check", RECORD)
        self.assertEqual(code, 0, out + err)
        code, out, _ = run(SCRIPTS / "render_page.py", "--verify", RECORD)
        self.assertEqual(code, 0, out)
        self.assertNotIn("stood", RECORD.read_text(encoding="utf-8"))
        # nothing later than either arrangement was born, so neither shows a count
        self.assertEqual(decided_lines(RECORD),
                         ['<p class="sub" dir="auto">decided <span class="fx" data-id="v.heating_tab">'
                          '2026-09-02</span></p>',
                          '<p class="sub" dir="auto">decided <span class="fx" data-id="v.glazing_tab">'
                          '2026-09-03</span></p>'])

    def test_an_arrangement_is_read_by_shape(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # a judgment over the page's counts that rests on no session source decides no
            # occasion: nothing stamps it, nothing holds a tab to it
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.both_tabs",
                                 "rests_on=[page.unserved, page.covered]", "verdict=two tabs",
                                 "wrong_if=page.unserved > 0", *AS_OF, rec)
            self.assertEqual(code, 0, out + err)
            self.assertNotIn("born", entry(rec, "v.both_tabs"))
            # a judgment resting on a session source with a count in its sign is one, whatever
            # its prefix
            code, out, err = run(SCRIPTS / "provenance.py", "add", "d.one_page",
                                 "rests_on=[s.2026_09_02_heating, graph.judgments]",
                                 "verdict=the page stays one", "wrong_if=graph.judgments > 9", *AS_OF, rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn('born: "2026-09-05"', entry(rec, "d.one_page"))
            self.assertIn('data-id="d.one_page">2026-09-05</span>', dom_of(rec))

    def test_born_is_stamped_and_refused_when_typed(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            wind_lands(rec)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.wind_tab",
                                 "rests_on=[s.2026_09_05_wind, page.spill]", "born=2026-09-05",
                                 "verdict=x", "wrong_if=page.spill > 0", *AS_OF, rec)
            self.assertEqual(code, 1)
            self.assertIn("born is written by this tool - the day the arrangement is decided - leave "
                          "it out", out + err)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.wind_tab",
                                 "rests_on=[s.2026_09_05_wind, page.spill]", "verdict=a wind tab",
                                 "wrong_if=page.spill > 0", *AS_OF, rec)
            self.assertEqual(code, 0, out + err)
            text = entry(rec, "v.wind_tab")
            self.assertIn('born: "2026-09-05"', text)
            self.assertLess(text.index("born:"), text.index("seen:"))

    def test_stood_counts_the_later_sessions_a_tab_served(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_05_more", "asked=what about the doors?",
                "name=the doors", "read=2026-09-05", *AS_OF, rec)
            run(SCRIPTS / "provenance.py", "add", "heat.door_kw", "v=1", "unit=kW",
                "name=loss through the doors", "from=s.2026_09_05_more", *AS_OF, rec)
            # a later session that landed unserved is the sign, not evidence: nothing counts yet
            self.assertNotIn("stood", "".join(decided_lines(rec)))
            edit(brief, "    serves: [s.2026_09_02_heating]\n",
                 "    serves: [s.2026_09_02_heating, s.2026_09_05_more]\n")
            lines = decided_lines(rec)
            self.assertIn('data-id="v.heating_tab">2026-09-02</span> &middot; stood 1 session</p>', lines[0])
            self.assertNotIn("stood", lines[1])      # the other tab served nothing later

    def test_a_first_arrangement_may_rest_on_drift(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, '    born: "2026-09-02"\n', "")
            edit(rec, '    born: "2026-09-03"\n', "")
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.drifty",
                                 "rests_on=[s.2026_09_02_heating, page.drift]", "verdict=drift is watched",
                                 "wrong_if=page.drift > 0.5", *AS_OF, rec)
            self.assertEqual(code, 0, out + err)
            # settled against its own born once written: the two sessions were read before it
            self.assertIn("page.drift: 0.0", entry(rec, "v.drifty"))
            self.assertEqual(run(SCRIPTS / "render_page.py", "--verify", rec)[0], 0)
        with tempfile.TemporaryDirectory() as d:
            # and decided once more against the counts as they stand with it written: a
            # session read on its born day with unpicked output makes the share 1.0, so the
            # arrangement would be born broken - undone, never left green
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, '    born: "2026-09-02"\n', "")
            edit(rec, '    born: "2026-09-03"\n', "")
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_05_misc", "asked=anything else?",
                "name=the rest", "read=2026-09-05", *AS_OF, rec)
            run(SCRIPTS / "provenance.py", "add", "misc.thing", "v=1", "name=a thing nothing picks",
                "from=s.2026_09_05_misc", *AS_OF, rec)
            before = rec.read_text(encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.drifty",
                                 "rests_on=[s.2026_09_02_heating, page.drift]", "verdict=drift is watched",
                                 "wrong_if=page.drift > 0.5", *AS_OF, rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("wrong_if already holds (page.drift > 0.5) once the page counts it - the "
                          "arrangement would be born broken", out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)


class TheSign(unittest.TestCase):
    def test_a_sign_is_one_comparison_that_can_hold(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            wind_lands(rec)
            base = ["add", "v.wind_tab", "verdict=x", *AS_OF]
            for deps, pred, why in (
                    ("[s.2026_09_05_wind, page.spill, page.unserved]", "page.spill > 0 or page.unserved > 0",
                     "wrong_if is not one comparison this reader decides (a name, an operator, one value)"),
                    ("[s.2026_09_05_wind, page.spill]", "page.spill < 0",
                     "wrong_if can never hold - a count is never below zero (page.spill < 0)"),
                    ("[s.2026_09_05_wind, page.drift]", "page.drift > 1",
                     "wrong_if can never hold - a share is never above one (page.drift > 1)")):
                code, out, err = run(SCRIPTS / "provenance.py", *base, f"rests_on={deps}",
                                     f"wrong_if={pred}", rec)
                self.assertEqual(code, 1, out + err)
                self.assertIn(why + " - an arrangement's sign is decided by the build, or it is decoration",
                              out + err)
            # a compound sign that reached the record another way fails check: the comparison
            # shape would read "0 or page.unserved > 0" as its value and decide it as text
            edit(rec, "rests_on: [s.2026_09_02_heating, graph.flagged, page.spill]",
                 "rests_on: [s.2026_09_02_heating, graph.flagged, page.spill, page.unserved]")
            edit(rec, 'wrong_if: "page.spill > 0"', 'wrong_if: "page.spill > 0 or page.unserved > 0"')
            edit(rec, "graph.flagged: 0, page.spill: 0}", "graph.flagged: 0, page.spill: 0, page.unserved: 0}")
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL v.heating_tab: wrong_if is not one comparison this reader decides", out)
        with tempfile.TemporaryDirectory() as d:
            # one value means one the build can compare: an entry with no value would leave the
            # sign green forever, and a count is never equal to a negative number
            rec = copy_fixture(pathlib.Path(d))
            wind_lands(rec)
            for deps, pred, why in (
                    ("[s.2026_09_05_wind, page.spill, doc.boiler_sheet]", "page.spill > doc.boiler_sheet",
                     "wrong_if compares against doc.boiler_sheet, which holds no value the build can compare"),
                    ("[s.2026_09_05_wind, page.spill]", "page.spill == -1",
                     "wrong_if can never hold - a count is never below zero (page.spill == -1)")):
                code, out, err = run(SCRIPTS / "provenance.py", "add", "v.wind_tab", "verdict=x",
                                     f"rests_on={deps}", f"wrong_if={pred}", *AS_OF, rec)
                self.assertEqual(code, 1, out + err)
                self.assertIn(why, out + err)

    def test_the_tab_first_then_the_decision(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            wind_lands(rec)
            # the intent is unserved, so "wrong if any intent is unserved" holds at birth - the
            # page's counts reach the birth check the way the record's own do
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.wind_tab",
                                 "rests_on=[s.2026_09_05_wind, page.unserved]", "verdict=a third tab",
                                 "wrong_if=page.unserved > 0", *AS_OF, rec)
            self.assertEqual(code, 1)
            self.assertIn("wrong_if already holds (page.unserved > 0) - the judgment would be born broken",
                          out + err)
            edit(brief, "groups:\n", WIND_TAB + "groups:\n")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("NOTE tab 'The wind' is an arrangement no decision records - add v.<slug> "
                          "rests_on=[s.2026_09_05_wind, page.unserved] verdict=... wrong_if='page.unserved > 0'",
                          out)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.wind_tab",
                                 "rests_on=[s.2026_09_05_wind, page.unserved]", "verdict=a third tab",
                                 "wrong_if=page.unserved > 0", *AS_OF, rec)
            self.assertEqual(code, 0, out + err)
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertNotIn("no decision records", out)


class TheShapeMove(unittest.TestCase):
    def test_a_move_inside_is_muted_and_review_settles_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.wind_kw", "v=3", "unit=kW",
                                 "name=loss to wind", "from=doc.boiler_sheet", *AS_OF, rec)
            self.assertEqual(code, 0, out + err)
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("NOTE tab 'The February night': shape moved (entries: 11 -> 12) - muted, "
                          "v.heating_tab's sign has not appeared", out)
            self.assertIn("NOTE tab 'The glazing quote': shape moved (entries: 11 -> 12) - muted, "
                          "v.glazing_tab's sign has not appeared", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("- muted, v.heating_tab's sign has not appeared", out)
            banners = re.findall(r'<div class="banner mut"[^>]*>.*?</div>', dom_of(rec))
            self.assertEqual(len(banners), 2)
            self.assertIn("The record has changed shape since this arrangement was written &mdash; entries: "
                          "11 &rarr; 12. Its sign has not appeared: spill 0 &middot; 0 intents no tab serves "
                          "&middot; drift 0.0 since 2026-09-03 &middot; since <span class=\"fx\" "
                          "data-id=\"v.heating_tab\">heating tab</span> was decided 0.0.",
                          banners[0])
            code, out, _ = run(SCRIPTS / "provenance.py", "review", "v.heating_tab", *AS_OF, rec)
            self.assertEqual(code, 0, out)
            self.assertIn("shape of tab 'The February night': entries: 12, judgments: 3, flagged: 0, blocked: 0",
                          out)
            self.assertEqual(len(re.findall("banner mut", dom_of(rec))), 1)

    def test_a_move_outside_fires_and_every_surface_says_so(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            wind_lands(rec)
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL v.glazing_tab: wrong_if holds (page.unserved > 0) - decided by the page", out)
            self.assertIn("- muted, v.heating_tab's sign has not appeared", out)
            self.assertNotIn("muted, v.glazing_tab", out)
            loud = re.findall(r'<div class="banner"[^>]*>.*?</div>', dom_of(rec))
            self.assertEqual(len(loud), 1)
            self.assertIn("spill 0 &middot; 1 intents no tab serves &middot; drift 0.0 since 2026-09-03 "
                          "&middot; since <span class=\"fx\" data-id=\"v.glazing_tab\">glazing tab</span> "
                          "was decided 0.0.", loud[0])
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("NOTE v.glazing_tab: wrong_if holds (page.unserved > 0) - decided by the page, "
                          "page.unserved is 1", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertTrue(out.endswith("next: check - v.glazing_tab fired (page.unserved > 0) · pull "
                                         "<entry|prefix> (values with sources) · affects <entry> (what a "
                                         "change reaches)\n"), out)


class TheReDecision(unittest.TestCase):
    def test_without_a_sign_it_lands_in_a_hypothesis_that_contests_the_arrangement(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", *REDECIDE, *AS_OF, rec)
            self.assertEqual(code, 1)
            self.assertIn("v.glazing_tab is already a judgment, concluding 'a second tab, for the day the "
                          "quote is read' - the standing judgment holds, and its wrong_if has not fired, "
                          "so a different verdict under the same id contradicts it, and a hypothesis holds "
                          "the other: add v.glazing_tab", out + err)
            named = re.search(r"add v\.glazing_tab .*--hypothesis (\S+)", out + err)
            command, name = named.group(0), named.group(1)
            base = rec.read_text(encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", *shlex.split(command), rec)
            self.assertEqual(code, 0, out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), base)
            self.assertIn('<p class="sub" dir="auto">A hypothesis contests this arrangement &mdash; '
                          f"{name}: the quote is read on the February night&#x27;s tab</p>", dom_of(rec))
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn(f"NOTE v.glazing_tab is contested by hypothesis {name}: the quote is read on "
                          "the February night's tab", out)
        with tempfile.TemporaryDirectory() as d:
            # a re-decision that keeps the verdict and moves the sign is a contest too
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.glazing_tab",
                                 "rests_on=[s.2026_09_03_glazing, page.spill]",
                                 "verdict=a second tab, for the day the quote is read",
                                 "wrong_if=page.spill > 0", *AS_OF, "--hypothesis", "quieter_sign", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("NOTE v.glazing_tab is contested by hypothesis quieter_sign: a second tab, for the "
                          "day the quote is read", run(SCRIPTS / "provenance.py", "check", rec)[1])

    def test_on_a_fired_sign_it_is_admitted_once_a_day_and_keeps_what_it_replaced(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # a sign the record alone decides, so the page cannot be blamed for it
            edit(rec, "rests_on: [s.2026_09_03_glazing, page.unserved]",
                 "rests_on: [s.2026_09_03_glazing, graph.judgments]")
            edit(rec, 'wrong_if: "page.unserved > 0"', 'wrong_if: "graph.judgments > 3"')
            edit(rec, 'seen: {s.2026_09_03_glazing: "read 2026-09-03", page.unserved: 0}',
                 'seen: {s.2026_09_03_glazing: "read 2026-09-03", graph.judgments: 3}')
            run(SCRIPTS / "provenance.py", "add", "c.wind_matters", "rests_on=[heat.loss_kw]",
                "verdict=the wind matters", "wrong_if=heat.loss_kw < 20", *AS_OF, rec)
            self.assertIn("FAIL v.glazing_tab: wrong_if holds (graph.judgments > 3)",
                          run(SCRIPTS / "provenance.py", "check", rec)[1])
            # the page it is drawn on says it fired, so --verify fails it whichever count it reads
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL v.glazing_tab: wrong_if holds (graph.judgments > 3) - the arrangement fired", out)
            # what replaces an arrangement is an arrangement: a body that drops the count is refused
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.glazing_tab",
                                 "rests_on=[s.2026_09_03_glazing, heat.loss_kw]", REDECIDE[2],
                                 "wrong_if=heat.loss_kw > 40", *AS_OF, rec)
            self.assertEqual(code, 1)
            self.assertIn("what replaces an arrangement is an arrangement - rest on the session sources of "
                          "the occasion it decides and give it a sign over a count", out + err)
            code, out, err = run(SCRIPTS / "provenance.py", "add", *REDECIDE,
                                 "because=the sign appeared: a fourth judgment", *AS_OF, rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("supersede v.glazing_tab: a second tab, for the day the quote is read -> the quote "
                          "is read on the February night's tab - its sign holds (graph.judgments > 3) with "
                          "its tab intact - graph.judgments is 4\n", out)
            text = entry(rec, "v.glazing_tab")
            self.assertIn('born: "2026-09-05"', text)
            self.assertIn('replaced: ["born 2026-09-03, stood 0 sessions; its sign holds (graph.judgments > 3) '
                          'with its tab intact - graph.judgments is 4 on 2026-09-05"]', text)
            self.assertEqual(rec.read_text(encoding="utf-8").count("v.glazing_tab:"), 1)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
            # twice in a day is a contradiction, whatever the sign says
            code, out, err = run(SCRIPTS / "provenance.py", "add", *REDECIDE[:2], "verdict=changed my mind",
                                 REDECIDE[3], *AS_OF, rec)
            self.assertEqual(code, 1)
            self.assertIn("it was decided on 2026-09-05 - a second decision on the same day is a contradiction, "
                          "not a change", out + err)
            # it now rests on both occasions, so it governs both tabs and its review settles both
            code, out, _ = run(SCRIPTS / "provenance.py", "review", "v.glazing_tab", "--as-of", "2026-09-06", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("shape of tab 'The February night': entries: 11, judgments: 4, flagged: 0, blocked: 0\n"
                          "  shape of tab 'The glazing quote': entries: 11, judgments: 4, flagged: 0, blocked: 0\n",
                          out)
            self.assertEqual(run(SCRIPTS / "render_page.py", "--verify", rec)[0], 0)
            # and naming whose asking it was taken from is not a way round the day either
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_05_merge",
                "asked=One tab is enough - read the quote on the February night's tab.",
                "name=the merge, as asked", "read=2026-09-05", *AS_OF, rec)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.glazing_tab",
                                 "rests_on=[s.2026_09_03_glazing, s.2026_09_02_heating, s.2026_09_05_merge, "
                                 "page.unserved]", "request=s.2026_09_05_merge", "verdict=changed my mind",
                                 REDECIDE[3], *AS_OF, rec)
            self.assertEqual(code, 1)
            self.assertIn("it was decided on 2026-09-05 - a second decision on the same day is a contradiction, "
                          "not a change", out + err)

    def test_with_the_link_cut_it_is_refused(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            text = brief.read_text(encoding="utf-8")
            brief.write_text(text[:text.index('  - title: "The glazing quote"')] + text[text.index("groups:"):],
                             encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "add", *REDECIDE, *AS_OF, rec)
            self.assertEqual(code, 1)
            self.assertIn("the brief no longer carries what it decided - no tab's sections earn "
                          "s.2026_09_03_glazing - restore the tab, then re-decide", out + err)
        with tempfile.TemporaryDirectory() as d:
            # gutted, with its serves: line kept, cuts the link the same way
            rec = copy_fixture(pathlib.Path(d))
            edit(pathlib.Path(d) / "PROVENANCE.view.yaml", "pick: glaze.\n        as: table",
                 "pick: heat.\n        as: table")
            code, out, err = run(SCRIPTS / "provenance.py", "add", *REDECIDE, *AS_OF, rec)
            self.assertEqual(code, 1)
            self.assertIn("restore the tab, then re-decide", out + err)
            # and a person's word is not a way round a deleted tab: it is restored, then re-decided
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_05_merge",
                "asked=One tab is enough - read the quote on the February night's tab.",
                "name=the merge, as asked", "read=2026-09-05", *AS_OF, rec)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.glazing_tab",
                                 "rests_on=[s.2026_09_03_glazing, s.2026_09_02_heating, s.2026_09_05_merge, "
                                 "page.unserved]", "request=s.2026_09_05_merge", REDECIDE[2], REDECIDE[3],
                                 *AS_OF, rec)
            self.assertEqual(code, 1)
            self.assertIn("restore the tab, then re-decide", out + err)

    def test_a_hypothesis_flips_an_arrangement_no_more_easily_than_a_write(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # a sign the record alone decides, so the first re-decision is admitted in place
            edit(rec, "rests_on: [s.2026_09_03_glazing, page.unserved]",
                 "rests_on: [s.2026_09_03_glazing, graph.judgments]")
            edit(rec, 'wrong_if: "page.unserved > 0"', 'wrong_if: "graph.judgments > 3"')
            edit(rec, 'seen: {s.2026_09_03_glazing: "read 2026-09-03", page.unserved: 0}',
                 'seen: {s.2026_09_03_glazing: "read 2026-09-03", graph.judgments: 3}')
            run(SCRIPTS / "provenance.py", "add", "c.wind_matters", "rests_on=[heat.loss_kw]",
                "verdict=the wind matters", "wrong_if=heat.loss_kw < 20", *AS_OF, rec)
            run(SCRIPTS / "provenance.py", "add", *REDECIDE, *AS_OF, rec)
            run(SCRIPTS / "provenance.py", "add", *REDECIDE[:2], "verdict=changed my mind, one tab",
                REDECIDE[3], *AS_OF, "--hypothesis", "flip", rec)
            # the same day, the fold is refused where the write was: a hypothesis is not a
            # way round the rules the arrangement is decided by
            code, out, err = run(SCRIPTS / "consolidate.py", "--dry-run", "flip", *AS_OF, rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("it was decided on 2026-09-05 - a second decision on the same day is a "
                          "contradiction, not a change", out)
            # a day later it still waits for a person's name - its sign has not fired - and
            # taken by name it folds, and leaves the trail a re-decision in place leaves
            code, out, err = run(SCRIPTS / "consolidate.py", "flip", "--as-of", "2026-09-06", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("consolidate flip --take v.glazing_tab", out + err)
            code, out, err = run(SCRIPTS / "consolidate.py", "flip", "--take", "v.glazing_tab",
                                 "--as-of", "2026-09-06", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("born renewed, and what it replaced kept", out)
            text = entry(rec, "v.glazing_tab")
            self.assertIn('born: "2026-09-06"', text)
            self.assertIn('"born 2026-09-05, stood 0 sessions; the standing judgment holds, and a '
                          'person takes this over it by name on 2026-09-06"', text)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
            self.assertEqual(run(SCRIPTS / "render_page.py", "--verify", rec)[0], 0)

    def test_no_name_takes_a_body_that_is_not_an_arrangement_over_one(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            (pathlib.Path(d) / "PROVENANCE.d").mkdir(exist_ok=True)
            (pathlib.Path(d) / "PROVENANCE.d" / "flat.yaml").write_text(
                'hypothesis:\n  claim: "one tab is enough"\n  born: "2026-09-05"\n\n'
                'judgments:\n  v.glazing_tab:\n    rests_on: [heat.loss_kw]\n'
                '    verdict: "one tab is enough"\n    wrong_if: "heat.loss_kw > 40"\n'
                '    seen: {heat.loss_kw: 31}\n', encoding="utf-8")
            code, out, err = run(SCRIPTS / "consolidate.py", "--dry-run", "flat", *AS_OF, rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("what replaces an arrangement is an arrangement - rest on the session sources of "
                          "the occasion it decides and give it a sign over a count; no name takes a body that "
                          "drops them", out)
            code, out, err = run(SCRIPTS / "consolidate.py", "flat", "--take", "v.glazing_tab", *AS_OF, rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("refused - no name takes v.glazing_tab: what replaces an arrangement is an "
                          "arrangement", out + err)
            self.assertIn('born: "2026-09-03"', entry(rec, "v.glazing_tab"))

    def test_a_cut_tab_refuses_the_fold_as_it_refuses_the_write(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            run(SCRIPTS / "provenance.py", "add", *REDECIDE, *AS_OF, "--hypothesis", "one_tab", rec)
            text = brief.read_text(encoding="utf-8")
            brief.write_text(text[:text.index('  - title: "The glazing quote"')] + text[text.index("groups:"):],
                             encoding="utf-8")
            code, out, err = run(SCRIPTS / "consolidate.py", "--dry-run", "one_tab", *AS_OF, rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn("restore the tab, then re-decide", out)

    def test_a_persons_word_names_the_asking_and_admits_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_05_merge",
                "asked=Read the quote on the February night's tab, one tab is enough.",
                "name=the merge, as asked", "read=2026-09-05", *AS_OF, rec)
            base = rec.read_text(encoding="utf-8")
            # the session writes the source itself, and the method asks every session to, so
            # naming one is not a person's authority: the sign has not fired, and the
            # re-decision waits beside the record until a person folds it
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.glazing_tab",
                                 "rests_on=[s.2026_09_03_glazing, s.2026_09_02_heating, s.2026_09_05_merge, "
                                 "page.unserved]", "request=s.2026_09_05_merge", REDECIDE[2], REDECIDE[3],
                                 *AS_OF, rec)
            self.assertEqual(code, 1)
            self.assertIn("the standing judgment holds, and its wrong_if has not fired", out + err)
            command = re.search(r"add v\.glazing_tab .*--hypothesis \S+", out + err).group(0)
            code, out, err = run(SCRIPTS / "provenance.py", *shlex.split(command), rec)
            self.assertEqual(code, 0, out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), base)
            self.assertIn("A hypothesis contests this arrangement", dom_of(rec))
            # and the word travels with it: what a person reads at the fold says whose asking
            # the session took the change from
            held = pathlib.Path(d) / "PROVENANCE.d" / f"{shlex.split(command)[-1]}.yaml"
            self.assertIn("    request: s.2026_09_05_merge\n", held.read_text(encoding="utf-8"))

    def test_the_word_on_a_decision_is_read_on_every_surface(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            wind_lands(rec)
            edit(brief, "groups:\n", WIND_TAB + "groups:\n")
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.wind_tab",
                                 "rests_on=[s.2026_09_05_wind, page.unserved]", "request=s.2026_09_05_wind",
                                 "verdict=a third tab", "wrong_if=page.unserved > 0", *AS_OF, rec)
            self.assertEqual(code, 0, out + err)
            self.assertEqual(run(SCRIPTS / "render_page.py", "--verify", rec)[0], 0)
            self.assertIn("on the word of <span class=\"fx\" data-id=\"s.2026_09_05_wind\" "
                          "data-request=\"s.2026_09_05_wind\">how much does "
                          "the wind add?</span></p>", "".join(decided_lines(rec)))
            code, out, _ = run(SCRIPTS / "provenance.py", "pull", "v.wind_tab", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("    on the word of: s.2026_09_05_wind\n", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "open", "--chars", "4000", rec)
            self.assertIn("  = v.wind_tab (on the word of s.2026_09_05_wind): a third tab", out)
            # a request that is not a session source, or not rested on, is refused
            code, out, err = run(SCRIPTS / "provenance.py", "add", "v.second_wind",
                                 "rests_on=[s.2026_09_05_wind, page.spill]", "request=doc.boiler_sheet",
                                 "verdict=x", "wrong_if=page.spill > 0", "--as-of", "2026-09-06", rec)
            self.assertEqual(code, 1)
            self.assertIn("request: doc.boiler_sheet is not a session source carrying what was asked, that "
                          "the judgment also rests on", out + err)
            # and one that reached the record another way fails check and is nobody's word
            edit(rec, "    request: s.2026_09_05_wind\n", "    request: doc.boiler_sheet\n")
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL v.wind_tab: request: doc.boiler_sheet is not a session source carrying what "
                          "was asked, that it rests on", out)
            self.assertNotIn("on the word of", "".join(decided_lines(rec)))


class TheBriefHeldAgainstTheDecisions(unittest.TestCase):
    def test_the_brief_alone_cannot_reverse_an_arrangement(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            state = pathlib.Path(d) / "state"
            self.assertEqual(run(SCRIPTS / "provenance.py", "mark", state, rec)[0], 0)
            text = brief.read_text(encoding="utf-8")
            brief.write_text(text[:text.index('  - title: "The glazing quote"')] + text[text.index("groups:"):],
                             encoding="utf-8")
            reversal = ("FAIL the brief does not serve s.2026_09_03_glazing together, as v.glazing_tab decided "
                        "(no tab's sections earn s.2026_09_03_glazing) - serve them on a tab whose sections "
                        "pick what they wrote, or re-decide v.glazing_tab")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn(reversal, out)
            # the record's own claim, so check fails it too - and the gate bounces once, which is
            # the one enforcement an out-of-tree record has
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn(reversal, out)
            code, out, _ = run(SCRIPTS / "provenance.py", "gate", state, rec)
            self.assertEqual(code, 2, out)
            self.assertIn("fails check with 1 problems (0 at session start)", out)
        with tempfile.TemporaryDirectory() as d:
            # two occasions folded into one tab, every section kept: no count moves, and the
            # brief still contradicts the two decisions that keep them apart
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            text = brief.read_text(encoding="utf-8")
            i, j = text.index('  - title: "The glazing quote"'), text.index("groups:")
            second = text[i:j]
            merged = (text[:i].replace("    serves: [s.2026_09_02_heating]",
                                       "    serves: [s.2026_09_02_heating, s.2026_09_03_glazing]")
                      .replace("    shape: {entries: 11, judgments: 3, flagged: 0, blocked: 0}\n", "")
                      + second[second.index("      - title:"):second.index("    shape:")]
                      + "    shape: {entries: 11, judgments: 3, flagged: 0, blocked: 0}\n" + text[j:])
            brief.write_text(merged, encoding="utf-8")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("coverage: 10 covered · spill 0 · 0 intents no tab serves", out)
            self.assertIn("FAIL the brief reads v.glazing_tab and v.heating_tab on one tab ('The February "
                          "night'), which neither decided - re-decide the one whose occasion changed, and "
                          "review the other", out)

    def test_a_brief_with_no_arrangement_is_said_once_by_verify_alone(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            lines = rec.read_text(encoding="utf-8").split("\n")
            for nid in ("v.glazing_tab", "v.heating_tab"):
                _, _, s, e = P._locate(lines, nid)
                del lines[s:e]
            rec.write_text("\n".join(lines), encoding="utf-8")
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertEqual(out.count("NOTE no arrangement decision is recorded, so the brief is held against "
                                       "none - an arrangement is a judgment resting on the session sources a "
                                       "tab serves, with a sign over a count"), 1)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertNotIn("no arrangement decision", out)

    def test_a_question_contests_an_arrangement(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "q.one_tab",
                                 "one tab for both occasions - contests v.glazing_tab, whose sign has not appeared",
                                 *AS_OF, rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn('<p class="sub" dir="auto">A question contests this arrangement &mdash; <span '
                          'class="fx" data-id="q.one_tab">one tab for both occasions - contests glazing tab, '
                          "whose sign has not appeared</span></p>", dom_of(rec))
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("NOTE v.glazing_tab is contested by question q.one_tab: one tab for both occasions - "
                          "contests v.glazing_tab, whose sign has not appeared", out)


if __name__ == "__main__":
    unittest.main()
