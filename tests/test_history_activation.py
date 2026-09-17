"""Disposable authority transitions; no real target or installed-runtime claim."""
import contextlib
import copy
from pathlib import Path
import sys
import unittest
from unittest import mock

from scripts import history_activation as A, history_migration as M, history_runtime as R
from scripts import history_contract as C, history_transaction as T, history_store as H
from scripts import history_authoring as W, provenance as P, project_modes as Modes
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_migration as migration_fixtures


class Activation(unittest.TestCase):
    def setUp(self):
        fixture = migration_fixtures.Migration()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture, self.root = fixture, fixture.root
        self.entry = fixture.fixture(core=True)
        self.original = self.entry.read_bytes()
        root = Path(A.__file__).resolve().parent
        self.inventory = [{'id': 'fixture-source', 'argv': [str(Path(sys.executable).resolve()), '-B', str(root / 'kpopper')],
                           'package_root': str(root), 'executable': str(Path(sys.executable).resolve())}]
        declaration = R.describe('fixture_declaration_nonce_0123456789')
        self.expected = {'fixture-source': {key: declaration[key]['digest'] for key in ('sources', 'schemas', 'native')}}
        self.guarded = False
        self.guard_entries = 0

    @contextlib.contextmanager
    def guard(self, inventory, expected):
        # Disposable fixture owns its only writer; this models the externally
        # supplied managed-deployment exclusion, never a runtime attestation.
        self.assertFalse(self.guarded)
        self.assertEqual(inventory, self.inventory)
        self.assertEqual(expected, self.expected)
        self.guarded = True
        self.guard_entries += 1
        try:
            yield
        finally:
            self.guarded = False

    def prepare(self, **kwargs):
        return A.prepare_activation(self.entry, inventory=self.inventory, expected_digests=self.expected,
                                    deployment_guard=self.guard, **kwargs)

    def publish(self, mutation):
        return A.publish(self.entry, mutation, deployment_guard=self.guard)

    def deactivate(self, activation):
        return A.prepare_deactivation(self.entry, activation, inventory=self.inventory,
            expected_digests=self.expected, deployment_guard=self.guard)

    def test_activation_and_clean_deactivation_keep_immutable_history_and_original_bytes(self):
        mutation = self.prepare()
        self.assertEqual(self.entry.read_bytes(), self.original)
        self.assertFalse(Path(P.layout(self.entry)['history_authority']).exists())
        restored = T.PreparedMutation.from_bytes(mutation.to_bytes())
        self.assertEqual(restored.to_bytes(), mutation.to_bytes())
        self.assertEqual(self.publish(restored)['state'], 'activated')
        captured = H.Store(self.entry).capture()
        self.assertEqual(captured.marker['authority'], 'history')
        snapshot = Snapshot.capture([str(self.entry)], read_mode='frozen')
        self.assertEqual(snapshot.snapshot_id, Snapshot.from_json(snapshot.to_json()).snapshot_id)
        immutable = {path: A._tree(self.entry.parent, path) for path in A._trees(self.entry)}
        inverse = self.deactivate(restored)
        self.assertEqual(self.publish(inverse)['state'], 'deactivated')
        self.assertEqual(self.entry.read_bytes(), self.original)
        marker = C.decode_document(Path(P.layout(self.entry)['history_authority']).read_bytes())
        self.assertEqual((marker['authority'], marker['generation']), ('legacy', 2))
        self.assertEqual(immutable, {path: A._tree(self.entry.parent, path) for path in A._trees(self.entry)})
        self.assertEqual(M._entry_identity(Snapshot.capture([str(self.entry)], read_mode='frozen').to_data()['document']),
                         M._entry_identity(C.decode_document(self.original)))
        self.assertEqual(self.guard_entries, 4)

    def test_captured_import_seed_reconstructs_identical_plan(self):
        one = M.prepare(self.entry, operation='captured-operation', recorded_at='2026-09-17T12:00:00Z', record_id='captured-record')
        two = M.prepare(self.entry, operation='captured-operation', recorded_at='2026-09-17T12:00:00Z', record_id='captured-record')
        self.assertEqual(one.marker, two.marker)
        self.assertEqual(one.files, two.files)

    def test_interrupted_activation_reader_refuses_then_exact_recovery_finishes(self):
        mutation = self.prepare()
        target = self.entry.resolve()
        replace = T._replace
        def crash(path, data):
            self.assertTrue(self.guarded)
            if Path(path) == target:
                raise OSError('entry publication interrupted')
            return replace(path, data)
        with mock.patch.object(T, '_replace', side_effect=crash):
            with self.assertRaisesRegex(OSError, 'entry publication interrupted'):
                self.publish(mutation)
        with self.assertRaisesRegex((C.HistoryError, P.Refused, ValueError), 'recovery_required'):
            Snapshot.capture([str(self.entry)], read_mode='frozen')
        raw = T._read(T._target(self.entry.parent, T.journal_for(self.entry)))
        self.assertEqual(raw, mutation.to_bytes())
        result = A.recover(self.entry, deployment_guard=self.guard)
        self.assertEqual(result['operation'], mutation.to_data()['operation'])
        self.assertEqual(set(H.Store(self.entry).capture().commits), {mutation.to_data()['operation']})

    def test_cancel_interrupted_activation_retains_staged_history_inactive(self):
        mutation = self.prepare()
        replace = T._replace
        def crash(path, data):
            if Path(path) == self.entry:
                raise OSError('cancel fixture')
            return replace(path, data)
        with mock.patch.object(T, '_replace', side_effect=crash):
            with self.assertRaisesRegex(OSError, 'cancel fixture'):
                self.publish(mutation)
        before = {path: A._tree(self.entry.parent, path) for path in A._trees(self.entry)}
        self.assertTrue(any(before.values()))
        A.recover(self.entry, deployment_guard=self.guard, direction='before')
        self.assertEqual(self.entry.read_bytes(), self.original)
        self.assertEqual(before, {path: A._tree(self.entry.parent, path) for path in A._trees(self.entry)})
        marker = Path(P.layout(self.entry)['history_authority'])
        self.assertTrue(not marker.exists() or C.decode_document(marker.read_bytes())['authority'] == 'legacy')
        Snapshot.capture([str(self.entry)], read_mode='frozen')

    def test_changed_archive_hypothesis_or_runtime_proof_refuses_before_publication(self):
        mutation = self.prepare()
        archive = Path(P.layout(self.entry)['replaced'])
        archive.write_bytes(archive.read_bytes() + b'# concurrent archive\n')
        with self.assertRaisesRegex(C.HistoryError, 'transition_source_changed'):
            self.publish(mutation)
        self.assertFalse(Path(P.layout(self.entry)['history_authority']).exists())
        self.assertEqual(self.entry.read_bytes(), self.original)

    def test_bad_deployment_digest_and_missing_guard_refuse(self):
        bad = copy.deepcopy(self.expected)
        bad['fixture-source']['sources'] = '0' * 64
        with self.assertRaisesRegex(C.HistoryError, 'deployment_guard_required'):
            A.prepare_activation(self.entry, inventory=self.inventory, expected_digests=self.expected, deployment_guard=None)
        @contextlib.contextmanager
        def guard(*args):
            yield
        with self.assertRaisesRegex(R.RuntimeDeclarationError, 'runtime_digest_mismatch'):
            A.prepare_activation(self.entry, inventory=self.inventory, expected_digests=bad, deployment_guard=guard)
        self.assertFalse(Path(P.layout(self.entry)['history_authority']).exists())
        self.assertEqual(self.entry.read_bytes(), self.original)

    def test_newer_history_blocks_old_yaml_rollback_without_deleting_evidence(self):
        mutation = self.prepare()
        self.publish(mutation)
        write = W.prepare(self.entry, {'kind': 'set', 'id': 'p.input', 'value': 4, 'as_of': '2026-09-17'})
        W.commit(self.entry, write, verify=lambda data: None)
        before = H.Store(self.entry).capture()
        with self.assertRaisesRegex(C.HistoryError, 'transition_history_changed|newer_history_not_representable'):
            self.deactivate(mutation)
        self.assertEqual(before.inventory, H.Store(self.entry).capture().inventory)

    def test_changed_project_config_blocks_prepared_transition(self):
        mutation = self.prepare()
        project = Modes.Project(self.entry.parent)
        config = project.config()
        config['generation'] += 1
        import json
        project.config_path.write_text(json.dumps(config))
        with self.assertRaisesRegex(C.HistoryError, 'transition_project_changed'):
            self.publish(mutation)
        self.assertEqual(self.entry.read_bytes(), self.original)

    def test_malformed_pending_and_distinct_worktree_authorities_refuse(self):
        project = Modes.Project(self.entry.parent)
        with mock.patch.object(A, '_pending', return_value={'ledger': {'ref': 'new', 'events': [{'pending': True}], 'bundles': {}}, 'publication': None}):
            with self.assertRaisesRegex(C.HistoryError, 'invalid_transition_pending'):
                self.prepare()
        other = self.root / 'other-worktree'
        other.mkdir()
        with mock.patch.object(Modes.Project, 'worktrees', return_value=[project.root, other]):
            with self.assertRaisesRegex(C.HistoryError, 'history_group_transition_required'):
                self.prepare()
        self.assertEqual(self.entry.read_bytes(), self.original)

    def test_interrupted_deactivation_recovers_without_rewriting_history(self):
        activation = self.prepare()
        self.publish(activation)
        inverse = self.deactivate(activation)
        immutable = {path: A._tree(self.entry.parent, path) for path in A._trees(self.entry)}
        replace = T._replace
        def crash(path, data):
            if Path(path) == self.entry:
                raise OSError('inverse interruption')
            return replace(path, data)
        with mock.patch.object(T, '_replace', side_effect=crash):
            with self.assertRaisesRegex(OSError, 'inverse interruption'):
                self.publish(inverse)
        with self.assertRaisesRegex(ValueError, 'recovery_required'):
            Snapshot.capture([str(self.entry)], read_mode='frozen')
        A.recover(self.entry, deployment_guard=self.guard)
        self.assertEqual(self.entry.read_bytes(), self.original)
        self.assertEqual(immutable, {path: A._tree(self.entry.parent, path) for path in A._trees(self.entry)})

    def test_hypothesis_addition_after_preparation_refuses_and_probe_stays_guarded(self):
        real_probe = R.probe_launchers
        def checked(*args, **kwargs):
            self.assertTrue(self.guarded)
            return real_probe(*args, **kwargs)
        with mock.patch.object(R, 'probe_launchers', side_effect=checked):
            activation = self.prepare()
        path = Path(P.layout(self.entry)['hypotheses']) / 'new.yaml'
        path.parent.mkdir(parents=True)
        path.write_text('hypothesis: {claim: new}\nknown: {p.new: {v: 2}}\n')
        with self.assertRaisesRegex(C.HistoryError, 'transition_source_changed'):
            self.publish(activation)
        self.assertEqual(self.entry.read_bytes(), self.original)

    def test_nested_member_reader_is_guarded_during_interrupted_transition(self):
        self.entry = self.fixture.fixture(shape='pointer', name='PROVENANCE.yaml', core=True)
        self.original = self.entry.read_bytes()
        member = self.entry.parent / 'data/values.yaml'
        activation = self.prepare()
        replace = T._replace
        def crash(path, data):
            if Path(path) == self.entry:
                raise OSError('nested interruption')
            return replace(path, data)
        with mock.patch.object(T, '_replace', side_effect=crash):
            with self.assertRaisesRegex(OSError, 'nested interruption'):
                self.publish(activation)
        with self.assertRaisesRegex((P.Refused, ValueError), 'recovery_required'):
            Snapshot.capture([str(member)], read_mode='frozen')
        A.recover(self.entry, deployment_guard=self.guard)
        Snapshot.capture([str(self.entry)], read_mode='frozen')

    def test_nested_retained_member_path_survives_activation_and_inverse(self):
        self.entry = self.fixture.fixture(shape='pointer', name='custom.yml', core=True)
        self.original = self.entry.read_bytes()
        member = self.entry.parent / 'data/values.yaml'
        original_member = member.read_bytes()
        activation = self.prepare()
        self.publish(activation)
        self.assertEqual(member.read_bytes(), original_member)
        Snapshot.capture([str(self.entry)], read_mode='frozen')
        inverse = self.deactivate(activation)
        self.publish(inverse)
        self.assertEqual(self.entry.read_bytes(), self.original)
        self.assertEqual(member.read_bytes(), original_member)

    def test_every_selected_launcher_must_declare_transition_schema(self):
        legacy = {'declaration': {'schemas': {'history': {'prepared_mutation': [1]}}}}
        current = {'declaration': {'schemas': {'history': {'prepared_mutation': [1, 2]}}}}
        for launchers in ([legacy], [current, legacy], [current, {'declaration': {}}]):
            with mock.patch.object(R, 'probe_launchers', return_value={'complete': True, 'launchers': launchers}):
                with self.assertRaisesRegex(C.HistoryError, 'history_transition_runtime_unsupported'):
                    self.prepare()
        self.assertEqual(self.entry.read_bytes(), self.original)
        self.assertFalse(Path(P.layout(self.entry)['history_authority']).exists())

    def test_hypothesis_and_original_member_bytes_survive_same_path_transition(self):
        self.entry = self.fixture.fixture(shape='hypothesis', name='PROVENANCE.yaml', core=True)
        self.original = self.entry.read_bytes()
        hypothesis = Path(P.layout(self.entry)['hypotheses']) / 'alternative.yaml'
        original_hypothesis = hypothesis.read_bytes()
        mutation = self.prepare()
        self.publish(mutation)
        self.assertEqual(hypothesis.read_bytes(), original_hypothesis)
        inverse = self.deactivate(mutation)
        self.publish(inverse)
        self.assertEqual(self.entry.read_bytes(), self.original)
        self.assertEqual(hypothesis.read_bytes(), original_hypothesis)


class PendingActivation(unittest.TestCase):
    guard = Activation.guard
    prepare = Activation.prepare
    publish = Activation.publish
    deactivate = Activation.deactivate

    def setUp(self):
        Activation.setUp(self)
        # This lane exercises pending preservation, not executable-source
        # attestation. The original Activation tests retain real launcher probes.
        patch = mock.patch.object(A, '_probe', return_value={'complete': True, 'launchers': []})
        patch.start()
        self.addCleanup(patch.stop)
        Modes.git(self.entry.parent, 'init', '-b', 'trunk')
        Modes.git(self.entry.parent, 'config', 'user.name', 'Fixture')
        Modes.git(self.entry.parent, 'config', 'user.email', 'fixture@example.test')
        Modes.git(self.entry.parent, 'add', '.')
        Modes.git(self.entry.parent, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Record')
        self.project = Modes.Project(self.entry.parent)
        self.pending = A.G.Store(self.project)
        from tests.test_pending_grounding import fixture_bundle
        self.bundle = fixture_bundle()
        self.pending._capture(self.bundle, event_id='pending-first', contribution_id='pending-first', shareability='project')

    def observed(self):
        return Snapshot.capture(self.entry, read_mode='live')

    def test_nonempty_pending_is_preserved_across_activation_and_exact_inverse(self):
        before = self.observed()
        ledger = self.pending.snapshot()
        mutation = self.prepare()
        self.assertIn('live_observation', mutation.to_data()['baseline'])
        self.assertTrue(any(item['path'] == M.ARTIFACTS + '/live.json' for item in mutation.files))
        self.publish(mutation)
        after = self.observed()
        self.assertEqual(self.pending.snapshot(), ledger)
        A._same_live_context(after, before)
        self.assertNotIn('api.limit', A.G.entries(after.to_data()['document']))
        self.assertIn('pending-' + self.bundle['revision'], after.to_data()['hypotheses'])
        inverse = self.deactivate(mutation)
        self.publish(inverse)
        self.assertEqual(self.entry.read_bytes(), self.original)
        self.assertEqual(self.pending.snapshot(), ledger)
        A._same_live_context(self.observed(), before)

    def test_new_pending_prevents_activation_and_newer_pending_prevents_inverse(self):
        from tests.test_pending_grounding import fixture_bundle
        mutation = self.prepare()
        self.pending._capture(fixture_bundle(value=11), event_id='second', contribution_id='second', shareability='project')
        with self.assertRaisesRegex(ValueError, 'transition_project_changed'):
            self.publish(mutation)
        self.assertEqual(self.entry.read_bytes(), self.original)
        activation = self.prepare()
        self.publish(activation)
        self.pending._capture(fixture_bundle(value=12), event_id='third', contribution_id='third', shareability='project')
        before = self.pending.snapshot()
        with self.assertRaisesRegex(ValueError, 'transition_project_changed'):
            self.deactivate(activation)
        self.assertEqual(self.pending.snapshot(), before)

    def test_interrupted_transition_recovery_keeps_pending_exact_and_context_replayable(self):
        mutation = self.prepare()
        expected = self.pending.snapshot()
        replace = T._replace
        def crash(path, raw):
            if Path(path) == self.entry:
                raise OSError('pending activation interruption')
            return replace(path, raw)
        with mock.patch.object(T, '_replace', side_effect=crash):
            with self.assertRaisesRegex(OSError, 'pending activation interruption'):
                self.publish(mutation)
        self.assertEqual(self.pending.snapshot(), expected)
        A.recover(self.entry, deployment_guard=self.guard)
        self.assertEqual(self.pending.snapshot(), expected)
        live = self.observed()
        self.assertEqual(Snapshot.from_json(live.to_json()).snapshot_id, live.snapshot_id)

    def test_publication_decision_or_target_movement_invalidates_preparation(self):
        from scripts.pending_publication import Publisher
        mutation = self.prepare()
        Publisher(self.project).action('withdraw', revisions=[self.bundle['revision']], reason='fixture decision')
        with self.assertRaisesRegex(ValueError, 'transition_project_changed'):
            self.publish(mutation)
        # Retired obligations remain preserved rather than silently removed.
        retired = self.prepare()
        self.publish(retired)
        self.assertEqual(self.pending.head(), mutation.to_data()['baseline']['project']['pending']['ledger']['ref'])
        self.assertNotIn('pending-' + self.bundle['revision'], self.observed().to_data()['hypotheses'])
        with self.assertRaisesRegex(ValueError, 'transition_project_changed'):
            self.deactivate(mutation)

    def test_v3_history_pending_remains_independent_and_unaccepted(self):
        from tests import test_history_bundles as F
        source = F.HistoryBundles('test_source_free_replay_retains_exact_bytes_types_and_profile')
        source.setUp()
        self.addCleanup(source.doCleanups)
        bundle = source.bundle()
        self.pending._capture(bundle, event_id='v3', contribution_id='v3', shareability='project')
        before = self.observed()
        activation = self.prepare()
        self.publish(activation)
        actual = self.observed()
        A._same_live_context(actual, before)
        history = actual.to_data()['context']['history_contributions'][bundle['revision']]['projection']
        self.assertEqual(history['authority']['record_id'], source.marker['record_id'])
        self.assertNotEqual(history['authority'], actual.to_data()['context']['history']['authority'])
        self.assertEqual(len(self.pending.events()), 2)

    def test_target_ref_change_after_preparation_refuses_without_authority_write(self):
        import json
        config = self.project.config()
        config['publication'] = {'remote': 'origin', 'repository': 'https://example.test/fixture.git',
                                 'target': 'trunk', 'branch': 'pending', 'standing_permission': False}
        self.project.config_path.write_text(json.dumps(config))
        activation = self.prepare()
        target = Modes.git(self.entry.parent, 'rev-parse', 'HEAD').stdout.decode().strip()
        Modes.git(self.entry.parent, 'update-ref', 'refs/remotes/origin/trunk', target)
        with self.assertRaisesRegex(ValueError, 'transition_project_changed'):
            self.publish(activation)
        self.assertFalse(Path(P.layout(self.entry)['history_authority']).exists())
        self.assertEqual(self.entry.read_bytes(), self.original)

    def test_pending_bytes_and_capabilities_are_rechecked_before_activation(self):
        snapshot = self.pending.snapshot()
        bundle = snapshot['bundles'][self.bundle['revision']]
        bundle['files']['evidence/vendor.txt'] += b'changed'
        with mock.patch.object(A.G.Store, 'snapshot', return_value=snapshot), self.assertRaises(ValueError):
            self.prepare()
        self.assertFalse(Path(P.layout(self.entry)['history_authority']).exists())
        self.assertEqual(self.entry.read_bytes(), self.original)



if __name__ == '__main__':
    unittest.main()
