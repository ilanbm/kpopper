"""The record forks on a contradiction: hypotheses beside the record, layered over it by
every command, contested where two disagree, refused into the base where a write contradicts
it. Runs against the fixture in tests/fixtures/hypotheses - a base and two hypotheses that
disagree about the loss - with no browser and no network:

    python3 -m unittest discover -s tests
"""
import datetime
import os
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
FIXTURE = ROOT / "tests" / "fixtures" / "hypotheses"
RECORD = FIXTURE / "PROVENANCE.yaml"
PAGE_FIXTURE = ROOT / "tests" / "fixtures" / "page"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402

PLAIN_NEXT = ("next: pull <entry|prefix> (values with sources) · affects <entry> (what a change "
              "reaches) · check\n")


def run(*args, cwd=None):
    """The scripts as a session runs them: a subprocess, its exit code and both streams."""
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd,
                       capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def copy_fixture(into):
    """A scratch copy of the fixture - the base, its brief and the hypotheses beside it."""
    shutil.copytree(FIXTURE, into, dirs_exist_ok=True)
    return into / "PROVENANCE.yaml"


def dom_of(page):
    return re.sub(r"<script>.*?</script>", "", page, flags=re.S)


class HypothesesBesideTheRecord(unittest.TestCase):
    """The reader loads PROVENANCE.d/ in every command and lays it over the base for reading;
    the base alone is what is evaluated."""

    def test_the_base_stays_green_and_check_says_what_is_contested(self):
        code, out, err = run(SCRIPTS / "provenance.py", "check", RECORD)
        self.assertEqual(code, 0, out + err)
        self.assertNotIn("FAIL", out)
        self.assertIn("CONTESTED heat.loss_kw: bigger_boiler says 33, glazing_redo says 28 - one of "
                      "them folds, or neither; a person decides\n", out)
        self.assertTrue(out.endswith("1 judgments, 8 entries, 0 problems, 1 contested\n"), out)

    def test_the_counts_are_computed_from_the_layer(self):
        doc = P.load([str(RECORD)])
        self.assertEqual(sorted(doc.hypotheses), ["bigger_boiler", "glazing_redo"])
        self.assertEqual(doc.hypotheses["bigger_boiler"]["head"]["folds"], "never")
        ids, jud, fields = P.infer(doc)
        # the base is the base: nothing a hypothesis holds is an entry of it
        self.assertNotIn("c.boiler_enough", ids)
        self.assertNotIn("doc.boiler_catalogue", ids)
        values = P.counts(doc, ids, jud, fields, P.bodies(doc))
        self.assertEqual(values["graph.hypotheses"], 2)
        self.assertEqual(values["graph.contested"], 1)
        self.assertEqual(values["graph.judgments"], 1)
        self.assertEqual(P.contested(doc), {"heat.loss_kw": [("bigger_boiler", 33), ("glazing_redo", 28)]})
        # two hypotheses that agree contest nothing
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            h = pathlib.Path(d) / "PROVENANCE.d" / "glazing_redo.yaml"
            h.write_text(h.read_text(encoding="utf-8").replace("v: 28", "v: 33"), encoding="utf-8")
            self.assertEqual(P.contested(P.load([str(rec)])), {})
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertNotIn("CONTESTED", out)

    def test_a_judgment_may_rest_on_the_counts(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.forks",
                                 "rests_on=[graph.hypotheses, graph.contested]",
                                 "verdict=the record forks, and someone reads the count",
                                 "wrong_if=graph.contested > 3", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("the new judgment holds: wrong_if does not hold (graph.contested > 3)", out)
            self.assertIn("seen: {graph.hypotheses: 2, graph.contested: 1}", rec.read_text(encoding="utf-8"))
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
            code, out, _ = run(SCRIPTS / "provenance.py", "pull", "graph.contested", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("graph.contested: 1 (ids two hypotheses hold with different claims)", out)

    def test_the_opener_counts_what_waits_and_ranks_the_contested_first(self):
        code, out, _ = run(SCRIPTS / "provenance.py", "open", RECORD)
        self.assertEqual(code, 0, out)
        age = P._age("2026-09-03", datetime.date.today())
        self.assertIn(f"8 entries, 1 judgments, 1 open questions, updated 2026-09-03\n"
                      f"2 hypotheses wait - bigger_boiler ({age}, never folds, 2 rest on it) · "
                      f"glazing_redo ({age}, 1 rests on it)\n", out)
        self.assertIn("needs a person (1):\n  heat.loss_kw: CONTESTED - bigger_boiler says 33, "
                      "glazing_redo says 28\n", out)
        # nothing else about the hypotheses enters the opener
        self.assertNotIn("36", out)
        self.assertNotIn("c.boiler_enough", out)
        self.assertTrue(out.endswith(PLAIN_NEXT), out)
        # the line survives the character budget, with the head
        code, out, _ = run(SCRIPTS / "provenance.py", "open", "--chars", "300", RECORD)
        self.assertIn("2 hypotheses wait - bigger_boiler", out)

    def test_the_standing_line_says_who_needs_a_person_and_calls_nothing_else_contested(self):
        # contested is one thing - an id two hypotheses hold with different claims; what the
        # opener lists above the standing judgments needs a person, whatever the reason
        code, out, _ = run(SCRIPTS / "provenance.py", "open", "--chars", "3000", RECORD)
        self.assertEqual(code, 0, out)
        self.assertIn("\nstanding:  (1 above needs a person)\n  = c.boiler_short:", out)
        self.assertEqual(out.count("CONTESTED"), 1)
        self.assertNotIn("are contested", out)

    def test_pull_reads_the_layer(self):
        code, out, _ = run(SCRIPTS / "provenance.py", "pull", "heat", RECORD)
        self.assertEqual(code, 0, out)
        self.assertIn("heat.boiler_kw: 24 (boiler output) <- doc.boiler_sheet\n"
                      "    proposes 24 -> 36, from bigger_boiler\n", out)
        self.assertIn("    proposes 31 -> 33, from bigger_boiler\n"
                      "    proposes 31 -> 28, from glazing_redo\n"
                      "    CONTESTED: bigger_boiler says 33, glazing_redo says 28\n", out)
        # a judgment only a hypothesis holds, read against the record as it stands under it
        self.assertIn("+ c.boiler_enough (in hypothesis bigger_boiler): a 36 kW boiler holds 12°C", out)
        self.assertIn("    holds\n    because: It gives 36 kW against a loss of 33 kW with the wind counted in.\n", out)
        # the base's own judgment is read against the base
        self.assertIn("+ c.boiler_short: the old boiler cannot hold 12°C on the coldest February night\n"
                      "    holds\n    because: It gives 24 kW against a loss of 31 kW", out)
        # an entry only a hypothesis holds is pulled by its prefix, and says who holds it
        code, out, _ = run(SCRIPTS / "provenance.py", "pull", "doc", RECORD)
        self.assertEqual(code, 0, out)
        self.assertIn("doc.boiler_catalogue:  (the supplier's catalogue, the 36 kW model) as of 2026-09-03 "
                      "- held by bigger_boiler\n", out)

    def test_affects_reaches_a_hypothesis_judgment(self):
        code, out, _ = run(SCRIPTS / "provenance.py", "affects", "heat.loss_kw", RECORD)
        self.assertEqual(code, 0, out)
        self.assertIn("c.boiler_enough (in hypothesis bigger_boiler)\n    via heat.loss_kw -> evaluate the "
                      "predicate against heat.loss_kw\n", out)
        self.assertIn("c.boiler_short\n    via heat.loss_kw", out)
        self.assertIn("2 judgments reached", out)

    def test_a_record_without_hypotheses_reads_as_before(self):
        rec = PAGE_FIXTURE / "PROVENANCE.yaml"
        doc = P.load([str(rec)])
        self.assertEqual(doc.hypotheses, {})
        ids, jud, fields = P.infer(doc)
        values = P.counts(doc, ids, jud, fields, P.bodies(doc))
        self.assertEqual((values["graph.hypotheses"], values["graph.contested"]), (0, 0))
        self.assertEqual(P.hypothesis_line(doc), "")
        code, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
        self.assertNotIn("hypothes", out)
        self.assertNotIn("CONTESTED", run(SCRIPTS / "provenance.py", "check", rec)[1])
        self.assertNotIn("hypothes", run(SCRIPTS / "render_page.py", rec)[1])

    def test_an_unreadable_hypothesis_fails_check_and_is_named_by_the_opener(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            (pathlib.Path(d) / "PROVENANCE.d" / "broken.yaml").write_text("hypothesis: [1, 2\n",
                                                                           encoding="utf-8")
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL hypothesis broken could not be read: while parsing", out)
            self.assertIn("1 problems, 1 contested", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("3 hypotheses wait - ", out)
            self.assertIn("3 hypotheses wait - broken (unreadable: while parsing", out)
            self.assertIn("(unreadable: while parsing a flow sequence in \"<unicode string>\", line 1, column "
                          "13: hypothesis: [1, 2 ^ expected ',' or ']', but got)",
                          P.hypothesis_line(P.load([str(rec)]), width=400))
            # a write into it is refused, and the file is left alone
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35",
                                 "--hypothesis", "broken", rec)
            self.assertEqual(code, 1)
            self.assertIn("hypothesis broken could not be read", out + err)


class TheWritePathForks(unittest.TestCase):
    """A write that contradicts the base is refused into it and named a hypothesis; with the
    flag it lands in the file beside the record and moves nothing else."""

    def test_a_reading_no_newer_than_the_base_is_refused_and_named(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            for as_of, why in (("2026-09-02", "a reading of the same day"),
                               ("2026-09-01", "a reading from 2026-09-01 that is older than the base's")):
                code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35",
                                     "--as-of", as_of, rec)
                self.assertEqual(code, 1, as_of)
                self.assertEqual((out + err).strip(),
                                 f"refused - heat.loss_kw holds 31 as of 2026-09-02, and {why} says 35 - "
                                 "the base keeps what it holds and a hypothesis holds the other: set "
                                 f"heat.loss_kw 35 --as-of {as_of} --hypothesis heat_loss_kw")
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            # the same value is no contradiction, and nothing to write
            code, out, _ = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "31", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("heat.loss_kw is already 31; nothing written", out)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)

    def test_a_newer_reading_updates_the_base(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35",
                                 "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("set heat.loss_kw: 31 -> 35 (as of 2026-09-03)", out)
            self.assertIn("MUTED     c.boiler_short: heat.loss_kw moved 31 -> 35", out)
            # dated through its source: the boiler's reading is the sheet's, 2026-09-02
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.boiler_kw", "25",
                                 "--as-of", "2026-09-02", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("heat.boiler_kw holds 24 as of 2026-09-02, and a reading of the same day says 25",
                          out + err)
            code, out, _ = run(SCRIPTS / "provenance.py", "set", "heat.boiler_kw", "25",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            # a value nothing dates is superseded by one something does, and the clock starts
            code, out, _ = run(SCRIPTS / "provenance.py", "set", "when.first_cold_night",
                               "2027-02-02", "--as-of", "2026-09-01", rec)
            self.assertEqual(code, 1, out)     # its source dates it: 2026-09-02
            rec.write_text(rec.read_text(encoding="utf-8").replace(
                '    name: "first night that matters"\n    from: s.2026_09_02_heating\n',
                '    name: "first night that matters"\n'), encoding="utf-8")
            code, out, _ = run(SCRIPTS / "provenance.py", "set", "when.first_cold_night",
                               "2027-02-02", "--as-of", "2026-09-01", rec)
            self.assertEqual(code, 0, out)
            code, out, err = run(SCRIPTS / "provenance.py", "set", "when.first_cold_night",
                                 "2027-02-03", "--as-of", "2026-09-01", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("when.first_cold_night holds 2027-02-02 as of 2026-09-01, and a reading of the "
                          "same day says 2027-02-03", out + err)

    def test_the_door_is_one_function(self):
        doc = P.load([str(RECORD)])
        ids, jud, fields = P.infer(doc)
        raw = P.with_builtins(doc, ids, jud, fields)
        entry = raw["heat.loss_kw"]
        self.assertEqual(P.may_supersede("heat.loss_kw", entry, 35, raw, ids, jud, fields, "2026-09-03"),
                         (True, "a reading from 2026-09-03 that is newer than the base's"))
        self.assertEqual(P.may_supersede("heat.loss_kw", entry, 35, raw, ids, jud, fields, "2026-09-02"),
                         (False, "a reading of the same day"))
        self.assertEqual(P.may_supersede("heat.loss_kw", {"v": 31}, 35, raw, ids, jud, fields, "2026-09-02"),
                         (True, "nothing dates the reading the base holds"))
        body = {"rests_on": ["heat.loss_kw"], "verdict": "other"}
        self.assertEqual(P.may_supersede("c.boiler_short", jud["c.boiler_short"]["body"], body, raw, ids,
                                         jud, fields)[0], False)

    def test_set_with_the_flag_writes_the_hypothesis_and_not_the_base(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35", "--as-of",
                                 "2026-09-02", "--why", "recounted with the wind",
                                 "--hypothesis", "heat_loss_kw", rec)
            self.assertEqual(code, 0, out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            self.assertIn("carry heat.loss_kw into known, its first entry of hypothesis heat_loss_kw\n"
                          "set heat.loss_kw in hypothesis heat_loss_kw: 31 -> 35 (as of 2026-09-02)\n"
                          "worked out from it: heat.deficit_kw\n"
                          "rests on it, under heat_loss_kw:\n"
                          "  MUTED     c.boiler_short: heat.loss_kw moved 31 -> 35, inside wrong_if", out)
            self.assertIn("the base is untouched; heat_loss_kw holds 1 entry and 0 judgments\n", out)
            h = (pathlib.Path(d) / "PROVENANCE.d" / "heat_loss_kw.yaml").read_text(encoding="utf-8")
            # the entry carried over whole, set there, its reason kept
            self.assertEqual(h, 'hypothesis: {born: "2026-09-02"}\n\nknown:\n  heat.loss_kw:\n    v: 35\n'
                                '    # set 2026-09-02: recounted with the wind\n    unit: kW\n'
                                '    name: "heat loss on a -5°C night"\n    from: s.2026_09_02_heating\n'
                                '    at: "worked out from the glazing area during the session"\n'
                                '    of: "2026-09-02"\n')
            # every reader sees it at once
            code, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertIn("3 hypotheses wait - heat_loss_kw (", out)
            self.assertIn("heat.loss_kw: CONTESTED - bigger_boiler says 33, glazing_redo says 28, "
                          "heat_loss_kw says 35", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("0 problems, 1 contested", out)
            # a second set within the hypothesis needs no carry
            code, out, _ = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "36", "--as-of",
                               "2026-09-03", "--hypothesis", "heat_loss_kw", rec)
            self.assertEqual(code, 0, out)
            self.assertNotIn("carry", out)
            self.assertIn("set heat.loss_kw in hypothesis heat_loss_kw: 35 -> 36", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "36",
                               "--hypothesis", "heat_loss_kw", rec)
            self.assertIn("heat.loss_kw is already 36 in hypothesis heat_loss_kw; nothing written", out)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)

    def test_add_of_an_id_the_base_holds_is_refused_and_named(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.loss_kw", "v=35", "name=x",
                                 "--as-of", "2026-09-02", rec)
            self.assertEqual(code, 1)
            self.assertEqual((out + err).strip(),
                             "refused - heat.loss_kw is already an entry, holding 31 as of 2026-09-02, and a "
                             "reading of the same day says 35 - the base keeps what it holds and a hypothesis "
                             "holds the other: add heat.loss_kw v=35 name=x --as-of 2026-09-02 --hypothesis "
                             "heat_loss_kw")
            # a newer reading is an update, and add says which command that is
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.loss_kw", "v=35", "name=x",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 1)
            self.assertIn("a newer reading updates it: set heat.loss_kw 35; one that disagrees opens a "
                          "hypothesis: add heat.loss_kw v=35 name=x --as-of 2026-09-04 --hypothesis "
                          "heat_loss_kw", out + err)
            # the same value is what it always was: already an entry
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.loss_kw", "v=31", rec)
            self.assertEqual(code, 1)
            self.assertIn("heat.loss_kw is already an entry - set changes its value, review its snapshot",
                          out + err)
            # a judgment under a standing judgment's id, with a different verdict
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short",
                                 "rests_on=[heat.boiler_kw, heat.loss_kw]",
                                 "verdict=the old boiler holds after all", "wrong_if=heat.loss_kw > 40",
                                 "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 1)
            self.assertEqual((out + err).strip(),
                             "refused - c.boiler_short is already a judgment, concluding 'the old boiler cannot "
                             "hold 12°C on the coldest February nig…' - the standing judgment holds, and its "
                             "wrong_if has not fired, so a different verdict under "
                             "the same id contradicts it, and a hypothesis holds the other: add c.boiler_short "
                             "'rests_on=[heat.boiler_kw, heat.loss_kw]' 'verdict=the old boiler holds after "
                             "all' 'wrong_if=heat.loss_kw > 40' --as-of 2026-09-03 --hypothesis c_boiler_short")
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            # the refusal joins the others, it does not replace them
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short",
                                 "rests_on=[heat.boiler_kw, heat.loss_kw]",
                                 "verdict=the old boiler holds after all",
                                 "wrong_if=heat.loss_kw > heat.boiler_kw", rec)
            self.assertEqual(code, 1)
            self.assertIn("wrong_if already holds (heat.loss_kw > heat.boiler_kw) - the judgment would be "
                          "born broken\n          c.boiler_short is already a judgment", out + err)

    def test_add_with_the_flag_opens_a_hypothesis(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short",
                                 "rests_on=[heat.boiler_kw, heat.loss_kw]",
                                 "verdict=the old boiler holds after all", "wrong_if=heat.loss_kw > 40",
                                 "--as-of", "2026-09-03", "--hypothesis", "c_boiler_short", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("add c.boiler_short into judgments, its first entry of hypothesis c_boiler_short\n"
                          "the new judgment holds: wrong_if does not hold (heat.loss_kw > 40)\n", out)
            self.assertIn("the base is untouched; c_boiler_short holds 0 entries and 1 judgment\n", out)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            h = pathlib.Path(d) / "PROVENANCE.d" / "c_boiler_short.yaml"
            self.assertEqual(h.read_text(encoding="utf-8"),
                             'hypothesis: {born: "2026-09-03"}\n\njudgments:\n  c.boiler_short:\n'
                             '    rests_on: [heat.boiler_kw, heat.loss_kw]\n'
                             '    verdict: "the old boiler holds after all"\n    wrong_if: "heat.loss_kw > 40"\n'
                             '    seen: {heat.boiler_kw: 24, heat.loss_kw: 31}\n')
            # pull reads the proposal beside the standing judgment
            code, out, _ = run(SCRIPTS / "provenance.py", "pull", "c.boiler_short", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("+ c.boiler_short: the old boiler cannot hold 12°C on the coldest February night\n", out)
            self.assertIn("    proposes instead, from c_boiler_short: the old boiler holds after all\n", out)
            # a second write lands in the same file, in id order, with its seen taken from the
            # record as it stands under the hypothesis
            code, out, err = run(SCRIPTS / "provenance.py", "set", "heat.boiler_kw", "40", "--as-of",
                                 "2026-09-03", "--hypothesis", "c_boiler_short", rec)
            self.assertEqual(code, 0, out + err)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.margin", "rests_on=[heat.boiler_kw]",
                                 "verdict=there is a margin", "wrong_if=heat.boiler_kw < 32",
                                 "--as-of", "2026-09-03", "--hypothesis", "c_boiler_short", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("add c.margin into judgments, after c.boiler_short of hypothesis c_boiler_short", out)
            text = h.read_text(encoding="utf-8")
            self.assertIn("    seen: {heat.boiler_kw: 40}\n", text)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_what_rests_on_a_hypothesis_goes_into_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.catalogue",
                                 "rests_on=[doc.boiler_catalogue]", "verdict=the catalogue is trusted",
                                 "wrong_if=", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 1)
            self.assertEqual((out + err).strip(),
                             "refused - rests on doc.boiler_catalogue, which only hypothesis bigger_boiler "
                             "holds - what rests on a hypothesis goes into it: add c.catalogue "
                             "'rests_on=[doc.boiler_catalogue]' 'verdict=the catalogue is trusted' wrong_if= "
                             "--as-of 2026-09-03 --hypothesis bigger_boiler")
            # a reference is a dependency the sentence declares
            code, out, err = run(SCRIPTS / "provenance.py", "add", "note.catalogue", "v=1",
                                 "via=see {{doc.boiler_catalogue}}", rec)
            self.assertEqual(code, 1)
            self.assertIn("references doc.boiler_catalogue, which only hypothesis bigger_boiler holds", out + err)
            # set of an id only a hypothesis holds says where it is set
            code, out, err = run(SCRIPTS / "provenance.py", "set", "doc.boiler_catalogue", "1", rec)
            self.assertEqual(code, 1)
            self.assertIn("doc.boiler_catalogue is held only by hypothesis bigger_boiler - it is set there: "
                          "set doc.boiler_catalogue 1 --hypothesis bigger_boiler", out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            # into the hypothesis it goes, resting on what the hypothesis proposes
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.catalogue",
                                 "rests_on=[doc.boiler_catalogue, heat.boiler_kw]",
                                 "verdict=the catalogue figure stands", "wrong_if=heat.boiler_kw < 30",
                                 "--as-of", "2026-09-03", "--hypothesis", "bigger_boiler", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("add c.catalogue into judgments, after c.boiler_enough of hypothesis bigger_boiler\n"
                          "the new judgment holds: wrong_if does not hold (heat.boiler_kw < 30)\n", out)
            self.assertIn("the base is untouched; bigger_boiler holds 3 entries and 2 judgments, and never folds", out)
            h = (pathlib.Path(d) / "PROVENANCE.d" / "bigger_boiler.yaml").read_text(encoding="utf-8")
            self.assertIn('    seen: {doc.boiler_catalogue: "read 2026-09-03", heat.boiler_kw: 36}\n', h)

    def test_a_hypothesis_write_is_validated_like_a_base_write(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            h = pathlib.Path(d) / "PROVENANCE.d" / "glazing_redo.yaml"
            before = h.read_text(encoding="utf-8")
            cases = (
                (["add", "heat.loss_kw", "v=29", "--hypothesis", "glazing_redo"],
                 "heat.loss_kw is already in hypothesis glazing_redo - set changes its value there"),
                (["add", "c.x", "rests_on=[heat.nothing]", "verdict=x", "wrong_if=heat.nothing > 1",
                  "--hypothesis", "glazing_redo"],
                 "rests on heat.nothing, which is not an entry - add it first"),
                (["add", "c.x", "rests_on=[heat.loss_kw]", "verdict=x", "wrong_if=heat.loss_kw > 1",
                  "--hypothesis", "glazing_redo"], "would be born broken"),
                (["add", "c.x", "rests_on=[heat.loss_kw]", "verdict=x", "wrong_if=heat.loss_kw > 90",
                  "seen={heat.loss_kw: 28}", "--hypothesis", "glazing_redo"], "seen is written by this tool"),
                (["set", "heat.nothing", "1", "--hypothesis", "glazing_redo"], "heat.nothing is not an entry"),
                (["review", "c.boiler_short", "--hypothesis", "glazing_redo"],
                 "c.boiler_short is not in hypothesis glazing_redo - review it in the base"),
                (["set", "heat.loss_kw", "1", "--hypothesis", "no/slash"], "--hypothesis takes a name"),
            )
            for args, why in cases:
                code, out, err = run(SCRIPTS / "provenance.py", *args, rec)
                self.assertEqual(code, 1, args)
                self.assertIn(why, out + err, args)
            self.assertEqual(h.read_text(encoding="utf-8"), before)
            self.assertFalse((pathlib.Path(d) / "PROVENANCE.d" / "no").exists())

    def test_review_in_a_hypothesis(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = rec.read_text(encoding="utf-8")
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "35", "--as-of", "2026-09-04",
                "--hypothesis", "bigger_boiler", rec)
            code, out, err = run(SCRIPTS / "provenance.py", "review", "c.boiler_enough", "--as-of",
                                 "2026-09-04", "--hypothesis", "bigger_boiler", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("review c.boiler_enough in hypothesis bigger_boiler: seen rewritten from what the "
                          "record holds under it (2026-09-04)\n  heat.loss_kw: 33 -> 35\n"
                          "  c.boiler_enough holds: wrong_if does not hold (heat.loss_kw > heat.boiler_kw)\n", out)
            h = (pathlib.Path(d) / "PROVENANCE.d" / "bigger_boiler.yaml").read_text(encoding="utf-8")
            self.assertIn("seen: {heat.boiler_kw: 36, heat.loss_kw: 35}", h)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)

    def test_a_superseding_verdict_replaces_the_standing_judgment(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # the standing judgment's wrong_if holds: the new verdict is its repair
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", "2026-09-04", rec)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 1)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short",
                                 "rests_on=[heat.boiler_kw, heat.loss_kw]",
                                 "verdict=the old boiler holds on the coldest night",
                                 "wrong_if=heat.loss_kw > heat.boiler_kw", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("supersede c.boiler_short: the old boiler cannot hold 12°C on the coldest February "
                          "nig… -> the old boiler holds on the coldest night - its wrong_if holds "
                          "(heat.loss_kw <= heat.boiler_kw)\n"
                          "the new judgment holds: wrong_if does not hold (heat.loss_kw > heat.boiler_kw)\n", out)
            text = rec.read_text(encoding="utf-8")
            self.assertIn("  c.boiler_short:\n    rests_on: [heat.boiler_kw, heat.loss_kw]\n"
                          '    verdict: "the old boiler holds on the coldest night"\n'
                          '    wrong_if: "heat.loss_kw > heat.boiler_kw"\n'
                          "    seen: {heat.boiler_kw: 24, heat.loss_kw: 20}\n", text)
            self.assertEqual(text.count("c.boiler_short:"), 1)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
    def test_a_request_a_session_wrote_itself_admits_nothing_and_the_fold_does(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # the source carrying what was asked is written by the session, and every
            # session's first write is one, so naming it is a claim and never a key
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_04_ask",
                "asked=Say the boiler is short by 7 kW, not that it cannot hold - the number is what I need.",
                "name=the ask", "read=2026-09-04", "--as-of", "2026-09-04", rec)
            before = rec.read_text(encoding="utf-8")
            args = ["add", "c.boiler_short",
                    "rests_on=[heat.boiler_kw, heat.loss_kw, heat.deficit_kw, s.2026_09_04_ask]",
                    "verdict=the old boiler is short by {{heat.deficit_kw}} kW on the coldest night",
                    "wrong_if=heat.loss_kw <= heat.boiler_kw", "--as-of", "2026-09-04"]
            code, out, err = run(SCRIPTS / "provenance.py", *args, rec)
            self.assertEqual(code, 1)
            self.assertIn("the standing judgment holds, and its wrong_if has not fired", out + err)
            code, out, err = run(SCRIPTS / "provenance.py", *args[:5], "request=s.2026_09_02_heating",
                                 *args[5:], rec)
            self.assertEqual(code, 1)
            self.assertIn("request: s.2026_09_02_heating is not a session source carrying what was asked, "
                          "that the judgment also rests on", out + err)
            code, out, err = run(SCRIPTS / "provenance.py", *args[:5], "request=s.2026_09_04_ask",
                                 *args[5:], rec)
            self.assertEqual(code, 1)
            self.assertIn("the standing judgment holds, and its wrong_if has not fired, so a different "
                          "verdict under the same id contradicts it, and a hypothesis holds the other: "
                          "add c.boiler_short", out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            # the same rewrite, beside the record: it waits there, word and all, until a
            # person folds it - and the fold is what lays it over the standing judgment
            command = re.search(r"add c\.boiler_short .*--hypothesis \S+", out + err).group(0)
            name = shlex.split(command)[-1]
            code, out, err = run(SCRIPTS / "provenance.py", *shlex.split(command), rec)
            self.assertEqual(code, 0, out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            self.assertIn("    request: s.2026_09_04_ask\n",
                          (pathlib.Path(d) / "PROVENANCE.d" / f"{name}.yaml").read_text(encoding="utf-8"))
            code, out, err = run(SCRIPTS / "consolidate.py", name, "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("    the standing judgment holds, and a person folds this over it\n", out)
            text = rec.read_text(encoding="utf-8")
            self.assertIn('    verdict: "the old boiler is short by {{heat.deficit_kw}} kW on the coldest '
                          'night"\n', text)
            self.assertIn("    request: s.2026_09_04_ask\n", text)
            self.assertEqual(text.count("c.boiler_short:"), 1)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_the_gate_counts_a_hypothesis_write(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            state = pathlib.Path(d) / "state"
            self.assertEqual(run(SCRIPTS / "provenance.py", "mark", state, rec)[0], 0)
            self.assertEqual(run(SCRIPTS / "provenance.py", "gate", state, rec)[0], 0)
            run(SCRIPTS / "provenance.py", "add", "glaze.saving_kw", "v=4", "unit=kW", "name=loss stopped",
                "--as-of", "2026-09-04", "--hypothesis", "glazing_redo", rec)
            code, out, _ = run(SCRIPTS / "provenance.py", "gate", state, rec)
            self.assertEqual(code, 2, out)
            self.assertIn("this session wrote 1 entry (glaze.saving_kw) and recorded no intent", out)
            # attributed to the hypothesis's own session source, the gate has nothing to say
            run(SCRIPTS / "provenance.py", "add", "glaze.cost_eur", "v=2400", "name=the quote",
                "from=s.2026_09_03_recount", "--as-of", "2026-09-04", "--hypothesis", "glazing_redo", rec)
            self.assertEqual(run(SCRIPTS / "provenance.py", "gate", state, rec)[0], 0)


class ThePageDrawsTheBase(unittest.TestCase):
    def test_the_page_carries_the_count_and_not_the_proposals(self):
        code, page, err = run(SCRIPTS / "render_page.py", RECORD)
        self.assertEqual(code, 0, err)
        dom = dom_of(page)
        self.assertIn('<h1 dir="auto">Greenhouse</h1><p class="meta" dir="ltr">2 hypotheses wait beside this '
                      'record, 1 contested. The page draws the base.</p>', dom)
        self.assertNotIn('<td class="v" dir="auto">36</td>', dom)
        self.assertNotIn('data-id="heat.boiler_kw">36<', dom)
        self.assertNotIn("c.boiler_enough", page)
        self.assertNotIn("bigger_boiler", page)
        self.assertIn('data-id="heat.loss_kw">31</span>', dom)
        _, again, _ = run(SCRIPTS / "render_page.py", RECORD)
        self.assertEqual(page, again)

    def test_verify_is_clean(self):
        code, out, err = run(SCRIPTS / "render_page.py", "--verify", RECORD)
        self.assertEqual(code, 0, out + err)
        self.assertNotIn("FAIL", out)
        self.assertIn("8 elements, 7 entries, 1 judgments, 2 tabs, 0 problems", out)

    def test_the_help_names_the_flag(self):
        for cmd in ("set", "add", "review"):
            code, out, _ = run(SCRIPTS / "provenance.py", cmd, "--help")
            self.assertEqual(code, 0)
            self.assertIn("--hypothesis NAME", out)
        code, out, _ = run(SCRIPTS / "kpopper")
        self.assertIn("--hypothesis NAME", out)


class TheEdgesHold(unittest.TestCase):
    """What a review of the fork found at its edges, and what now holds there."""

    def test_a_judgment_is_replaced_only_by_a_judgment(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "set", "heat.loss_kw", "20", "--as-of", "2026-09-04", rec)
            before = rec.read_text(encoding="utf-8")
            for extra in ([], ["--hypothesis", "bigger_boiler"]):
                code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short",
                                     "verdict=the old boiler holds", "wrong_if=heat.loss_kw > 90", *extra, rec)
                self.assertEqual(code, 1, extra)
                self.assertIn("c.boiler_short is a judgment - what replaces it rests on something: give "
                              "rests_on=[...] with the new verdict, or review it", out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), before)
            self.assertIn("c.boiler_short:\n    rests_on:", before)
            doc = P.load([str(rec)])
            ids, jud, fields = P.infer(doc)
            raw = P.with_builtins(doc, ids, jud, fields)
            self.assertEqual(P.may_supersede("c.boiler_short", jud["c.boiler_short"]["body"],
                                             {"verdict": "x"}, raw, ids, jud, fields),
                             (False, "what replaces a judgment must rest on something, and this carries no rests_on"))

    def test_a_hypothesis_the_base_cannot_read_by_shape_fails_check(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # a second field with a dependency list's shape: over the base the reader cannot
            # tell which is the role, and it does not guess
            (pathlib.Path(d) / "PROVENANCE.d" / "odd.yaml").write_text(
                "judgments:\n  c.odd:\n    depends: [heat.loss_kw]\n    verdict: odd\n"
                "    wrong_if: \"heat.loss_kw > 90\"\n    seen: {heat.loss_kw: 31}\n", encoding="utf-8")
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 1, out)
            self.assertIn("FAIL hypothesis odd cannot be read over the base: two fields fit 'deps'", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "open", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("3 hypotheses wait - odd (unreadable over the base: two fields fit 'deps'", out)
            code, out, err = run(SCRIPTS / "provenance.py", "pull", "heat", rec)
            self.assertEqual(code, 0, out + err)
            self.assertTrue(out.startswith("! hypothesis odd cannot be read over the base: two fields fit 'deps'"), out)
            self.assertIn("proposes 31 -> 28, from glazing_redo", out)

    def test_affects_walks_every_world(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.boiler_short",
                                 "rests_on=[heat.boiler_kw, heat.loss_kw, when.first_cold_night]",
                                 "verdict=the old boiler cannot hold 12°C on the first cold night",
                                 "wrong_if=heat.loss_kw > 90", "--as-of", "2026-09-04",
                                 "--hypothesis", "c_boiler_short", rec)
            self.assertEqual(code, 0, out + err)
            # a dependency only the hypothesis's variant of the judgment rests on
            code, out, _ = run(SCRIPTS / "provenance.py", "affects", "when.first_cold_night", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("c.boiler_short (in hypothesis c_boiler_short)\n    via when.first_cold_night", out)
            self.assertIn("1 judgments reached", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "affects", "heat.loss_kw", rec)
            self.assertIn("c.boiler_short\n    via heat.loss_kw", out)
            self.assertIn("c.boiler_enough (in hypothesis bigger_boiler)", out)
            self.assertIn("c.boiler_short (in hypothesis c_boiler_short)", out)
            self.assertIn("3 judgments reached", out)

    def test_a_hypothesis_judgment_may_rest_on_a_page_count(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.served", "rests_on=[page.unserved]",
                                 "verdict=every intent is served", "wrong_if=page.unserved > 0",
                                 "--as-of", "2026-09-04", "--hypothesis", "glazing_redo", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("seen: {page.unserved: 0}",
                          (pathlib.Path(d) / "PROVENANCE.d" / "glazing_redo.yaml").read_text(encoding="utf-8"))

    def test_pull_shows_a_source_read_again(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            (pathlib.Path(d) / "PROVENANCE.d" / "resheet.yaml").write_text(
                "sources:\n  doc.boiler_sheet:\n    name: \"the boiler's service sheet\"\n"
                "    file: \"boiler/service-2026.pdf\"\n    read: \"2026-09-04\"\n", encoding="utf-8")
            code, out, _ = run(SCRIPTS / "provenance.py", "pull", "doc.boiler_sheet", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("doc.boiler_sheet:  (the boiler's service sheet) as of 2026-09-02\n"
                          "    proposes instead, from resheet: (the boiler's service sheet) as of 2026-09-04\n", out)

    def test_a_title_backed_conclusion_is_a_claim(self):
        with tempfile.TemporaryDirectory() as d:
            rec = pathlib.Path(d) / "PROVENANCE.yaml"
            rec.write_text("meta:\n  updated: 2026-09-01\nknown:\n  x.one: {v: 1, name: one, of: \"2026-09-01\"}\n"
                           "judgments:\n  c.one:\n    rests_on: [x.one]\n    title: \"one is small\"\n"
                           "    wrong_if: \"x.one > 5\"\n    seen: {x.one: 1}\n", encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.one", "rests_on=[x.one]",
                                 "title=one is large", "wrong_if=x.one > 9", "--as-of", "2026-09-02", rec)
            self.assertEqual(code, 1)
            self.assertIn("c.one is already a judgment, concluding 'one is small'", out + err)
            self.assertIn("--hypothesis c_one", out + err)
            for name, title, seen in (("h1", "one is small", "1"), ("h2", "one is small", "2")):
                (pathlib.Path(d) / "PROVENANCE.d").mkdir(exist_ok=True)
                (pathlib.Path(d) / "PROVENANCE.d" / f"{name}.yaml").write_text(
                    f"judgments:\n  c.one:\n    rests_on: [x.one]\n    title: \"{title}\"\n"
                    f"    wrong_if: \"x.one > 5\"\n    seen: {{x.one: {seen}}}\n", encoding="utf-8")
            self.assertEqual(P.contested(P.load([str(rec)])), {})
            (pathlib.Path(d) / "PROVENANCE.d" / "h2.yaml").write_text(
                "judgments:\n  c.one:\n    rests_on: [x.one]\n    title: \"one is large\"\n"
                "    wrong_if: \"x.one > 5\"\n    seen: {x.one: 1}\n", encoding="utf-8")
            self.assertEqual(P.contested(P.load([str(rec)])),
                             {"c.one": [("h1", "one is small"), ("h2", "one is large")]})

    def test_an_occupied_name_is_not_reused(self):
        doc = P.load([str(RECORD)])
        taken = {"name": "a_b", "path": "", "head": {}, "doc": {}, "ids": {"a.b"},
                 "raw": {"a.b": {"v": 1}}, "error": None}
        doc.hypotheses = {"a_b": taken}
        self.assertEqual(P._hypothesis_name(doc, "a.b", 1), "a_b")        # the same claim: its own
        self.assertEqual(P._hypothesis_name(doc, "a.b", 2), "a_b_2")      # another claim on the id
        self.assertEqual(P._hypothesis_name(doc, "a_b", 5), "a_b_2")      # an unrelated id: not there


if __name__ == "__main__":
    unittest.main()
