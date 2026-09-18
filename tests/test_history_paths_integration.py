"""Opaque logical IDs use safe new paths without rewriting retained old evidence."""
import copy
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

from scripts import history_paths as HP, history_contract as C, history_store as H
from scripts import history_transaction as T, history_authoring as A, history_bundle as B
from scripts import history_migration as M, pending_grounding as G
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_store as fixtures
from tests.test_history_snapshot_capture import FIELDS

SCOPE = {'kind': 'project', 'environment': 'fixture'}


def claim(subject, value=1, collection='readings'):
    return C.make_object(subject=subject, kind='reading', by='writer', on='2026-09-17',
        operation='claim-' + C.sha256(subject.encode())[:16], body={'v': value, 'scope': SCOPE},
        authored={'collection': collection, 'fields': FIELDS, 'profile': 'core/v1'})


class OpaqueHistoryPaths(unittest.TestCase):
    def setUp(self):
        self.fixture = fixtures.Storage()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.entry, self.store = self.fixture.entry.resolve(), self.fixture.store
        self.root = self.entry.parent
        self.old = claim('p.input')
        self.fixture.publish([self.old], op='legacy-root')

    def publish(self, objects, operation='opaque'):
        captured = self.store.capture()
        template = self.store._template(captured.commits)
        for obj in objects:
            template.setdefault(obj['authored']['collection'], {})
        pairs = [(obj, C.encode_document(obj)) for obj in objects]
        receipt = T.semantic_receipt(profile='core/v1', capabilities={}, before={}, after={})
        args = dict(marker=captured.marker, operation=operation, parents=C.commit_frontier(captured.commits),
                    baseline=captured.baseline, objects=pairs, receipt=receipt, view_template=template,
                    requires=[HP.CAPABILITY])
        draft = C.make_commit(**args, view=b'')
        after = self.store.render(captured, objects={**captured.objects, **{obj['id']: obj for obj in objects}},
                                  commits={**captured.commits, operation: C.encode_document(draft)})
        manifest = C.make_commit(**args, view=after)
        files = [{'role': 'record', 'path': self.entry.name, 'before': captured.entry_bytes, 'after': after}]
        files += [{'role': 'history_object', 'path': '.kpopper/history/' + HP.object_path(obj['subject'], obj['id']),
                   'before': None, 'after': raw} for obj, raw in pairs]
        files += [{'role': 'history_commit', 'path': '.kpopper/history-commits/' + operation + '.yaml',
                   'before': None, 'after': C.encode_document(manifest)}]
        mutation = T.PreparedMutation(operation=operation, authority=captured.marker, baseline=captured.baseline,
                                      files=files, receipt=receipt)
        self.store.commit(mutation, verify=lambda data: None)
        return mutation

    def test_unicode_opaque_case_and_normalization_names_preserve_original_text(self):
        names = ['p.עלות', '../outside', 'a/b', 'A', 'a', 'Café', 'Cafe\u0301', '', 'nul\x00id']
        objects = [claim(name, index, collection='מדידות' if index == 0 else 'readings')
                   for index, name in enumerate(names)]
        self.publish(objects)
        captured = self.store.capture()
        snapshot = Snapshot.capture(self.entry, read_mode='frozen')
        for obj in objects:
            self.assertEqual(captured.objects[obj['id']], obj)
            self.assertEqual(snapshot.to_data()['nodes'][obj['subject']]['body'], obj['body'])
            relative = captured.object_paths[(obj['subject'], obj['id'])]
            self.assertEqual(HP.validate_object_path(relative, obj['subject'], obj['id']), HP.HASHED)
            self.assertTrue((Path(self.store.layout['history']) / relative).is_file())
        self.assertEqual(Snapshot.from_json(snapshot.to_json()).snapshot_id, snapshot.snapshot_id)
        self.assertEqual(captured.object_paths[('p.input', self.old['id'])], 'p.input/' + self.old['id'] + '.yaml')

    def test_fresh_writer_hashes_paths_but_retained_old_mutation_replays_byte_exact(self):
        with HP.replay_layout({'requires': [C.EXPLICIT_ROOT_DISPOSITION]}):
            old = A.prepare(self.entry, {'kind': 'set', 'id': 'p.input', 'value': 2},
                            operation='old-writer', recorded_at='2026-09-17T01:02:03Z')
        raw = old.to_bytes()
        self.assertTrue(all('/p.input/' in item['path'] for item in old.files if item['role'] == 'history_object'))
        restored = T.PreparedMutation.from_bytes(raw)
        A.verify_prepared(self.entry, restored)
        A.commit(self.entry, restored, verify=lambda data: None)
        A.commit(self.entry, restored, verify=lambda data: None)
        self.assertEqual(restored.to_bytes(), raw)
        fresh = A.prepare(self.entry, {'kind': 'set', 'id': 'p.input', 'value': 2}, operation='new-writer')
        self.assertTrue(all('/~' in item['path'] for item in fresh.files if item['role'] == 'history_object'))
        manifest = C.decode_document(next(item['after'] for item in fresh.files if item['role'] == 'history_commit'))
        self.assertIn(HP.CAPABILITY, manifest['requires'])
        A.commit(self.entry, fresh, verify=lambda data: None)
        self.assertEqual(self.store.capture().objects[self.old['id']], self.old)

    def test_existing_unicode_subject_can_be_set_without_normalizing(self):
        obj = claim('p.עלות')
        self.publish([obj])
        mutation = A.prepare(self.entry, {'kind': 'set', 'id': obj['subject'], 'value': 2})
        A.commit(self.entry, mutation, verify=lambda data: None)
        self.assertEqual(self.store.state()['subjects'][obj['subject']]['body']['v'], 2)
        self.assertEqual(self.store.capture().objects[obj['id']], obj)

    def test_malformed_hashed_orphan_stays_private_and_is_not_decoded(self):
        relative = HP.object_path('uncommitted', 'c' * 64)
        path = Path(self.store.layout['history']) / relative
        path.parent.mkdir()
        raw = b'broken yaml : ['
        path.write_bytes(raw)
        decode = C.decode_document
        def checked(value):
            self.assertNotEqual(value, raw, 'orphan staging was decoded')
            return decode(value)
        with mock.patch.object(C, 'decode_document', side_effect=checked):
            captured = self.store.capture()
        self.assertEqual(captured.storage_bytes[relative], raw)
        self.assertNotIn(('uncommitted', 'c' * 64), captured.object_bytes)
        artifact = B.export(captured, roots=['p.input'], scope=SCOPE, shareability='project')
        self.assertNotIn('objects/' + relative, artifact['files'])

    def test_duplicate_raw_hashed_member_and_missing_capability_refuse(self):
        duplicate = Path(self.store.layout['history']) / HP.object_path('p.input', self.old['id'])
        duplicate.parent.mkdir()
        duplicate.write_bytes(C.encode_document(self.old))
        with self.assertRaisesRegex(C.HistoryError, 'duplicate_history_object_path'):
            self.store.capture()
        duplicate.unlink()
        obj = claim('p.חדש')
        mutation = self.publish([obj])
        item = next(item for item in mutation.files if item['role'] == 'history_commit')
        manifest = C.decode_document(item['after'])
        manifest.pop('requires')
        (self.root / item['path']).write_bytes(C.encode_document(manifest))
        with self.assertRaisesRegex(C.HistoryError, 'subject_path_capability_required'):
            self.store.capture()

    def test_full_and_subset_transport_preserve_paths_objects_and_capability(self):
        obj = claim('p.עלות')
        self.publish([obj])
        captured = self.store.capture()
        artifact = B.export(captured, roots=['p.input', obj['subject']], scope=SCOPE, shareability='project')
        self.assertIn(HP.CAPABILITY, artifact['manifest']['requires'])
        self.assertIn('objects/p.input/' + self.old['id'] + '.yaml', artifact['files'])
        self.assertEqual(B.validate(artifact).objects[obj['id']], obj)
        subset = B.prepare_subset(captured, [obj['subject']], scope=SCOPE, shareability='project',
            operation='subset', recorded_at='2026-09-17T00:00:00Z', source_entry=self.entry.name)
        self.assertTrue(all(path.startswith('objects/~') for path in subset['files'] if path.startswith('objects/')))
        bundle = G.prepare(B.adapt(subset).document, [obj['subject']], scope=SCOPE, shareability='project', history=subset)
        G.validate_bundle(bundle)
        self.assertEqual(B.from_contribution(bundle), subset)
        self.assertEqual(B.validate(subset).object_bytes[(obj['subject'], obj['id'])], C.encode_document(obj))

    def test_copy_migration_uses_safe_paths_for_opaque_subjects_and_unicode_collection(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / 'SOURCE.yaml'
            source.write_bytes(C.encode_document({'schema': FIELDS, 'מדידות': {
                'p.עלות': {'v': 1}, 'p/A': {'v': 2}, 'Cafe\u0301': {'v': 3}, 'Café': {'v': 4}}}))
            plan = M.prepare(source, operation='import-opaque', recorded_at='2026-09-17T00:00:00Z')
            self.assertEqual(plan.problems, [])
            destination = Path(directory) / 'copy'
            plan.publish(destination)
            imported = H.Store(destination / source.name).capture()
            self.assertEqual(set(imported.state['subjects']), {'p.עלות', 'p/A', 'Cafe\u0301', 'Café'})
            self.assertTrue(all(path.startswith('~') for path in imported.object_paths.values()))
            self.assertEqual(imported.document['מדידות']['p.עלות'], {'v': 1})

    def test_subset_follows_exact_unicode_source_ids_without_ascii_token_guessing(self):
        source = claim('s.מקור')
        reading = claim('p.נתון')
        reading['body']['from'] = source['subject']
        reading['id'] = C.object_identity(reading)
        self.publish([source, reading])
        artifact = B.prepare_subset(self.store.capture(), [reading['subject']], scope=SCOPE, shareability='project',
            operation='unicode-source', recorded_at='2026-09-17T00:00:00Z', source_entry=self.entry.name)
        self.assertEqual(set(B.validate(artifact).state['subjects']), {source['subject'], reading['subject']})

    def test_materialization_preserves_mixed_legacy_and_hashed_paths_exactly(self):
        from tests.test_pending_grounding import Repository
        from scripts import knowledge_views as V
        obj = claim('p.עלות')
        self.publish([obj])
        captured = self.store.capture()
        artifact = B.export(captured, roots=['p.input', obj['subject']], scope=SCOPE, shareability='project')
        bundle = G.prepare(B.adapt(artifact).document, ['p.input', obj['subject']],
                           scope=SCOPE, shareability='project', history=artifact)
        repo = Repository()
        repo.setUp()
        self.addCleanup(repo.doCleanups)
        repo.store._capture(bundle, event_id='mixed-paths', contribution_id='mixed-paths', shareability='project')
        result = V.materialize(repo.root, bundle['revision'], repo.base / 'copy')
        copied = H.Store(result['record']).capture()
        self.assertEqual(copied.object_paths, captured.object_paths)
        self.assertEqual(copied.storage_bytes, captured.storage_bytes)
        self.assertEqual(copied.object_bytes, captured.object_bytes)

    def test_git_target_and_watch_capture_hashed_paths(self):
        obj = claim('p.עלות')
        self.publish([obj])
        subprocess.run(['git', 'init', '-q', '-b', 'main', str(self.root)], check=True)
        for key, value in [('user.name', 'fixture'), ('user.email', 'fixture@example.test')]:
            subprocess.run(['git', '-C', str(self.root), 'config', key, value], check=True)
        subprocess.run(['git', '-C', str(self.root), 'add', 'GROUNDING.yaml', '.kpopper/history', '.kpopper/history-commits', '.kpopper/history.yaml'], check=True)
        subprocess.run(['git', '-C', str(self.root), 'commit', '-qm', 'fixture'], check=True)
        from scripts import pending_publication as U, project_modes as Modes, watch
        project = Modes.Project(self.root)
        publisher = U.Publisher(project)
        commit = subprocess.check_output(['git', '-C', str(self.root), 'rev-parse', 'HEAD']).decode().strip()
        target = publisher._history(publisher._files(commit))
        self.assertIn(obj['subject'], target[0]['readings'] if isinstance(target, tuple) else target.document['readings'])
        token = watch.P._CORE_READS.set(True)
        try:
            watched = watch._records(self.root, self.entry.name, commit)
        finally:
            watch.P._CORE_READS.reset(token)
        self.assertIsNotNone(watched.get('history'))


if __name__ == '__main__':
    unittest.main()
