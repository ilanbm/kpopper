"""Search and page measurements bind the selected knowledge world and its evidence."""
import os
import subprocess
import sys
from pathlib import Path
import unittest
from unittest.mock import patch

from scripts import search as S, page_measurements as PM, export_graph as X
from scripts import provenance as P, pending_grounding as G, project_modes as M
from tests.test_pending_grounding import Repository, fixture_bundle


class ModeReaders(Repository):
    def setUp(self):
        super().setUp()
        self.record = self.root / 'GROUNDING.yaml'
        self.record.write_text('known:\n  local.one: {v: 1}\n')
        self.env = patch.dict(os.environ, {'XDG_STATE_HOME': str(self.base / 'state'),
                                         'KPOPPER_READ_MODE': 'live'})
        self.env.start()
        self.addCleanup(self.env.stop)

    def test_simple_resolves_missing_legacy_file_before_hashes_and_origins(self):
        shared = self.base / 'shared' / 'GROUNDING.yaml'
        shared.parent.mkdir()
        shared.write_text(self.record.read_text())
        (shared.parent / 'shared.txt').write_text('shared evidence')
        M.Project(self.root).configure('simple', record=str(shared))
        shared.write_text('sources:\n  s.shared: {file: shared.txt}\n')
        self.record.unlink()
        result = S.corpus(self.record)
        self.assertEqual(result['record'], str(shared))
        self.assertEqual(result['record_sha256'], S.I._sha(shared.read_bytes()))
        self.assertEqual(next(r for r in result['rows'] if r['kind'] == 'source')['content'], 'shared evidence')

    def test_pending_source_uses_immutable_bundle_even_when_checkout_disagrees(self):
        receipt = self.capture()
        shadow = self.root / 'evidence/vendor.txt'
        shadow.parent.mkdir()
        shadow.write_text('forged checkout evidence')
        result = S.corpus(self.record)
        source = next(r for r in result['rows'] if r['kind'] == 'source')
        self.assertEqual(source['content'], 'The limit is 10.\n')
        self.assertTrue(source['file'].startswith('git:'))
        self.assertEqual(source['publication']['revision'], receipt['revision'])
        shadow.unlink()
        self.assertEqual(result['revision'], S.corpus(self.record)['revision'])

    def test_pending_code_measurement_is_scoped_fact_and_conflicts_are_explicit(self):
        scope = {'kind': 'code', 'environment': 'unit checks',
                 'commit': M.git(self.root, 'rev-parse', 'HEAD').stdout.decode().strip()}
        bundle = G.prepare({'known': {'local.one': {'v': 7, 'scope': scope}}}, ['local.one'],
                           scope=scope, shareability='project')
        self.capture(bundle)
        result = S.corpus(self.record)
        row = next(r for r in result['rows'] if r['id'] == 'local.one' and r['scope'] != 'record')
        self.assertEqual(row['status'], 'PENDING')
        self.assertEqual(row['assessment'], 'RECORDED')
        self.assertEqual(row['publication']['scope'], scope)
        self.assertIn('local.one', result['conflicts'])

    def test_frozen_search_ignores_capture_and_external_source_even_with_grant(self):
        secret = self.base / 'private.txt'
        secret.write_text('private source sentinel')
        self.record.write_text(P.yaml.safe_dump({'sources': {'s.secret': {'file': str(secret)}}}))
        state = self.base / 'ingestion'
        S.I.capture({'source_quote': 'private report sentinel', 'question': 'Which entry?'},
                    self.record, state, start=False)
        with patch.dict(os.environ, {'KPOPPER_READ_MODE': 'frozen'}):
            before = S.corpus(self.record, state, [self.base])
            self.capture()
            secret.write_text('private changed sentinel')
            after = S.corpus(self.record, state, [self.base])
        self.assertEqual(before['revision'], after['revision'])
        self.assertFalse(any(r['kind'] == 'capture' for r in before['rows']))
        self.assertNotIn(str(secret), S.encode(before))
        self.assertNotIn('sentinel', S.encode(before))
        self.assertEqual(before['read_mode'], 'frozen')

    def test_pending_only_record_is_searchable_without_creating_checkout(self):
        self.record.unlink()
        self.capture()
        self.assertTrue(S.search('limit', self.record)['results'])
        self.assertFalse(self.record.exists())

    def test_page_live_pending_changes_invalidate_but_frozen_remains_stable(self):
        live = PM.snapshot([str(self.record)])
        with patch.dict(os.environ, {'KPOPPER_READ_MODE': 'frozen'}):
            frozen = PM.snapshot([str(self.record)])
        self.capture()
        self.assertNotEqual(live['identity'], PM.snapshot([str(self.record)])['identity'])
        with patch.dict(os.environ, {'KPOPPER_READ_MODE': 'frozen'}):
            self.assertEqual(frozen['identity'], PM.snapshot([str(self.record)])['identity'])

    def test_page_conflicting_pending_revisions_remain_measurable(self):
        self.capture()
        self.capture(fixture_bundle(value=20), event_id='second')
        result = PM.snapshot([str(self.record)])
        self.assertTrue(result['identity'])

    def test_page_publication_decision_invalidates_counts_without_record_change(self):
        from scripts.pending_publication import Publisher
        receipt = self.capture()
        before = PM.snapshot([str(self.record)])
        PM.publish(before, {'page.spill': 0})
        Publisher(self.root).action('withdraw', revisions=[receipt['revision']], reason='superseded measurement')
        after = PM.snapshot([str(self.record)])
        self.assertNotEqual(before['identity'], after['identity'])
        self.assertEqual(PM.read(after)[0], {})

    def test_pending_read_preserves_scope_and_full_source_hash(self):
        self.capture()
        found = S.search('limit', self.record, limit=10, chars=20000)
        source = next(row for row in found['results'] if row['kind'] == 'source')
        result = S.read(source['ref'], found['revision'], self.record, length=3)
        self.assertEqual(result['sha256'], S.digest('The limit is 10.\n'))
        self.assertEqual(result['publication']['scope']['environment'], 'API v2')
        self.assertFalse(result['complete'])

    def test_page_simple_shared_content_and_selected_path_invalidate(self):
        first, second = self.base / 'first.yaml', self.base / 'second.yaml'
        first.write_text(self.record.read_text())
        second.write_text(self.record.read_text())
        project = M.Project(self.root)
        project.configure('simple', record=str(first))
        before = PM.snapshot([str(self.record)])
        first.write_text('known:\n  local.one: {v: 2}\n')
        self.assertNotEqual(before['identity'], PM.snapshot([str(self.record)])['identity'])
        first.write_text(second.read_text())
        project.configure('simple', record=str(second))
        self.assertNotEqual(before['identity'], PM.snapshot([str(self.record)])['identity'])

    def simple_page_fixture(self):
        shared = self.base / 'shared' / 'GROUNDING.yaml'
        shared.parent.mkdir()
        doc = {'sources': {'s.evidence': {'name': 'Evidence', 'file': 'evidence.txt'}},
               'known': {'local.one': {'v': 1, 'from': 's.evidence'}}}
        self.record.write_text(P.yaml.safe_dump(doc))
        shared.write_text(self.record.read_text())
        M.Project(self.root).configure('simple', record=str(shared))
        for root, title in ((self.root, 'Checkout view'), (shared.parent, 'Shared view')):
            (root / 'evidence.txt').write_text(title + ' evidence')
            brief = root / '.kpopper/view.yaml'
            brief.parent.mkdir(exist_ok=True)
            brief.write_text(P.yaml.safe_dump({'title': title, 'sections': [
                {'title': title + ' links', 'pick': ['s.evidence'], 'as': 'links'}]}))
        return shared

    def assert_evidence_link(self, page, page_path, evidence):
        from tests.test_page_links import Anchors, resolved_file
        anchors = Anchors()
        anchors.feed(page)
        links = [anchor for anchor in anchors.items if 'lk' in anchor.get('class', '').split()]
        self.assertTrue(links)
        self.assertTrue(all(resolved_file(page_path, link['href']) == evidence for link in links))

    def test_simple_page_uses_shared_brief_sources_and_canonical_measurement(self):
        from scripts import render_page as R
        shared = self.simple_page_fixture()
        page_path = self.base / 'rendered.html'
        self.assertEqual(R.find_brief([str(self.record)]), str(shared.parent / '.kpopper/view.yaml'))
        page = R.measured_build([str(self.record)], page_path=page_path)[0]
        self.assertIn('<h1 dir="auto">Shared view</h1>', page)
        self.assertNotIn('Checkout view', page)
        self.assert_evidence_link(page, page_path, shared.parent / 'evidence.txt')
        expected = PM.snapshot([str(shared)])
        self.assertTrue(PM.cache_path(expected['scope']).exists())
        self.assertNotIn('stale', PM.read(expected)[1])
        (shared.parent / '.kpopper/view.yaml').write_text('title: Changed shared view\n')
        self.assertIn('stale', PM.read(PM.snapshot([str(shared)]))[1])

    def test_simple_measurement_alias_and_shared_consumers_agree_end_to_end(self):
        from scripts import render_page as R, followups as F
        shared = self.simple_page_fixture()
        doc = P.yaml.safe_load(shared.read_text())
        doc['judgments'] = {'c.page': {'rests_on': ['page.spill'], 'seen': {'page.spill': 0},
                                     'verdict': 'Covered', 'wrong_if': 'page.spill > 0'}}
        shared.write_text(P.yaml.safe_dump(doc))
        result = R.measured_build([str(self.record)])
        alias, direct = PM.snapshot([str(self.record)]), PM.snapshot([str(shared)])
        self.assertEqual(alias['scope'], direct['scope'])
        self.assertEqual(alias['identity'], direct['identity'])
        counts = {key: value for key, value in result[4]['page'].items() if value is not None}
        self.assertEqual(PM.read(alias)[0], counts)
        self.assertEqual(PM.read(direct)[0], counts)
        for record in (self.record, shared):
            values, _, error = F.graph(str(record))
            self.assertIsNone(error)
            self.assertEqual(values['page.spill'], result[4]['page']['page.spill'])
        with patch.dict(os.environ, {'KPOPPER_READ_MODE': 'frozen'}):
            self.assertNotEqual(PM.snapshot([str(self.record)])['scope'], PM.snapshot([str(shared)])['scope'])
            self.assertEqual(PM.read(PM.snapshot([str(self.record)]))[0], {})

    def test_simple_measurement_publication_rechecks_original_alias_route(self):
        shared = self.simple_page_fixture()
        before = PM.snapshot([str(self.record)])
        replacement = self.base / 'replacement.yaml'
        replacement.write_text(shared.read_text())
        M.Project(self.root).configure('simple', record=str(replacement))
        with self.assertRaisesRegex(ValueError, 'changed'):
            PM.publish(before, {'page.spill': 0})
        self.assertFalse(PM.cache_path(before['scope']).exists())

    def test_frozen_page_keeps_checkout_brief_sources_and_measurement(self):
        from scripts import render_page as R
        self.simple_page_fixture()
        page_path = self.base / 'frozen.html'
        with patch.dict(os.environ, {'KPOPPER_READ_MODE': 'frozen'}):
            self.assertEqual(R.find_brief([str(self.record)]), str(self.root / '.kpopper/view.yaml'))
            page = R.measured_build([str(self.record)], page_path=page_path)[0]
            self.assertIn('<h1 dir="auto">Checkout view</h1>', page)
            self.assertNotIn('Shared view', page)
            self.assert_evidence_link(page, page_path, self.root / 'evidence.txt')
            expected = PM.snapshot([str(self.record)])
            self.assertTrue(PM.cache_path(expected['scope']).exists())
            self.assertNotIn('stale', PM.read(expected)[1])

    def test_renderer_cli_preserves_selected_mode_for_explicit_checkout_path(self):
        from scripts import render_page as R
        shared = self.simple_page_fixture()
        for flags, title, source in (([], 'Shared view', shared.parent / 'evidence.txt'),
                                     (['--frozen'], 'Checkout view', self.root / 'evidence.txt')):
            page_path = self.base / ('frozen-cli.html' if flags else 'live-cli.html')
            result = subprocess.run([sys.executable, R.__file__, str(self.record),
                                     '--page-out', str(page_path), *flags],
                                    text=True, capture_output=True, env=os.environ, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('<h1 dir="auto">' + title + '</h1>', result.stdout)
            self.assert_evidence_link(result.stdout, page_path, source)

    def test_explicit_frozen_renderer_read_ignores_ambient_live_redirection(self):
        from scripts import render_page as R
        shared = self.simple_page_fixture()
        shared.write_text('known:\n  shared.only: {v: 2}\n')
        brief = R.find_brief([str(self.record)], read_mode='frozen')
        page_path = self.base / 'frozen-explicit.html'
        page, entries, _, _, _ = R.build([str(self.record)], brief, page_path, read_mode='frozen')
        self.assertIn('s.evidence', entries)
        self.assertNotIn('shared.only', entries)
        self.assert_evidence_link(page, page_path, self.root / 'evidence.txt')

    def test_page_labels_contributions_without_calling_them_hypotheses(self):
        from scripts import render_page as R
        self.capture()
        page = R.build([str(self.record)])[0]
        self.assertIn('PENDING', page)
        self.assertNotIn('1 hypothesis waits beside this record', page)
        self.assertIn('API v2', page)

    def test_export_distinguishes_unexpanded_contributions_from_hypotheses(self):
        self.capture()
        packet = X.project([str(self.record)], ['local.one'])
        self.assertEqual(packet['hypotheses'], 0)
        self.assertEqual(packet['contributions'], 1)
        self.assertIn('contribution', X.render_markdown(packet))
        with self.assertRaisesRegex(ValueError, 'pending.*not expanded'):
            X.project([str(self.record)], ['api.limit'])


if __name__ == '__main__':
    unittest.main()
