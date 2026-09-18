import copy
import unittest
from dataclasses import replace
from unittest import mock

from scripts import history_authoring as A
from scripts import history_contract as C
from scripts import history_hypotheses as HH
from scripts import history_branch as Branch
from scripts import history_prospective as P
from scripts import history_store as H
from scripts import history_transaction as T
from scripts import versions as V
from tests import test_history_store as fixtures
from tests import test_history_branch as branch_fixtures


class ProspectiveHistory(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.Storage()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        claim = fixtures.claim()
        claim['authored']['fields'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        claim['id'] = C.object_identity(claim)
        self.fixture.publish([C.validate_object(claim)], op='bootstrap')
        self.capture = self.fixture.store.capture()

    def test_candidate_is_supplied_and_round_trips_without_store_io(self):
        mutation = A.prepare(self.fixture.entry, {'kind': 'set', 'id': 'p.input', 'value': 2,
                                                   'as_of': '2026-09-18'}, operation='candidate')
        before = copy.deepcopy(self.capture)
        with mock.patch.object(V, '_evaluate', side_effect=AssertionError('evaluator')), \
             mock.patch.object(V, '_cross', side_effect=AssertionError('cross')):
            result = P.snapshot_after(self.capture, mutation)
        self.assertEqual(result.to_data()['context']['read_mode'], 'supplied')
        self.assertEqual(result.to_data()['context']['operation']['phase'], 'prospective')
        self.assertEqual(result.to_data()['context']['operation']['kind'], 'prepared_history')
        self.assertEqual(result.to_data()['context']['operation']['mutation_operation'], 'candidate')
        self.assertEqual(result.snapshot_id, P.Snapshot.from_json(result.to_json()).snapshot_id)
        self.assertEqual(self.capture.entry_bytes, before.entry_bytes)
        self.assertEqual(self.capture.commits, before.commits)
        self.assertEqual(self.capture.object_bytes, before.object_bytes)

    def test_context_cannot_replace_history_evidence(self):
        mutation = A.prepare(self.fixture.entry, {'kind': 'set', 'id': 'p.input', 'value': 2,
                                                   'as_of': '2026-09-18'}, operation='candidate')
        for key in ('history', 'history_hypotheses', 'history_view', 'operation'):
            with self.subTest(key=key), self.assertRaisesRegex(C.HistoryError, 'duplicate_prospective_context'):
                P.snapshot_after(self.capture, mutation, context={key: {}})

    def test_adoption_can_repeat_identical_target_objects(self):
        from scripts import history_bundle as B
        target = self.capture
        head = target.state['subjects']['p.input']['heads'][0]
        mutation = B._prepare_capture_adoption(self.fixture.entry, target, 'a' * 64,
            choices={'p.input': head}, by='reviewer', operation='adopt-retained',
            recorded_at='2026-09-18T00:00:00+00:00', capture=target)
        candidate = P.snapshot_after(target, mutation)
        self.assertEqual(candidate.to_data()['document']['readings']['p.input']['v'], 1)
        self.assertEqual(candidate.to_data()['context']['history']['subjects']['p.input']['heads'], [head])

    def test_named_hypothesis_fold_matches_published_semantics(self):
        proposal = HH.prepare(self.fixture.entry, 'alternative',
                              {'kind': 'set', 'id': 'p.input', 'value': 2,
                               'as_of': '2026-09-18'}, by='writer', operation='proposal')
        HH.commit(self.fixture.entry, proposal, verify=lambda data: None)
        captured = self.fixture.store.capture()
        mutation = HH.prepare_fold(self.fixture.entry, ['alternative'],
                                   because='verified alternative', by='reviewer',
                                   operation='fold')
        candidate = P.snapshot_after(captured, mutation)
        HH.commit(self.fixture.entry, mutation, verify=lambda data: None)
        actual = self.fixture.store.capture()
        published = actual
        from scripts import history_adapter as Adapter
        expected = Adapter.from_store_capture(published).snapshot()
        self.assertEqual(candidate.to_data()['document'], expected.to_data()['document'])
        self.assertEqual(candidate.to_data()['context']['history']['subjects'],
                         expected.to_data()['context']['history']['subjects'])
        self.assertEqual(candidate.to_data()['context']['history']['baseline'],
                         expected.to_data()['context']['history']['baseline'])

    def test_snapshot_after_has_no_store_or_filesystem_io(self):
        mutation = A.prepare(self.fixture.entry, {'kind': 'set', 'id': 'p.input', 'value': 2,
                                                   'as_of': '2026-09-18'}, operation='candidate')
        with (mock.patch.object(H.Store, 'capture', side_effect=AssertionError('store I/O')),
              mock.patch('pathlib.Path.read_bytes', side_effect=AssertionError('file I/O'))):
            result = P.snapshot_after(self.capture, mutation)
        self.assertEqual(result.to_data()['context']['read_mode'], 'supplied')

    def test_tampered_mutation_evidence_and_missing_closure_refuse(self):
        mutation = A.prepare(self.fixture.entry, {'kind': 'set', 'id': 'p.input', 'value': 2,
                                                   'as_of': '2026-09-18'}, operation='candidate')
        data = mutation.to_data()
        original_files = mutation.files
        def rebuilt(value, files=original_files):
            return T.PreparedMutation(operation=value['operation'], authority=value['authority'],
                                      baseline=value['baseline'], files=files,
                                      receipt=value['receipt'], entry=value['entry'])
        tampered = copy.deepcopy(data)
        tampered['baseline']['committed_set_digest'] = '0' * 64
        with self.assertRaises(C.HistoryError):
            P.snapshot_after(self.capture, rebuilt(tampered))
        for role in ('record', 'history_object'):
            altered = copy.deepcopy(data)
            altered_files = copy.deepcopy(original_files)
            item = next(i for i in altered_files if i['role'] == role)
            if role == 'record':
                item['after'] = item['before']
            else:
                item['after'] = item['after'][:-1] + bytes([item['after'][-1] ^ 1])
            with self.subTest(role=role), self.assertRaises(C.HistoryError):
                P.snapshot_after(self.capture, rebuilt(altered, altered_files))
        missing = copy.deepcopy(data)
        missing_files = [i for i in original_files if i['role'] != 'history_object']
        with self.assertRaises(C.HistoryError):
            P.snapshot_after(self.capture, rebuilt(missing, missing_files))

    def test_rehashed_forged_record_and_wrong_parent_frontier_refuse(self):
        mutation = A.prepare(self.fixture.entry, {'kind': 'set', 'id': 'p.input', 'value': 2,
                                                   'as_of': '2026-09-18'}, operation='candidate')
        files = copy.deepcopy(mutation.files)
        record = next(item for item in files if item['role'] == 'record')
        forged = C.decode_document(record['after'])
        forged['readings']['p.input']['v'] = 999
        record['after'] = C.encode_document(forged)
        commit = next(item for item in files if item['role'] == 'history_commit')
        manifest = C.decode_document(commit['after'])
        manifest['view_sha256'] = C.sha256(record['after'])
        commit['after'] = C.encode_document(manifest)
        forged_mutation = T.PreparedMutation(operation=mutation.to_data()['operation'],
            authority=mutation.to_data()['authority'], baseline=mutation.to_data()['baseline'],
            files=files, receipt=mutation.to_data()['receipt'], entry=mutation.to_data()['entry'])
        with self.assertRaisesRegex(C.HistoryError, 'view_mismatch'):
            P.snapshot_after(self.capture, forged_mutation)

        files = copy.deepcopy(mutation.files)
        commit = next(item for item in files if item['role'] == 'history_commit')
        manifest = C.decode_document(commit['after'])
        manifest['parents'] = {}
        commit['after'] = C.encode_document(manifest)
        wrong_parent = T.PreparedMutation(operation=mutation.to_data()['operation'],
            authority=mutation.to_data()['authority'], baseline=mutation.to_data()['baseline'],
            files=files, receipt=mutation.to_data()['receipt'], entry=mutation.to_data()['entry'])
        with self.assertRaisesRegex(C.HistoryError, 'parent_baseline_mismatch|commit_mismatch'):
            P.snapshot_after(self.capture, wrong_parent)

    def test_size_and_inactive_operation_guards_match_publication(self):
        mutation = A.prepare(self.fixture.entry, {'kind': 'set', 'id': 'p.input', 'value': 2,
                                                   'as_of': '2026-09-18'}, operation='candidate')
        with mock.patch.object(H, 'MAX_CAPTURE_BYTES', 1), self.assertRaisesRegex(C.HistoryError, 'history_limit'):
            P.snapshot_after(self.capture, mutation)
        inactive = replace(self.capture, inactive_generations={'2': {'commits': {'candidate': b'x'}}})
        with self.assertRaisesRegex(C.HistoryError, 'operation_collision'):
            P.snapshot_after(inactive, mutation)

    def test_real_branch_adoption_evidence_matches_published_capture(self):
        fixture = branch_fixtures.BranchAdoption()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        envelope = fixture.capture()
        source = Branch.validate(envelope)
        target = fixture.target()
        before = H.Store(target).capture()
        selected = source.state['subjects']['p.value']['head']
        mutation = fixture.prepared(target, envelope, {'p.value': selected})
        source_files = copy.deepcopy(envelope['files'])
        source_capture = copy.deepcopy(before)
        with mock.patch.object(H.Store, 'capture', side_effect=AssertionError('store I/O')), \
             mock.patch('pathlib.Path.read_bytes', side_effect=AssertionError('file I/O')), \
             mock.patch.object(V, '_evaluate', side_effect=AssertionError('evaluator')), \
             mock.patch.object(V, '_cross', side_effect=AssertionError('cross')):
            candidate = P.snapshot_after(before, mutation)
        self.assertEqual(envelope['files'], source_files)
        self.assertEqual(before.entry_bytes, source_capture.entry_bytes)
        self.assertEqual(before.object_bytes, source_capture.object_bytes)
        Branch.commit_adoption(target, mutation, envelope, verify=lambda data: None)
        after = H.Store(target).capture()
        published = __import__('scripts.history_adapter', fromlist=['from_store_capture']).from_store_capture(after).snapshot()
        self.assertEqual(candidate.to_data()['document'], published.to_data()['document'])
        self.assertEqual(candidate.to_data()['context']['history']['subjects'],
                         published.to_data()['context']['history']['subjects'])


if __name__ == '__main__':
    unittest.main()
