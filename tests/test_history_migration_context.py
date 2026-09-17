"""Copied external source topology and sealed Advanced observations are independent."""
import copy
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest import mock

from scripts import history_migration as M, history_contract as C, provenance as P
from scripts import pending_grounding as G, project_modes as Modes, history_store as H
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_bundles as history_fixture
from tests import test_pending_grounding as pending_fixture


class MigrationContext(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()

    def external(self):
        entry = self.root / 'source' / 'project' / 'GROUNDING.yaml'
        entry.parent.mkdir(parents=True)
        shared = self.root / 'source' / 'shared'
        shared.mkdir()
        (shared / 'parts').mkdir()
        entry.write_bytes(b'# exact entry\r\nrecord: ../shared/member.yaml\r\n')
        (shared / 'member.yaml').write_bytes(b'# exact member\nrecord: parts/values.yaml\nknown:\n  p.first: {v: 1}\n')
        (shared / 'parts' / 'values.yaml').write_bytes(b'# exact nested\nknown:\n  p.second: {v: 2}\n')
        (shared / 'private-unreferenced.yaml').write_bytes(b'private: true\n')
        return entry

    def test_external_nested_topology_exact_source_free_inverse(self):
        entry = self.external()
        plan = M.prepare(entry, operation='import-external', recorded_at='now', record_id='external-record')
        originals = {Path(path).relative_to(entry.parent.parent).as_posix(): raw for path, raw in plan.source_files.items()}
        self.assertNotIn('shared/private-unreferenced.yaml', originals)
        self.assertEqual(plan.manifest['topology']['entry'], 'project/GROUNDING.yaml')
        copied = self.root / 'copied'
        result = plan.publish(copied)
        captured = H.Store(result['record']).capture()
        self.assertTrue(captured.document['record'].startswith('_external/'))
        self.assertNotIn('..', Path(captured.document['record']).parts)
        self.assertEqual(set(G.entries(captured.document)), {'p.first', 'p.second'})
        for path, raw in plan.source_files.items():
            self.assertEqual((copied / M.ARTIFACTS / 'originals' / plan.mapping[path]).read_bytes(), raw)
        shutil.rmtree(entry.parent.parent)
        restored = self.root / 'restored'
        inverse = M.restore_from_copy(copied, restored)
        self.assertEqual(inverse['record'], str(restored / 'project/GROUNDING.yaml'))
        actual = {p.relative_to(restored).as_posix(): p.read_bytes() for p in restored.rglob('*') if p.is_file()}
        self.assertEqual(actual, originals)
        self.assertEqual(set(G.entries(Snapshot.capture(inverse['record'], read_mode='frozen').to_data()['document'])),
                         {'p.first', 'p.second'})
        self.assertEqual(M.replay_from_copy(copied).snapshot_id, plan.candidate.snapshot_id)

    def test_external_source_edit_and_topology_tamper_refuse(self):
        entry = self.external()
        plan = M.prepare(entry)
        member = entry.parent.parent / 'shared/parts/values.yaml'
        original = member.read_bytes()
        member.write_bytes(original + b'# changed\n')
        with self.assertRaisesRegex(ValueError, 'snapshot_changed'):
            plan.publish(self.root / 'changed')
        self.assertFalse((self.root / 'changed').exists())
        member.write_bytes(original)
        copied = self.root / 'copy'
        plan.publish(copied)
        receipt_path = copied / M.ARTIFACTS / 'receipt.json'
        receipt = M._copy_receipt(receipt_path.read_bytes())
        receipt['topology']['entry'] = 'elsewhere/GROUNDING.yaml'
        receipt['topology']['originals']['GROUNDING.yaml'] = receipt['topology']['entry']
        receipt_path.write_bytes(M._json(receipt))
        with self.assertRaisesRegex(ValueError, 'invalid_original_mapping'):
            M.restore_from_copy(copied, self.root / 'bad-restore')
        self.assertFalse((self.root / 'bad-restore').exists())

    def repository(self):
        repo = pending_fixture.Repository('run')
        repo.setUp()
        self.addCleanup(repo.doCleanups)
        entry = repo.root / 'GROUNDING.yaml'
        entry.write_text('known: {local.value: {v: 1}}\n')
        Modes.git(repo.root, 'add', '.')
        Modes.git(repo.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Record')
        project = Modes.Project(repo.root)
        publication = {'remote': 'origin', 'repository': 'https://private-token@example.test/repo.git',
                       'target': 'trunk', 'branch': 'pending', 'standing_permission': True}
        project.state.mkdir(parents=True, exist_ok=True)
        project.config_path.write_text(json.dumps({**project.config(), 'publication': publication, 'generation': 1}))
        return repo, entry

    def test_live_pending_target_observation_copy_and_offline_replay_without_adoption(self):
        repo, entry = self.repository()
        full = pending_fixture.fixture_bundle()
        repo.store._capture(full, event_id='legacy', contribution_id='legacy', shareability='project')
        history = history_fixture.HistoryBundles('test_source_free_replay_retains_exact_bytes_types_and_profile')
        history.setUp()
        self.addCleanup(history.doCleanups)
        bundle = history.bundle()
        repo.store._capture(bundle, event_id='history', contribution_id='history', shareability='project')
        # Target remains missing: this uncertainty must survive copied replay.
        plan = M.prepare(entry, read_mode='live', operation='import-live', recorded_at='now')
        self.assertEqual(set(plan.objects[obj]['subject'] for obj in plan.objects), {'local.value'})
        self.assertEqual(plan.candidate.to_data()['context']['target']['status'], 'unavailable')
        original_ref = repo.store.head()
        copied = self.root / 'advanced-copy'
        result = plan.publish(copied)
        ordinary = Snapshot.capture(result['record'], read_mode='frozen').to_data()
        self.assertEqual(ordinary['context']['pending']['bundles'], {})
        self.assertFalse(ordinary['hypotheses'])
        self.assertEqual(set(G.entries(ordinary['document'])), {'local.value'})
        self.assertEqual(repo.store.head(), original_ref)
        self.assertFalse((copied / '.git').exists())
        self.assertNotIn('private-token', plan.candidate.to_json())
        self.assertFalse(any('publication.json' in p or 'project.json' in p for p in plan.files))
        # Source removal cannot erase the sealed observation.
        shutil.rmtree(repo.root)
        history.temp.cleanup()
        with mock.patch.object(Modes, 'git', side_effect=AssertionError('must not query source Git')):
            replay = M.replay_from_copy(copied)
        data = replay.to_data()
        self.assertEqual(replay.snapshot_id, plan.candidate.snapshot_id)
        self.assertEqual(data['context']['read_mode'], 'frozen')
        self.assertEqual(data['context']['original_read_mode'], 'live')
        self.assertEqual(set(data['context']['pending']['bundles']), {full['revision'], bundle['revision']})
        self.assertEqual(data['context']['pending']['ref'], original_ref)
        self.assertEqual(len(data['hypotheses']), 2)
        self.assertEqual(data['context']['target']['status'], 'unavailable')
        restored = M.restore_from_copy(copied, self.root / 'advanced-restored')
        self.assertEqual(Path(restored['record']).read_text(), 'known: {local.value: {v: 1}}\n')

    def test_live_context_source_change_refuses_before_copy_publication(self):
        repo, entry = self.repository()
        plan = M.prepare(entry, read_mode='live')
        repo.store._capture(pending_fixture.fixture_bundle(), event_id='later', contribution_id='later', shareability='project')
        destination = self.root / 'stale-context'
        with self.assertRaisesRegex(ValueError, 'snapshot_changed'):
            plan.publish(destination)
        self.assertFalse(destination.exists())

    def test_existing_v1_frozen_receipt_and_replay_remain_supported(self):
        entry = self.root / 'GROUNDING.yaml'
        entry.write_text('known: {p.one: {v: 1}}\n')
        plan = M.prepare(entry)
        copied = self.root / 'frozen-copy'
        plan.publish(copied)
        self.assertNotIn('observation', plan.manifest)
        self.assertNotIn('topology', plan.manifest)
        self.assertEqual(M.replay_from_copy(copied).snapshot_id, plan.candidate.snapshot_id)
        self.assertEqual(Path(M.restore_from_copy(copied, self.root / 'frozen-restore')['record']).read_bytes(), entry.read_bytes())

    def test_absolute_original_pointer_copies_exact_evidence_but_names_inverse_constraint(self):
        entry = self.external()
        target = entry.parent.parent / 'shared/member.yaml'
        entry.write_text('record: ' + str(target) + '\n')
        original = entry.read_bytes()
        plan = M.prepare(entry)
        self.assertFalse(plan.summary()['inverse']['representable'])
        self.assertEqual(plan.summary()['inverse']['reason'], 'absolute_original_pointer')
        copied = self.root / 'absolute-copy'
        result = plan.publish(copied)
        self.assertEqual((copied / M.ARTIFACTS / 'originals/GROUNDING.yaml').read_bytes(), original)
        shutil.rmtree(entry.parent.parent)
        self.assertEqual(set(G.entries(Snapshot.capture(result['record'], read_mode='frozen').to_data()['document'])),
                         {'p.first', 'p.second'})
        self.assertEqual(M.replay_from_copy(copied).snapshot_id, plan.candidate.snapshot_id)
        destination = self.root / 'absolute-restore'
        with self.assertRaisesRegex(ValueError, 'nonrepresentable_inverse: absolute_original_pointer'):
            M.restore_from_copy(copied, destination)
        self.assertFalse(destination.exists())

    def test_live_captured_target_and_every_pending_format_survive_source_free_copy(self):
        repo, entry = self.repository()
        target = Modes.git(repo.root, 'rev-parse', 'HEAD').stdout.decode().strip()
        Modes.git(repo.root, 'update-ref', 'refs/remotes/origin/trunk', target)
        full = pending_fixture.fixture_bundle()
        legacy = G._prepare(full['manifest']['document'], full['manifest']['roots'],
                            scope=full['manifest']['scope'], shareability='project',
                            evidence=full['files'], version=1)
        repo.store._capture(legacy, event_id='v1', contribution_id='v1', shareability='project')
        repo.store._capture(full, event_id='v2', contribution_id='v2', shareability='project')
        plan = M.prepare(entry, read_mode='live')
        captured = plan.candidate.to_data()['context']['target']
        self.assertEqual(captured['status'], 'observed')
        self.assertEqual(captured['revision'], target)
        copied = self.root / 'observed-target-copy'
        plan.publish(copied)
        shutil.rmtree(repo.root)
        replay = M.replay_from_copy(copied)
        self.assertEqual(replay.to_data()['context']['target'], captured)
        self.assertEqual(set(replay.to_data()['context']['pending']['bundles']), {legacy['revision'], full['revision']})

    def test_live_target_or_policy_change_refuses_publication(self):
        for change in ('target', 'policy'):
            repo, entry = self.repository()
            plan = M.prepare(entry, read_mode='live')
            if change == 'target':
                target = Modes.git(repo.root, 'rev-parse', 'HEAD').stdout.decode().strip()
                Modes.git(repo.root, 'update-ref', 'refs/remotes/origin/trunk', target)
            else:
                project = Modes.Project(repo.root)
                config = project.config()
                config['generation'] += 1
                project.config_path.write_text(json.dumps(config))
            destination = self.root / (change + '-changed')
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, 'snapshot_changed'):
                plan.publish(destination)
            self.assertFalse(destination.exists())
