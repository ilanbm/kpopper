"""Advanced reports capture exact scoped history without publishing local knowledge."""
import copy
import json
from pathlib import Path
import subprocess
import unittest
from unittest import mock

from scripts import ingestion as I
from tests import test_history_store as fixtures
from tests.test_history_authoring import claim

SCOPE = {'kind': 'project', 'environment': 'fixture'}


class ScopedReports(unittest.TestCase):
    def setUp(self):
        fixture = fixtures.Storage()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture, self.entry, self.store = fixture, fixture.entry.resolve(), fixture.store
        self.root, self.state = self.entry.parent, self.entry.parent / 'private-state'
        subprocess.run(['git', 'init', '-q', '-b', 'main', str(self.root)], check=True)
        subprocess.run(['git', '-C', str(self.root), 'config', 'user.email', 'fixture@example.test'], check=True)
        subprocess.run(['git', '-C', str(self.root), 'config', 'user.name', 'fixture'], check=True)
        document = fixtures.C.decode_document(self.entry.read_bytes())
        document['sources'] = {}
        self.entry.write_bytes(fixtures.C.encode_document(document))
        source = claim('s.old', op='source', body={'file': 'old.md', 'read': '2026-09-01', 'scope': SCOPE})
        reading = claim(body={'v': 1, 'of': '2026-09-01', 'from': 's.old', 'scope': SCOPE})
        private = claim('private.sibling', op='private', body={'v': 7, 'private': True})
        fixture.publish([source, reading, private], op='bootstrap')
        (self.root / 'old.md').write_text('Original evidence.')
        self.G = I.P._peer('pending_grounding')
        self.B = I.P._peer('history_bundle')
        self.A = I.P._peer('history_authoring')
        self.T = I.P._peer('history_transaction')
        self.project = self.G.M.Project(self.root)
        I._save(self.project.config_path, {'version': 1, 'mode': 'advanced', 'record': 'GROUNDING.yaml',
                                        'publication': None, 'generation': 0})
        self.publisher = mock.patch.object(I.P._peer('pending_publication'), 'trigger_after_capture',
                                            return_value={'started': False, 'reason': 'fixture'})
        self.publisher.start()
        self.addCleanup(self.publisher.stop)
        self.envelope = {'event_id': 'scoped', 'date': '2026-09-17', 'target': 'p.input', 'value': 2,
                         'source_quote': 'Input is now 2.', 'scope': copy.deepcopy(SCOPE), 'shareability': 'project'}

    def capture(self, envelope=None):
        # project capture normally starts immediately even with start=False;
        # retain without processing so fault hooks have an exact captured event.
        with mock.patch.object(I, 'process', return_value=[]):
            result = I.capture(envelope or self.envelope, self.entry, self.state, start=False)
        self.eid = result['event_id']
        return result

    def process(self, **kwargs):
        return I.process(self.entry, self.state, event_id=self.eid, **kwargs)[0]

    def journal(self):
        return json.loads((self.state / 'journals' / (self.eid + '.json')).read_text())

    def test_scoped_capture_is_portable_pending_and_omits_private_sibling(self):
        self.capture()
        before = self.store.capture()
        result = self.process()
        self.assertEqual(result['state'], 'project_captured', result)
        self.assertEqual(self.store.capture().inventory, before.inventory)
        snapshot = self.G.Store(self.project).snapshot()
        bundle = snapshot['bundles'][result['pending']['revision']]
        self.assertEqual(bundle['manifest']['version'], 3)
        self.assertNotIn('private.sibling', str(bundle))
        self.assertNotIn(str(self.state), str(bundle))
        portable = '.kpopper/evidence/reports/' + self.eid + '.txt'
        self.assertEqual(bundle['files'][portable], self.envelope['source_quote'].encode())
        captured = self.B.validate(self.B.from_contribution(bundle))
        self.assertEqual(captured.state['subjects']['s.ingest_' + self.eid]['body']['file'], portable)
        origin = captured.document['meta']['history_subset']
        self.assertEqual(origin['subjects']['p.input']['source_state'], 'prepared_candidate')
        self.assertEqual(origin['subjects']['s.old']['source_state'], 'committed')
        self.assertEqual(captured.state['subjects']['p.input']['body']['v'], 2)
        self.assertEqual(self.store.state()['subjects']['p.input']['body']['v'], 1)
        self.assertFalse((self.root / portable).exists())
        mutation = I._mutation_from_journal(self.journal())
        candidate, digest = self.B._candidate(I.P._peer('history_store').Store(self.entry).capture(), mutation)
        record = next(item['after'] for item in mutation.files if item['role'] == 'record')
        self.assertEqual(candidate.entry_bytes, record)
        self.assertEqual(digest, I._sha(mutation.to_bytes()))
        manifest = fixtures.C.decode_document(next(item['after'] for item in mutation.files if item['role'] == 'history_commit'))
        self.assertEqual({item['id'] for item in manifest['objects']},
                         {fixtures.C.decode_document(item['after'])['id'] for item in mutation.files
                          if item['role'] == 'history_object'})
        self.assertEqual(len([item for item in mutation.files if item['role'] == 'history_evidence']), 1)

    def test_crash_after_ledger_commit_resumes_exact_receipt_without_reprepare(self):
        self.capture()
        with self.assertRaises(I._CrashAfterCommit):
            self.process(_crash_after_commit=True)
        original = self.journal()
        # Successful durable capture can be acknowledged even after local source edits.
        self.entry.write_bytes(self.entry.read_bytes() + b'# later annotation\n')
        with mock.patch.object(I, '_process_history_report', side_effect=AssertionError('reprepared')):
            result = self.process()
        self.assertEqual(result['state'], 'project_captured', result)
        self.assertEqual(original, self.journal())
        self.assertEqual(len(self.G.Store(self.project).snapshot()['events']), 1)
        self.assertEqual(I.capture(self.envelope, self.entry, self.state, start=False), result)

    def test_fault_before_ledger_cas_retries_retained_bundle(self):
        self.capture()
        with mock.patch.object(self.G.Store, '_cas', side_effect=OSError('ledger interruption')):
            result = self.process()
        self.assertEqual(result['state'], 'recovery_required', result)
        journal = self.journal()
        with mock.patch.object(I, '_process_history_report', side_effect=AssertionError('reprepared')):
            result = self.process()
        self.assertEqual(result['state'], 'project_captured', result)
        self.assertEqual(journal, self.journal())

    def test_source_bytes_change_between_preparation_and_capture_refuses(self):
        self.capture()
        original = self.G.Store.capture
        def interleave(store, bundle, **kwargs):
            (self.root / 'old.md').write_text('Concurrent evidence edit.')
            return original(store, bundle, **kwargs)
        before = self.entry.read_bytes()
        with mock.patch.object(self.G.Store, 'capture', interleave):
            result = self.process()
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('evidence changed', result['reason'])
        self.assertEqual(self.entry.read_bytes(), before)
        self.assertFalse(self.G.Store(self.project).snapshot()['events'])

    def test_private_historical_version_cannot_be_shared_by_replacing_it(self):
        private = claim('p.input', op='private-old', saw=[next(iter(self.store.state()['subjects']['p.input']['heads']))],
                        body={'v': 99, 'private': True, 'scope': SCOPE})
        self.fixture.publish([private], op='private-proposal')
        self.capture()
        result = self.process()
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('private', result['reason'])
        self.assertFalse(self.G.Store(self.project).snapshot()['events'])

    def test_scope_is_declared_and_propagated_before_identity(self):
        envelope = copy.deepcopy(self.envelope)
        envelope['scope'] = {'kind': 'external', 'environment': 'supplier'}
        self.capture(envelope)
        result = self.process()
        self.assertEqual(result['state'], 'project_captured', result)
        bundle = self.G.Store(self.project).snapshot()['bundles'][result['pending']['revision']]
        captured = self.B.validate(self.B.from_contribution(bundle))
        self.assertEqual(captured.state['subjects']['p.input']['body']['scope'], envelope['scope'])
        self.assertEqual(captured.state['subjects']['s.ingest_' + self.eid]['body']['scope'], envelope['scope'])
        self.assertTrue(any(obj['body'].get('scope') == SCOPE for obj in captured.objects.values()
                            if obj['kind'] != 'act' and obj['subject'] == 'p.input'))

    def test_changed_committed_source_between_prepare_and_ledger_refuses(self):
        self.capture()
        original = self.G.Store.capture
        def interleave(store, bundle, **kwargs):
            self.entry.write_bytes(self.entry.read_bytes() + b'# concurrent source annotation\n')
            return original(store, bundle, **kwargs)
        with mock.patch.object(self.G.Store, 'capture', interleave):
            result = self.process()
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('source changed', result['reason'])
        self.assertFalse(self.G.Store(self.project).snapshot()['events'])

    def test_private_or_unclear_scope_reports_never_publish(self):
        before = self.entry.read_bytes()
        for index, change in enumerate(({'privacy': 'private'}, {'scope': {'kind': 'unclear'}})):
            envelope = {**self.envelope, **change, 'event_id': 'private-' + str(index)}
            self.capture(envelope)
            result = self.process()
            self.assertEqual(result['state'], 'needs_primary', result)
            self.assertEqual(self.entry.read_bytes(), before)
        self.assertFalse(self.G.Store(self.project).snapshot()['events'])

    def test_locator_metadata_requires_explicit_exact_disclosure(self):
        imported = claim('p.imported', op='imported', body={'v': 1, 'of': '2026-09-01', 'scope': SCOPE})
        imported['authored']['locator'] = {'path': 'retained/original.yaml', 'sha256': 'a' * 64}
        imported['id'] = fixtures.C.object_identity(imported)
        self.fixture.publish([imported], op='imported')
        envelope = {**self.envelope, 'event_id': 'undisclosed', 'target': 'p.imported'}
        self.capture(envelope)
        result = self.process()
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('undisclosed_history_locator', result['reason'])
        envelope.update(event_id='disclosed', disclosed_locators=[{'path': 'retained/original.yaml', 'sha256': 'a' * 64}])
        self.capture(envelope)
        result = self.process()
        self.assertEqual(result['state'], 'project_captured', result)
        bundle = self.G.Store(self.project).snapshot()['bundles'][result['pending']['revision']]
        self.assertFalse(any(path.endswith('original.yaml') for path in bundle['files']))


if __name__ == '__main__':
    unittest.main()
