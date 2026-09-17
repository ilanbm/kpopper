"""Sameness is judged, not guessed: the entries nearest a new one at add, the candidate pairs
a merger walks when hypotheses consolidate, and the two commands that record the answer -
same, a migration that rewrites every reference and leaves check green, and distinct, the
edge that retires a pair. Runs against tests/fixtures/sameness - a base with one reading of
the boiler's sheet under two ids, and beside it two hypotheses that bring near-duplicates of
their own - with no browser and no network:

    python3 -m unittest discover -s tests
"""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "sameness"
RECORD = FIXTURE / "PROVENANCE.yaml"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402
import sameness as S  # noqa: E402

LOCATION = 'same from and at (doc.boiler_sheet, "rated output, p. 3") - certain'
PAIRS = [f"heat.nameplate_kw (resheet) and heat.boiler_kw: {LOCATION}",
         f"heat.nameplate_kw (resheet) and heat.output_kw: {LOCATION}",
         "c.wind_short (wind) and c.margin_thin: rests on heat.loss_kw, heat.output_kw too - verdicts differ, "
         "a pair to judge",
         "heat.gap_kw (wind) and heat.shortfall_kw: same rule (heat.loss_kw - heat.output_kw)"]


def run(*args, cwd=None):
    """The scripts as a session runs them: a subprocess, its exit code and both streams."""
    p = subprocess.run([sys.executable] + [str(a) for a in args], cwd=cwd,
                       capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def copy_fixture(into):
    """A scratch copy of the fixture - the base, its brief and the hypotheses beside it."""
    shutil.copytree(FIXTURE, into, dirs_exist_ok=True)
    return into / "PROVENANCE.yaml"


def edit(path, old, new):
    text = path.read_text(encoding="utf-8")
    assert old in text, old
    path.write_text(text.replace(old, new), encoding="utf-8")


def texts_under(d):
    """Every yaml file of a record, by its path under the record's directory."""
    return {str(p.relative_to(d)): p.read_text(encoding="utf-8")
            for p in sorted(pathlib.Path(d).rglob("*.yaml"))}


class TheFixtureHolds(unittest.TestCase):
    def test_check_is_green_and_the_page_verifies(self):
        code, out, err = run(SCRIPTS / "provenance.py", "check", RECORD)
        self.assertEqual(code, 0, out + err)
        self.assertTrue(out.endswith("2 judgments, 12 entries, 0 problems\n"), out)
        code, out, err = run(SCRIPTS / "render_page.py", "--verify", RECORD)
        self.assertEqual(code, 0, out + err)
        self.assertIn("12 elements, 10 entries, 2 judgments, 2 tabs, 0 problems", out)


class TheNearestExisting(unittest.TestCase):
    """add names the entries nearest the new one, from declared fields alone - a note in the
    reply, never a refusal - and says nothing when nothing is near."""

    def test_a_duplicate_by_location_is_certain(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.nameplate_kw", "v=26", "unit=kW",
                                 "name=nameplate output of the boiler", "from=doc.boiler_sheet",
                                 "at=rated output, p. 3", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out + err)
            self.assertTrue(out.startswith(
                "nearest existing:\n"
                f"  heat.boiler_kw: {LOCATION}\n"
                f"  heat.output_kw: {LOCATION}\n"
                "  one subject: same <id> heat.nameplate_kw folds it in · two: distinct heat.nameplate_kw "
                "<id> \"why\" keeps them apart\n"
                "add heat.nameplate_kw into known, before heat.output_kw\n"), out)
            # a note, never a refusal: the entry is in, and the record is green
            self.assertIn("  heat.nameplate_kw:\n    v: 26\n", rec.read_text(encoding="utf-8"))
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_the_same_premises_are_a_duplicate_or_a_pair_to_judge(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "c.short_again",
                               "rests_on=[heat.boiler_kw, heat.loss_kw]",
                               "verdict=the old boiler cannot hold 12°C on the coldest February night",
                               "wrong_if=heat.loss_kw <= heat.boiler_kw", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("nearest existing:\n  c.boiler_short: rests on heat.boiler_kw, heat.loss_kw too, "
                          "with the same verdict - a duplicate\n", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "c.wind_counted",
                               "rests_on=[heat.output_kw, heat.loss_kw]",
                               "verdict=the wind widens the gap", "wrong_if=heat.output_kw > heat.loss_kw",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("nearest existing:\n  c.margin_thin: rests on heat.loss_kw, heat.output_kw too - "
                          "verdicts differ, a pair to judge\n", out)
            # a session source is no premise: every judgment rests on the session that wrote it
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "c.by_the_session",
                               "rests_on=[s.2026_09_03_resheet]", "verdict=the sheet was read twice",
                               "blocked_on=observational: nothing here counts readings",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertNotIn("nearest existing", out)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_the_same_rule_and_the_same_source(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "heat.gap_kw",
                               "rule=heat.loss_kw - heat.output_kw", "name=the gap", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("nearest existing:\n  heat.shortfall_kw: same rule (heat.loss_kw - heat.output_kw)\n", out)
            # the same source, no place: less certain, and names rank the two but decide nothing
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "heat.sheet_kw", "v=24",
                               "name=rated output of the boiler, once more", "from=doc.boiler_sheet",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("nearest existing:\n  heat.output_kw: same from (doc.boiler_sheet)\n"
                          "  heat.boiler_kw: same from (doc.boiler_sheet)\n", out)
            # a name alone, however alike, is no reason
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "heat.again_kw", "v=24",
                               "name=boiler output", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertNotIn("nearest existing", out)

    def test_silent_without_a_candidate_and_quiet_about_a_declared_pair(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            for args in (["note.frost", "v=1", "name=frost"],
                         ["s.2026_09_04_wind", "asked=how much does the wind add?", "name=the wind question",
                          "read=2026-09-04"]):
                code, out, _ = run(SCRIPTS / "provenance.py", "add", *args, "--as-of", "2026-09-04", rec)
                self.assertEqual(code, 0, out)
                self.assertNotIn("nearest", out)
            # the writer already says which one it is not: that pair is not named
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "heat.nameplate_kw", "v=26",
                               "name=nameplate output", "from=doc.boiler_sheet", "at=rated output, p. 3",
                               "distinct_from=heat.boiler_kw", "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn(f"nearest existing:\n  heat.output_kw: {LOCATION}\n  one subject", out)
            self.assertNotIn("heat.boiler_kw:", out)

    def test_a_hypothesis_write_hears_what_the_hypothesis_holds(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.plate_kw", "v=26", "unit=kW",
                                 "name=the plate", "from=doc.boiler_sheet", "at=rated output, p. 3",
                                 "--as-of", "2026-09-03", "--hypothesis", "resheet", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn(f"  heat.nameplate_kw: {LOCATION}\n", out)
            self.assertIn("add heat.plate_kw into known, after heat.nameplate_kw of hypothesis resheet", out)


class TheCandidatesAMergerWalks(unittest.TestCase):
    """What arrives with a hypothesis is held against the base and against what the other
    hypotheses bring, grouped by subject; a prefix the base does not hold is a new subject."""

    def test_the_dry_run_lists_pairs_by_subject_and_the_new_prefixes(self):
        lines, news = S.candidate_lines(P.load([str(RECORD)]))
        self.assertEqual(lines, PAIRS)
        self.assertEqual(news, ["glaze (resheet): glaze.quote_eur"])
        pairs, subjects = S.candidates(P.load([str(RECORD)]))
        self.assertEqual(sorted(pairs), ["c.wind_short", "heat.gap_kw", "heat.nameplate_kw"])
        self.assertEqual(pairs["heat.nameplate_kw"]["hypothesis"], "resheet")
        self.assertEqual([(c["id"], c["rank"], c["hypothesis"]) for c in pairs["heat.nameplate_kw"]["near"]],
                         [("heat.boiler_kw", S.LOCATION, None), ("heat.output_kw", S.LOCATION, None)])
        self.assertEqual(subjects, {"glaze": [("resheet", "glaze.quote_eur")]})
        # the hypotheses to consolidate can be named
        pairs, subjects = S.candidates(P.load([str(RECORD)]), ["wind"])
        self.assertEqual(sorted(pairs), ["c.wind_short", "heat.gap_kw"])
        self.assertEqual(subjects, {})
        # or as the dicts the reader builds for them, the way consolidate holds them
        doc = P.load([str(RECORD)])
        pairs, _ = S.candidates(doc, [doc.hypotheses["wind"]])
        self.assertEqual(sorted(pairs), ["c.wind_short", "heat.gap_kw"])

    def test_the_dry_run_prints_the_candidates_and_the_new_subjects(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "consolidate.py", "--dry-run", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("candidates (4): pairs for a person to judge as the same subject or distinct\n"
                          + "".join(f"  {l}\n" for l in PAIRS)
                          + "new subjects (1): prefixes the base does not hold\n  glaze: glaze.quote_eur\n", out)
            # a distinct declared retires its pair from the next dry run
            run(SCRIPTS / "provenance.py", "distinct", "c.wind_short", "c.margin_thin", "the wind is a premise",
                "--as-of", "2026-09-04", rec)
            code, out, err = run(SCRIPTS / "consolidate.py", "--dry-run", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("candidates (3): pairs", out)
            self.assertNotIn("c.wind_short", out.split("candidates (3)")[1])

    def test_a_pair_two_arrivals_make_is_listed_once(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            (pathlib.Path(d) / "PROVENANCE.d" / "third.yaml").write_text(
                'hypothesis: {born: "2026-09-03"}\n\nknown:\n  heat.plate_kw:\n    v: 27\n    unit: kW\n'
                '    name: "the plate"\n    from: doc.boiler_sheet\n    at: "rated output, p. 3"\n',
                encoding="utf-8")
            pairs, _ = S.candidates(P.load([str(rec)]))
            self.assertEqual([(c["id"], c["hypothesis"]) for c in pairs["heat.nameplate_kw"]["near"]],
                             [("heat.boiler_kw", None), ("heat.output_kw", None), ("heat.plate_kw", "third")])
            self.assertEqual([(c["id"], c["hypothesis"]) for c in pairs["heat.plate_kw"]["near"]],
                             [("heat.boiler_kw", None), ("heat.output_kw", None)])

    def test_a_rule_is_read_through_what_was_retired(self):
        self.assertEqual(S.canon_rule("heat.loss_kw  -  heat.output_kw", {"heat.output_kw": "heat.boiler_kw"}),
                         P.E.convert("heat.loss_kw - heat.boiler_kw"))
        self.assertEqual(S.retired_into([{"a.x": {"v": 1, "also": ["a.y", "not an id", "a.x"]}, "b.z": {"v": 2}}],
                                        {"a.x", "b.z"}), {"a.y": "a.x"})
        self.assertEqual(S.distinct_pairs([{"a.x": {"distinct_from": "b.z, c.w"}}]),
                         {frozenset(("a.x", "b.z")), frozenset(("a.x", "c.w"))})
        self.assertAlmostEqual(S.overlap("boiler output", "rated output of the boiler"), 2 / 3)
        self.assertEqual(S.overlap("boiler output", ""), 0.0)


class SameIsAMigration(unittest.TestCase):
    """One subject under two ids: every reference rewritten across the base, the hypotheses
    and the brief, the bodies merged, check green - or nothing changed."""

    def test_same_rewrites_every_reference_and_leaves_check_green(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertEqual(out,
                "same heat.boiler_kw heat.output_kw: heat.output_kw retired into heat.boiler_kw\n"
                "  heat.boiler_kw takes heat.output_kw's reading: 24 -> 25 - a reading from 2026-09-03 that is "
                "newer than the base's\n"
                "  rewritten - rests_on: c.margin_thin, c.wind_short (in hypothesis wind) · seen: c.margin_thin, "
                "c.wind_short (in hypothesis wind) · wrong_if: c.margin_thin, c.wind_short (in hypothesis wind) · "
                "rule: heat.shortfall_kw, heat.gap_kw (in hypothesis wind) · text: c.margin_thin (because) · "
                "the brief: section 'What the numbers say' (text, seen, pick), groups and labels\n"
                "  5 mentions in PROVENANCE.yaml · 4 mentions in PROVENANCE.d/wind.yaml · 3 mentions in "
                "PROVENANCE.view.yaml\n"
                "  check: 2 judgments, 11 entries, 0 problems\n"
                "worked out from it: heat.deficit_kw, heat.shortfall_kw\n"
                "rests on it:\n"
                "  MUTED     c.boiler_short: heat.boiler_kw moved 24 -> 25, inside wrong_if "
                "(heat.loss_kw <= heat.boiler_kw) - nothing is asked\n"
                "  HOLDS     c.margin_thin: wrong_if does not hold (heat.boiler_kw > heat.loss_kw)\n"
                "text that saw it:\n"
                "  'What the numbers say' saw heat.boiler_kw = 24 - read it again, then: review "
                "\"What the numbers say\"\n"
                "\nthe record needs a person on 0 judgments - check says the rest\n")
            got = texts_under(d)
            # the retired id survives only in the survivor's also:
            for name, text in got.items():
                self.assertEqual(text.count("heat.output_kw"), 1 if name == "PROVENANCE.yaml" else 0, name)
            base = got["PROVENANCE.yaml"]
            self.assertIn('  updated: 2026-09-04\n', base)
            # the survivor: the newer reading, its date, the kept name, the retired id beside it
            self.assertIn('  heat.boiler_kw:\n    v: 25\n    unit: kW\n    name: "boiler output"\n'
                          '    from: doc.boiler_sheet\n    at: "rated output, p. 3"\n    of: "2026-09-03"\n'
                          '    also: [heat.output_kw]\n  heat.loss_kw:\n', base)
            # a rule, a dependency list, a predicate, a reference in text and a seen key, in the base
            self.assertIn('  heat.shortfall_kw:\n    rule: "heat.loss_kw - heat.boiler_kw"\n', base)
            self.assertIn('    rests_on: [heat.boiler_kw, heat.loss_kw, s.2026_09_03_resheet]\n', base)
            self.assertIn('    because: "{{heat.boiler_kw}} kW rated against {{heat.loss_kw}} kW lost', base)
            self.assertIn('    wrong_if: "heat.boiler_kw > heat.loss_kw"\n'
                          '    seen: {heat.boiler_kw: 25, heat.loss_kw: 31, s.2026_09_03_resheet: "read 2026-09-03"}\n',
                          base)
            # the judgment that saw the older value keeps its snapshot - that is what flags it
            self.assertIn("    seen: {heat.boiler_kw: 24, heat.loss_kw: 31,\n", base)
            # the hypothesis beside the record
            self.assertEqual(got["PROVENANCE.d/wind.yaml"],
                             'hypothesis:\n  claim: "the wind adds 2 kW to the loss on the coldest night"\n'
                             '  born: "2026-09-03"\n\nknown:\n  heat.gap_kw:\n'
                             '    rule: "heat.loss_kw - heat.boiler_kw"\n'
                             '    name: "the gap on the coldest night, wind not yet counted"\n\njudgments:\n'
                             '  c.wind_short:\n    rests_on: [heat.boiler_kw, heat.loss_kw]\n'
                             '    verdict: "with the wind counted the boiler is short by more than the sheet says"\n'
                             '    wrong_if: "heat.boiler_kw > heat.loss_kw"\n'
                             '    seen: {heat.boiler_kw: 25, heat.loss_kw: 31}\n')
            self.assertEqual(got["PROVENANCE.d/resheet.yaml"], (FIXTURE / "PROVENANCE.d" / "resheet.yaml")
                             .read_text(encoding="utf-8"))
            # the brief: the text's reference, its seen without the retired key, the pick, the label
            brief = got["PROVENANCE.view.yaml"]
            self.assertIn('text: "The sheet says {{heat.boiler_kw}} kW, read again as {{heat.boiler_kw}}; the coldest\n'
                          '               night takes {{heat.loss_kw}}. {{c.boiler_short}}"\n', brief)
            self.assertIn("        seen:\n          heat.boiler_kw: 24\n          heat.loss_kw: 31\n"
                          "          c.boiler_short:", brief)
            self.assertIn("        pick: [heat.boiler_kw, heat.loss_kw]\n", brief)
            self.assertIn('labels:\n  heat.boiler_kw: "rated output, second reading"\n', brief)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertTrue(out.endswith("2 judgments, 11 entries, 0 problems\n"), out)
            code, out, _ = run(SCRIPTS / "render_page.py", "--verify", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("NOTE section 'What the numbers say': its text saw heat.boiler_kw = 24, now 25", out)
            self.assertIn("11 elements, 9 entries, 2 judgments, 2 tabs, 0 problems", out)
            # the migration reveals the next pair: the rule the base now holds twice, the premises
            # the hypothesis's judgment now shares with two judgments
            lines, _ = S.candidate_lines(P.load([str(rec)]))
            self.assertEqual(lines, [
                f"heat.nameplate_kw (resheet) and heat.boiler_kw: {LOCATION}",
                "c.wind_short (wind) and c.boiler_short: rests on heat.boiler_kw, heat.loss_kw too - verdicts "
                "differ, a pair to judge",
                "c.wind_short (wind) and c.margin_thin: rests on heat.boiler_kw, heat.loss_kw too - verdicts "
                "differ, a pair to judge",
                "heat.gap_kw (wind) and heat.deficit_kw: same rule (heat.loss_kw - heat.boiler_kw)",
                "heat.gap_kw (wind) and heat.shortfall_kw: same rule (heat.loss_kw - heat.boiler_kw)"])

    def test_a_retired_id_points_at_its_survivor(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            self.assertEqual(run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw",
                                 "--as-of", "2026-09-04", rec)[0], 0)
            before = texts_under(d)
            pointed = ("heat.output_kw was retired into heat.boiler_kw - write heat.boiler_kw instead; it "
                       "carries also: [heat.output_kw]")
            for args in (["add", "heat.output_kw", "v=1", "name=x"], ["set", "heat.output_kw", "3"],
                         ["review", "heat.output_kw"],
                         ["add", "heat.output_kw", "v=1", "name=x", "--hypothesis", "wind"]):
                code, out, err = run(SCRIPTS / "provenance.py", *args, rec)
                self.assertEqual(code, 1, args)
                self.assertIn(pointed, out + err, args)
            self.assertEqual(texts_under(d), before)
            # and so does same, and distinct
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.loss_kw", "heat.output_kw", rec)
            self.assertEqual(code, 1)
            self.assertIn("heat.output_kw is not an entry of the record or of any hypothesis beside it - it was "
                          "retired into heat.boiler_kw", out + err)

    def test_the_migration_reaches_an_id_only_a_hypothesis_holds(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.nameplate_kw",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertTrue(out.startswith(
                "same heat.boiler_kw heat.nameplate_kw: heat.nameplate_kw retired into heat.boiler_kw\n"
                "  heat.boiler_kw takes heat.nameplate_kw's reading: 24 -> 26 - a reading from 2026-09-03 that "
                "is newer than the base's (in hypothesis resheet)\n"
                "  check: 2 judgments, 12 entries, 0 problems\n"), out)
            # the hypothesis now proposes the survivor, with the merged body; the base's copy is
            # untouched but for the retired id beside it
            h = (pathlib.Path(d) / "PROVENANCE.d" / "resheet.yaml").read_text(encoding="utf-8")
            self.assertNotIn("heat.nameplate_kw:", h)
            self.assertIn('  heat.boiler_kw:\n    v: 26\n    unit: kW\n    name: "boiler output"\n'
                          '    from: doc.boiler_sheet\n    at: "rated output, p. 3"\n    of: "2026-09-03"\n'
                          '    also: [heat.nameplate_kw]\n', h)
            base = rec.read_text(encoding="utf-8")
            self.assertIn('  heat.boiler_kw:\n    v: 24\n    unit: kW\n    name: "boiler output"\n'
                          '    from: doc.boiler_sheet\n    at: "rated output, p. 3"\n    also: [heat.nameplate_kw]\n',
                          base)
            code, out, _ = run(SCRIPTS / "provenance.py", "pull", "heat.boiler_kw", rec)
            self.assertIn("heat.boiler_kw: 24 (boiler output) <- doc.boiler_sheet, at rated output, p. 3\n"
                          "    proposes 24 -> 26, from resheet\n", out)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
        with tempfile.TemporaryDirectory() as d:
            # --keep: the base's id retires into the hypothesis's, everywhere
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.nameplate_kw",
                                 "--keep", "b", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertTrue(out.startswith(
                "same heat.boiler_kw heat.nameplate_kw: heat.boiler_kw retired into heat.nameplate_kw "
                "(kept heat.nameplate_kw)\n"
                "  rewritten - rests_on: c.boiler_short · seen: c.boiler_short · wrong_if: c.boiler_short · "
                "rule: heat.deficit_kw · text: c.boiler_short (because) · the brief: section 'What the numbers "
                "say' (text, seen, pick)\n"), out)
            got = texts_under(d)
            for name, text in got.items():
                self.assertEqual(text.count("heat.boiler_kw"),
                                 1 if name in ("PROVENANCE.yaml", "PROVENANCE.d/resheet.yaml") else 0, name)
            self.assertIn('  heat.nameplate_kw:\n    v: 24\n    unit: kW\n    name: "boiler output"\n'
                          '    from: doc.boiler_sheet\n    at: "rated output, p. 3"\n    also: [heat.boiler_kw]\n'
                          '  heat.output_kw:\n', got["PROVENANCE.yaml"])
            self.assertIn('    wrong_if: "heat.loss_kw <= heat.nameplate_kw"\n', got["PROVENANCE.yaml"])
            self.assertIn("    also: [heat.boiler_kw]\n", got["PROVENANCE.d/resheet.yaml"])
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_two_readings_that_disagree_are_refused_and_nothing_moves(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, '    at: "rated output, p. 3"\n    of: "2026-09-03"\n', '    at: "rated output, p. 3"\n    of: "2026-09-02"\n')
            before = texts_under(d)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw", rec)
            self.assertEqual(code, 1)
            self.assertEqual((out + err).strip(),
                             "refused - heat.boiler_kw holds 24 as of 2026-09-02 and heat.output_kw holds 25 as of "
                             "2026-09-02 - a reading of the same day: two readings that disagree are a "
                             "contradiction, not one subject twice; set the one that is right, or open a "
                             "hypothesis, then same")
            self.assertEqual(texts_under(d), before)
            # undated on both sides: nothing orders them
            edit(rec, '    of: "2026-09-02"\n  heat.loss_kw:', '  heat.loss_kw:')
            edit(rec, '    file: "boiler/service-2025.pdf"\n    read: "2026-09-02"\n', '    file: "boiler/service-2025.pdf"\n')
            before = texts_under(d)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw", rec)
            self.assertEqual(code, 1)
            self.assertIn("nothing dates either reading", out + err)
            self.assertEqual(texts_under(d), before)
            # the older reading loses, whichever id is kept: the sheet read again on the 3rd dates
            # heat.boiler_kw through its source, and heat.output_kw's own day is the 2nd
            edit(rec, '    file: "boiler/service-2025.pdf"\n', '    file: "boiler/service-2025.pdf"\n    read: "2026-09-03"\n')
            edit(rec, '    at: "rated output, p. 3"\n  heat.loss_kw:', '    at: "rated output, p. 3"\n    of: "2026-09-02"\n  heat.loss_kw:')
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.output_kw", "heat.boiler_kw",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("  heat.output_kw takes heat.boiler_kw's reading: 25 -> 24 - a reading from 2026-09-03 "
                          "that is newer than the base's\n", out)

    def test_a_migration_that_would_fail_check_is_undone(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, "  heat.output_kw:\n    v: 25\n", "  heat.output_kw:\n    v: 40\n")
            before = texts_under(d)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 1)
            self.assertEqual((out + err).strip(),
                             "refused - check would fail after the migration, so nothing was changed:\n"
                             "  c.boiler_short: wrong_if holds (heat.loss_kw <= heat.boiler_kw) - broken by its "
                             "own condition")
            self.assertEqual(texts_under(d), before)

    def test_judgments_merge_only_through_the_one_door(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = texts_under(d)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "c.boiler_short", "c.margin_thin", rec)
            self.assertEqual(code, 1)
            self.assertIn("refused - c.boiler_short concludes 'the old boiler cannot hold 12°C on the coldest "
                          "February nig…' and c.margin_thin 'even at the rated output the boiler is short by a "
                          "fifth of …' - the standing judgment holds, and its wrong_if has not fired: two "
                          "verdicts on one subject are a contradiction", out + err)
            self.assertEqual(texts_under(d), before)
            # the same verdict: one subject, the survivor's body kept
            run(SCRIPTS / "provenance.py", "add", "c.short_again", "rests_on=[heat.boiler_kw, heat.loss_kw]",
                "verdict=the old boiler cannot hold 12°C on the coldest February night",
                "wrong_if=heat.loss_kw <= heat.boiler_kw", "--as-of", "2026-09-03", rec)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "c.boiler_short", "c.short_again",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            base = rec.read_text(encoding="utf-8")
            self.assertNotIn("c.short_again:", base)
            self.assertIn("    wrong_if: \"heat.loss_kw <= heat.boiler_kw\"\n    seen: {heat.boiler_kw: 24, "
                          "heat.loss_kw: 31,\n           heat.deficit_kw: \"heat.loss_kw - heat.boiler_kw\"}\n"
                          "    also: [c.short_again]\n", base)
            # naming whose asking the other was taken from does not open the door: the
            # session wrote that source itself, as the method asks every session to
            run(SCRIPTS / "provenance.py", "add", "s.2026_09_04_ask", "asked=Say it as a margin, not a verdict.",
                "name=the ask", "read=2026-09-04", "--as-of", "2026-09-04", rec)
            code, out, err = run(SCRIPTS / "provenance.py", "add", "c.as_a_margin",
                                 "rests_on=[heat.boiler_kw, heat.loss_kw, s.2026_09_04_ask]",
                                 "request=s.2026_09_04_ask",
                                 "verdict=the boiler is short of the coldest night, put as a margin",
                                 "wrong_if=heat.loss_kw > 90", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            base = rec.read_text(encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "same", "c.boiler_short", "c.as_a_margin",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 1)
            self.assertIn("the standing judgment holds, and its wrong_if has not fired: two verdicts on one "
                          "subject are a contradiction, not one subject twice; keep the one that holds, or "
                          "write the other through add --hypothesis, for a person to fold", out + err)
            self.assertEqual(rec.read_text(encoding="utf-8"), base)
            # the standing one broken by a newer reading, and the same merge goes through
            run(SCRIPTS / "provenance.py", "set", "heat.boiler_kw", "40", "--as-of", "2026-09-04", rec)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "c.boiler_short", "c.as_a_margin",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("  c.boiler_short takes c.as_a_margin's verdict - its wrong_if holds "
                          "(heat.loss_kw <= heat.boiler_kw)\n", out)
            base = rec.read_text(encoding="utf-8")
            self.assertIn("  c.boiler_short:\n    rests_on: [heat.boiler_kw, heat.loss_kw, s.2026_09_04_ask]\n"
                          '    verdict: "the boiler is short of the coldest night, put as a margin"\n', base)
            self.assertIn("    also: [c.short_again, c.as_a_margin]\n", base)
            self.assertIn("    request: s.2026_09_04_ask\n", base)
            self.assertNotIn("because:", base.split("c.boiler_short:")[1].split("c.margin_thin:")[0])
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_what_same_refuses_before_touching_anything(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = texts_under(d)
            cases = (
                (["heat.boiler_kw", "heat.boiler_kw"], "heat.boiler_kw and heat.boiler_kw are one id already"),
                (["c.boiler_short", "heat.loss_kw"],
                 "c.boiler_short is a judgment and heat.loss_kw an entry: one subject cannot be both"),
                (["heat.boiler_kw", "heat.nothing"],
                 "heat.nothing is not an entry of the record or of any hypothesis beside it"),
                (["heat.boiler_kw", "graph.entries"], "graph.entries is counted by the reader, never written"),
                (["heat.boiler_kw", "heat.output_kw", "--keep", "x"],
                 "--keep names the id that survives: heat.boiler_kw or heat.output_kw"),
                (["heat.boiler_kw", "heat.output_kw", "--as-of", "yesterday"], "--as-of takes a date"),
                (["heat.boiler_kw"], "same <a> <b> [--keep a|b]"),
            )
            for args, why in cases:
                code, out, err = run(SCRIPTS / "provenance.py", "same", *args, rec)
                self.assertEqual(code, 1, args)
                self.assertIn(why, out + err, args)
            self.assertEqual(texts_under(d), before)


class WhatTheReviewFound(unittest.TestCase):
    """The edges of the migration: a record in shards, a hypothesis naming both ids, a survivor
    written inline, a write that fails halfway, and the arguments' edges."""

    def test_a_sharded_record_keeps_one_copy_of_the_survivor(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            base = rec.read_text(encoding="utf-8")
            block = ('  heat.output_kw:\n    v: 25\n    unit: kW\n    name: "rated output of the boiler"\n'
                     '    from: doc.boiler_sheet\n    at: "rated output, p. 3"\n    of: "2026-09-03"\n')
            self.assertIn(block, base)
            (pathlib.Path(d) / "core.yaml").write_text(base.replace(block, ""), encoding="utf-8")
            (pathlib.Path(d) / "more.yaml").write_text("known:\n" + block, encoding="utf-8")
            rec.write_text("record: [core.yaml, more.yaml]\n", encoding="utf-8")
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("  5 mentions in core.yaml · 4 mentions in PROVENANCE.d/wind.yaml · 3 mentions in "
                          "PROVENANCE.view.yaml\n", out)
            got = texts_under(d)
            self.assertEqual(got["more.yaml"], "")           # the shard is left empty, not a null known:
            self.assertEqual(got["core.yaml"].count("  heat.boiler_kw:\n"), 1)
            self.assertIn("  updated: 2026-09-04\n", got["core.yaml"])
            self.assertIn("    also: [heat.output_kw]\n", got["core.yaml"])
            self.assertEqual(got["PROVENANCE.yaml"], "record: [core.yaml, more.yaml]\n")
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_a_hypothesis_naming_both_ids_rests_on_the_survivor_once(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            both = pathlib.Path(d) / "PROVENANCE.d" / "both.yaml"
            both.write_text('hypothesis: {born: "2026-09-03"}\n\njudgments:\n  c.two_readings:\n'
                            "    rests_on: [heat.boiler_kw, heat.output_kw]\n"
                            '    verdict: "the two readings of the sheet agree to a kilowatt"\n'
                            '    wrong_if: "heat.output_kw < heat.boiler_kw"\n'
                            "    seen: {heat.boiler_kw: 24, heat.output_kw: 25}\n", encoding="utf-8")
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("c.two_readings (in hypothesis both)", out)
            self.assertEqual(both.read_text(encoding="utf-8"),
                             'hypothesis: {born: "2026-09-03"}\n\njudgments:\n  c.two_readings:\n'
                             "    rests_on: [heat.boiler_kw]\n"
                             '    verdict: "the two readings of the sheet agree to a kilowatt"\n'
                             '    wrong_if: "heat.boiler_kw < heat.boiler_kw"\n'
                             "    seen: {heat.boiler_kw: 24}\n")
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_an_inline_survivor_keeps_its_comments_and_its_bare_also(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            edit(rec, '  heat.boiler_kw:\n    v: 24\n    unit: kW\n    name: "boiler output"\n'
                      '    from: doc.boiler_sheet\n    at: "rated output, p. 3"\n',
                 '  heat.boiler_kw: {v: 24, name: "boiler output", from: doc.boiler_sheet, also: heat.plate_kw}\n'
                 '    # set 2026-09-01: read off the plate\n')
            edit(rec, '    v: 25\n    unit: kW\n    name: "rated output of the boiler"\n    from: doc.boiler_sheet\n'
                      '    at: "rated output, p. 3"\n',
                 '    v: 25\n    name: "rated output of the boiler"\n    from: doc.boiler_sheet\n')
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            base = rec.read_text(encoding="utf-8")
            self.assertIn('  heat.boiler_kw: {v: 25, name: "boiler output", from: doc.boiler_sheet, of: "2026-09-03", '
                          'also: [heat.plate_kw, heat.output_kw]}\n'
                          "    # set 2026-09-01: read off the plate\n  heat.loss_kw:\n", base)
            # a survivor written in block style keeps its comments last, the retired id before them
            edit(rec, '    of: "2026-09-02"\n  heat.deficit_kw:', '    of: "2026-09-02"\n    # set 2026-09-02: the glazing area\n  heat.deficit_kw:')
            run(SCRIPTS / "provenance.py", "add", "heat.loss2_kw", "v=31", "unit=kW", "name=the loss again",
                "from=s.2026_09_02_heating", "--as-of", "2026-09-03", rec)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.loss_kw", "heat.loss2_kw",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn('    of: "2026-09-02"\n    also: [heat.loss2_kw]\n    # set 2026-09-02: the glazing area\n'
                          '  heat.deficit_kw:\n', rec.read_text(encoding="utf-8"))
            # both retired ids point at it now
            code, out, err = run(SCRIPTS / "provenance.py", "add", "heat.plate_kw", "v=1", rec)
            self.assertEqual(code, 1)
            self.assertIn("heat.plate_kw was retired into heat.boiler_kw", out + err)
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)

    def test_the_newer_reading_brings_its_own_provenance_or_none(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            # the retired reading is newer and names no place: the survivor's old place goes with
            # the old value, and only the unit stays
            edit(rec, '    name: "rated output of the boiler"\n    from: doc.boiler_sheet\n    at: "rated output, p. 3"\n'
                      '    of: "2026-09-03"\n',
                 '    name: "rated output of the boiler"\n    of: "2026-09-03"\n')
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn('  heat.boiler_kw:\n    v: 25\n    unit: kW\n    name: "boiler output"\n    of: "2026-09-03"\n'
                          '    also: [heat.output_kw]\n  heat.loss_kw:\n', rec.read_text(encoding="utf-8"))

    def test_a_list_that_repeats_something_else_keeps_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            brief = pathlib.Path(d) / "PROVENANCE.view.yaml"
            edit(brief, "        pick: [heat.boiler_kw, heat.output_kw, heat.loss_kw]\n",
                 "        pick: [heat.boiler_kw, heat.output_kw, heat.loss_kw, heat.loss_kw]\n")
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw",
                                 "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("        pick: [heat.boiler_kw, heat.loss_kw, heat.loss_kw]\n", brief.read_text(encoding="utf-8"))

    def test_a_write_that_fails_halfway_puts_everything_back(self):
        if os.geteuid() == 0:
            self.skipTest("permissions do not bind root")
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = texts_under(d)
            hyp_dir = pathlib.Path(d) / "PROVENANCE.d"
            os.chmod(hyp_dir, 0o555)
            try:
                code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw",
                                     "--as-of", "2026-09-04", rec)
            finally:
                os.chmod(hyp_dir, 0o755)
            self.assertEqual(code, 1, out + err)
            self.assertIn("recovery_required", out + err)
            self.assertEqual(texts_under(d), before)

    def test_open_questions_are_not_folded(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            run(SCRIPTS / "provenance.py", "add", "q.glazing", "is glazing the north wall cheaper?",
                "--as-of", "2026-09-03", rec)
            before = texts_under(d)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "q.second_boiler", "q.glazing", rec)
            self.assertEqual(code, 1)
            self.assertIn("q.second_boiler is a line, not an entry with fields - an open question is closed by "
                          "answering it, not folded into another", out + err)
            self.assertEqual(texts_under(d), before)

    def test_the_why_may_end_like_a_file_name(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "distinct", "heat.output_kw", "heat.boiler_kw",
                                 "the second reading is in boiler/notes.yaml", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("    # distinct 2026-09-04: the second reading is in boiler/notes.yaml\n",
                          rec.read_text(encoding="utf-8"))


class DistinctRetiresThePair(unittest.TestCase):
    def test_distinct_writes_the_edge_and_the_dry_run_forgets_the_pair(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "distinct", "c.wind_short", "c.margin_thin",
                                 "the wind is a premise the second reading never counted", "--as-of", "2026-09-04",
                                 rec)
            self.assertEqual(code, 0, out + err)
            self.assertEqual(out, "distinct c.wind_short from c.margin_thin: the wind is a premise the second "
                                  "reading never counted (in hypothesis wind)\n"
                                  "  the pair returns as no candidate; c.wind_short carries distinct_from: "
                                  "c.margin_thin\nnothing rests on it\n\n"
                                  "the record needs a person on 0 judgments - check says the rest\n")
            h = pathlib.Path(d) / "PROVENANCE.d" / "wind.yaml"
            self.assertTrue(h.read_text(encoding="utf-8").endswith(
                "    seen: {heat.output_kw: 25, heat.loss_kw: 31}\n    distinct_from: c.margin_thin\n"
                "    # distinct 2026-09-04: the wind is a premise the second reading never counted\n"))
            # the base is not touched: the edge is written where the id lives
            self.assertEqual(rec.read_text(encoding="utf-8"), RECORD.read_text(encoding="utf-8"))
            lines, _ = S.candidate_lines(P.load([str(rec)]))
            self.assertFalse([l for l in lines if l.startswith("c.wind_short (wind)")], lines)
            self.assertNotIn("c.margin_thin", "\n".join(lines))
            # declared once is declared; a second distinct on the same id joins the first
            before = h.read_text(encoding="utf-8")
            code, out, _ = run(SCRIPTS / "provenance.py", "distinct", "c.margin_thin", "c.wind_short", "again", rec)
            self.assertEqual(code, 0, out)
            self.assertEqual(out, "c.margin_thin and c.wind_short are already declared distinct; nothing written\n")
            self.assertEqual(h.read_text(encoding="utf-8"), before)
            code, out, _ = run(SCRIPTS / "provenance.py", "distinct", "c.wind_short", "c.boiler_short",
                               "the wind again", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out)
            self.assertIn('    distinct_from: "c.margin_thin, c.boiler_short"\n'
                          "    # distinct 2026-09-04: the wind is a premise the second reading never counted\n"
                          "    # distinct 2026-09-04: the wind again\n", h.read_text(encoding="utf-8"))
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
            self.assertEqual(S.distinct_pairs([P.load([str(rec)]).hypotheses["wind"]["raw"]]),
                             {frozenset(("c.wind_short", "c.margin_thin")), frozenset(("c.wind_short", "c.boiler_short"))})

    def test_distinct_in_the_base_and_what_reads_it(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            code, out, err = run(SCRIPTS / "provenance.py", "distinct", "heat.output_kw", "heat.boiler_kw",
                                 "the nameplate and the rated output are two numbers", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn("distinct heat.output_kw from heat.boiler_kw: the nameplate and the rated output are two "
                          "numbers\n  the pair returns as no candidate; heat.output_kw carries distinct_from: "
                          "heat.boiler_kw\nworked out from it: heat.shortfall_kw\nrests on it:\n"
                          "  HOLDS     c.margin_thin:", out)
            base = rec.read_text(encoding="utf-8")
            self.assertIn('    of: "2026-09-03"\n    distinct_from: heat.boiler_kw\n'
                          '    # distinct 2026-09-04: the nameplate and the rated output are two numbers\n'
                          '  heat.loss_kw:\n', base)
            self.assertIn("  updated: 2026-09-04\n", base)
            # read by same, by add, and by the reader as prose - never as a dependency or a predicate
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw", rec)
            self.assertEqual(code, 1)
            self.assertIn("heat.boiler_kw and heat.output_kw were declared distinct; remove the distinct_from that "
                          "says so before saying otherwise", out + err)
            code, out, _ = run(SCRIPTS / "provenance.py", "add", "heat.nameplate_kw", "v=26",
                               "name=nameplate output", "from=doc.boiler_sheet", "at=rated output, p. 3",
                               "--as-of", "2026-09-03", rec)
            self.assertEqual(code, 0, out)
            self.assertIn(f"  heat.boiler_kw: {LOCATION}\n  heat.output_kw: {LOCATION}\n", out)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertTrue(out.endswith("2 judgments, 13 entries, 0 problems\n"), out)
            self.assertEqual(run(SCRIPTS / "render_page.py", "--verify", rec)[0], 0)
            # a flow-written entry takes the field inside its braces
            edit(rec, '  when.first_cold_night:\n    v: "2027-02-01"\n    name: "first night that matters"\n'
                      '    from: s.2026_09_02_heating\n',
                 '  when.first_cold_night: {v: "2027-02-01", name: "first night that matters", '
                 'from: s.2026_09_02_heating}\n')
            code, out, err = run(SCRIPTS / "provenance.py", "distinct", "when.first_cold_night", "heat.loss_kw",
                                 "a date is not a loss", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn('  when.first_cold_night: {v: "2027-02-01", name: "first night that matters", '
                          'from: s.2026_09_02_heating, distinct_from: heat.loss_kw}\n'
                          '    # distinct 2026-09-04: a date is not a loss\n', rec.read_text(encoding="utf-8"))
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
            # a second id on a flow entry is quoted, or the comma would split the mapping
            code, out, err = run(SCRIPTS / "provenance.py", "distinct", "when.first_cold_night", "heat.boiler_kw",
                                 "a date is not an output either", "--as-of", "2026-09-04", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn('from: s.2026_09_02_heating, distinct_from: "heat.loss_kw, heat.boiler_kw"}\n'
                          '    # distinct 2026-09-04: a date is not a loss\n'
                          '    # distinct 2026-09-04: a date is not an output either\n', rec.read_text(encoding="utf-8"))
            self.assertEqual(run(SCRIPTS / "provenance.py", "check", rec)[0], 0)
            # an unreadable hypothesis stops a migration: it would not reach what the file holds
            (pathlib.Path(d) / "PROVENANCE.d" / "broken.yaml").write_text("hypothesis: [1, 2\n", encoding="utf-8")
            before = texts_under(d)
            code, out, err = run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.nameplate_kw", rec)
            self.assertEqual(code, 1)
            self.assertIn("hypothesis broken could not be read (while parsing", out + err)
            self.assertIn("so the migration could not reach what it holds", out + err)
            self.assertEqual(texts_under(d), before)

    def test_what_distinct_refuses(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            before = texts_under(d)
            cases = (
                (["heat.boiler_kw", "heat.boiler_kw", "why"], "one id: distinct needs two"),
                (["heat.boiler_kw", "heat.nothing", "why"], "heat.nothing is not an entry"),
                (["heat.boiler_kw", "heat.output_kw"], 'distinct <a> <b> "<why>"'),
                (["heat.boiler_kw", "heat.output_kw", "  "], "distinct takes the why"),
                (["q.second_boiler", "heat.output_kw", "why"], "q.second_boiler is a line, not an entry with fields"),
            )
            for args, why in cases:
                code, out, err = run(SCRIPTS / "provenance.py", "distinct", *args, rec)
                self.assertEqual(code, 1, args)
                self.assertIn(why, out + err, args)
            self.assertEqual(texts_under(d), before)


class AlsoIsReadByAbsence(unittest.TestCase):
    """`also:` names other identities of a subject: the reader never votes on it, reads it as a
    retirement only where the named id is absent everywhere, and says so where it is not."""

    def test_a_retirement_that_came_back_is_read_and_reported(self):
        with tempfile.TemporaryDirectory() as d:
            rec = pathlib.Path(d) / "PROVENANCE.yaml"
            # what a merge produces: one branch retired x.two into x.one, the other still holds it
            rec.write_text('meta:\n  updated: 2026-09-04\nknown:\n  x.one:\n    v: 1\n    name: "one"\n'
                           '    of: "2026-09-01"\n    also: [x.two]\n  x.two:\n    v: 2\n    name: "two"\n'
                           '    of: "2026-09-01"\njudgments:\n  c.small:\n    rests_on: [x.one]\n'
                           '    verdict: "one is small"\n    wrong_if: "x.one > 5"\n    seen: {x.one: 1}\n',
                           encoding="utf-8")
            # the field casts no vote, so the record is read at all - one judgment is enough to tie
            ids, jud, fields = P.infer(P.load([str(rec)]))
            self.assertEqual(fields["deps"], "rests_on")
            code, out, err = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out + err)
            self.assertIn('NOTE x.one carries also: x.two, and x.two is an entry - a retirement that came '
                          'back, or a sibling this record declares; the reader reads it as neither: '
                          'same x.one x.two folds them, distinct x.one x.two "why" tells them apart\n', out)
            self.assertTrue(out.endswith("1 judgments, 3 entries, 0 problems, 1 declared\n"), out)
            # and the two commands the note names do answer it
            self.assertEqual(run(SCRIPTS / "provenance.py", "distinct", "x.one", "x.two", "two numbers",
                                 "--as-of", "2026-09-04", rec)[0], 0)
            code, out, _ = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out)
            self.assertIn("carries also: x.two", out)      # the pair is told apart, the field still says it

    def test_the_healthy_retirement_says_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            rec = copy_fixture(pathlib.Path(d))
            self.assertEqual(run(SCRIPTS / "provenance.py", "same", "heat.boiler_kw", "heat.output_kw",
                                 "--as-of", "2026-09-04", rec)[0], 0)
            self.assertIn("    also: [heat.output_kw]\n", rec.read_text(encoding="utf-8"))
            code, out, err = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out + err)
            self.assertNotIn("carries also:", out)
            doc = P.load([str(rec)])
            base, hyps, live = S._every_raw(doc)
            raws = [base] + [h["raw"] for h in hyps.values()]
            self.assertEqual(S.retired_into(raws, live), {"heat.output_kw": "heat.boiler_kw"})
            self.assertEqual(S.also_held(raws, live), [])

    def test_a_record_may_still_keep_its_dependencies_there(self):
        with tempfile.TemporaryDirectory() as d:
            rec = pathlib.Path(d) / "PROVENANCE.yaml"
            rec.write_text('schema:\n  deps: also\nmeta:\n  updated: 2026-09-04\nknown:\n'
                           '  x.one: {v: 1, name: "one", of: "2026-09-01"}\njudgments:\n  c.small:\n'
                           '    also: [x.one]\n    verdict: "one is small"\n    wrong_if: "x.one > 5"\n'
                           '    seen: {x.one: 1}\n', encoding="utf-8")
            ids, jud, fields = P.infer(P.load([str(rec)]))
            self.assertEqual(fields["deps"], "also")
            self.assertEqual(jud["c.small"]["deps"], ["x.one"])
            code, out, err = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out + err)
            self.assertNotIn("carries also:", out)

    def test_the_other_reading_of_the_field_is_left_alone(self):
        # a record whose also: names siblings that are not entries - resource ids, say - is
        # neither a retirement nor a pair: nothing is read into it and nothing is said
        with tempfile.TemporaryDirectory() as d:
            rec = pathlib.Path(d) / "PROVENANCE.yaml"
            rec.write_text('meta:\n  updated: 2026-09-04\nknown:\n  src.table:\n    v: 1\n'
                           '    name: "the table"\n    of: "2026-09-01"\n'
                           '    also: ["4e6b9724-4c1e-43f0-909a-154d4cc4e046", "ec8cbc34-72e1"]\n'
                           'judgments:\n  c.one:\n    rests_on: [src.table]\n    verdict: "it is one"\n'
                           '    wrong_if: "src.table > 5"\n    seen: {src.table: 1}\n', encoding="utf-8")
            code, out, err = run(SCRIPTS / "provenance.py", "check", rec)
            self.assertEqual(code, 0, out + err)
            self.assertNotIn("carries also:", out)
            doc = P.load([str(rec)])
            base, hyps, live = S._every_raw(doc)
            self.assertEqual(S.retired_into([base], live), {})
            self.assertEqual(S.also_held([base], live), [])


class TheHelpNamesTheCommands(unittest.TestCase):
    def test_the_dispatcher_and_the_reader_list_them(self):
        code, out, _ = run(SCRIPTS / "kpopper")
        self.assertRegex(out, r"kpop same\s")
        self.assertRegex(out, r"kpop distinct\s")
        for cmd in ("same", "distinct"):
            code, out, _ = run(SCRIPTS / "kpopper", cmd, "--help")
            self.assertEqual(code, 0)
            self.assertIn(f"  {cmd} <a> <b>", out)
        code, out, _ = run(SCRIPTS / "sameness.py")
        self.assertEqual(code, 2)
        self.assertIn("Sameness is judged, not guessed", out)


if __name__ == "__main__":
    unittest.main()
