"""Which callers of the page build hand it the record they already read, and which must not.

A read command loads the record once and gives that document to the page build: `open` and
`check` ask the page what it knows about the very record they just read, in the same instant,
so a second read is a second chance for the two views to disagree.

The write path does the opposite, on purpose. `_page_side` takes no document and reloads on
every call, because the counts taken before a write, the arrangement facts taken before it,
and the read-back after it must each see the record as it stands at that moment - the write
itself changes what the page counts. That asymmetry is the whole subject of this file.

Runs against the fixture record in tests/fixtures/page with no browser and no network:

    python3 -m unittest discover -s tests
"""
import contextlib
import copy
import inspect
import io
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts import pending_grounding as G, pending_publication as C, project_modes as M

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
FIXTURE = ROOT / "tests" / "fixtures" / "page"

sys.path.insert(0, str(SCRIPTS))
import provenance as P      # noqa: E402
import render_page as R     # noqa: E402

AS_OF = ["--as-of", "2026-09-05"]


def run(*args):
    p = subprocess.run([sys.executable] + [str(a) for a in args], capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def copy_fixture(into):
    shutil.copy(FIXTURE / "PROVENANCE.yaml", into / "PROVENANCE.yaml")
    shutil.copy(FIXTURE / "PROVENANCE.view.yaml", into / "PROVENANCE.view.yaml")
    return into / "PROVENANCE.yaml"


OVERLAY = ("read_mode", "knowledge_conflicts", "contributions", "pending_ref", "pending_snapshot",
           "publication", "knowledge_target", "target_unavailable", "private_drafts",
           "history_contributions", "capture_config", "knowledge_target_snapshot")

# what a hypothesis carries that something downstream reads: `raw` by sameness and
# consolidate, `path` by the consolidate that removes the file, `doc` by every layered read
LAYER = ("ids", "error", "kind", "head", "name", "path", "raw", "doc")


def document_state(doc):
    """What a later reader can observe of a loaded record, as a comparable value. `Record`
    keeps hypotheses and origins as attributes rather than keys, so `dict(doc)` alone sees
    neither - and a hypothesis is compared by the document it carries, not by its name, or a
    layer body could be rewritten under a name that never moved. An attribute the overlay
    never set is absent rather than None, so injecting one is a difference too - and
    `attributes` is what keeps that true of *every* attribute rather than only the named
    ones, so a later overlay field cannot drop out of the bar by not being listed here."""
    layers = {}
    for name, h in (getattr(doc, "hypotheses", None) or {}).items():
        layers[name] = {k: copy.deepcopy(dict(h[k]) if k == "doc" else h[k])
                        for k in LAYER if k in h}
    present = {a: copy.deepcopy(getattr(doc, a)) for a in OVERLAY if hasattr(doc, a)}
    return {"mapping": copy.deepcopy(dict(doc)),
            "origins": copy.deepcopy(getattr(doc, "origins", None)),
            "hypotheses": layers, "attributes": sorted(vars(doc)),
            "overlay_attributes": sorted(present), **present}


def provenance_modules():
    """Every live copy of the reader. It is imported by file path, as `scripts.provenance`,
    and as `_kpopper_runtime.provenance` through `_peer`, and those are different module
    objects holding different `load` attributes - so a counter installed on one of them is
    blind to a load made through another."""
    seen, out = set(), []
    for module in list(sys.modules.values()):
        if getattr(module, "__name__", "").split(".")[-1] == "provenance" \
                and hasattr(module, "load") and id(module) not in seen:
            seen.add(id(module))
            out.append(module)
    return out


@contextlib.contextmanager
def counted_loads():
    """Every record load the code under test performs, with the paths each was given -
    counted through every copy of the reader, not just the one this file imported."""
    calls, modules = [], provenance_modules()
    originals = {id(m): m.load for m in modules}

    def counter(module):
        original = originals[id(module)]

        def load(paths, *, read_mode=None):
            calls.append(list(paths))
            return original(paths, read_mode=read_mode)
        return load

    with contextlib.ExitStack() as stack:
        for module in modules:
            stack.enter_context(mock.patch.object(module, "load", counter(module)))
        yield calls


class WithTheFixture(unittest.TestCase):
    """A record with a brief beside it, so every page count below is a real one."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.record = copy_fixture(pathlib.Path(self.dir.name))
        self.paths = [str(self.record)]
        self.brief = R.find_brief(self.paths)
        # without a brief there is no second load to remove and every count below is vacuous
        self.assertIsNotNone(self.brief)
        self.assertIsNotNone(P._page_info(self.paths))


class AReadCommandLoadsTheRecordOnce(WithTheFixture):
    def test_the_opening_loads_the_record_once(self):
        with counted_loads() as calls, contextlib.redirect_stdout(io.StringIO()):
            P.opening(self.paths)
        self.assertEqual(len(calls), 1, calls)

    def test_check_loads_the_record_once(self):
        with counted_loads() as calls:
            fail, note, moved, contested, summary = P.check_lines(self.paths)
        self.assertEqual(len(calls), 1, calls)
        self.assertEqual(fail, [], fail)

    def test_the_opening_still_says_what_the_page_knows(self):
        # the count above is only worth having while the page is still consulted: an
        # arrangement's sign is decided by the build, so a footer that says one fired can
        # only come from a page that was built
        record = self.wind_lands()
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            P.opening([str(record)])
        self.assertIn("v.glazing_tab fired (page.unserved > 0)", printed.getvalue())

    def test_check_still_says_what_the_page_knows(self):
        record = self.wind_lands()
        note = P.check_lines([str(record)])[1]
        self.assertIn("v.glazing_tab: wrong_if holds (page.unserved > 0) - decided by the page, "
                      "page.unserved is 1", note)
        self.assertTrue(any("s.2026_09_05_wind is served by no tab" in line for line in note), note)

    def wind_lands(self):
        """A later session records an intent, and what it wrote, that no tab of the page serves."""
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        record = copy_fixture(pathlib.Path(directory.name))
        for entry in (["s.2026_09_05_wind", "asked=how much does the wind add?",
                       "name=the wind question", "read=2026-09-05"],
                      ["heat.gust_kw", "v=2", "unit=kW", "name=loss in a gust",
                       "from=s.2026_09_05_wind"]):
            code, out, err = run(SCRIPTS / "provenance.py", "add", *entry, *AS_OF, record)
            self.assertEqual(code, 0, out + err)
        return record


class HandoverMixin:
    """One document is now shared where there were two, so it must survive the sharing: what
    the reader does with it before asking the page must leave it as it was read, or the page
    draws a record no reader ever had - and what the page does with it must leave it as it
    was given, or the rest of the command reads a record the page invented. The comparison is
    taken after the whole command has run, so it holds both ways."""

    def given_to_the_page_by(self, command):
        seen, original = {}, R.build

        def build(paths, brief_path=None, page_path=None, **keywords):
            seen["doc"] = keywords.get("doc")
            return original(paths, brief_path, page_path, **keywords)

        with mock.patch.object(R, "build", build), contextlib.redirect_stdout(io.StringIO()):
            command(self.paths)
        self.assertIn("doc", seen, "the page was never built, so nothing was handed over")
        self.assertIsNotNone(seen["doc"], "the page was built from a reading of its own")
        return seen["doc"]

    def assert_as_read(self, given):
        self.assertEqual(document_state(given), document_state(P.load(self.paths)))

    def test_the_opening_hands_the_page_the_record_as_it_was_read(self):
        self.assert_as_read(self.given_to_the_page_by(P.opening))

    def test_check_hands_the_page_the_record_as_it_was_read(self):
        self.assert_as_read(self.given_to_the_page_by(P.check_lines))


class TheDocumentTheReaderHandsOver(HandoverMixin, WithTheFixture):
    pass


class ThePageBuildUsesTheDocumentItIsGiven(WithTheFixture):
    def test_a_given_document_is_not_read_again(self):
        doc = P.load(self.paths)
        with counted_loads() as calls:
            R.build(self.paths, self.brief, doc=doc)
        self.assertEqual(calls, [])

    def test_a_given_document_draws_the_same_page_as_a_fresh_load(self):
        fresh = R.build(self.paths, self.brief)
        given = R.build(self.paths, self.brief, doc=P.load(self.paths))
        self.assertEqual(given[0], fresh[0])          # the page, character for character
        self.assertEqual(given[1:], fresh[1:])        # entries, judgments, ids, and what it counted

    def test_what_the_page_knows_is_the_same_whether_or_not_it_is_given_the_document(self):
        fresh = P._page_info(self.paths)
        given = P._page_info(self.paths, doc=P.load(self.paths))
        self.assertEqual(given, fresh)
        self.assertEqual(P._page_or_error(self.paths, doc=P.load(self.paths)), fresh)

    def test_a_document_read_in_another_mode_is_refused_rather_than_drawn(self):
        frozen = P.load(self.paths, read_mode="frozen")
        self.assertEqual(frozen.read_mode, "frozen")
        with self.assertRaisesRegex(ValueError, "read in another mode"):
            R.build(self.paths, self.brief, read_mode="live", doc=frozen)

    def test_the_core_profile_refuses_a_document_rather_than_quietly_reading_its_own(self):
        # core/v1 builds from its own reading; a caller that hands it a document is wrong
        # about what it gets back, and is told so rather than served a second read
        with self.assertRaisesRegex(ValueError, "core"):
            R.build(self.paths, self.brief, profile="core/v1", doc=P.load(self.paths))

    def test_only_a_stated_disagreement_is_refused(self):
        # a document carrying no mode is taken as given - a caller that built one by hand is
        # not second-guessed. On this fixture the drawn page is the same either way, which is
        # exactly why the leniency needs the live fixture to show its teeth; see
        # AnAdvancedRecordWithSomethingPending for what it costs there
        frozen = P.load(self.paths, read_mode="frozen")
        del frozen.read_mode
        self.assertEqual(R.build(self.paths, self.brief, read_mode="live", doc=frozen)[0],
                         R.build(self.paths, self.brief)[0])
        with self.assertRaisesRegex(ValueError, "read in another mode"):
            R.build(self.paths, self.brief, read_mode="live",
                    doc=P.load(self.paths, read_mode="frozen"))


PENDING_RECORD = """\
meta:
  scope: a record with a contribution pending beside it
sources:
  s.vendor: {name: Vendor, file: evidence/vendor.txt, read: "2026-09-14"}
known:
  api.limit: {name: Limit, v: 10, from: s.vendor}
  api.window: {name: Window, v: 60, from: s.vendor}
judgments:
  c.fits:
    rests_on: [api.limit]
    verdict: "the limit fits"
    seen: {api.limit: 10}
    wrong_if: "api.limit < 5"
"""

PENDING_BRIEF = """\
title: pending
intent: "what is open"
tabs:
  - title: Now
    occasion: "you are picking this up"
    serves: [s.vendor]
    sections:
      - title: What is known
        why: the numbers this record holds
        pick: [api.limit, api.window]
        as: lines
"""


class AnAdvancedRecordWithSomethingPending(HandoverMixin, unittest.TestCase):
    """Where the live overlay actually does its work. On anything that is not a Git project in
    Advanced mode, `knowledge_views.overlay` returns before it fills any of the attributes the
    correctness bar names, so a reuse test on a bare fixture compares None with None ten times
    over. This builds the real thing: a contribution captured against the record, disagreeing
    with the base on `api.limit`, which gives a live `knowledge_conflicts`, a `pending_ref`, a
    `pending_snapshot`, a `publication` and a pending hypothesis injected into `doc.hypotheses`
    - and it is the reading that cost the seconds this change removes.

    What this fixture still does not populate, so nothing here should be read as covering it:
    `knowledge_target` and `target_unavailable` need a configured publication target and are
    absent; `private_drafts` is empty; `history_contributions` is empty because the bundle is
    not a committed-history one. A target-configured project also reaches `watch`, which reads
    the record again from inside the overlay - out of this change's scope, and out of what the
    load counts below claim."""

    def setUp(self):
        trigger = mock.patch.object(C, "trigger_after_capture",
                                    return_value={"started": False, "reason": "test fixture"})
        trigger.start()
        self.addCleanup(trigger.stop)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name).resolve() / "project"
        self.root.mkdir()
        M.git(self.root, "init", "-b", "trunk")
        M.git(self.root, "config", "user.name", "Fixture")
        M.git(self.root, "config", "user.email", "fixture@example.test")
        (self.root / "GROUNDING.yaml").write_text(PENDING_RECORD, encoding="utf-8")
        (self.root / ".kpopper").mkdir(exist_ok=True)
        (self.root / ".kpopper" / "view.yaml").write_text(PENDING_BRIEF, encoding="utf-8")
        M.git(self.root, "add", ".")
        M.git(self.root, "-c", "commit.gpgsign=false", "commit", "-m", "Record")
        project = M.Project(self.root)
        self.assertEqual(project.configure("advanced", record="GROUNDING.yaml")["mode"], "advanced")
        contribution = {"sources": {"s.vendor": {"name": "Vendor", "file": "evidence/vendor.txt",
                                                 "read": "2026-09-14"}},
                        "known": {"api.limit": {"name": "Limit", "v": 25, "from": "s.vendor",
                                                "at": "table 1",
                                                "scope": {"kind": "external", "environment": "API v2"}}}}
        bundle = G.prepare(contribution, ["api.limit"],
                           scope={"kind": "external", "environment": "API v2"},
                           shareability="project",
                           evidence={"evidence/vendor.txt": b"The limit is 25.\n"})
        G.Store(project).capture(bundle, event_id="event-1", contribution_id="limit",
                                 shareability="project")
        self.paths = [str(self.root / "GROUNDING.yaml")]
        # the fixture is only worth having while it is the live case: if any of this goes
        # empty the comparisons below stop comparing anything
        doc = P.load(self.paths)
        self.assertEqual(doc.read_mode, "live")
        self.assertTrue(doc.contributions)
        self.assertIn("api.limit", doc.knowledge_conflicts)
        self.assertTrue(doc.pending_ref)
        self.assertTrue(doc.hypotheses)
        self.assertIsNotNone(P._page_info(self.paths))

    def test_the_opening_loads_the_live_record_once(self):
        with counted_loads() as calls, contextlib.redirect_stdout(io.StringIO()):
            P.opening(self.paths)
        self.assertEqual(len(calls), 1, calls)

    def test_check_loads_the_live_record_once(self):
        with counted_loads() as calls:
            P.check_lines(self.paths)
        self.assertEqual(len(calls), 1, calls)

    def test_the_conflict_the_page_is_given_is_the_conflict_the_reader_read(self):
        given = self.given_to_the_page_by(P.check_lines)
        self.assertIn("api.limit", given.knowledge_conflicts)
        self.assertEqual(given.knowledge_conflicts, P.load(self.paths).knowledge_conflicts)

    def test_a_given_document_draws_the_same_page_as_a_fresh_load(self):
        brief = R.find_brief(self.paths)
        self.assertEqual(R.build(self.paths, brief, doc=P.load(self.paths))[0],
                         R.build(self.paths, brief)[0])

    def test_a_document_with_no_stated_mode_is_drawn_and_what_that_costs(self):
        # the mode check only refuses a STATED disagreement, so a frozen document stripped of
        # its read_mode is drawn live without complaint. Here that is not harmless: a frozen
        # reading never ran the live overlay, so the page it draws silently loses the pending
        # contribution the live page shows. Nothing in the reader can reach this - both call
        # sites pass the document's own mode - and it is pinned so that stays true
        brief = R.find_brief(self.paths)
        live = R.build(self.paths, brief)[0]
        frozen = P.load(self.paths, read_mode="frozen")
        self.assertIn("pending-", live)
        del frozen.read_mode
        drawn = R.build(self.paths, brief, read_mode="live", doc=frozen)[0]
        self.assertNotIn("pending-", drawn)
        # and with the mode left on it, the same document is refused rather than drawn
        with self.assertRaisesRegex(ValueError, "read in another mode"):
            R.build(self.paths, brief, read_mode="live",
                    doc=P.load(self.paths, read_mode="frozen"))


class TheWritePathReadsTheRecordItIsWriting(WithTheFixture):
    def test_page_side_takes_no_document_and_so_cannot_be_handed_a_stale_one(self):
        self.assertNotIn("doc", inspect.signature(P._page_side).parameters)

    def test_page_side_reports_the_record_as_it_stands_at_every_call(self):
        # the write path calls it before a write and again after: each call reloads, so a
        # record that moved between two calls is reported as it stands at the second
        before = P._page_side(self.paths)
        self.record.write_text(self.record.read_text(encoding="utf-8")
                               + "  heat.gust_kw: {v: 2, unit: kW, name: loss in a gust}\n",
                               encoding="utf-8")
        after = P._page_side(self.paths)
        self.assertEqual(after[1]["entries"], before[1]["entries"] + 1)

    def test_a_write_records_the_page_counts_as_they_stand_after_it(self):
        before = P._page_side(self.paths)[0]["page.covered"]
        code, out, err = run(SCRIPTS / "provenance.py", "add", "v.covered_tab",
                             "rests_on=[s.2026_09_02_heating, page.covered]",
                             "verdict=the page keeps covering what it drew",
                             "wrong_if=page.covered < 1", *AS_OF, self.record)
        self.assertEqual(code, 0, out + err)
        after = P._page_side(self.paths)[0]["page.covered"]
        # the write moved the count it was snapshotted against - otherwise this proves nothing
        self.assertEqual(after, before + 1)
        lines = self.record.read_text(encoding="utf-8").split("\n")
        _, _, start, end = P._locate(lines, "v.covered_tab")
        written = "\n".join(lines[start:end])
        self.assertIn(f"page.covered: {after}", written)
        self.assertNotIn(f"page.covered: {before}", written)


if __name__ == "__main__":
    unittest.main()
