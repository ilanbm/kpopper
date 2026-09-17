"""Independent Advanced histories survive live capture and source-free replay."""
import copy
import json
from pathlib import Path
import shutil
import unittest
from unittest import mock

from scripts import history_bundle as B, history_contract as C, history_store as H
from scripts import pending_grounding as G, knowledge_views as V, project_modes as M
from scripts.reasoning import snapshot as S
from tests import test_history_bundles as fixture
from tests import test_pending_grounding as repositories
from tests.test_history_snapshot_capture import TEMPLATE, act


class HistoryAdvanced(unittest.TestCase):
    def setUp(self):
        repositories.Repository.setUp(self)
        self.record = self.root / 'GROUNDING.yaml'
        self.record.write_text('known:\n  p.input: {v: checkout}\n')
        M.git(self.root, 'add', '.')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Record')
        self.base_commit = M.git(self.root, 'rev-parse', 'HEAD').stdout.decode().strip()
        self.source = self.new_source('pending-authority')

    def new_source(self, record_id):
        source = fixture.HistoryBundles('test_source_free_replay_retains_exact_bytes_types_and_profile')
        source.setUp()
        self.addCleanup(source.doCleanups)
        # Build this authority at its own bootstrap; do not relabel existing acts.
        shutil.rmtree(Path(source.store.layout['history']))
        shutil.rmtree(Path(source.store.layout['history_commits']))
        source.marker = C.authority(record_id=record_id, authority='history', generation=1)
        source.marker_path.write_bytes(C.encode_document(source.marker))
        document = copy.deepcopy(TEMPLATE)
        document['meta']['history'] = H.baseline(source.marker, {}, H.reduce({}))
        source.entry.write_bytes(C.encode_document(document))
        source.publish([source.first], 'initial')
        return source

    def pending(self, source=None, event='history', evidence=None):
        bundle = (source or self.source).bundle(evidence=evidence)
        self.store._capture(bundle, event_id=event, contribution_id=event, shareability='project')
        return bundle

    def snapshot(self, mode='live'):
        return S.Snapshot.capture(self.record, read_mode=mode, as_of='2026-09-17')

    def configure_target(self):
        project = M.Project(self.root)
        publication = {'remote': 'origin', 'repository': 'https://example.test/project.git',
                       'target': 'trunk', 'branch': 'pending', 'standing_permission': True}
        config = {**project.config(), 'publication': publication, 'generation': 1}
        project.state.mkdir(parents=True, exist_ok=True)
        project.config_path.write_text(json.dumps(config))

    def target(self, source=None, *, corrupt=False):
        source = source or self.source
        shutil.copytree(source.root, self.root, dirs_exist_ok=True)
        if corrupt:
            next(Path(H.Store(self.record).layout['history']).rglob('*.yaml')).unlink()
        M.git(self.root, 'add', '.')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Target history')
        revision = M.git(self.root, 'rev-parse', 'HEAD').stdout.decode().strip()
        M.git(self.root, 'update-ref', 'refs/remotes/origin/trunk', revision)
        M.git(self.root, 'reset', '--hard', self.base_commit)
        self.configure_target()
        return revision

    def test_pending_is_independent_hypothesis_and_frozen_replay_has_no_reads(self):
        bundle = self.pending()
        snapshot = self.snapshot()
        data = snapshot.to_data()
        revision = bundle['revision']
        self.assertEqual(data['document']['known']['p.input']['v'], 'checkout')
        self.assertNotIn('history', data['context'])
        self.assertEqual(data['context']['history_contributions'][revision]['projection']['authority']['record_id'],
                         'pending-authority')
        self.assertEqual(data['hypotheses']['pending-' + revision]['document'], bundle['manifest']['document'])
        self.source.temp.cleanup()
        with mock.patch.object(M, 'git', side_effect=AssertionError('Git replay read')), \
             mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('filesystem replay read')):
            self.assertEqual(S.Snapshot.from_json(snapshot.to_json()).snapshot_id, snapshot.snapshot_id)
        self.assertNotIn('history_contributions', self.snapshot('frozen').to_data()['context'])

    def test_active_checkout_and_unrelated_pending_authority_never_union(self):
        base = self.new_source('checkout-authority')
        shutil.copytree(base.root, self.root, dirs_exist_ok=True)
        bundle = self.pending()
        snapshot = self.snapshot().to_data()
        self.assertEqual(snapshot['context']['history']['authority']['record_id'], 'checkout-authority')
        self.assertEqual(snapshot['context']['history_contributions'][bundle['revision']]['projection']['authority']['record_id'],
                         'pending-authority')
        self.assertEqual(set(snapshot['context']['history']['subjects']), {'p.input'})
        self.assertIn('p.input', snapshot['context']['conflicts'])

    def test_rehashed_snapshot_cannot_drop_bytes_projection_or_hypothesis_binding(self):
        bundle = self.pending()
        original = self.snapshot().to_data()
        for corruption in ('bytes', 'projection', 'hypothesis', 'witness'):
            data = copy.deepcopy(original)
            if corruption == 'bytes':
                files = data['context']['pending']['bundles'][bundle['revision']]['files']
                del files[next(p for p in files if '/objects/' in p)]
            elif corruption == 'projection':
                data['context']['history_contributions'][bundle['revision']]['projection']['authority']['record_id'] = 'other'
            elif corruption == 'hypothesis':
                data['hypotheses']['pending-' + bundle['revision']]['document']['readings']['p.input']['v'] = 99
            else:
                del data['context']['history_contributions']
            data['snapshot_id'] = S.digest(S._snapshot_preimage(data))
            with self.subTest(corruption=corruption), self.assertRaises(ValueError):
                S.Snapshot.from_snapshot(data)

    def test_corrupt_pending_bytes_refuse_live_capture(self):
        self.pending()
        pending = self.store.snapshot()
        bundle = next(iter(pending['bundles'].values()))
        path = next(path for path in bundle['files'] if '/objects/' in path)
        bundle['files'][path] += b'\n'
        with mock.patch.object(G.Store, 'snapshot', return_value=pending), self.assertRaises(ValueError):
            self.snapshot()

    def test_missing_and_corrupt_targets_are_explicitly_unavailable(self):
        self.pending()
        self.configure_target()
        first = self.snapshot().to_data()['context']['target']
        self.assertEqual(first['status'], 'unavailable')
        self.assertIsNone(first['revision'])
        revision = self.target(corrupt=True)
        second = self.snapshot().to_data()['context']['target']
        self.assertEqual(second['status'], 'unavailable')
        self.assertEqual(second['revision'], revision)
        self.assertNotIn('snapshot', second)
        self.assertIn('incomplete_commit', second['reason'])

    def test_committed_target_captures_full_history_and_replays_after_branch_moves(self):
        revision = self.target()
        snapshot = self.snapshot()
        data = snapshot.to_data()
        target = data['context']['target']
        self.assertEqual(target['status'], 'observed', target.get('reason'))
        self.assertEqual(target['revision'], revision)
        self.assertEqual(target['snapshot']['history']['projection']['authority']['record_id'], 'pending-authority')
        self.assertEqual(data['document']['known']['p.input']['v'], 'checkout')
        M.git(self.root, 'update-ref', 'refs/remotes/origin/trunk', self.base_commit)
        later = self.snapshot()
        self.assertNotEqual(snapshot.snapshot_id, later.snapshot_id)
        with mock.patch.object(M, 'git', side_effect=AssertionError('Git replay read')):
            self.assertEqual(S.Snapshot.from_json(snapshot.to_json()).snapshot_id, snapshot.snapshot_id)
        tampered = copy.deepcopy(data)
        target_history = tampered['context']['target']['snapshot']['history']
        target_history['projection']['subjects']['p.input']['acceptance'] = 'refuted'
        tampered['snapshot_id'] = S.digest(S._snapshot_preimage(tampered))
        with self.assertRaises(ValueError):
            S.Snapshot.from_snapshot(tampered)

    def test_materialized_history_is_complete_and_survives_source_removal(self):
        bundle = self.pending()
        original = B.adapt(B.from_contribution(bundle))
        destination = self.base / 'materialized'
        result = V.materialize(self.root, bundle['revision'], destination)
        self.source.temp.cleanup()
        actual = S.Snapshot.capture(result['record'], read_mode='frozen').to_data()
        self.assertEqual(actual['document'], original.document)
        self.assertEqual(actual['context']['history'], original.projection)
        captured = H.Store(result['record']).capture()
        for operation, raw in B.validate(B.from_contribution(bundle)).commits.items():
            self.assertEqual(captured.commits[operation], raw)
        self.assertEqual((destination / 'history-closure/entry.yaml').read_bytes(),
                         bundle['files']['history-closure/entry.yaml'])
        metadata = json.loads((destination / 'snapshot.json').read_bytes())
        manifest = G._decode(metadata['contribution'])
        restored = {'revision': metadata['revision'], 'manifest': manifest,
                    'files': {path: (destination / path).read_bytes() for path in manifest['evidence']}}
        G.validate_bundle(restored)
        self.assertEqual(restored, bundle)
        with self.assertRaises(ValueError):
            V.materialize(self.root, bundle['revision'], destination)

    def test_materialized_stale_observation_is_regenerated_without_inventing_acts(self):
        original = self.source.entry.read_bytes()
        newer = fixture.reading(value=7, operation='newer')
        self.source.publish([newer, act(newer, 'correct', operation='correct', over=[self.source.first['id']])], 'newer')
        self.source.entry.write_bytes(original)
        bundle = self.pending()
        result = V.materialize(self.root, bundle['revision'], self.base / 'stale-copy')
        captured = H.Store(result['record']).capture()
        self.assertEqual(captured.document['readings']['p.input']['v'], 7)
        self.assertEqual(set(captured.commits), {'initial', 'newer'})
        self.assertEqual((Path(result['record']).parent / 'history-closure/entry.yaml').read_bytes(), original)

    def test_materialization_authority_collision_refuses_without_destination(self):
        newer = fixture.reading(value=7, operation='file', file='.kpopper/history.yaml')
        self.source.publish([newer], 'file')
        bundle = self.pending(evidence={'.kpopper/history.yaml': b'unrelated bytes'})
        destination = self.base / 'collision'
        with self.assertRaisesRegex(ValueError, 'conflicts with history'):
            V.materialize(self.root, bundle['revision'], destination)
        self.assertFalse(destination.exists())


class HistoryPublication(unittest.TestCase):
    from tests import test_pending_publication as publication_fixture
    setUp = publication_fixture.PublicationTests.setUp
    remote_head = publication_fixture.PublicationTests.remote_head
    merge = publication_fixture.PublicationTests.merge
    new_source = HistoryAdvanced.new_source

    def seed_target(self, source):
        shutil.copytree(source.root, self.root, dirs_exist_ok=True)
        M.git(self.root, 'add', '.')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Activate fixture authority')
        M.git(self.root, 'push', 'team', 'trunk')

    def queue(self, source):
        bundle = source.bundle()
        self.store._capture(bundle, event_id='history', contribution_id='history', shareability='project')
        return bundle

    def test_same_authority_proposal_union_and_squash_acceptance_retain_exact_history(self):
        source = self.new_source('shared-authority')
        self.seed_target(source)
        initial_raw = source.store.capture().commits['initial']
        newer = fixture.reading(value=12, operation='newer')
        source.publish([newer, act(newer, 'correct', operation='correct', over=[source.first['id']])], 'newer')
        bundle = self.queue(source)
        before = (self.root / 'GROUNDING.yaml').read_bytes()
        proposed = self.publisher.run(force_retry=True)
        self.assertEqual(proposed['outcome'], 'proposed', proposed)
        self.assertEqual((self.root / 'GROUNDING.yaml').read_bytes(), before)
        head = self.remote_head()
        self.assertEqual(M.git(self.remote, 'show', head + ':.kpopper/history-commits/initial.yaml').stdout, initial_raw)
        self.assertEqual(M.git(self.remote, 'show', head + ':app.txt').stdout, b'target code\n')
        self.assertTrue(M.git(self.remote, 'show', head + ':.kpopper-contributions/' + bundle['revision'] + '/manifest.json').stdout)
        self.merge('squash')
        result = self.publisher.run(force_retry=True)
        self.assertEqual(result['states'][bundle['revision']], 'accepted', result)
        self.assertEqual(self.publisher.verify_obligations()['unresolved'], [])

    def test_other_authority_and_legacy_target_never_accept_equal_yaml(self):
        source = self.new_source('pending-authority')
        bundle = self.queue(source)
        result = self.publisher.run(force_retry=True)
        self.assertEqual(result['outcome'], 'attention')
        self.assertIn('migration', result['detail'])
        target = self.new_source('target-authority')
        self.seed_target(target)
        result = self.publisher.run(force_retry=True)
        self.assertEqual(result['outcome'], 'attention')
        self.assertIn('different history authority', result['detail'])
        self.assertNotEqual(result['states'].get(bundle['revision']), 'accepted')
        self.assertEqual(self.provider.creates, 0)

    def test_bundle_artifact_retention_alone_does_not_prove_acceptance(self):
        source = self.new_source('pending-authority')
        bundle = self.queue(source)
        directory = self.root / '.kpopper-contributions' / bundle['revision']
        directory.mkdir(parents=True)
        (directory / 'manifest.json').write_bytes(G.json_bytes(G._encode(bundle['manifest'])))
        for name, raw in bundle['files'].items():
            path = directory / 'evidence' / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)
        M.git(self.root, 'add', '.')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Retain artifact only')
        M.git(self.root, 'push', 'team', 'trunk')
        result = self.publisher.run(force_retry=True)
        self.assertNotEqual(result['states'].get(bundle['revision']), 'accepted')
        self.assertEqual(result['outcome'], 'attention')
