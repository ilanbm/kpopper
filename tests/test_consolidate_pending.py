"""The dry run tests the pending findings the way it tests hypotheses: each active contribution
of the pending ledger laid over the record, what accepting it would break reported after the
hypotheses' part, the run red when a finding would break something - and nothing adopted.
A frozen run has no pending part."""
from pathlib import Path
import subprocess
import sys
import unittest

from scripts import pending_grounding as G
from scripts import pending_publication
from tests.test_pending_grounding import Repository, fixture_bundle

CLI = Path(__file__).resolve().parents[1] / 'scripts' / 'cli.py'
SCOPE = {'kind': 'external', 'environment': 'API v2'}
RECORD = ("sources:\n  s.vendor: {name: Vendor, file: evidence/vendor.txt, read: '2026-09-14'}\n"
          "known:\n  api.limit: {name: Limit, v: 10, from: s.vendor, at: table 1}\n"
          "judgments:\n  d.x:\n    verdict: the client stays under the vendor limit\n"
          "    rests_on: [api.limit]\n    wrong_if: \"api.limit > 10\"\n")
NO_HYPOTHESES = "no hypotheses beside the record - nothing to consolidate\n"
RED = ("  a person decides each: one that is wrong is rejected with its reason (pending reject "
       "<revision> --reason \"<why>\"); one that stands is read again and set in the base, or "
       "accepted in the knowledge PR\n")


class PendingDryRun(Repository):
    def setUp(self):
        super().setUp()
        self.record = self.root / 'GROUNDING.yaml'
        self.record.write_text(RECORD, encoding='utf-8')
        (self.root / 'evidence').mkdir()
        (self.root / 'evidence' / 'vendor.txt').write_text('The limit is 10.\n', encoding='utf-8')

    def dry_run(self, *names, frozen=False):
        out = subprocess.run([sys.executable, str(CLI), *(['--frozen'] if frozen else []),
                              'consolidate', '--dry-run', *names],
                             cwd=self.root, capture_output=True, text=True)
        self.assertEqual(out.stderr, '')
        return out.returncode, out.stdout

    def share(self, event, doc, roots):
        for root in roots:
            for members in doc.values():
                if isinstance(members, dict) and root in members:
                    members[root].setdefault('scope', dict(SCOPE))
        bundle = G.prepare(doc, roots, scope=SCOPE, shareability='project')
        return self.store.capture(bundle, event_id=event, contribution_id=event,
                                  shareability='project')['revision']

    def hypothesis(self, name, text):
        path = self.root / '.kpopper' / 'hypotheses' / (name + '.yaml')
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding='utf-8')

    def test_a_pending_value_that_breaks_a_falsifier_makes_the_live_dry_run_red(self):
        ten = self.capture(fixture_bundle(value=10), event_id='limit-10')['revision']
        eleven = self.capture(fixture_bundle(value=11), event_id='limit-11')['revision']
        head = self.store.head()
        code, out = self.dry_run()
        self.assertEqual(out, NO_HYPOTHESES + "\n"
            "pending (2): shared findings, each laid over the base alone - tested here, never written\n"
            f"  pending-{ten[:12]} · captured locally · external: API v2 · api.limit\n"
            f"  pending-{eleven[:12]} · captured locally · external: API v2 · api.limit\n"
            "\n"
            "contested (1): an id two pending findings hold differently - at most one of them is accepted\n"
            "  api.limit:\n"
            "    the base holds api.limit: 10 (Limit) <- s.vendor, at table 1\n"
            f"    pending-{ten[:12]} says api.limit: 10 (Limit) <- s.vendor, at table 1\n"
            f"    pending-{eleven[:12]} says api.limit: 11 (Limit) <- s.vendor, at table 1\n"
            f"pending-{ten[:12]}, if accepted: nothing changes - the base already holds what it says\n"
            f"pending-{eleven[:12]}, if accepted:\n"
            "  api.limit: 10 -> 11\n"
            "    a reading of the same day - the base keeps what it holds\n"
            "  FALSIFIED d.x: wrong_if holds (api.limit > 10) - broken by its own condition\n"
            "\n"
            "pending not clean: an id two findings contest, a falsifier holds on a pending value, a "
            "contested reading - the pending findings alone make this run red; no fold, commit or "
            "merge waits on it\n" + RED)
        self.assertEqual(code, 1)
        # nothing is adopted: the record and the ledger stay exactly as they were
        self.assertEqual(self.record.read_text(encoding='utf-8'), RECORD)
        self.assertEqual(self.store.head(), head)
        self.assertEqual(self.dry_run(frozen=True), (0, NO_HYPOTHESES))

    def test_a_rejected_or_withdrawn_finding_is_no_longer_tested(self):
        ten = self.capture(fixture_bundle(value=10), event_id='limit-10')['revision']
        eleven = self.capture(fixture_bundle(value=11), event_id='limit-11')['revision']
        publisher = pending_publication.Publisher(self.store.project)
        publisher.action('reject', revisions=[eleven], reason='the vendor table says 10')
        self.assertEqual(self.dry_run(), (0, NO_HYPOTHESES + "\n"
            "pending (1): shared findings, each laid over the base alone - tested here, never written\n"
            f"  pending-{ten[:12]} · captured locally · external: API v2 · api.limit\n"
            "\n"
            f"pending-{ten[:12]}, if accepted: nothing changes - the base already holds what it says\n"
            "\n"
            "pending clean: accepting it breaks nothing the base holds\n"))
        publisher.action('withdraw', revisions=[ten], reason='superseded by a later reading')
        self.assertEqual(self.dry_run(), (0, NO_HYPOTHESES))

    def test_hypotheses_come_first_and_a_named_run_is_about_them_alone(self):
        self.capture(fixture_bundle(value=11), event_id='limit-11')
        self.hypothesis('trial', 'known:\n  api.window: {v: 5}\n')
        code, named = self.dry_run('trial')
        self.assertTrue(named.endswith('clean: trial may fold - consolidate trial\n'), named)
        self.assertNotIn('pending', named)
        self.assertEqual(code, 0)
        code, every = self.dry_run()
        self.assertTrue(every.startswith(named), every)
        pending = every[len(named):]
        self.assertTrue(pending.startswith('\npending (1): shared findings, each laid over the base '
                                           'alone'), pending)
        self.assertIn('  FALSIFIED d.x: wrong_if holds (api.limit > 10)', pending)
        self.assertTrue(pending.endswith(RED))
        self.assertEqual(code, 1)

    def test_a_hole_names_no_pending_finding_to_consolidate_with(self):
        self.record.write_text('schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\n'
                               'known:\n  local.one: {v: 1}\n', encoding='utf-8')
        revision = self.capture(fixture_bundle(value=10), event_id='limit-10')['revision']
        self.hypothesis('trial', 'judgments:\n  d.h: {verdict: fits, rests_on: [api.limit], '
                                 'wrong_if: "api.limit > 100"}\n')
        code, out = self.dry_run()
        self.assertIn('  FAIL d.h: rests on api.limit, which is not an entry\n', out)
        self.assertNotIn('held by', out)
        self.assertIn(f'pending-{revision[:12]}, if accepted: nothing breaks - it adds api.limit, '
                      f's.vendor\n', out)
        self.assertEqual(code, 1)

    def test_a_moved_premise_waits_for_a_review_and_stays_green(self):
        self.record.write_text(
            "sources:\n  s.vendor: {name: Vendor, read: '2026-09-10'}\n"
            "known:\n  api.limit: {name: Limit, v: 9, from: s.vendor}\n"
            "  api.window: {name: Window, v: 5, from: s.vendor}\n"
            "judgments:\n  d.x:\n    verdict: the client stays under the vendor limit\n"
            "    rests_on: [api.limit, api.window]\n    seen: {api.limit: 9, api.window: 5}\n"
            "    wrong_if: \"api.window > 20\"\n", encoding='utf-8')
        self.share('limit-10', {'sources': {'s.vendor': {'name': 'Vendor', 'read': '2026-09-14'}},
                                'known': {'api.limit': {'name': 'Limit', 'v': 10, 'from': 's.vendor'}}},
                   ['api.limit'])
        code, out = self.dry_run()
        self.assertIn("  api.limit: 9 -> 10\n"
                      "    a reading from 2026-09-14 that is newer than the base's\n", out)
        self.assertIn("  MOVED d.x: api.limit differs from its snapshot (9 -> 10) - re-review, or "
                      "refresh seen\n", out)
        self.assertTrue(out.endswith("\npending: 1 judgment to re-review when accepted - a premise "
                                     "moved under it\n"), out)
        self.assertEqual(code, 0)

    def test_a_verdict_laid_over_a_standing_judgment_waits_for_a_person(self):
        doc = {'sources': {'s.vendor': {'name': 'Vendor', 'file': 'evidence/vendor.txt',
                                        'read': '2026-09-14'}},
               'known': {'api.limit': {'name': 'Limit', 'v': 10, 'from': 's.vendor',
                                       'at': 'table 1'}},
               'judgments': {'d.x': {'verdict': 'the client needs request batching',
                                     'rests_on': ['api.limit'], 'wrong_if': 'api.limit > 100',
                                     'scope': dict(SCOPE)}}}
        bundle = G.prepare(doc, ['d.x'], scope=SCOPE, shareability='project',
                           evidence={'evidence/vendor.txt': b'The limit is 10.\n'})
        self.store.capture(bundle, event_id='batching', contribution_id='batching',
                           shareability='project')
        code, out = self.dry_run()
        self.assertIn("  d.x: the client stays under the vendor l… -> the client needs request batching\n"
                      "    the standing judgment holds, and its wrong_if has not fired - a person "
                      "decides whether it stands\n", out)
        self.assertIn("pending not clean: 1 reversal a person decides - ", out)
        self.assertEqual(code, 1)

    def test_what_only_every_finding_together_breaks_is_reported_apart(self):
        self.record.write_text(
            "sources:\n  s.vendor: {name: Vendor, read: '2026-09-14'}\n"
            "known:\n  api.limit: {name: Limit, v: 10, from: s.vendor}\n"
            "  api.window: {name: Window, v: 5, from: s.vendor}\n", encoding='utf-8')
        source = {'s.vendor': {'name': 'Vendor', 'read': '2026-09-20'}}
        # a decision whose condition reads what the other finding brings: each passes alone
        self.share('decision', {'sources': dict(source),
                                'known': {'api.limit': {'name': 'Limit', 'v': 10, 'from': 's.vendor'}},
                                'judgments': {'d.new': {'verdict': 'one burst fits the window',
                                                        'rests_on': ['api.limit'],
                                                        'wrong_if': 'api.window > 5'}}}, ['d.new'])
        self.share('window', {'sources': dict(source),
                              'known': {'api.window': {'name': 'Window', 'v': 6, 'from': 's.vendor'}}},
                   ['api.window'])
        code, out = self.dry_run()
        together = out[out.index('together, if every one is accepted:\n'):]
        self.assertTrue(together.startswith(
            "together, if every one is accepted:\n"
            "  FALSIFIED d.new: wrong_if holds (api.window > 5) - broken by its own condition\n\n"
            "pending not clean: a falsifier holds on a pending value, a hole - "), together)
        self.assertEqual(out.count('wrong_if holds (api.window > 5)'), 1)
        self.assertEqual(code, 1)

    def test_one_value_from_two_sources_is_two_versions(self):
        for name in ('s.a', 's.b'):
            self.share('limit-' + name[-1],
                       {'sources': {name: {'name': 'Vendor ' + name[-1], 'read': '2026-09-14'}},
                        'known': {'api.limit': {'name': 'Limit', 'v': 10, 'from': name}}},
                       ['api.limit'])
        code, out = self.dry_run()
        block = out[out.index('contested (1): '):out.index('\n', out.index('<- s.b')) + 1]
        self.assertTrue(block.startswith(
            "contested (1): an id two pending findings hold differently - at most one of them is "
            "accepted\n  api.limit:\n    the base holds api.limit: 10 (Limit) <- s.vendor, at table 1\n"),
            block)
        self.assertRegex(block, r"    pending-\w{12} says api.limit: 10 \(Limit\) <- s\.a\n"
                                r"    pending-\w{12} says api.limit: 10 \(Limit\) <- s\.b\n$")
        self.assertIn("pending not clean: an id two findings contest - ", out)
        self.assertEqual(code, 1)

    def test_a_finding_read_by_other_fields_is_reported_and_never_passes(self):
        self.record.write_text('schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\n'
                               + RECORD, encoding='utf-8')
        revision = self.share('fails-if', {
            'schema': {'deps': 'requires', 'predicate': 'fails_if'},
            'sources': {'s.vendor': {'name': 'Vendor', 'read': '2026-09-14'}},
            'known': {'api.limit': {'name': 'Limit', 'v': 10, 'from': 's.vendor'}},
            'judgments': {'d.z': {'verdict': 'the limit leaves no headroom',
                                  'requires': ['api.limit'], 'fails_if': 'api.limit > 5'}}},
            ['d.z'])
        code, out = self.dry_run()
        self.assertIn(f"pending-{revision[:12]}, if accepted: it cannot be read over the base - it "
                      f"holds d.z as a judgment, and the base would read an entry\n", out)
        self.assertIn("\npending not clean: a finding that cannot be read over the base - ", out)
        self.assertEqual(code, 1)

    def test_a_finding_the_base_cannot_read_is_reported_and_the_run_goes_on(self):
        self.hypothesis('trial', 'known:\n  api.window: {v: 5}\n')
        revision = self.share('depends-on', {
            'schema': {'deps': 'depends_on'},
            'sources': {'s.vendor': {'name': 'Vendor', 'read': '2026-09-14'}},
            'known': {'api.limit': {'name': 'Limit', 'v': 10, 'from': 's.vendor'}},
            'judgments': {'d.z': {'verdict': 'the limit is enough', 'depends_on': ['api.limit'],
                                  'wrong_if': 'api.limit < 5'}}},
            ['d.z'])
        code, out = self.dry_run()
        self.assertIn('\nclean: trial may fold - consolidate trial\n\npending (1): ', out)
        self.assertIn(f"\npending-{revision[:12]}, if accepted: it cannot be read over the base - "
                      f"check says why\n", out)
        self.assertIn("\npending not clean: a finding that cannot be read over the base - ", out)
        self.assertEqual(code, 1)

    def test_a_finding_that_changes_how_the_base_is_read_never_passes(self):
        self.record.write_text(RECORD.replace(", file: evidence/vendor.txt", ""), encoding='utf-8')
        # two judgments on another dependency field outvote the base's one, and d.x - whose
        # falsifier the newer limit would break - would drop out of the test unseen
        revision = self.share('switch', {
            'sources': {'s.vendor': {'name': 'Vendor', 'read': '2026-09-20'}},
            'known': {'api.limit': {'name': 'Limit', 'v': 11, 'from': 's.vendor'}},
            'judgments': {'d.p': {'verdict': 'one', 'requires': ['api.limit'],
                                  'fails_if': 'api.limit > 100'},
                          'd.q': {'verdict': 'two', 'requires': ['api.limit'],
                                  'fails_if': 'api.limit > 200'}}},
            ['api.limit', 'd.p', 'd.q'])
        code, out = self.dry_run()
        self.assertIn(f"pending-{revision[:12]}, if accepted: it cannot be read over the base - laid "
                      f"over the base, it would turn the base's d.x from a judgment into an entry\n",
                      out)
        self.assertNotIn("pending clean", out)
        self.assertEqual(code, 1)

    def test_a_finding_the_base_cannot_read_leaves_a_brief_beside_the_record_alone(self):
        brief = self.root / '.kpopper' / 'view.yaml'
        brief.parent.mkdir()
        brief.write_text('sections: []\n', encoding='utf-8')
        revision = self.share('depends-on', {
            'schema': {'deps': 'depends_on'},
            'sources': {'s.vendor': {'name': 'Vendor', 'read': '2026-09-14'}},
            'known': {'api.limit': {'name': 'Limit', 'v': 10, 'from': 's.vendor'}},
            'judgments': {'d.z': {'verdict': 'the limit is enough', 'depends_on': ['api.limit'],
                                  'wrong_if': 'api.limit < 5'}}},
            ['d.z'])
        code, out = self.dry_run()
        self.assertIn(f"\npending-{revision[:12]}, if accepted: it cannot be read over the base - "
                      f"check says why\n", out)
        self.assertEqual(code, 1)


if __name__ == '__main__':
    unittest.main()
