"""Real copied fixture activation, frozen replay, and exact inverse export."""
import copy
import datetime
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import history_migration as M, history_contract as C, history_store as H
from scripts import history_authoring as W, provenance as P, versions as V
from scripts.pending_grounding import identity, entries
from scripts.reasoning.snapshot import Snapshot


class Migration(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()

    def fixture(self, shape='single', name='GROUNDING.yaml', *, core=False, archive=True, scalars=False):
        directory = self.root / (shape + '-' + name.replace('.', '-') + ('-core' if core else ''))
        directory.mkdir()
        document = {'known': {'p.input': {'v': 2, 'of': datetime.date(2026, 9, 16), 'also': ['p.old']},
                              'p.total': {'rule': {'expr': 'p.input / 3'}}},
            'judgments': {'d.stable': {'verdict': 'stable', 'rests_on': ['p.total'],
                'wrong_if': {'expr': 'p.total > 5'}, 'seen': {'p.total': 'original uncomputed reading'}}}}
        if core:
            document['meta'] = {'reasoning': {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}}
        if scalars:
            document['known'].update({'p.text': 'literal', 'p.null': None, 'p.bool': True,
                'p.date': datetime.date(2026, 9, 12), 'p.integer': 9007199254740993})
        record = directory / name
        if shape in ('sharded', 'pointer'):
            member = directory / ('part.yaml' if shape == 'sharded' else 'data/values.yaml')
            member.parent.mkdir(parents=True, exist_ok=True)
            member.write_bytes(C.encode_document(document))
            record.write_text('record: ' + str(member.relative_to(directory)) + '\n')
        else:
            record.write_bytes(C.encode_document(document))
        layout = P.layout(record)
        for role, raw in (('view', b'sections: []\n'), ('measure', b'recipes: {}\n'),
                          ('session', b'{"retained":true}\n')):
            path = Path(layout[role])
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)
        if archive:
            historical = {'d.stable': [{'verdict': 'previous', 'rests_on': ['p.input'],
                'wrong_if': 'p.input > 1', 'seen': {'p.input': 0}, 'day': datetime.date(2026, 9, 15),
                'ended': 'replaced because condition fired'}]}
            Path(layout['replaced']).write_bytes(b'# original archive\r\n' + C.encode_document(historical).replace(b'\n', b'\r\n'))
        if shape == 'hypothesis':
            path = Path(layout['hypotheses']) / 'alternative.yaml'
            path.parent.mkdir(parents=True)
            path.write_bytes(C.encode_document({'hypothesis': {'claim': 'Alternative', 'folds': 'never'},
                                              'known': {'p.input': {'v': 3}}}))
        return record

    def test_four_original_shapes_keep_paths_bytes_collections_and_replay(self):
        for name in ('GROUNDING.yaml', 'PROVENANCE.yaml', 'custom.yml'):
            for shape in ('single', 'sharded', 'pointer', 'hypothesis'):
                with self.subTest(name=name, shape=shape):
                    record = self.fixture(shape, name)
                    source = {p.relative_to(record.parent).as_posix(): p.read_bytes()
                              for p in record.parent.rglob('*') if p.is_file()}
                    original = Snapshot.capture([str(record)], read_mode='frozen')
                    plan = M.prepare(record, operation='import-fixture', recorded_at='2026-09-17T12:00:00+00:00')
                    self.assertFalse(plan.problems)
                    destination = self.root / ('copy-' + record.parent.name)
                    result = plan.publish(destination)
                    self.assertEqual(Path(result['record']).name, name)
                    copied = Snapshot.capture([result['record']], read_mode='frozen')
                    self.assertEqual(M._entry_identity(copied.to_data()['document']),
                                     M._entry_identity(original.to_data()['document']))
                    self.assertEqual(Snapshot.from_json(copied.to_json()).snapshot_id, copied.snapshot_id)
                    self.assertEqual({p.relative_to(record.parent).as_posix(): p.read_bytes()
                                      for p in record.parent.rglob('*') if p.is_file()}, source)
                    for path, raw in source.items():
                        self.assertEqual((destination / M.ARTIFACTS / 'originals' / path).read_bytes(), raw)
                        if path != name:
                            self.assertEqual((destination / path).read_bytes(), raw)
                    store = H.Store(destination / name)
                    capture = store.capture()
                    current = capture.objects[capture.state['subjects']['d.stable']['head']]
                    self.assertEqual(current['body']['seen'], {'p.total': 'original uncomputed reading'})
                    self.assertEqual(current['pins'], {})
                    self.assertEqual(current['pin_gaps'], {'p.total': 'not_recorded'})
                    self.assertEqual(current['on'], '2026-09-17T12:00:00+00:00')
                    self.assertIsNone(current['by'])
                    self.assertTrue(all(value is None for value in current['authored']['locator']['original'].values()))
                    self.assertEqual(current['authored']['profile'], 'ordinary-reader/v1')
                    restored = self.root / ('restored-' + record.parent.name)
                    plan.restore_copy(destination, restored)
                    self.assertEqual({p.relative_to(restored).as_posix(): p.read_bytes()
                                      for p in restored.rglob('*') if p.is_file()}, source)

    def test_core_predicates_and_scalar_bodies_keep_typed_meaning(self):
        record = self.fixture(core=True, scalars=True)
        plan = M.prepare(record)
        destination = self.root / 'core-copy'
        plan.publish(destination)
        snapshot = Snapshot.capture([str(destination / record.name)], read_mode='frozen')
        source = entries(plan.original.to_data()['document'])
        self.assertEqual(M._entry_identity(snapshot.to_data()['document']), M._entry_identity(plan.original.to_data()['document']))
        capture = H.Store(destination / record.name).capture()
        for subject in ('p.null', 'p.bool', 'p.date', 'p.integer'):
            obj = capture.objects[capture.state['subjects'][subject]['head']]
            self.assertEqual(identity(obj['body']), identity(source[subject][1]))
        from scripts.reasoning.evaluate import Evaluator
        value = Evaluator(snapshot).evaluate({'ref': 'p.total'}, declared=['p.total'])
        self.assertEqual(value['value'], {'type': 'number', 'numerator': '2', 'denominator': '3'})
        self.assertEqual(capture.objects[capture.state['subjects']['d.stable']['head']]['authored']['profile'], 'core/v1')

    def test_inactive_prototype_ids_and_bytes_are_retained_without_rehashing(self):
        record = self.fixture()
        prototype = V.version('old.reading', 'reading', None, {'v': 9}, on='2020-01-01', op='original-operation')
        path = Path(P.layout(record)['history']) / prototype['subject'] / (prototype['id'] + '.yaml')
        path.parent.mkdir(parents=True)
        raw = P.yaml.safe_dump(prototype, sort_keys=False).encode()
        path.write_bytes(raw)
        plan = M.prepare(record)
        destination = self.root / 'prototype-copy'
        plan.publish(destination)
        relative = path.relative_to(record.parent)
        self.assertEqual((destination / relative).read_bytes(), raw)
        self.assertEqual((destination / M.ARTIFACTS / 'originals' / relative).read_bytes(), raw)
        self.assertNotIn(prototype['id'], H.Store(destination / record.name).capture().objects)

    def test_unknown_archive_successor_blocks_activation_but_preserves_preparation(self):
        record = self.fixture()
        archive = Path(P.layout(record)['replaced'])
        document = C.decode_document(archive.read_bytes())
        document['d.missing'] = copy.deepcopy(document['d.stable'])
        archive.write_bytes(C.encode_document(document))
        plan = M.prepare(record)
        self.assertIn('archive_successor_unknown: d.missing', plan.problems)
        destination = self.root / 'blocked-copy'
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_history_import'):
            plan.publish(destination)
        self.assertFalse(destination.exists())

    def test_changed_source_and_interrupted_validation_publish_no_destination(self):
        record = self.fixture()
        plan = M.prepare(record)
        destination = self.root / 'interrupted-copy'
        with mock.patch.object(plan, 'validate_destination', side_effect=OSError('validation interruption')):
            with self.assertRaisesRegex(OSError, 'validation interruption'):
                plan.publish(destination)
        self.assertFalse(destination.exists())
        Path(P.layout(record)['replaced']).write_bytes(b'concurrent change')
        with self.assertRaisesRegex(ValueError, 'snapshot_changed'):
            plan.publish(destination)
        self.assertFalse(destination.exists())

    def test_core_archive_retains_condition_and_seen_without_guessing_original_profile(self):
        record = self.fixture(core=True)
        archive = Path(P.layout(record)['replaced'])
        old = {'verdict': 'old', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input > 1'},
               'seen': {'p.input': {'computed': {'version': 2, 'value': {'type': 'number',
                    'numerator': '1', 'denominator': '1'}, 'basis': {'original': 'retained verbatim'}}}},
               'day': datetime.date(2026, 9, 16), 'ended': 'replaced'}
        raw = C.encode_document({'d.stable': [old]})
        archive.write_bytes(raw)
        plan = M.prepare(record)
        self.assertIn('archive_condition_profile_unknown: d.stable:1', plan.problems)
        retired = next(obj for obj in plan.objects.values()
                       if obj.get('authored', {}).get('locator', {}).get('archive_index') == 1)
        self.assertEqual(retired['body'], old)
        self.assertIsNone(retired['authored']['locator']['original']['condition_profile'])
        self.assertEqual(retired['authored']['locator']['interpretation']['scope'], 'retained_archive_only')
        self.assertEqual(archive.read_bytes(), raw)
        self.assertEqual(plan.files[M.ARTIFACTS + '/originals/.kpopper/replaced.yaml'], raw)
        with self.assertRaisesRegex(C.HistoryError, 'archive_condition_profile_unknown'):
            plan.publish(self.root / 'unsupported-core-archive')

    def test_restore_refuses_new_committed_knowledge(self):
        record = self.fixture(core=True)
        plan = M.prepare(record)
        destination = self.root / 'changed-copy'
        plan.publish(destination)
        copied = destination / record.name
        mutation = W.prepare(copied, {'kind': 'set', 'id': 'p.input', 'value': 4,
                                     'as_of': '2026-09-17'})
        W.commit(copied, mutation, verify=lambda data: None)
        with self.assertRaisesRegex(C.HistoryError, 'migration_inventory_changed|migration_bytes_changed'):
            plan.restore_copy(destination, self.root / 'must-not-restore')


if __name__ == '__main__':
    unittest.main()
