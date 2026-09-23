"""A record may name its snapshot field `reviewed`, the name a review also dates a judgment by.
The review keeps what the judgment saw and writes no day over it. The native writer's tests in
native/tests/public_authoring.rs hold the same records and expect the same bytes and output."""
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
CLI = ROOT / "scripts" / "cli.py"

SCHEMA = 'schema: {deps: relies_on, snapshot: reviewed, predicate: invalid_when}\n'
BASE = SCHEMA + 'known:\n  api.limit: {v: 10}\n'
JUDGMENT = ('judgments:\n'
            '  d.w:\n'
            '    verdict: known\n'
            '    relies_on: [api.limit]\n'
            '    invalid_when: "api.limit > 100"\n'
            '    reviewed: {api.limit: 5}\n')
RECORD = BASE + JUDGMENT
TRAIL = '    replaced: ["its condition fired on 2026-01-01"]\n'
# what a review used to leave: a day where the snapshot was
DAY_OVER_IT = RECORD.replace('reviewed: {api.limit: 5}', 'reviewed: "2026-01-01"')
NOTHING_TO_SEE = (SCHEMA + 'judgments:\n'
                  '  d.w:\n'
                  '    verdict: known\n'
                  '    relies_on: []\n'
                  '    reviewed: {}\n')
# a day over the snapshot of a judgment with nothing to see, which cleared its reversal
DAY_OVER_NOTHING = (SCHEMA + 'judgments:\n'
                    '  d.w:\n'
                    '    verdict: known\n'
                    '    relies_on: []\n'
                    '    reopened_by: "a new vendor contract"\n'
                    '    reviewed: "2026-01-01"\n'
                    '    replaced: ["its condition fired on 2025-12-20"]\n')
ONE_LINE = BASE + ('judgments:\n'
                   '  d.w: {verdict: known, relies_on: [api.limit], invalid_when: "api.limit > 100", '
                   'reviewed: {api.limit: 10}}\n')
ONE_LINE_MOVED = BASE + ('judgments:\n'
                         '  d.w: {verdict: known, relies_on: [api.limit], invalid_when: "api.limit > 100", '
                         'reviewed: {api.limit: 5}%s}\n')
HYPOTHESIS = 'hypothesis: {born: "2025-12-01"}\n\n' + JUDGMENT
REWRITTEN = "review d.w: seen rewritten from what the record holds (2026-01-02)\n"
HOLDS = "  d.w holds: wrong_if does not hold (api.limit > 100)\n"


def now(text):
    return text.replace('reviewed: {api.limit: 5}', 'reviewed: {api.limit: 10}')


def stays_open(day):
    return f"  reversed on {day} stays open - the snapshot field is named reviewed, so review writes no day\n"


def cli(root, *args):
    """The command line as a session runs it, from the record's folder, with no cached
    programs to read by."""
    with tempfile.TemporaryDirectory() as cache:
        p = subprocess.run([sys.executable, str(CLI), *args], cwd=root, capture_output=True,
                           text=True, env=dict(os.environ, XDG_CACHE_HOME=cache))
    return p.returncode, p.stdout, p.stderr


class ASnapshotNamedReviewed(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.root = pathlib.Path(folder.name)
        self.record = self.root / "GROUNDING.yaml"
        self.proposal = self.root / ".kpopper" / "hypotheses" / "limit.yaml"

    def review(self, text, *args):
        self.record.write_text(text, encoding="utf-8")
        code, out, err = cli(self.root, "review", "d.w", "--as-of", "2026-01-02", *args)
        self.assertEqual(code, 0, out + err)
        return out

    def review_proposed(self, text):
        self.proposal.parent.mkdir(parents=True, exist_ok=True)
        self.proposal.write_text(text, encoding="utf-8")
        out = self.review(BASE, "--hypothesis", "limit")
        self.assertEqual(self.record.read_text(encoding="utf-8"), BASE)
        return out

    def test_review_keeps_the_snapshot_and_writes_no_day(self):
        self.assertEqual(self.review(RECORD), REWRITTEN + "  api.limit: 5 -> 10\n" + HOLDS)
        self.assertEqual(self.record.read_text(encoding="utf-8"), now(RECORD))
        code, out, err = cli(self.root, "check")
        self.assertEqual(code, 0, out + err)

    def test_a_reversal_stays_open_and_the_review_says_so(self):
        # a day written into the field would be the one that clears it
        self.assertEqual(self.review(RECORD + TRAIL),
                         REWRITTEN + "  api.limit: 5 -> 10\n" + stays_open("2026-01-01") + HOLDS)
        self.assertEqual(self.record.read_text(encoding="utf-8"), now(RECORD + TRAIL))
        code, out, err = cli(self.root, "check")
        self.assertEqual(code, 0, out + err)
        self.assertIn("d.w: reversed on 2026-01-01", out)

    def test_a_day_written_over_the_snapshot_is_read_as_never_checked_and_replaced(self):
        self.assertEqual(self.review(DAY_OVER_IT),
                         REWRITTEN + "  api.limit: 10 (never checked against it before)\n" + HOLDS)
        self.assertEqual(self.record.read_text(encoding="utf-8"), now(RECORD))

    def test_a_day_over_a_snapshot_with_nothing_to_see_is_replaced_and_the_reversal_shows(self):
        self.record.write_text(DAY_OVER_NOTHING, encoding="utf-8")
        self.assertNotIn("reversed on", cli(self.root, "check")[1])
        self.assertEqual(self.review(DAY_OVER_NOTHING),
                         REWRITTEN + stays_open("2025-12-20")
                         + "  d.w holds: decided; reopened by a new vendor contract\n")
        self.assertEqual(self.record.read_text(encoding="utf-8"),
                         DAY_OVER_NOTHING.replace('reviewed: "2026-01-01"', 'reviewed: {}'))
        code, out, err = cli(self.root, "check")
        self.assertEqual(code, 0, out + err)
        self.assertIn("d.w: reversed on 2025-12-20", out)

    def test_an_empty_snapshot_with_nothing_to_see_is_left_as_it_is(self):
        self.assertEqual(self.review(NOTHING_TO_SEE),
                         "review d.w: what it saw is what the record holds (2026-01-02)\n"
                         "  d.w no_predicate: nothing evaluable would say otherwise\n")
        self.assertEqual(self.record.read_text(encoding="utf-8"), NOTHING_TO_SEE)

    def test_a_judgment_on_one_line_with_nothing_to_rewrite_is_reviewed(self):
        # with no day to write, nothing needs a line of its own under it
        self.assertEqual(self.review(ONE_LINE),
                         "review d.w: what it saw is what the record holds (2026-01-02)\n" + HOLDS)
        self.assertEqual(self.record.read_text(encoding="utf-8"), ONE_LINE)

    def test_a_judgment_on_one_line_keeps_its_snapshot_inside_its_braces(self):
        for trail, said in (("", ""),
                            (', replaced: ["its condition fired on 2026-01-01"]', stays_open("2026-01-01"))):
            with self.subTest(trail=trail):
                text = ONE_LINE_MOVED % trail
                self.assertEqual(self.review(text), REWRITTEN + "  api.limit: 5 -> 10\n" + said + HOLDS)
                self.assertEqual(self.record.read_text(encoding="utf-8"), now(text))

    def test_a_hypothesis_keeps_its_snapshot(self):
        self.assertEqual(self.review_proposed(HYPOTHESIS),
                         "review d.w in hypothesis limit: seen rewritten from what the record holds "
                         "under it (2026-01-02)\n  api.limit: 5 -> 10\n" + HOLDS +
                         "\nthe base is untouched; limit holds 0 entries and 1 judgment\n")
        self.assertEqual(self.proposal.read_text(encoding="utf-8"), now(HYPOTHESIS))

    def test_a_reversal_in_a_hypothesis_stays_open_and_the_review_says_so(self):
        self.assertEqual(self.review_proposed(HYPOTHESIS + TRAIL),
                         "review d.w in hypothesis limit: seen rewritten from what the record holds "
                         "under it (2026-01-02)\n  api.limit: 5 -> 10\n" + stays_open("2026-01-01")
                         + HOLDS + "\nthe base is untouched; limit holds 0 entries and 1 judgment\n")
        self.assertEqual(self.proposal.read_text(encoding="utf-8"), now(HYPOTHESIS + TRAIL))


if __name__ == "__main__":
    unittest.main()
