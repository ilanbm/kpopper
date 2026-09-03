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
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "priors"
RECORD = FIXTURE / "PROVENANCE.yaml"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402
import render_page as R  # noqa: E402
import yaml  # noqa: E402

DECIDED = "c.order_is_the_contract"
PRIOR = "prior.macros_bind_by_position"
# the fixture's other prior-resting judgment: a local claim below the line, tried rather
# than decided, so the count has one judgment on each side of it
TRIED = "c.monthly_export_regenerated"
LOW = "prior.regeneration_is_deterministic"
OWN = ROOT / "PROVENANCE.yaml"                      # this project's own record
NO_PRIORS = ROOT / "tests" / "fixtures" / "hypotheses" / "PROVENANCE.yaml"
COUNT = "{} judgments rest on prior.* claims, {} of them on a prior at 0.8 or above"
RATE = "graph.prior_reversal_rate"
DEMOTION = "d.prior_confidence_survives_refutation"
REVERSALS = FIXTURE / "reversals"


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


class TheReaderCountsReversalShare(unittest.TestCase):
    def copy_reversals(self, into, findings=True):
        shutil.copytree(REVERSALS, into, dirs_exist_ok=True)
        rec = into / "PROVENANCE.yaml"
        if not findings:
            doc = yaml.safe_load(rec.read_text(encoding="utf-8"))
            for col in P.collections_of(doc).values():
                for k in list(col):
                    if k.startswith("hyp."):
                        del col[k]
            rec.write_text(yaml.safe_dump(doc, sort_keys=False), encoding="utf-8")
        return rec

    def add_rule(self, rec):
        code, out, err = run(SCRIPTS / "kpopper", "add", DEMOTION,
                             f"rests_on=[{RATE}]",
                             "verdict=prior confidence remains useful until more than one in five "
                             "high-confidence judgments is refuted, then the kind is a note",
                             f"wrong_if={RATE} > 0.2", "--as-of", "2026-09-04", rec)
        self.assertEqual(code, 0, out + err)

    def refute(self, rec, name, target, claim=None):
        # The hypothesis really holds the judgment it claims; add takes its snapshot,
        # then the head names that claim for consolidation to preserve in the finding.
        _, _, jud, _, _ = read(rec)
        body = dict(jud[target]["body"])
        body.pop("seen", None)
        code, out, err = run(SCRIPTS / "kpopper", "add", target,
                             yaml.safe_dump(body, default_flow_style=True),
                             "--hypothesis", name, "--as-of", "2026-09-04", rec)
        self.assertEqual(code, 0, out + err)
        hyp = rec.parent / "PROVENANCE.d" / (name + ".yaml")
        doc = yaml.safe_load(hyp.read_text(encoding="utf-8"))
        doc["hypothesis"]["claim"] = target if claim is None else claim
        hyp.write_text(yaml.safe_dump(doc, sort_keys=False), encoding="utf-8")
        code, out, err = run(SCRIPTS / "kpopper", "consolidate", "--refute", name,
                             "the export check contradicted the claim", "--as-of", "2026-09-04", rec)
        self.assertEqual(code, 0, out + err)
        self.assertFalse(hyp.exists())
        return read(rec)[4]["hyp." + name]

    def test_two_real_refutations_of_five_fire_the_rule(self):
        with tempfile.TemporaryDirectory() as d:
            rec = self.copy_reversals(pathlib.Path(d), findings=False)
            self.add_rule(rec)
            _, ids, jud, fields, raw = read(rec)
            self.assertEqual(P.value_of(raw, ids, RATE), 0.0)
            self.assertEqual(jud[DEMOTION]["body"]["seen"][RATE], 0.0)
            self.assertIs(P.evaluate(jud[DEMOTION]["pred"], raw, ids), False)
            self.assertEqual(P.flags(ids, jud, fields, raw)[DEMOTION], set())
            self.assertEqual(run(SCRIPTS / "kpopper", "check", rec)[0], 0)
            finding = self.refute(rec, "column_order", DECIDED)
            self.assertEqual(finding, {
                "v": "refuted", "name": DECIDED, "from": "s.2026_09_03_export",
                "at": "the export check contradicted the claim", "of": "2026-09-04"})
            _, ids, jud, _, raw = read(rec)
            self.assertEqual(P.value_of(raw, ids, RATE), 0.2)
            self.assertIs(P.evaluate(jud[DEMOTION]["pred"], raw, ids), False)
            self.assertEqual(run(SCRIPTS / "kpopper", "check", rec)[0], 0)
            self.refute(rec, "stable_names", "c.names_are_stable")
            _, ids, jud, fields, raw = read(rec)
            self.assertEqual(P.value_of(raw, ids, RATE), 0.4)
            self.assertIs(P.evaluate(jud[DEMOTION]["pred"], raw, ids), True)
            self.assertIn("falsified", P.flags(ids, jud, fields, raw)[DEMOTION])
            code, out, err = run(SCRIPTS / "kpopper", "check", rec)
            self.assertEqual(code, 1, out + err)
            self.assertIn(f"FAIL {DEMOTION}: wrong_if holds ({RATE} > 0.2)", out)
            brief = rec.parent / "PROVENANCE.view.yaml"
            brief.write_text("sections:\n  - title: Prior policy\n    pick: " + DEMOTION
                             + "\n    as: alerts\n", encoding="utf-8")
            page, _, _, _, info = R.build([str(rec)], str(brief))
            self.assertIn("falsified", info["flags"][DEMOTION])
            self.assertIn("its own condition for being wrong now holds", page)
            self.assertIn(f'data-id="{DEMOTION}"', page)
            self.assertEqual(jud[DEMOTION]["body"]["seen"][RATE], 0.0)

    def test_committed_findings_read_as_a_share_but_only_when_named(self):
        doc, ids, jud, fields, raw = read(REVERSALS / "PROVENANCE.yaml")
        self.assertNotIn(RATE, ids)
        self.assertNotIn(RATE, raw)
        self.assertEqual(P.counts(doc, ids, jud, fields, raw)[RATE], 0.4)
        self.assertEqual(P.priors_line(ids, jud, raw), COUNT.format(6, 5))
        self.assertEqual(run(SCRIPTS / "kpopper", "check", REVERSALS / "PROVENANCE.yaml")[0], 0)

    def test_the_denominator_and_membership_come_from_priors_line(self):
        doc, ids, jud, fields, raw = read(REVERSALS / "PROVENANCE.yaml")
        with mock.patch.object(P, "priors_line", return_value=("counted", {DECIDED})) as line:
            self.assertEqual(P.counts(doc, ids, jud, fields, raw)[RATE], 1.0)
        line.assert_called_once_with(ids, jud, raw, with_high=True)

    def test_refutations_are_linked_explicitly_and_count_each_judgment_once(self):
        with tempfile.TemporaryDirectory() as d:
            rec = self.copy_reversals(pathlib.Path(d), findings=False)
            self.add_rule(rec)
            self.refute(rec, "first", DECIDED)
            self.refute(rec, "again", DECIDED,
                        "the claim {{" + DECIDED + "}} no longer holds")
            self.refute(rec, "low", TRIED)
            self.refute(rec, "measured", "c.fourteen_fit")
            self.refute(rec, "unlinked", DECIDED, "the column order is a contract")
            _, ids, _, _, raw = read(rec)
            self.assertEqual(P.value_of(raw, ids, RATE), 0.2)
            doc = P.load([str(rec)])
            doc["known"]["hyp.not_refuted"] = {"v": "pending", "name": "c.names_are_stable"}
            doc["known"]["other.refuted"] = {"v": "refuted", "name": "c.names_are_stable"}
            for i, name in enumerate(("c.names_are_stable_extra", "c.gone", "prior.macros_bind_by_position",
                                       "do not confuse c.names_are_stable with the claim", [DECIDED], None)):
                doc["known"]["hyp.ignored_" + str(i)] = {"v": "refuted", "name": name}
            ids, jud, fields = P.infer(doc)
            self.assertEqual(P.counts(doc, ids, jud, fields, P.bodies(doc))[RATE], 0.2)

    def test_the_shared_line_handles_boundary_multiple_and_nonnumeric_priors(self):
        doc, ids, jud, fields, raw = read(REVERSALS / "PROVENANCE.yaml")
        line, high = P.priors_line(ids, jud, raw, with_high=True)
        self.assertEqual(line, COUNT.format(6, 5))
        self.assertEqual(len(high), 5)
        self.assertIn("c.names_are_stable", high)  # exactly HIGH_CONFIDENCE
        self.assertIn("c.append_is_safe", high)  # two high priors, still one judgment
        for value in (0.5, "unknown", None, {}, [], float("nan")):
            with self.subTest(value=value):
                changed = dict(raw)
                for k in ids:
                    if k.startswith("prior."):
                        changed[k] = {"v": value}
                self.assertEqual(P.counts(doc, ids, jud, fields, changed)[RATE], 0.0)
                self.assertEqual(P.priors_line(ids, jud, changed), COUNT.format(6, 0))

    def test_no_priors_has_no_new_entry_or_changed_output(self):
        doc, ids, jud, fields, raw = read(NO_PRIORS)
        self.assertNotIn(RATE, ids)
        self.assertNotIn(RATE, raw)
        self.assertEqual(P.priors_line(ids, jud, raw, with_high=True), ("", set()))
        for cmd in ("check", "open"):
            code, out, err = run(SCRIPTS / "kpopper", cmd, NO_PRIORS)
            self.assertEqual(code, 0, out + err)
            self.assertNotIn(RATE, out)

    def test_the_record_keeps_the_old_rule_and_adds_an_evaluable_one(self):
        _, ids, jud, fields, raw = read(OWN)
        old = jud["d.prior_carries_its_own_falsifier"]["body"]
        self.assertIn("reopened_by", old)
        self.assertEqual(old["wrong_if"], "prior.confidence_needs_calibration < 0.8")
        rule = jud[DEMOTION]
        self.assertIn("d.prior_carries_its_own_falsifier", rule["deps"])
        self.assertEqual(rule["pred"], f"{RATE} > 0.2")
        self.assertEqual(rule["body"]["seen"][RATE], 0.0)
        self.assertEqual(P.value_of(raw, ids, RATE), 0.0)
        self.assertEqual(P.flags(ids, jud, fields, raw)[DEMOTION], set())
        self.assertFalse(P.is_arrangement(rule, raw))
        source = "s.2026_09_04_prior_reversal_share"
        self.assertEqual(rule["body"]["from"], source)
        self.assertIn(DEMOTION, R.recorded_by(source, ids, jud, raw))
        self.assertEqual(raw["p.prior_count_thresholds"]["v"], 1)


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
