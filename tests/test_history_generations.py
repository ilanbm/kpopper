"""Exact marker generations select computation while all older bytes remain audit evidence."""
import copy
from dataclasses import replace
from pathlib import Path
import shutil
import unittest
from unittest import mock

from scripts import history_activation as X, history_migration as M, history_bundle as B
from scripts import history_contract as C, history_store as H, history_transaction as T
from scripts import knowledge_views as V, pending_grounding as G, provenance as P
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_activation as fixture
from tests.test_history_snapshot_capture import TEMPLATE
from tests.test_history_bundles import SCOPE


class Generations(unittest.TestCase):
    guard = fixture.Activation.guard
    prepare = fixture.Activation.prepare
    publish = fixture.Activation.publish
    deactivate = fixture.Activation.deactivate

    def setUp(self):
        fixture.Activation.setUp(self)
        patch = mock.patch.object(X, '_probe', return_value={'complete': True, 'launchers': []})
        patch.start()
        self.addCleanup(patch.stop)
        Path(P.layout(self.entry)['replaced']).unlink()
        self.set_legacy(1)
        self.original = self.entry.read_bytes()

    def set_legacy(self, value):
        document = copy.deepcopy(TEMPLATE)
        document['readings']['p.input'] = {'v': value, 'scope': SCOPE}
        self.entry.write_bytes(C.encode_document(document))

    def next_generation(self):
        first = self.prepare(operation='activate-one')
        self.publish(first)
        initial = H.Store(self.entry).capture()
        self.publish(self.deactivate(first))
        self.set_legacy(2)
        second = self.prepare(operation='activate-three')
        self.publish(second)
        return first, initial, second, H.Store(self.entry).capture()

    def test_repeated_activation_selects_exact_marker_and_retains_every_prior_byte(self):
        first, old, second, current = self.next_generation()
        self.assertEqual(current.marker['generation'], 3)
        self.assertEqual(set(current.commits), {'activate-three'})
        self.assertEqual(current.document['readings']['p.input']['v'], 2)
        self.assertEqual(current.inactive_generations['1']['commits'], old.commits)
        self.assertEqual(current.inactive_generations['1']['objects'], old.objects)
        self.assertNotEqual(set(old.objects), set(current.objects))
        self.publish(self.deactivate(second))
        self.set_legacy(3)
        third = self.prepare(operation='activate-five')
        self.publish(third)
        newest = H.Store(self.entry).capture()
        self.assertEqual(newest.marker['generation'], 5)
        self.assertEqual(set(newest.inactive_generations), {'1', '3'})
        self.assertEqual(newest.document['readings']['p.input']['v'], 3)
        for obj, raw in old.object_bytes.items():
            self.assertEqual(newest.object_bytes[obj], raw)
        snapshot = Snapshot.capture(self.entry, read_mode='frozen')
        self.assertEqual(snapshot.to_data()['context']['history']['authority']['generation'], 5)
        self.assertEqual(Snapshot.from_json(snapshot.to_json()).snapshot_id, snapshot.snapshot_id)

    def test_old_future_foreign_and_missing_generation_evidence_refuse(self):
        _, old, _, current = self.next_generation()
        store = H.Store(self.entry)
        old_path = Path(store.layout['history_commits']) / 'activate-one.yaml'
        raw = old_path.read_bytes()
        for change in ('foreign', 'future', 'missing-object'):
            altered = C.decode_document(raw)
            if change == 'foreign':
                altered['record_id'] = 'foreign'
            elif change == 'future':
                altered['authority_generation'] = 7
            else:
                altered['objects'][0]['sha256'] = '0' * 64
            old_path.write_bytes(C.encode_document(altered))
            with self.subTest(change=change), self.assertRaises(ValueError):
                store.capture()
            old_path.write_bytes(raw)
        marker = Path(store.layout['history_authority'])
        current_marker = marker.read_bytes()
        marker.write_bytes(C.encode_document(C.authority(record_id=current.marker['record_id'], authority='history', generation=1)))
        self.entry.write_bytes(old.entry_bytes)
        with self.assertRaisesRegex(ValueError, 'future_history_generation'):
            store.capture()
        marker.write_bytes(current_marker)
        self.entry.write_bytes(current.entry_bytes)

    def test_full_artifact_has_explicit_retained_capability_and_subset_omission(self):
        _, old, _, current = self.next_generation()
        artifact = B.export(current, roots=['p.input'], scope=SCOPE, shareability='project')
        self.assertEqual(artifact['manifest']['version'], 3)
        self.assertEqual(artifact['manifest']['requires'], ['history-closure/v1', 'history-generations/v1', 'subject-paths/v2'])
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('offline replay')):
            restored = B.validate(artifact)
            self.assertEqual(restored.inactive_generations['1']['commits'], old.commits)
            self.assertEqual(B.adapt(artifact).document['readings']['p.input']['v'], 2)
        downgraded = copy.deepcopy(artifact)
        downgraded['manifest']['version'] = 1
        downgraded['manifest']['requires'] = ['history-closure/v1']
        downgraded['manifest'].pop('inactive_generations')
        downgraded['revision'] = G.identity(downgraded['manifest'])
        with self.assertRaisesRegex(ValueError, 'history_generation_capability_required'):
            B.validate(downgraded)
        observation = V.history_evidence(current)
        self.assertEqual(observation['version'], 2)
        self.assertEqual(V.validate_history_evidence(observation).inactive_generations['1']['commits'], old.commits)
        # Imported locators require explicit disclosure; this is source metadata
        # consent only and never an implicit read of archived original files.
        disclosed = []
        subset = B.prepare_subset(current, ['p.input'], scope=SCOPE, shareability='project',
            operation='selected', recorded_at='now', source_entry=self.entry.name, disclosed_locators=disclosed)
        self.assertEqual(subset['manifest']['version'], 2)
        self.assertFalse(B.validate(subset).inactive_generations)
        self.assertEqual(B.adapt(subset).projection['origin']['inactive_audit']['status'], 'not_transferred')

    def test_reactivated_copied_import_and_standalone_inverse_keep_inactive_generations(self):
        first = self.prepare(operation='activate-one')
        self.publish(first)
        self.publish(self.deactivate(first))
        self.set_legacy(4)
        plan = M.prepare(self.entry, operation='copy-reactivation', recorded_at='now')
        self.assertEqual(plan.artifacts, M.ARTIFACTS + '/generations/3')
        original = dict(plan.source_files)
        copied = self.root / 'copied-reactivation'
        result = plan.publish(copied)
        self.assertEqual(set(H.Store(result['record']).capture().inactive_generations), {'1'})
        self.assertEqual(M.replay_from_copy(copied).snapshot_id, plan.candidate.snapshot_id)
        restored = self.root / 'restored-inactive'
        inverse = M.restore_from_copy(copied, restored)
        for path, raw in original.items():
            self.assertEqual((restored / Path(path).relative_to(self.entry.parent)).read_bytes(), raw)
        self.assertEqual(C.decode_document(Path(P.layout(inverse['record'])['history_authority']).read_bytes())['authority'], 'legacy')

    def test_same_object_identity_can_be_retained_in_two_generations(self):
        _, old, _, current = self.next_generation()
        commits = dict(old.commits)
        original_commit = C.decode_document(next(iter(old.commits.values())))
        migrated = copy.deepcopy(original_commit)
        migrated['operation'] = 'same-objects'
        migrated['authority_generation'] = 3
        commits['same-objects'] = C.encode_document(migrated)
        groups = C.committed_generations(current.marker, commits, old.object_bytes)
        self.assertEqual(groups['1']['objects'], groups['3']['objects'])
        self.assertEqual(groups['1']['object_bytes'], groups['3']['object_bytes'])

    def test_interrupted_reactivation_replays_without_mutating_older_generation(self):
        first = self.prepare(operation='activate-one')
        self.publish(first)
        old = H.Store(self.entry).capture()
        self.publish(self.deactivate(first))
        self.set_legacy(6)
        next_mutation = self.prepare(operation='activate-three')
        real = T._replace
        def interrupt(path, raw):
            if Path(path) == self.entry:
                raise OSError('new generation interruption')
            return real(path, raw)
        with mock.patch.object(T, '_replace', side_effect=interrupt), self.assertRaisesRegex(OSError, 'new generation interruption'):
            self.publish(next_mutation)
        X.recover(self.entry, deployment_guard=self.guard)
        current = H.Store(self.entry).capture()
        self.assertEqual(current.document['readings']['p.input']['v'], 6)
        self.assertEqual(current.inactive_generations['1']['commits'], old.commits)

    def test_crash_after_new_object_manifest_or_marker_keeps_older_bytes_and_recovers(self):
        for phase in ('object', 'manifest', 'marker'):
            child = Generations('run')
            child.setUp()
            self.addCleanup(child.doCleanups)
            first = child.prepare(operation='one')
            child.publish(first)
            old = H.Store(child.entry).capture()
            child.publish(child.deactivate(first))
            child.set_legacy(11)
            mutation = child.prepare(operation='three')
            immutable, replace_file = T.publish_immutable, T._replace
            fired = [False]
            def publish_file(path, raw, **kwargs):
                result = immutable(path, raw, **kwargs)
                hit = (phase == 'object' and str(Path(H.Store(child.entry).layout['history'])) in str(path)) or (
                    phase == 'manifest' and str(path).endswith('/three.yaml'))
                if hit and not fired[0]:
                    fired[0] = True
                    raise OSError('generation phase interrupted')
                return result
            def replace(path, raw):
                result = replace_file(path, raw)
                if phase == 'marker' and str(path) == H.Store(child.entry).layout['history_authority'] and not fired[0]:
                    fired[0] = True
                    raise OSError('generation phase interrupted')
                return result
            with mock.patch.object(T, 'publish_immutable', side_effect=publish_file), \
                 mock.patch.object(T, '_replace', side_effect=replace):
                with self.subTest(phase=phase), self.assertRaisesRegex(OSError, 'generation phase interrupted'):
                    child.publish(mutation)
            X.recover(child.entry, deployment_guard=child.guard)
            current = H.Store(child.entry).capture()
            self.assertEqual(current.marker['generation'], 3)
            self.assertEqual(current.inactive_generations['1']['commits'], old.commits)
            self.assertEqual(current.document['readings']['p.input']['v'], 11)

    def test_runtime_declares_supported_generation_and_disposition_wire_versions(self):
        from scripts import history_runtime as R
        schemas = R._schemas(Path(R.__file__).resolve().parent)
        self.assertEqual(schemas['history']['bundle'], [1, 2, 3])
        self.assertIn('history-generations/v1', schemas['history']['bundle_capabilities'])
        self.assertIn('explicit-root-disposition/v1', schemas['history']['commit_capabilities'])
        self.assertIn('propose', schemas['history']['act_kinds'])
        self.assertIn('retire', schemas['history']['act_kinds'])
        self.assertEqual(schemas['history']['group_transition'], [1])
        self.assertIn('history_group_activation.py', R.REQUIRED)

    def test_full_generation_contribution_materializes_every_retained_committed_byte(self):
        from tests import test_pending_grounding as R
        _, old, _, current = self.next_generation()
        artifact = B.export(current, roots=['p.input'], scope=SCOPE, shareability='project')
        document = B.adapt(artifact).document
        evidence = {item['path']: (self.entry.parent / item['path']).read_bytes()
                    for item in document['meta']['history_import']['members']}
        bundle = G.prepare(document, ['p.input'], scope=SCOPE, shareability='project', history=artifact, evidence=evidence)
        G.validate_bundle(bundle)
        repo = R.Repository('run')
        repo.setUp()
        self.addCleanup(repo.doCleanups)
        repo.store._capture(bundle, event_id='generation', contribution_id='generation', shareability='project')
        result = V.materialize(repo.root, bundle['revision'], repo.base / 'generations-copy')
        copied = H.Store(result['record']).capture()
        self.assertEqual(copied.inactive_generations['1']['commits'], old.commits)
        for key, raw in current.object_bytes.items():
            self.assertEqual(copied.object_bytes[key], raw)
        self.assertTrue(G.equivalent(bundle, copied.document, evidence, history=V.history_evidence(copied)))

    def test_retained_artifact_corruption_blocks_reactivation_without_deleting_it(self):
        first = self.prepare(operation='one')
        self.publish(first)
        self.publish(self.deactivate(first))
        archived = self.entry.parent / M.ARTIFACTS / 'original.json'
        archived.write_bytes(archived.read_bytes() + b'corrupt')
        with self.assertRaisesRegex(ValueError, 'retained_artifact_mismatch'):
            self.prepare(operation='three')
        self.assertTrue(archived.read_bytes().endswith(b'corrupt'))

    def test_old_generation_operation_collision_refuses_before_new_objects(self):
        _, old, _, current = self.next_generation()
        from scripts import history_authoring as W
        mutation = W.prepare(self.entry, {'kind': 'set', 'id': 'p.input', 'value': 9},
                             operation='activate-one', recorded_at='now')
        before = H.Store(self.entry).capture()
        with self.assertRaisesRegex(ValueError, 'operation_collision'):
            H.Store(self.entry).commit(mutation, verify=lambda _: None)
        self.assertEqual(H.Store(self.entry).capture().inventory, before.inventory)

    def test_new_generation_originals_alias_prior_immutable_storage_without_recursive_copy(self):
        first = self.prepare(operation='one')
        self.publish(first)
        old = H.Store(self.entry).capture()
        self.publish(self.deactivate(first))
        self.set_legacy(2)
        plan = M.prepare(self.entry, operation='copy-three', recorded_at='now')
        self.assertEqual(plan.manifest['version'], 2)
        for name, original in plan.manifest['originals'].items():
            if M._immutable_original(name, self.entry.name, plan.artifacts):
                self.assertEqual(original['storage'], 'retained')
                self.assertEqual(original['path'], name)
                self.assertNotIn(plan.artifacts + '/originals/' + name, plan.files)
            else:
                self.assertEqual(original['storage'], 'copy')
                self.assertEqual(original['path'], plan.artifacts + '/originals/' + name)
        destination = self.root / 'shared-storage-copy'
        plan.publish(destination)
        stored = M._copy_receipt((destination / plan.artifacts / 'receipt.json').read_bytes())
        self.assertEqual(stored['originals'], plan.manifest['originals'])
        original_bytes = dict(plan.source_files)
        restored = self.root / 'shared-storage-restore'
        M.restore_from_copy(destination, restored)
        for path, raw in original_bytes.items():
            self.assertEqual((restored / Path(path).relative_to(self.entry.parent)).read_bytes(), raw)
        marker = P.layout(self.entry)['history_authority']
        receipt = copy.deepcopy(stored)
        receipt['originals'][self.entry.name] = {'path': self.entry.name, 'storage': 'retained', 'sha256': '0' * 64}
        with self.assertRaisesRegex(ValueError, 'invalid_original_mapping'):
            M._copy_receipt(M._json(receipt))

    def cancelled_generation(self, phase='manifest'):
        first = self.prepare(operation='one')
        self.publish(first)
        self.publish(self.deactivate(first))
        self.set_legacy(42)
        original = self.entry.read_bytes()
        mutation = self.prepare(operation='cancelled-three')
        publish_file, replace_file = T.publish_immutable, T._replace
        fired = [False]
        def immutable(path, raw, **kwargs):
            result = publish_file(path, raw, **kwargs)
            hit = (phase == 'object' and str(Path(H.Store(self.entry).layout['history'])) + '/' in str(path)) or (
                phase == 'manifest' and str(path).endswith('/cancelled-three.yaml'))
            if hit and not fired[0]:
                fired[0] = True
                raise OSError('cancel fixture')
            return result
        def mutable(path, raw):
            result = replace_file(path, raw)
            if phase == 'marker' and str(path) == H.Store(self.entry).layout['history_authority'] and not fired[0]:
                fired[0] = True
                raise OSError('cancel fixture')
            return result
        with mock.patch.object(T, 'publish_immutable', side_effect=immutable), \
             mock.patch.object(T, '_replace', side_effect=mutable), self.assertRaisesRegex(OSError, 'cancel fixture'):
            self.publish(mutation)
        return mutation, original

    def test_cancel_after_object_manifest_or_marker_then_fresh_activation(self):
        for phase in ('object', 'manifest', 'marker'):
            child = Generations('run')
            child.setUp()
            self.addCleanup(child.doCleanups)
            mutation, original = child.cancelled_generation(phase)
            plan = X.cancellation_plan(child.entry, mutation)
            result = X.recover(child.entry, deployment_guard=child.guard, direction='before')
            self.assertEqual(result['cancelled_generation'], 3)
            self.assertEqual(result['authority']['authority'], 'legacy')
            self.assertEqual(result['authority']['generation'], 4)
            self.assertEqual(result['cancellation_receipt'], plan['receipt_path'])
            self.assertEqual(child.entry.read_bytes(), original)
            marker = C.decode_document(Path(P.layout(child.entry)['history_authority']).read_bytes())
            self.assertEqual((marker['version'], marker['authority'], marker['generation']), (2, 'legacy', 4))
            self.assertEqual(set(marker['cancellations']), {'3'})
            self.assertEqual((child.entry.parent / plan['receipt_path']).read_bytes(), plan['receipt_bytes'])
            child.set_legacy(99)
            fresh = child.prepare(operation='five')
            child.publish(fresh)
            captured = H.Store(child.entry).capture()
            self.assertEqual(captured.marker['generation'], 5)
            self.assertEqual(captured.document['readings']['p.input']['v'], 99)
            self.assertEqual(captured.inactive_generations['3']['disposition'], 'cancelled')
            cancelled = captured.inactive_generations['3']['objects']
            self.assertFalse(set(cancelled) & set(captured.objects))
            self.assertEqual(captured.cancellation_bytes['cancelled-three.yaml'], plan['receipt_bytes'])

    def test_cancel_compensation_resumes_after_legacy_marker_before_guard_clear(self):
        mutation, original = self.cancelled_generation()
        plan = X.cancellation_plan(self.entry, mutation)
        real = T._replace
        def fail(path, raw):
            result = real(path, raw)
            if raw == plan['marker_bytes']:
                raise OSError('compensation interrupted')
            return result
        with mock.patch.object(T, '_replace', side_effect=fail), self.assertRaisesRegex(OSError, 'compensation interrupted'):
            X.recover(self.entry, deployment_guard=self.guard, direction='before')
        self.assertTrue((self.entry.parent / T.journal_for(self.entry)).exists())
        with self.assertRaisesRegex(ValueError, 'generation_cancellation_recovery_required'):
            X.recover(self.entry, deployment_guard=self.guard, direction='after')
        X.recover(self.entry, deployment_guard=self.guard, direction='before')
        self.assertEqual(self.entry.read_bytes(), original)
        self.assertFalse((self.entry.parent / T.journal_for(self.entry)).exists())

    def test_cancel_proof_tampering_and_stripped_marker_cannot_lose_classification(self):
        mutation, original = self.cancelled_generation()
        plan = X.cancellation_plan(self.entry, mutation)
        X.recover(self.entry, deployment_guard=self.guard, direction='before')
        receipt_path = self.entry.parent / plan['receipt_path']
        receipt_path.write_bytes(plan['receipt_bytes'] + b'# changed')
        with self.assertRaisesRegex(ValueError, 'cancellation_bytes_mismatch'):
            self.prepare(operation='five')
        receipt_path.write_bytes(plan['receipt_bytes'])
        marker_path = Path(P.layout(self.entry)['history_authority'])
        marker = C.decode_document(marker_path.read_bytes())
        marker_path.write_bytes(C.encode_document(C.authority(record_id=marker['record_id'], authority='legacy', generation=4)))
        with self.assertRaisesRegex(ValueError, 'cancellation_membership_mismatch'):
            self.prepare(operation='five')

    def test_cancelled_audit_full_artifact_preserves_exact_marker_and_receipt(self):
        mutation, original = self.cancelled_generation()
        X.recover(self.entry, deployment_guard=self.guard, direction='before')
        self.set_legacy(7)
        self.publish(self.prepare(operation='five'))
        captured = H.Store(self.entry).capture()
        artifact = B.export(captured, roots=['p.input'], scope=SCOPE, shareability='project')
        self.assertIn(C.CANCELLATION_CAPABILITY, artifact['manifest']['requires'])
        restored = B.validate(artifact)
        self.assertEqual(restored.authority_bytes, captured.authority_bytes)
        self.assertEqual(restored.cancellation_bytes, captured.cancellation_bytes)
        self.assertEqual(restored.inactive_generations['3']['disposition'], 'cancelled')
        stripped = copy.deepcopy(artifact)
        marker = restored.marker
        stripped['files']['authority.yaml'] = C.encode_document(C.authority(
            record_id=marker['record_id'], authority='history', generation=marker['generation']))
        stripped['files'].pop('cancellations/cancelled-three.yaml')
        stripped['manifest']['requires'].remove(C.CANCELLATION_CAPABILITY)
        stripped['manifest']['files'] = {path: C.sha256(raw) for path, raw in stripped['files'].items()}
        stripped['revision'] = G.identity(stripped['manifest'])
        with self.assertRaisesRegex(ValueError, 'authority_digest_mismatch'):
            B.validate(stripped)
        bad = copy.deepcopy(artifact)
        del bad['files']['cancellations/cancelled-three.yaml']
        bad['manifest']['files'].pop('cancellations/cancelled-three.yaml')
        bad['revision'] = G.identity(bad['manifest'])
        with self.assertRaisesRegex(ValueError, 'cancellation_membership_mismatch'):
            B.validate(bad)
