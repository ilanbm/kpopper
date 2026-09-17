"""Scoped history is an attested observation, adopted only by explicit target acts."""
import copy
from dataclasses import replace
from pathlib import Path
import unittest
from unittest import mock

from scripts import history_bundle as B, history_contract as C, history_store as H
from scripts import history_authoring as A, history_transaction as T, pending_grounding as G
from scripts import knowledge_views as V
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_bundles as fixture
from tests.test_history_snapshot_capture import claim, act


class Subsets(unittest.TestCase):
    def setUp(self):
        self.source = self.store('source')
        self.target = self.store('target')

    def store(self, name):
        result = fixture.HistoryBundles('test_source_free_replay_retains_exact_bytes_types_and_profile')
        result.setUp()
        self.addCleanup(result.doCleanups)
        return result

    def subset(self, roots=None, **extra):
        return B.prepare_subset(self.source.store.capture(), roots or ['p.input'], scope=fixture.SCOPE,
            shareability='project', operation='subset-observation', recorded_at='2026-09-17T12:00:00Z',
            source_entry='GROUNDING.yaml', **extra)

    def adoption(self, artifact, choices):
        return B.prepare_adoption(self.target.entry, artifact, choices=choices, by='adopter',
            operation='adopt', recorded_at='2026-09-17T13:00:00Z')

    def publish(self, mutation):
        self.target.store.commit(mutation, verify=lambda _: None)

    def test_subset_omits_private_sibling_and_original_manifests_retry_exact(self):
        private = fixture.reading('private.sibling', operation='private', private=True)
        self.source.publish([private], 'private-commit-name')
        artifact = self.subset()
        self.assertEqual(artifact, self.subset())
        text = str(artifact)
        self.assertNotIn('private.sibling', text)
        self.assertNotIn('private-commit-name', text)
        captured = B.validate(artifact)
        self.assertEqual(set(captured.objects), {self.source.first['id']})
        self.assertEqual(captured.object_bytes[('p.input', self.source.first['id'])], C.encode_document(self.source.first))
        projected = B.adapt(artifact)
        self.assertEqual(projected.projection['coverage']['scope'], 'selected')
        self.assertEqual(projected.projection['origin']['source_authority'], self.source.marker)
        self.assertEqual(projected.projection['dispositions']['p.input']['source_state'], 'committed')
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('no replay reads')):
            snapshot = projected.snapshot()
            self.assertEqual(Snapshot.from_json(snapshot.to_json()).snapshot_id, snapshot.snapshot_id)

    def test_dependency_scope_and_full_history_preserved(self):
        old = claim('p.dep', value=1, operation='dep-old', body={'v': 1, 'scope': {'kind': 'external', 'environment': 'API'}})
        new = claim('p.dep', value=2, operation='dep-new', body={'v': 2, 'scope': {'kind': 'external', 'environment': 'API'}})
        correction = act(new, 'correct', operation='dep-correct', over=[old['id']])
        root = claim('p.root', operation='root', kind='judgment', pins={'p.dep': old['id']},
                     body={'verdict': 'holds', 'rests_on': ['p.dep'], 'seen': {'p.dep': 1},
                           'wrong_if': {'op': 'lt', 'args': [{'ref': 'p.dep'}, {'int': 0}]}, 'scope': fixture.SCOPE})
        # Predicate isn't needed to establish the immutable pin dependency.
        root['body'].pop('wrong_if'); root['id'] = C.object_identity(root)
        self.source.publish([old, new, correction, root], 'dependencies')
        artifact = self.subset(['p.root'])
        captured = B.validate(artifact)
        self.assertEqual(set(captured.state['subjects']), {'p.root', 'p.dep'})
        self.assertEqual(B.adapt(artifact).document['readings']['p.dep']['v'], 2)
        self.assertEqual(captured.objects[root['id']]['pins'], {'p.dep': old['id']})
        self.assertEqual(captured.objects[old['id']], old)

    def test_private_retired_dependency_refuses(self):
        dep = fixture.reading('p.dep', operation='private-dep', private=True)
        root = claim('p.root', operation='root', kind='judgment', pins={'p.dep': dep['id']},
                     body={'rests_on': ['p.dep'], 'scope': fixture.SCOPE})
        self.source.publish([dep, root], 'private-dep')
        with self.assertRaisesRegex(ValueError, 'private'):
            self.subset(['p.root'])

    def test_locator_requires_exact_path_hash_disclosure_and_never_fetches(self):
        obj = fixture.reading('p.imported', operation='imported')
        obj['authored']['locator'] = {'path': 'private/shared.yaml', 'sha256': 'a' * 64}
        obj['id'] = C.object_identity(obj)
        self.source.publish([obj], 'imported')
        with self.assertRaisesRegex(ValueError, 'undisclosed_history_locator'):
            self.subset(['p.imported'])
        with self.assertRaisesRegex(ValueError, 'undisclosed_history_locator'):
            self.subset(['p.imported'], disclosed_locators=[{'path': 'private/shared.yaml', 'sha256': 'b' * 64}])
        artifact = self.subset(['p.imported'], disclosed_locators=[{'path': 'private/shared.yaml', 'sha256': 'a' * 64}])
        self.assertEqual(B.validate(artifact).objects[obj['id']], obj)
        self.assertFalse(any('shared.yaml' in path for path in artifact['files']))

    def test_prepared_source_label_and_mutation_digest_survive_snapshot(self):
        mutation = A.prepare(self.source.entry, {'kind': 'add', 'id': 'p.new', 'into': 'readings',
                    'body': {'v': 8, 'scope': fixture.SCOPE}, 'as_of': '2026-09-17'},
                    operation='prepared', recorded_at='2026-09-17T12:00:00Z')
        artifact = self.subset(['p.new'], prepared=mutation)
        projection = B.adapt(artifact).projection
        self.assertEqual(projection['dispositions']['p.new']['source_state'], 'prepared_candidate')
        self.assertEqual(projection['origin']['prepared_digest'], C.sha256(mutation.to_bytes()))
        self.assertNotIn('prepared', self.source.store.capture().commits)
        self.assertEqual(Snapshot.from_json(B.adapt(artifact).snapshot().to_json()).to_data()['context']['history'], projection)

    def test_outer_v3_binds_subset_capability(self):
        artifact = self.subset()
        bundle = G.prepare(B.adapt(artifact).document, ['p.input'], scope=fixture.SCOPE,
                           shareability='project', history=artifact)
        self.assertEqual(bundle['manifest']['requires'], ['history-closure/v1', 'history-subset/v1'])
        G.validate_bundle(bundle)
        bad = copy.deepcopy(artifact)
        bad['manifest']['version'] = 1
        bad['manifest']['requires'] = ['history-closure/v1']
        bad['revision'] = G.identity(bad['manifest'])
        with self.assertRaisesRegex(ValueError, 'subset_capability_required'):
            B.validate(bad)

    def test_adoption_requires_every_overlapping_dependency_choice(self):
        dep = fixture.reading('p.dep', operation='dep', value=3)
        root = claim('p.root', operation='root', kind='judgment', pins={'p.dep': dep['id']},
                     body={'rests_on': ['p.dep'], 'scope': fixture.SCOPE})
        self.source.publish([dep, root], 'deps')
        target_dep = fixture.reading('p.dep', operation='target-dep', value=9)
        self.target.publish([target_dep], 'target-dep')
        artifact = self.subset(['p.root'])
        preview = B.preview_adoption(self.target.store.capture(), artifact)
        self.assertTrue(preview['subjects']['p.dep']['requires_choice'])
        with self.assertRaisesRegex(ValueError, 'adoption_choice_required'):
            self.adoption(artifact, {})
        mutation = self.adoption(artifact, {'p.dep': target_dep['id']})
        self.publish(mutation)
        target = self.target.store.capture()
        self.assertEqual(target.document['readings']['p.dep']['v'], 9)
        self.assertEqual(target.objects[root['id']]['pins'], {'p.dep': dep['id']})
        self.assertTrue(B.adopted_by(target, artifact))

    def test_adoption_receipt_exact_inventory_and_retry_are_required(self):
        artifact = self.subset()
        with self.assertRaisesRegex(ValueError, 'adoption_choice_required'):
            self.adoption(artifact, {})
        mutation = self.adoption(artifact, {'p.input': self.source.first['id']})
        self.assertEqual(mutation.to_bytes(), self.adoption(artifact, {'p.input': self.source.first['id']}).to_bytes())
        self.assertFalse(B.adopted_by(self.target.store.capture(), artifact))
        self.publish(mutation)
        target = self.target.store.capture()
        self.assertTrue(B.adopted_by(target, artifact))
        bundle = G.prepare(B.adapt(artifact).document, ['p.input'], scope=fixture.SCOPE,
                           shareability='project', history=artifact)
        self.assertTrue(G.equivalent(bundle, target.document, {}, history=V.history_evidence(target)))
        self.assertTrue(B.commit_adoption(self.target.entry, T.PreparedMutation.from_bytes(mutation.to_bytes()), artifact, verify=lambda _: None))
        raw = C.decode_document(target.commits['adopt'])
        raw['receipt']['after']['history_adoption']['artifact_revision'] = '0' * 64
        changed = replace(target, commits={**target.commits, 'adopt': C.encode_document(raw)})
        with self.assertRaises(ValueError):
            B.adopted_by(changed, artifact)

    def test_adoption_verifier_rejects_changed_explicit_choice(self):
        artifact = self.subset()
        mutation = self.adoption(artifact, {'p.input': self.source.first['id']})
        self.assertEqual(B.verify_adoption(self.target.entry, mutation, artifact).to_bytes(), mutation.to_bytes())
        B.commit_adoption(self.target.entry, mutation, artifact, verify=lambda _: None)
        self.assertEqual(B.verify_adoption(self.target.entry, mutation, artifact).to_bytes(), mutation.to_bytes())
        data = mutation.to_data()
        bad = copy.deepcopy(artifact)
        bad['manifest']['scope']['environment'] = 'different scope'
        bad['revision'] = G.identity(bad['manifest'])
        with self.assertRaises(ValueError):
            B.verify_adoption(self.target.entry, mutation, bad)

    def test_pin_gap_executable_dependency_keeps_current_history_and_unknown_pin(self):
        dep = fixture.reading('p.dep', operation='dep', value=4)
        root = claim('p.gap', operation='gap', kind='judgment', pins={},
            body={'rests_on': ['p.dep'], 'seen': {'p.dep': 2},
                  'wrong_if': {'expr': 'p.dep > 5'}, 'scope': fixture.SCOPE})
        root['pin_gaps'] = {'p.dep': 'not_recorded'}
        root['id'] = C.object_identity(root)
        self.source.publish([dep, root], 'gap')
        artifact = self.subset(['p.gap'])
        projected = B.adapt(artifact)
        self.assertEqual(projected.document['readings']['p.dep']['v'], 4)
        self.assertEqual(B.validate(artifact).objects[root['id']]['pins'], {})
        self.assertFalse(projected.projection['coverage']['complete'])
        self.assertEqual(projected.projection['coverage']['scope'], 'selected')
        self.assertTrue(any(item['code'] == 'dependency_pin_not_recorded'
                            for item in projected.projection['integrity']['findings']))

    def test_unavailable_seen_subject_refuses_without_receiver_synthesis(self):
        root = claim('p.seen', operation='seen', kind='judgment', pins={},
                     body={'rests_on': [], 'seen': {'missing.subject': 3}, 'scope': fixture.SCOPE})
        self.source.publish([root], 'seen')
        with self.assertRaisesRegex(ValueError, 'incomplete_subset_dependency'):
            self.subset(['p.seen'])

    def test_at_locator_disclosure_and_current_root_scope_required(self):
        obj = fixture.reading('p.other', operation='other', value=2)
        marker = act(obj, 'accept', operation='imported-accept')
        marker['at'] = {'imported_replacement': {'path': 'private/archive.yaml', 'sha256': 'b' * 64}}
        marker['id'] = C.object_identity(marker)
        self.source.publish([obj, marker], 'other')
        with self.assertRaisesRegex(ValueError, 'undisclosed_history_locator'):
            self.subset(['p.other'])
        artifact = self.subset(['p.other'], disclosed_locators=[{'path': 'private/archive.yaml', 'sha256': 'b' * 64}])
        self.assertEqual(B.validate(artifact).objects[marker['id']], marker)
        with self.assertRaisesRegex(ValueError, 'history_scope_mismatch'):
            B.prepare_subset(self.source.store.capture(), ['p.other'], scope={'kind': 'project', 'environment': 'other'},
                shareability='project', operation='other-scope', recorded_at='now', source_entry='GROUNDING.yaml',
                disclosed_locators=[{'path': 'private/archive.yaml', 'sha256': 'b' * 64}])

    def test_projection_cannot_claim_all_or_drop_origin_or_prepared_state(self):
        artifact = self.subset()
        original = B.adapt(artifact).snapshot().to_data()
        from scripts.reasoning import snapshot as S
        for corruption in ('all', 'origin', 'state', 'meta'):
            value = copy.deepcopy(original)
            projection = value['context']['history']
            if corruption == 'all':
                projection['coverage']['scope'] = 'all'
            elif corruption == 'origin':
                projection.pop('origin')
            elif corruption == 'state':
                projection['dispositions']['p.input']['source_state'] = 'prepared_candidate'
            else:
                value['document']['meta'].pop('history_subset')
            value['snapshot_id'] = S.digest(S._snapshot_preimage(value))
            with self.subTest(corruption=corruption), self.assertRaises(ValueError):
                Snapshot.from_snapshot(value)

    def test_adoption_choice_resolves_distinct_values_and_preserves_old_marks(self):
        chosen = fixture.reading(operation='new', value=8)
        self.source.publish([chosen, act(chosen, 'correct', operation='correct', over=[self.source.first['id']])], 'new')
        target = fixture.reading(operation='target-new', value=9)
        self.target.publish([target, act(target, 'correct', operation='target-correct', over=[self.target.first['id']])], 'target-new')
        artifact = self.subset()
        before = B.validate(artifact).objects
        mutation = self.adoption(artifact, {'p.input': chosen['id']})
        self.publish(mutation)
        captured = self.target.store.capture()
        self.assertEqual(captured.document['readings']['p.input']['v'], 8)
        for version, obj in before.items():
            self.assertEqual(captured.objects[version], obj)
        self.assertEqual(captured.state['subjects']['p.input']['marks'][self.source.first['id']], 'corrected')
        resolution = next(obj for obj in captured.objects.values() if obj['op'] == 'adopt')
        self.assertNotIn(self.source.first['id'], resolution['body']['over'])

    def test_metadata_without_inventory_is_not_adoption(self):
        artifact = self.subset()
        mutation = self.adoption(artifact, {'p.input': self.source.first['id']})
        self.publish(mutation)
        target = self.target.store.capture()
        manifest = C.decode_document(target.commits['adopt'])
        manifest['objects'] = [item for item in manifest['objects'] if item['id'] != self.source.first['id']]
        changed = replace(target, commits={**target.commits, 'adopt': C.encode_document(manifest)})
        self.assertFalse(B.adopted_by(changed, artifact))

    def test_subset_materialization_and_advanced_overlay_keep_selected_origin(self):
        from scripts import project_modes as M
        from tests import test_pending_grounding as R
        repo = R.Repository('run')
        repo.setUp()
        self.addCleanup(repo.doCleanups)
        entry = repo.root / 'GROUNDING.yaml'
        entry.write_text('known: {local.value: {v: 0}}')
        artifact = self.subset()
        bundle = G.prepare(B.adapt(artifact).document, ['p.input'], scope=fixture.SCOPE,
                           shareability='project', history=artifact)
        repo.store._capture(bundle, event_id='subset', contribution_id='subset', shareability='project')
        snapshot = Snapshot.capture(entry, read_mode='live')
        projection = snapshot.to_data()['context']['history_contributions'][bundle['revision']]['projection']
        self.assertEqual(projection['coverage']['scope'], 'selected')
        self.assertEqual(projection['origin'], B.adapt(artifact).projection['origin'])
        destination = repo.base / 'subset-materialized'
        V.materialize(repo.root, bundle['revision'], destination)
        frozen = Snapshot.capture(destination / 'GROUNDING.yaml', read_mode='frozen')
        self.assertEqual(frozen.to_data()['context']['history'], B.adapt(artifact).projection)
        self.assertEqual(Snapshot.from_json(frozen.to_json()).snapshot_id, frozen.snapshot_id)

    def test_collection_scope_includes_complete_current_members_and_privacy(self):
        one = fixture.reading('p.one', operation='one')
        scope = claim('p.scope', operation='scope', body={
            'collection_scope': {'collection': 'readings', 'fields': ['v']}, 'scope': fixture.SCOPE})
        self.source.publish([one, scope], 'scope')
        artifact = self.subset(['p.scope'])
        self.assertEqual(set(B.validate(artifact).state['subjects']), {'p.scope', 'p.one', 'p.input'})
        private = fixture.reading('p.private', operation='private', private=True)
        self.source.publish([private], 'private')
        with self.assertRaisesRegex(ValueError, 'private'):
            self.subset(['p.scope'])

    def test_publisher_accepts_only_committed_explicit_subset_adoption(self):
        from tests import test_pending_publication as F
        from tests import test_history_advanced as Advanced
        pub = F.PublicationTests('run')
        pub.setUp()
        self.addCleanup(pub.doCleanups)
        artifact = self.subset()
        bundle = G.prepare(B.adapt(artifact).document, ['p.input'], scope=fixture.SCOPE,
                           shareability='project', history=artifact)
        pub.store._capture(bundle, event_id='subset', contribution_id='subset', shareability='project')
        Advanced.HistoryPublication.seed_target(pub, self.target)
        pending = pub.publisher.run(force_retry=True)
        self.assertNotEqual(pending['states'].get(bundle['revision']), 'accepted')
        self.assertEqual(pub.provider.creates, 0)
        mutation = self.adoption(artifact, {'p.input': self.source.first['id']})
        B.commit_adoption(self.target.entry, mutation, artifact, verify=lambda _: None)
        Advanced.HistoryPublication.seed_target(pub, self.target)
        result = pub.publisher.run(force_retry=True)
        self.assertEqual(result['states'][bundle['revision']], 'accepted', result)
        self.assertEqual(pub.publisher.verify_obligations()['unresolved'], [])
        self.assertEqual(pub.provider.creates, 0)

    def test_new_history_membership_refuses_before_pending_ledger_blob_write(self):
        from tests import test_pending_grounding as R
        repo = R.Repository('run')
        repo.setUp()
        self.addCleanup(repo.doCleanups)
        captured = self.source.store.capture()
        artifact = self.subset()
        bundle = G.prepare(B.adapt(artifact).document, ['p.input'], scope=fixture.SCOPE,
                           shareability='project', history=artifact)
        def verify_source():
            C._require(self.source.store.capture().inventory == captured.inventory, 'source_changed')
        # A new manifest becomes visible, while the old generated entry survives.
        original = self.source.entry.read_bytes()
        self.source.publish([act(self.source.first, 'review', operation='review')], 'review')
        self.source.entry.write_bytes(original)
        with mock.patch.object(repo.store, '_write_blob', side_effect=AssertionError('must not write')):
            with self.assertRaisesRegex(ValueError, 'source_changed'):
                repo.store.capture(bundle, event_id='race', contribution_id='race',
                                   shareability='project', verify_source=verify_source)
        self.assertIsNone(repo.store.head())
        with mock.patch.object(repo.store, '_write_blob', side_effect=AssertionError('must not write')):
            with self.assertRaisesRegex(ValueError, 'source changed'):
                repo.store._capture(bundle, event_id='false', contribution_id='false',
                                    shareability='project', verify_source=lambda: False)
