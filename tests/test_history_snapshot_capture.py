"""Disk history capture binds committed evidence without evaluating generated YAML."""
import copy
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import history_contract as C, history_store as H, history_transaction as T
from scripts import provenance as P, project_modes as M, versions as V
from scripts.pending_grounding import identity
from scripts.reasoning import snapshot as S
from scripts.reasoning.snapshot import Snapshot, SnapshotError, capture_source


FIELDS = {'value': 'v', 'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
TEMPLATE = {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
            'schema': FIELDS, 'readings': {}, 'decisions': {}}


def claim(subject='p.input', value=1, *, operation='reading', body=None, kind='reading', pins=None, saw=()):
    return C.make_object(subject=subject, kind=kind, by='writer', on='2026-09-17', operation=operation,
                         body={'v': value} if body is None else body, pins=pins, saw=saw,
                         authored={'collection': 'readings' if kind == 'reading' else 'decisions',
                                   'fields': FIELDS, 'profile': 'core/v1'})


def act(target, what='review', *, operation='review', read=None, over=(), saw=()):
    return C.make_object(subject=target['subject'], kind='act', by='reviewer', on='2026-09-17',
                         operation=operation, saw=saw, body={'act': what, 'of': target['id'],
                         'over': sorted(over), 'because': 'recorded decision', **({'read': read} if read is not None else {})})


class HistorySnapshotCapture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        patcher = mock.patch.dict(os.environ, {'KPOPPER_NO_CACHE': '1'})
        patcher.start()
        self.addCleanup(patcher.stop)
        self.root = Path(self.temp.name).resolve()
        self.entry = self.root / 'GROUNDING.yaml'
        self.store = H.Store(self.entry)
        self.marker = C.authority(record_id='capture-fixture', authority='history', generation=1)
        self.marker_path = Path(self.store.layout['history_authority'])
        self.marker_path.parent.mkdir(parents=True)
        self.marker_path.write_bytes(C.encode_document(self.marker))
        template = copy.deepcopy(TEMPLATE)
        template['meta']['history'] = H.baseline(self.marker, {}, H.reduce({}))
        self.entry.write_bytes(C.encode_document(template))
        self.source = claim()
        self.publish([self.source], 'initial')

    def publish(self, objects, operation):
        captured = self.store.capture()
        pairs = [(obj, C.encode_document(obj)) for obj in objects]
        receipt = T.semantic_receipt(profile='core/v1', capabilities={}, before={}, after={})
        arguments = dict(marker=self.marker, operation=operation,
                         parents=C.commit_frontier(captured.commits),
                         baseline=captured.baseline, objects=pairs, receipt=receipt,
                         view_template=C.document_template(captured.document))
        draft = C.make_commit(**arguments, view=b'')
        commits = {**captured.commits, operation: C.encode_document(draft)}
        after = self.store.render(captured, objects={**captured.objects, **{obj['id']: obj for obj in objects}}, commits=commits)
        manifest = C.make_commit(**arguments, view=after)
        files = [{'path': self.entry.name, 'role': 'record', 'before': captured.entry_bytes, 'after': after}]
        files += [{'path': str(Path(self.store.layout['history']).relative_to(self.root) / obj['subject'] / (obj['id'] + '.yaml')),
                   'role': 'history_object', 'before': None, 'after': raw} for obj, raw in pairs]
        files.append({'path': str(Path(self.store.layout['history_commits']).relative_to(self.root) / (operation + '.yaml')),
                      'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)})
        mutation = T.PreparedMutation(operation=operation, authority=self.marker, baseline=captured.baseline,
                                      files=files, receipt=receipt)
        self.store.commit(mutation, verify=lambda data: None)
        return mutation

    def snapshot(self, mode='frozen'):
        return Snapshot.capture(self.entry, read_mode=mode, as_of='2026-09-17')

    def test_disk_capture_is_read_only_and_evaluator_free(self):
        before = {str(path): path.read_bytes() for path in self.root.rglob('*') if path.is_file()}
        with mock.patch.object(V, '_evaluate', side_effect=AssertionError('float fallback')), \
                mock.patch('scripts.reasoning.evaluate.Evaluator.evaluate_many', side_effect=AssertionError('evaluator')):
            snapshot = self.snapshot()
        data = snapshot.to_data()
        self.assertEqual(data['nodes']['p.input']['body'], {'v': 1})
        self.assertEqual(data['context']['history']['subjects']['p.input']['acceptance'], 'accepted')
        self.assertEqual(data['context']['history_view']['status'], 'current')
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.root.rglob('*') if path.is_file()})

    def test_same_values_new_review_changes_identity_and_roundtrips_actual_review(self):
        first = self.snapshot()
        review = act(self.source, read={'p.input': self.source['id']})
        self.publish([review], 'review')
        second = self.snapshot()
        self.assertEqual(first.to_data()['nodes'], second.to_data()['nodes'])
        self.assertNotEqual(first.snapshot_id, second.snapshot_id)
        projection = second.to_data()['context']['history']
        self.assertEqual(projection['dispositions']['p.input']['reviews'], [review])
        self.assertEqual(projection['pins'][self.source['id']]['object'], self.source)
        self.assertEqual(Snapshot.from_json(second.to_json()).to_data()['context']['history'], projection)

    def test_known_stale_view_is_projected_without_rewriting(self):
        before_bytes = self.entry.read_bytes()
        self.publish([act(self.source)], 'review')
        self.entry.write_bytes(before_bytes)
        snapshot = self.snapshot()
        self.assertEqual(snapshot.to_data()['context']['history_view']['status'], 'stale_generated')
        self.assertEqual(len(snapshot.to_data()['context']['history']['dispositions']['p.input']['reviews']), 1)
        self.assertEqual(self.entry.read_bytes(), before_bytes)

    def test_hand_edited_value_or_header_refuses(self):
        original = self.entry.read_bytes()
        for header in (False, True):
            document = C.decode_document(original)
            if header:
                document['meta']['forged_header'] = 'not committed'
            else:
                document['readings']['p.input']['v'] = 900
            self.entry.write_bytes(C.encode_document(document))
            with self.subTest(header=header), self.assertRaisesRegex(SnapshotError, 'unresolved_view_edit'):
                self.snapshot()

    def test_public_revision_omits_history_inventory_but_private_capture_retains_bytes(self):
        source = capture_source(self.entry, read_mode='frozen')
        files = source.files
        object_path = str(Path(self.store.layout['history']) / self.source['subject'] / (self.source['id'] + '.yaml'))
        commit_path = str(Path(self.store.layout['history_commits']) / 'initial.yaml')
        self.assertEqual(files[object_path], C.encode_document(self.source))
        self.assertIn(commit_path, files)
        public = source.snapshot.to_data()['authored_revision']['files']
        self.assertFalse(any('/history/' in item['origin'] or '/history-commits/' in item['origin'] for item in public))
        self.assertLessEqual(len(public), 2)
        source.verify()
        serialized = source.snapshot.to_json()
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('source I/O')), \
                mock.patch.object(P, 'load', side_effect=AssertionError('source I/O')), \
                mock.patch.object(H.Store, 'capture', side_effect=AssertionError('store I/O')):
            replay = Snapshot.from_json(serialized)
        self.assertEqual(replay.snapshot_id, source.snapshot.snapshot_id)

    def test_orphan_staging_changes_private_verification_not_semantic_identity(self):
        before = capture_source(self.entry, read_mode='frozen')
        orphan = claim('p.orphan')
        path = Path(self.store.layout['history']) / orphan['subject'] / (orphan['id'] + '.yaml')
        path.parent.mkdir()
        path.write_bytes(b'not valid yaml: [')
        after = capture_source(self.entry, read_mode='frozen')
        self.assertEqual(before.snapshot.snapshot_id, after.snapshot.snapshot_id)
        self.assertEqual(before.snapshot.to_data()['authored_revision'], after.snapshot.to_data()['authored_revision'])
        self.assertEqual(after.files[str(path)], b'not valid yaml: [')
        with self.assertRaisesRegex(SnapshotError, 'snapshot_changed'):
            before.verify()

    def test_missing_or_corrupt_committed_object_never_yields_partial_snapshot(self):
        path = Path(self.store.layout['history']) / self.source['subject'] / (self.source['id'] + '.yaml')
        raw = path.read_bytes()
        path.unlink()
        with self.assertRaisesRegex(SnapshotError, 'incomplete_commit'):
            self.snapshot()
        path.write_bytes(raw + b'changed')
        with self.assertRaisesRegex(SnapshotError, 'object_bytes_mismatch'):
            self.snapshot()

    def test_marker_missing_malformed_inactive_and_mismatched_are_explicit(self):
        self.marker_path.unlink()
        with self.assertRaisesRegex(P.Refused, 'missing_history_authority'):
            self.snapshot()
        self.marker_path.write_bytes(b'bad: [')
        with self.assertRaisesRegex(SnapshotError, 'invalid_history_yaml'):
            self.snapshot()
        inactive = C.authority(record_id='capture-fixture', authority='legacy', generation=1)
        self.marker_path.write_bytes(C.encode_document(inactive))
        with self.assertRaisesRegex(P.Refused, 'authority_mismatch'):
            self.snapshot()
        mismatched = C.authority(record_id='different', authority='history', generation=1)
        self.marker_path.write_bytes(C.encode_document(mismatched))
        with self.assertRaisesRegex(SnapshotError, 'authority_mismatch'):
            self.snapshot()

    def test_inactive_marker_does_not_activate_or_read_inactive_store(self):
        self.marker_path.write_bytes(C.encode_document(C.authority(record_id='inactive', authority='legacy', generation=3)))
        self.entry.write_text('known:\n  p.old: {v: 7}\n')
        with mock.patch.object(H.Store, 'capture', side_effect=AssertionError('inactive store')):
            result = self.snapshot()
        self.assertNotIn('history', result.to_data()['context'])
        self.assertEqual(result.to_data()['nodes']['p.old']['body'], {'v': 7})

    def test_ordinary_and_core_authoring_readers_refuse_generated_view(self):
        for permission in (False, True):
            token = P._CORE_READS.set(permission)
            try:
                with self.assertRaisesRegex(P.Refused, 'history_reader_unsupported'):
                    P.load([str(self.entry)], read_mode='frozen')
            finally:
                P._CORE_READS.reset(token)
        pointer = self.root / 'pointer.yaml'
        pointer.write_text('record: GROUNDING.yaml\n')
        with self.assertRaisesRegex(P.Refused, 'history_reader_unsupported'):
            P.load([str(pointer)], read_mode='frozen')

    def test_simple_live_retains_project_and_hypothesis_context(self):
        directory = Path(self.store.layout['hypotheses'])
        directory.mkdir()
        (directory / 'alternative.yaml').write_text('readings:\n  p.input: {v: 2}\n')
        snapshot = self.snapshot('live')
        context = snapshot.to_data()['context']
        self.assertEqual(context['project']['mode'], 'simple')
        self.assertEqual(context['read_mode'], 'captured-live')
        self.assertEqual(context['pending']['bundles'], {})
        self.assertIn('alternative', snapshot.to_data()['hypotheses'])
        self.assertIn('target', context)
        (directory / 'invalid.yaml').write_text('meta:\n  reasoning: {version: 2, profile: unknown, requires: []}\n')
        with self.assertRaisesRegex(P.Refused, 'unsupported_capability'):
            self.snapshot('live')

    def test_advanced_live_hold_is_explicit_and_frozen_history_remains_available(self):
        M.git(self.root, 'init', '-b', 'test')
        with self.assertRaisesRegex(SnapshotError, 'history_advanced_live_unsupported'):
            self.snapshot('live')
        self.assertEqual(self.snapshot().to_data()['context']['read_mode'], 'frozen')

    def test_malformed_disposition_and_missing_review_pin_refuse_replay(self):
        self.publish([act(self.source, read={'p.input': self.source['id']})], 'review')
        original = self.snapshot().to_data()
        for kind in ('mark', 'pin'):
            data = copy.deepcopy(original)
            history = data['context']['history']
            if kind == 'mark':
                history['dispositions']['p.input']['marks'][self.source['id']] = 'true'
            else:
                history['pins'].pop(self.source['id'])
            data['snapshot_id'] = S.digest(S._snapshot_preimage(data))
            with self.subTest(kind=kind), self.assertRaisesRegex(SnapshotError, 'invalid_history'):
                Snapshot.from_snapshot(data)

    def test_pin_and_original_seen_survive_actual_capture(self):
        decision = claim('d.ready', body={'rests_on': {'p.input': self.source['id']},
                         'seen': {'p.input': 8}, 'wrong_if': 'unsupported prose'}, kind='judgment',
                         pins={'p.input': self.source['id']}, operation='decision')
        self.publish([decision], 'decision')
        data = self.snapshot().to_data()
        self.assertEqual(data['nodes']['d.ready']['body']['rests_on'], ['p.input'])
        self.assertEqual(data['nodes']['d.ready']['body']['seen'], {'p.input': 8})
        self.assertEqual(data['context']['history']['pins'][decision['id']]['object']['body'], decision['body'])
        self.assertEqual(data['context']['history']['subjects']['d.ready']['acceptance'], 'accepted')

    def test_inventory_brackets_new_history_membership_during_capture(self):
        original = H.Store.capture
        calls = 0
        def changing(store, *args, **kwargs):
            nonlocal calls
            result = original(store, *args, **kwargs)
            calls += 1
            if calls == 1:
                path = Path(store.layout['history']) / 'new'
                path.mkdir()
            return result
        with mock.patch.object(H.Store, 'capture', changing):
            with self.assertRaisesRegex(SnapshotError, 'snapshot_changed'):
                self.snapshot()


if __name__ == '__main__':
    unittest.main()
