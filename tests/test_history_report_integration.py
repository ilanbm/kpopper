"""Atomic report history, exact batch replay, and retained legacy replacements."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import ingestion as I
from tests import test_history_store as fixtures
from tests.test_history_authoring import claim


class HistoryReports(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.Storage()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.entry, self.store = self.fixture.entry, self.fixture.store
        doc = fixtures.C.decode_document(self.entry.read_bytes())
        doc['sources'] = {}
        self.entry.write_bytes(fixtures.C.encode_document(doc))
        self.source = claim('s.old', op='old-source', body={'file': 'old.md', 'read': '2026-09-01'})
        self.reading = claim(body={'v': 1, 'of': '2026-09-01', 'from': 's.old'})
        self.decision = claim('d.ready', op='decision', kind='judgment', body={
            'verdict': 'ready', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'},
            'seen': {'p.input': 1}}, pins={'p.input': self.reading['id']})
        self.fixture.publish([self.source, self.reading, self.decision], op='bootstrap')
        self.state = self.entry.parent / 'private-state'
        self.A, self.H, self.T = (I.P._peer(n) for n in ('history_authoring', 'history_store', 'history_transaction'))
        self.report = {'event_id': 'history-report', 'source_quote': 'Input is now 2.',
                       'date': '2026-09-17', 'target': 'p.input', 'value': 2}

    def capture(self, envelope=None):
        event = I.capture(envelope or self.report, self.entry, self.state, start=False)
        self.event_id = event['event_id']
        return event

    def process(self):
        return I.process(self.entry, self.state, event_id=self.event_id)[0]

    def journal(self):
        return json.loads((self.state / 'journals' / (self.event_id + '.json')).read_text())

    def test_report_commits_complete_source_and_history_once(self):
        self.capture()
        before = self.store.capture()
        result = self.process()
        self.assertEqual(result['state'], 'applied', result)
        current = self.store.capture()
        self.assertEqual(len(current.commits), 2)
        self.assertEqual(current.state['subjects']['p.input']['body']['v'], 2)
        self.assertEqual(current.objects[self.reading['id']], self.reading)
        self.assertEqual(current.objects[self.decision['id']]['body']['seen'], {'p.input': 1})
        source_id = 's.ingest_' + self.event_id
        self.assertEqual(current.state['subjects'][source_id]['body']['file'], result['source_file'])
        self.assertEqual(Path(result['source_file']).read_text(), self.report['source_quote'])
        mutation = I._mutation_from_journal(self.journal())
        self.assertEqual(len([f for f in mutation.files if f['role'] == 'history_commit']), 1)
        self.assertEqual(mutation.to_data()['baseline'], before.baseline)
        self.assertEqual(set(current.commits), {'bootstrap', mutation.to_data()['operation']})
        self.assertFalse(I._history_shared(self.entry).exists())
        self.assertEqual(I.capture(self.report, self.entry, self.state, start=False), result)
        self.assertEqual(I.process(self.entry, self.state), [])

    def test_batch_reading_and_judgment_replacement_is_one_generation(self):
        envelope = {'event_id': 'batch', 'date': '2026-09-17', 'source_quote': 'Input is 8; decision repaired.',
            'record_sha256': I._sha(self.entry.read_bytes()), 'updates': [
                {'kind': 'set', 'id': 'p.input', 'value': 8},
                {'kind': 'add', 'id': 'd.ready', 'body': {'verdict': 'revised',
                    'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 20'}}}]}
        self.capture(envelope)
        result = self.process()
        self.assertEqual(result['state'], 'applied', result)
        current = self.store.capture()
        self.assertEqual(len(current.commits), 2)
        self.assertEqual(current.state['subjects']['d.ready']['body']['verdict'], 'revised')
        self.assertEqual(current.objects[self.decision['id']], self.decision)
        self.assertEqual(current.objects[current.state['subjects']['d.ready']['head']]['pins'],
                         {'p.input': current.state['subjects']['p.input']['head']})

    def test_invalid_later_batch_action_publishes_nothing(self):
        before = self.entry.read_bytes()
        self.capture({'event_id': 'invalid', 'date': '2026-09-17', 'source_quote': 'Input is 8.',
            'record_sha256': I._sha(before), 'updates': [
                {'kind': 'set', 'id': 'p.input', 'value': 8},
                {'kind': 'add', 'id': 'd.bad', 'body': {'verdict': 'bad',
                    'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 2'}}}]})
        result = self.process()
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertEqual(self.entry.read_bytes(), before)
        self.assertEqual(len(self.store.capture().commits), 1)

    def assert_crash(self, *, manifest=False, callback=False):
        self.capture()
        original = self.T.publish_immutable if manifest else self.T._replace
        def fault(path, *args, **kwargs):
            if (manifest and Path(path).parent.resolve() == Path(self.store.layout['history_commits']).resolve()) or \
                    (not manifest and Path(path).resolve() == (self.state / 'journals' / (self.event_id + '.json') if callback else self.entry).resolve()):
                raise OSError('injected history interruption')
            return original(path, *args, **kwargs)
        with mock.patch.object(self.T, 'publish_immutable' if manifest else '_replace', side_effect=fault):
            result = self.process()
        self.assertEqual(result['state'], 'recovery_required', result)
        self.assertTrue(I._history_shared(self.entry).exists())
        original_mutation = I._mutation_from_journal(self.journal()).to_bytes()
        with mock.patch.object(I, '_process_history_report', side_effect=AssertionError('regenerated report intent')):
            result = self.process()
        self.assertEqual(result['state'], 'applied', result)
        self.assertTrue(result['recovered'])
        self.assertEqual(I._mutation_from_journal(self.journal()).to_bytes(), original_mutation)
        self.assertEqual(len(self.store.capture().commits), 2)
        self.assertFalse(I._history_shared(self.entry).exists())

    def test_before_manifest_failure_retries_complete_batch(self):
        self.assert_crash(manifest=True)

    def test_after_manifest_view_failure_retries_without_new_evidence(self):
        self.assert_crash()

    def test_durable_event_receipt_failure_keeps_shared_recovery(self):
        self.assert_crash(callback=True)

    def test_cancel_before_commit_preserves_original_record(self):
        self.capture()
        before = self.entry.read_bytes()
        original = self.T.publish_immutable
        def fail_manifest(path, *args, **kwargs):
            if Path(path).parent.resolve() == Path(self.store.layout['history_commits']).resolve():
                raise OSError('before manifest')
            return original(path, *args, **kwargs)
        with mock.patch.object(self.T, 'publish_immutable', side_effect=fail_manifest):
            result = self.process()
        self.assertEqual(result['state'], 'recovery_required', result)
        cancelled = I.recover_history(self.entry, self.state, self.event_id, direction='before')
        self.assertEqual(cancelled['state'], 'needs_primary')
        self.assertIn('cancelled', cancelled['reason'])
        self.assertEqual(self.entry.read_bytes(), before)
        self.assertEqual(len(self.store.capture().commits), 1)
        self.assertFalse(I._history_shared(self.entry).exists())
        self.assertEqual(I.process(self.entry, self.state), [])

    def test_cancel_after_manifest_is_refused(self):
        self.capture()
        original = self.T._replace
        def fail_view(path, *args, **kwargs):
            if Path(path).resolve() == self.entry.resolve():
                raise OSError('view interruption')
            return original(path, *args, **kwargs)
        with mock.patch.object(self.T, '_replace', side_effect=fail_view):
            self.assertEqual(self.process()['state'], 'recovery_required')
        with self.assertRaisesRegex(ValueError, 'history_already_committed'):
            I.recover_history(self.entry, self.state, self.event_id, direction='before')
        self.assertEqual(len(self.store.capture().commits), 2)

    def test_missing_dependency_and_runtime_refusal_leave_batch_unpublished(self):
        for mode in ('missing', 'runtime'):
            before = self.entry.read_bytes()
            envelope = {'event_id': mode, 'date': '2026-09-17', 'source_quote': 'New finding.',
                'record_sha256': I._sha(before), 'updates': [{'kind': 'add', 'id': 'd.new', 'body': {
                    'verdict': 'new', 'rests_on': ['p.missing' if mode == 'missing' else 'p.input'],
                    'wrong_if': {'expr': 'p.missing > 5' if mode == 'missing' else 'p.input > 5'},
                    **({'blocked_on': 'missing evidence'} if mode == 'missing' else {})}}]}
            self.capture(envelope)
            if mode == 'runtime':
                with mock.patch.object(self.A.authoring.World, 'assessment', side_effect=self.A.P.Refused('runtime_timeout')):
                    result = self.process()
                self.assertIn('runtime_timeout', result['reason'])
            else:
                result = self.process()
            self.assertEqual(result['state'], 'needs_primary')
            self.assertEqual(self.entry.read_bytes(), before)
            self.assertEqual(len(self.store.capture().commits), 1)

    def test_changed_target_after_capture_is_not_overwritten(self):
        self.capture()
        mutation = self.A.prepare(self.entry, {'kind': 'set', 'id': 'p.input', 'value': 3,
                                             'as_of': '2026-09-17'})
        self.A.commit(self.entry, mutation, verify=lambda data: None)
        before = self.entry.read_bytes()
        result = self.process()
        self.assertEqual(result['state'], 'needs_primary')
        self.assertIn('changed', result['reason'])
        self.assertEqual(self.entry.read_bytes(), before)

    def test_forged_completed_phase_without_manifest_cannot_acknowledge(self):
        self.capture()
        original = self.T.publish_immutable
        def fail_manifest(path, *args, **kwargs):
            if Path(path).parent.resolve() == Path(self.store.layout['history_commits']).resolve():
                raise OSError('manifest absent')
            return original(path, *args, **kwargs)
        with mock.patch.object(self.T, 'publish_immutable', side_effect=fail_manifest):
            self.assertEqual(self.process()['state'], 'recovery_required')
        journal = self.journal()
        journal.update(phase='record_committed', committed_mutation=I._mutation_from_journal(journal).to_data()['digest'])
        I._save(self.state / 'journals' / (self.event_id + '.json'), journal)
        I._history_shared(self.entry).unlink()
        result = self.process()
        self.assertEqual(result['state'], 'needs_primary')
        self.assertIn('no matching committed operation', result['reason'])
        self.assertEqual(len(self.store.capture().commits), 1)

    def test_stale_report_and_source_tampering_do_not_publish(self):
        self.capture()
        before = self.entry.read_bytes()
        Path(I.status(self.event_id, self.entry, self.state)['source_file']).write_text('tampered')
        result = self.process()
        self.assertEqual(result['state'], 'needs_primary')
        self.assertEqual(self.entry.read_bytes(), before)

    def test_batch_api_replays_exact_receipts_and_has_no_virtual_parent_commits(self):
        A = self.A
        original = self.H.Store(self.entry).capture()
        mutation = A.prepare_batch(self.entry, [
            {'kind': 'set', 'id': 'p.input', 'value': 8, 'as_of': '2026-09-17'},
            {'kind': 'add', 'id': 'd.ready', 'body': {'verdict': 'new',
                'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 20'}}, 'as_of': '2026-09-17'}],
            operation='exact-batch', recorded_at='2026-09-17T12:13:14+00:00')
        self.assertEqual(self.H.Store(self.entry).capture().inventory, original.inventory)
        restored = self.T.PreparedMutation.from_bytes(mutation.to_bytes())
        A.commit(self.entry, restored, verify=lambda data: None)
        A.commit(self.entry, restored, verify=lambda data: None)
        self.assertEqual(set(self.H.Store(self.entry).capture().commits), {'bootstrap', 'exact-batch'})


class LegacyCoreReplacement(unittest.TestCase):
    def test_batch_preserves_complete_old_judgments_in_one_archive_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            entry, state = root / 'GROUNDING.yaml', root / 'private-state'
            old = {'verdict': 'old', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 5'},
                   'seen': {'p.input': 1}, 'because': 'original grounds'}
            document = {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
                'sources': {'s.old': {'file': 'old.md', 'read': '2026-09-01'}},
                'known': {'p.input': {'v': 1, 'of': '2026-09-01', 'from': 's.old'}},
                'decisions': {'d.ready': old}}
            entry.write_text(I.P.yaml.safe_dump(document, sort_keys=False))
            envelope = {'event_id': 'legacy-batch', 'source_quote': 'Input is 8, old decision replaced.',
                'date': '2026-09-17', 'record_sha256': I._sha(entry.read_bytes()), 'updates': [
                    {'kind': 'set', 'id': 'p.input', 'value': 8},
                    {'kind': 'add', 'id': 'd.ready', 'body': {'verdict': 'new',
                        'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 20'}}}]}
            event = I.capture(envelope, entry, state, start=False)
            result = I.process(entry, state, event['event_id'])[0]
            self.assertEqual(result['state'], 'applied', result)
            retained = I.P.read_replaced([str(entry)])['d.ready'][0]
            self.assertEqual(retained['seen'], old['seen'])
            self.assertEqual(retained['wrong_if'], old['wrong_if'])
            self.assertEqual(retained['because'], old['because'])
            journal = json.loads((state / 'journals' / (event['event_id'] + '.json')).read_text())
            mutation = I._mutation_from_journal(journal)
            archived = next(f for f in mutation.files if f['role'] == 'replaced')
            self.assertIsNone(archived['before'])
            self.assertEqual(archived['after'], Path(I.P.layout(entry)['replaced']).read_bytes())


if __name__ == '__main__':
    unittest.main()
