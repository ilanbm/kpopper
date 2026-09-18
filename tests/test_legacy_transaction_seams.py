"""Legacy newborn and hypothesis/identity changes publish complete generations."""
import contextlib
import copy
import io
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import consolidate as C, sameness as S
P = C.P
T = P._peer("history_transaction")


class LegacySeams(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.entry = self.root / 'GROUNDING.yaml'
        self.paths = [str(self.entry)]
        self.entry.write_text('sources:\n  s.session: {asked: "Record the evidence", of: "this session"}\nknown:\n  p.value: {v: 1}\n  p.other: {v: 1}\n')
        self.hyp = Path(P.hypothesis_path(self.paths, 'proposal'))
        self.journal = self.root / T.journal_for(self.entry)

    def quiet(self, fn, *args, **kwargs):
        with contextlib.redirect_stdout(io.StringIO()):
            return fn(*args, **kwargs)

    def apply(self, action):
        return self.quiet(P.apply, self.paths, copy.deepcopy(action))

    def hypothesize(self):
        self.apply({'kind': 'add', 'id': 'p.new', 'body': {'v': 2}, 'hypothesis': 'proposal'})

    def fault(self, path):
        original = T._replace
        def fail(target, data):
            if Path(target).resolve() == Path(path).resolve():
                raise OSError('injected publication')
            return original(target, data)
        return mock.patch.object(T, '_replace', side_effect=fail)

    def assert_recover(self):
        mutation = T.PreparedMutation.from_bytes(self.journal.read_bytes())
        with self.assertRaisesRegex((ValueError, P.Refused), 'recovery_required'):
            P.load(self.paths)
        self.quiet(P.recover_direct, self.paths)
        for item in mutation.files:
            path = self.root / item['path']
            self.assertEqual(path.read_bytes() if path.exists() else None, item['after'])
        self.assertFalse(self.journal.exists())
        return mutation

    def test_hypothesis_add_set_review_are_staged_and_recoverable(self):
        base = self.entry.read_bytes()
        with self.fault(self.hyp), self.assertRaisesRegex((OSError, P.Refused), 'injected'):
            self.hypothesize()
        self.assertFalse(self.hyp.exists())
        with self.assertRaisesRegex((ValueError, P.Refused), 'recovery_required'):
            P.load([str(self.hyp)])
        mutation = self.assert_recover()
        self.assertEqual({f['role'] for f in mutation.files}, {'hypothesis'})
        self.assertEqual(self.entry.read_bytes(), base)
        self.apply({'kind': 'set', 'id': 'p.new', 'value': 3, 'hypothesis': 'proposal'})
        self.apply({'kind': 'add', 'id': 'd.new', 'hypothesis': 'proposal', 'body': {
            'verdict': 'ready', 'rests_on': ['p.new'], 'wrong_if': 'p.new > 5'}})
        self.apply({'kind': 'set', 'id': 'p.new', 'value': 4, 'hypothesis': 'proposal'})
        self.apply({'kind': 'review', 'id': 'd.new', 'hypothesis': 'proposal'})
        self.assertEqual(P.load(self.paths).hypotheses['proposal']['doc']['judgments']['d.new']['seen'], {'p.new': 4})
        self.assertEqual(self.entry.read_bytes(), base)

    def test_fold_deletion_and_base_change_share_one_recoverable_generation(self):
        self.hypothesize()
        original = self.hyp.read_bytes()
        with self.fault(self.entry), self.assertRaisesRegex((OSError, P.Refused), 'injected'):
            self.quiet(C.fold, self.paths, ['proposal'])
        self.assertFalse(self.hyp.exists())
        with self.assertRaisesRegex((ValueError, P.Refused), 'recovery_required'):
            P.load([str(self.hyp)])
        mutation = self.assert_recover()
        deleted = next(f for f in mutation.files if f['role'] == 'hypothesis')
        self.assertEqual(deleted['before'], original)
        self.assertIsNone(deleted['after'])
        self.assertEqual(P.bodies(P.load(self.paths))['p.new']['v'], 2)

    def test_fold_rollback_restores_deleted_hypothesis(self):
        self.hypothesize()
        before = {self.entry: self.entry.read_bytes(), self.hyp: self.hyp.read_bytes()}
        with self.fault(self.entry), self.assertRaises((OSError, P.Refused)):
            self.quiet(C.fold, self.paths, ['proposal'])
        self.quiet(P.recover_direct, self.paths, direction='before')
        self.assertEqual({p: p.read_bytes() for p in before}, before)

    def test_refutation_finding_and_delete_commit_together(self):
        self.hypothesize()
        with self.fault(self.entry), self.assertRaisesRegex((OSError, P.Refused), 'injected'):
            self.quiet(C.refute, self.paths, 'proposal', 'Evidence rejects it', source='s.session')
        self.assert_recover()
        self.assertFalse(self.hyp.exists())
        self.assertEqual(P.bodies(P.load(self.paths))['hyp.proposal']['v'], 'refuted')

    def test_same_and_distinct_use_prepared_generation(self):
        for fn, args in [(S.distinct, ('p.value', 'p.other', 'Separate observations')),
                         (S.same, ('p.value', 'p.other'))]:
            with self.subTest(fn=fn.__name__):
                before = self.entry.read_bytes()
                with self.fault(self.entry), self.assertRaisesRegex((OSError, P.Refused), 'injected'):
                    self.quiet(fn, self.paths, *args)
                self.assertEqual(self.entry.read_bytes(), before)
                self.assert_recover()
                if fn is S.distinct:
                    # Restore fixture for independent sameness mutation.
                    self.entry.write_bytes(before)

    def test_unknown_new_hypothesis_blocks_recovery(self):
        self.hypothesize()
        with self.fault(self.entry), self.assertRaises((OSError, P.Refused)):
            self.quiet(C.fold, self.paths, ['proposal'])
        other = self.hyp.with_name('concurrent.yaml')
        other.write_text('known:\n  p.concurrent: {v: 7}\n')
        with self.assertRaisesRegex((ValueError, P.Refused), 'membership changed'):
            self.quiet(P.recover_direct, self.paths)
        self.assertEqual(other.read_text(), 'known:\n  p.concurrent: {v: 7}\n')

    def test_newborn_failed_publication_retains_absence_not_empty_head(self):
        self.entry.unlink()
        def location():
            return {'status': 'found' if self.entry.exists() else 'missing',
                    'record': str(self.entry), 'workspace': str(self.root)}
        with mock.patch.object(P, '_workspace_location', side_effect=location), \
             self.fault(self.entry), self.assertRaisesRegex((OSError, P.Refused), 'injected'):
            self.quiet(P._apply_first_add, {'kind': 'add', 'id': 'p.first', 'body': {'v': 3}})
        self.assertFalse(self.entry.exists())
        mutation = T.PreparedMutation.from_bytes(self.journal.read_bytes())
        self.assertTrue(mutation.to_data()['baseline']['source_absent'])
        self.assertIsNone(next(f['before'] for f in mutation.files if f['role'] == 'record'))
        self.assert_recover()
        self.assertEqual(P.bodies(P.load(self.paths))['p.first']['v'], 3)

    def test_newborn_validation_refusal_creates_no_record(self):
        self.entry.unlink()
        location = {'status': 'missing', 'record': str(self.entry), 'workspace': str(self.root)}
        with mock.patch.object(P, '_workspace_location', return_value=location), self.assertRaises(P.Refused):
            self.quiet(P._apply_first_add, {'kind': 'add', 'id': 'd.bad', 'body': {
                'verdict': 'bad', 'rests_on': ['p.missing'], 'wrong_if': 'p.missing > 0'}})
        self.assertFalse(self.entry.exists())
        self.assertFalse(self.journal.exists())

    def test_schema_does_not_grant_arbitrary_hypothesis_or_absent_member_paths(self):
        authority = T.legacy_authority(self.entry)
        receipt = T.semantic_receipt(profile='ordinary-reader/v1', capabilities={}, before={}, after={})
        for relative in ('elsewhere.yaml', '.kpopper/hypotheses/nested/bad.yaml'):
            with self.subTest(relative=relative), self.assertRaisesRegex(ValueError, 'invalid_hypothesis_members'):
                T.PreparedMutation(operation='invalid', authority=authority, entry=self.entry.name,
                    baseline={'record_members': {self.entry.name: T.C.sha256(self.entry.read_bytes())},
                              'hypothesis_members': {relative: None}}, receipt=receipt,
                    files=[{'role': 'hypothesis', 'path': relative, 'before': None, 'after': b'known: {}\n'}])
        with self.assertRaisesRegex(ValueError, 'invalid_record_members'):
            T.PreparedMutation(operation='invalid', authority=authority, entry=self.entry.name,
                baseline={'source_absent': True, 'record_members': {self.entry.name: None, 'extra.yaml': None}},
                receipt=receipt, files=[{'role': 'record', 'path': self.entry.name, 'before': None, 'after': b'known: {}\n'}])

    def test_scalar_collections_keep_questions_dates_but_never_metadata_nodes(self):
        import datetime
        document = {'meta': {'name': 'record', 'scope': 'private', 'updated': '2026-09-17'},
                    'questions': {'q.open': 'What changed?'},
                    'known': {'p.day': datetime.date(2026, 9, 17)}}
        self.assertEqual(set(P.bodies(document)), {'q.open', 'p.day'})
        self.assertEqual(P.infer(document)[0], {'q.open', 'p.day'})
        self.assertNotIn('meta', P.collections_of(document))


class TransitionBoundary(unittest.TestCase):
    def setUp(self):
        from tests import test_history_store as fixtures
        self.fixture = fixtures.Storage()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.entry = self.fixture.entry.resolve()
        prepared = self.fixture.mutation([fixtures.claim()], op='activate')
        self.T = fixtures.T
        self.C = fixtures.C
        old = self.C.authority(record_id=self.fixture.marker['record_id'], authority='legacy', generation=0)
        marker_path = Path(self.fixture.store.layout['history_authority']).resolve()
        marker_path.unlink()
        before = self.C.encode_document({'readings': {}})
        self.entry.write_bytes(before)
        files = prepared.files
        next(f for f in files if f['role'] == 'record')['before'] = before
        files.append({'path': str(marker_path.relative_to(self.entry.parent)), 'role': 'history_authority',
                      'before': None, 'after': self.C.encode_document(self.fixture.marker)})
        self.mutation = self.T.PreparedMutation(operation='activate', authority=old,
            baseline={'kind': 'history-authority-transition/v1', 'direction': 'activate',
                      'history_baseline': prepared.to_data()['baseline'],
                      'record_members': {self.entry.name: self.C.sha256(before)}},
            files=files, receipt=prepared.to_data()['receipt'], entry=self.entry.name,
            transition={'version': 1, 'after': self.fixture.marker})
        self.journal = self.T.journal_for(self.entry)

    def test_v2_roundtrip_and_legacy_publication_refuse(self):
        self.assertEqual(self.mutation.to_data()['version'], 2)
        self.assertEqual(self.T.PreparedMutation.from_bytes(self.mutation.to_bytes()).to_bytes(), self.mutation.to_bytes())
        with self.assertRaisesRegex(ValueError, 'invalid_authority'):
            self.T.publish_legacy(self.entry.parent, self.journal, self.mutation, verify=lambda data: None)

    def test_interruption_blocks_both_readers_and_rollback_retains_immutable_evidence(self):
        original = self.T._replace
        def fail(path, data):
            if path == self.entry:
                raise OSError('before view')
            return original(path, data)
        with mock.patch.object(self.T, '_replace', side_effect=fail), self.assertRaisesRegex(OSError, 'before view'):
            self.T.publish_transition(self.entry.parent, self.journal, self.mutation, verify=lambda data: None)
        with self.assertRaisesRegex(ValueError, 'recovery_required'):
            with self.T.reader_guard(self.entry.parent, self.journal):
                self.fail('partial transition leaked')
        self.T.recover_transition(self.entry.parent, self.journal, direction='before', verify=lambda data: None)
        for item in self.mutation.files:
            path = self.entry.parent / item['path']
            expected = item['after'] if item['role'].startswith('history_') and item['role'] != 'history_authority' else item['before']
            self.assertEqual(path.read_bytes() if path.exists() else None, expected)

    def test_verifier_mutation_is_rechecked_and_never_published(self):
        unrelated = b'known: {p.concurrent: 1}\n'
        with self.assertRaisesRegex(ValueError, 'concurrent_edit'):
            self.T.publish_transition(self.entry.parent, self.journal, self.mutation,
                verify=lambda data: self.entry.write_bytes(unrelated))
        self.assertEqual(self.entry.read_bytes(), unrelated)
        self.assertFalse((self.entry.parent / self.journal).exists())


if __name__ == '__main__':
    unittest.main()
