"""Adversarial checks of the callable history boundary, without activating it.

All persistence is confined to temporary directories. A valid detached digest
is consistency evidence; these tests do not treat it as an authenticity proof.
"""
import copy
import datetime
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts import history_contract as H, history_transaction as T
from scripts.pending_grounding import _encode, identity, json_bytes
from tests.test_history_contract import baseline, claim, projection, receipt


class CommittedVisibilityConformance(unittest.TestCase):
    def setUp(self):
        self.marker = H.authority(record_id='record-1', authority='history', generation=1)
        self.first = claim(operation='first')
        self.second = claim('p.other', operation='second')
        self.objects = {(obj['subject'], obj['id']): H.encode_document(obj)
                        for obj in (self.first, self.second)}
        self.first_commit = self.commit('first', self.first)
        self.second_commit = self.commit('second', self.second,
                                         {'first': H.sha256(self.first_commit)})
        self.commits = {'first': self.first_commit, 'second': self.second_commit}

    def commit(self, operation, obj, parents=None):
        raw = self.objects[(obj['subject'], obj['id'])]
        return H.encode_document(H.make_commit(
            marker=self.marker, operation=operation, parents=parents or {},
            baseline=baseline(obj), objects=[(obj, raw)], receipt=receipt(), view=b'view'))

    def assert_refused(self, code, *, commits=None, objects=None, marker=None):
        with self.assertRaises(H.HistoryError) as caught:
            H.committed_objects(self.marker if marker is None else marker,
                                self.commits if commits is None else commits,
                                self.objects if objects is None else objects)
        self.assertEqual(caught.exception.code, code)

    def test_staged_objects_including_corrupt_residue_are_invisible(self):
        objects = {**self.objects, ('p.staged', 'e' * 64): b'not even valid YAML'}
        result = H.committed_objects(self.marker, {'first': self.first_commit}, objects)
        self.assertEqual(result, {self.first['id']: self.first})
        self.assertEqual(H.committed_objects(self.marker, {}, objects), {})

    def test_valid_parent_closure_returns_both_committed_objects(self):
        result = H.committed_objects(self.marker, self.commits, self.objects)
        self.assertEqual(set(result), {self.first['id'], self.second['id']})
        result[self.first['id']]['body']['v'] = 99
        self.assertEqual(H.committed_objects(self.marker, self.commits, self.objects)
                         [self.first['id']]['body']['v'], 1)

    def test_missing_or_corrupt_committed_object_refuses_entire_closure(self):
        for obj in (self.first, self.second):
            key = (obj['subject'], obj['id'])
            with self.subTest(object=obj['id'], failure='missing'):
                objects = {k: v for k, v in self.objects.items() if k != key}
                self.assert_refused('incomplete_commit', objects=objects)
            with self.subTest(object=obj['id'], failure='corrupt'):
                self.assert_refused('object_bytes_mismatch',
                                    objects={**self.objects, key: self.objects[key] + b'\n'})

    def test_missing_or_changed_parent_refuses_child_and_valid_siblings(self):
        self.assert_refused('incomplete_commit', commits={'second': self.second_commit})
        self.assert_refused('parent_bytes_mismatch',
                            commits={**self.commits, 'first': self.first_commit + b'\n'})

    def test_one_bad_manifest_never_returns_the_other_valid_manifest(self):
        for field, value, code in [('record_id', 'other-record', 'authority_mismatch'),
                                   ('authority_generation', 2, 'authority_mismatch'),
                                   ('operation', 'renamed', 'operation_mismatch')]:
            with self.subTest(field=field):
                manifest = H.decode_document(self.second_commit)
                manifest[field] = value
                self.assert_refused(code, commits={**self.commits,
                                                   'second': H.encode_document(manifest)})

    def test_valid_hash_cannot_hide_typed_identity_tampering(self):
        obj = copy.deepcopy(self.second)
        obj['body']['v'] = True  # Python considers this equal to the original 1.
        raw = H.encode_document(obj)
        manifest = H.decode_document(self.second_commit)
        manifest['objects'][0]['sha256'] = H.sha256(raw)
        self.assert_refused('identity_mismatch',
                            commits={**self.commits, 'second': H.encode_document(manifest)},
                            objects={**self.objects, (obj['subject'], obj['id']): raw})

    def test_object_at_wrong_subject_key_is_not_visible(self):
        manifest = H.decode_document(self.second_commit)
        manifest['objects'][0]['subject'] = 'p.wrong'
        objects = {**self.objects, ('p.wrong', self.second['id']):
                   self.objects[(self.second['subject'], self.second['id'])]}
        self.assert_refused('reference_mismatch', objects=objects,
                            commits={**self.commits, 'second': H.encode_document(manifest)})

    def test_staged_pin_target_does_not_complete_committed_closure(self):
        judgment = claim('d.ready', kind='judgment', operation='judgment',
                         pins={self.first['subject']: self.first['id']})
        self.objects[(judgment['subject'], judgment['id'])] = H.encode_document(judgment)
        self.assert_refused('incomplete_closure',
                            commits={'judgment': self.commit('judgment', judgment)})

    def test_legacy_marker_cannot_gain_authority_from_commit_presence(self):
        self.assert_refused('authority_mismatch', marker={**self.marker, 'authority': 'legacy'})


class TypedBindingsConformance(unittest.TestCase):
    def test_document_baseline_cannot_coalesce_boolean_or_float_generation(self):
        context = projection(claim())
        for generation in (True, 1.0):
            document_baseline = copy.deepcopy(context['baseline'])
            document_baseline['authority_generation'] = generation
            with self.subTest(generation=repr(generation)), self.assertRaises(H.HistoryError):
                H.CapturedHistory({'meta': {'history': document_baseline}}, context)

    def test_document_baseline_requires_exact_committed_set(self):
        context = projection(claim())
        document_baseline = copy.deepcopy(context['baseline'])
        document_baseline['committed_set_digest'] = identity('different set')
        with self.assertRaisesRegex(H.HistoryError, 'baseline_mismatch'):
            H.CapturedHistory({'meta': {'history': document_baseline}}, context)

    def test_typed_body_and_authored_mapping_are_both_identity_bound(self):
        original = claim(body={'v': datetime.date(2026, 9, 17)})
        for change in ('date-to-text', 'profile', 'field-role', 'pins'):
            changed = copy.deepcopy(original)
            if change == 'date-to-text':
                changed['body']['v'] = '2026-09-17'
            elif change == 'profile':
                changed['authored']['profile'] = 'checked-reader/v1'
            elif change == 'field-role':
                changed['authored']['fields']['value'] = 'another'
            else:
                changed['pins'] = {'p.other': 'a' * 64}
            with self.subTest(change=change), self.assertRaisesRegex(H.HistoryError, 'identity_mismatch'):
                H.validate_object(changed)

    def test_stripping_v2_scheme_cannot_reinterpret_retained_id_as_legacy(self):
        obj = claim()
        for field in ('schema_version', 'id_scheme', 'authored', 'pins'):
            obj.pop(field)
        with self.assertRaisesRegex(H.HistoryError, 'identity_mismatch'):
            H.validate_object(obj)

    def test_commit_constructor_binds_serialized_object_not_python_equality(self):
        obj = claim(body={'v': 1})
        different = claim(body={'v': True})
        with self.assertRaisesRegex(H.HistoryError, 'object_bytes_mismatch'):
            H.make_commit(marker=H.authority(record_id='record-1', authority='history', generation=1),
                          operation='op-1', parents={}, baseline=baseline(obj),
                          objects=[(obj, H.encode_document(different))], receipt=receipt(), view=b'view')

    def test_projection_pin_witness_rejects_same_subject_different_version(self):
        context = projection(claim())
        witness = next(iter(context['pins'].values()))
        witness['object'] = claim(operation='new-observation')
        with self.assertRaisesRegex(H.HistoryError, 'reference_mismatch'):
            H.validate_projection(context)


class ExactJournalConformance(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.journal = '.kpopper/.transaction.json'
        (self.root / '.kpopper').mkdir()
        self.record = self.root / 'GROUNDING.yaml'
        self.archive = self.root / '.kpopper/replaced.yaml'
        self.record.write_bytes(b'record-before\r\n# retain formatting\n')
        self.before = self.record.read_bytes()
        self.after = b'record-after\n# exact prepared bytes\n'
        self.mutation = T.PreparedMutation(
            operation='stable-operation',
            authority=H.authority(record_id='record-1', authority='legacy', generation=0),
            baseline={'typed': [True, 1, 1.0, datetime.date(2026, 9, 17)]}, receipt=receipt(),
            files=[{'path': 'GROUNDING.yaml', 'role': 'record', 'before': self.before, 'after': self.after},
                   {'path': '.kpopper/replaced.yaml', 'role': 'replaced', 'before': None,
                    'after': b'new archive\n'}])

    def interrupt_after_archive(self):
        replace = T._replace
        def interrupt(path, data):
            if path == self.record:
                raise OSError('injected interruption')
            return replace(path, data)
        with mock.patch.object(T, '_replace', side_effect=interrupt):
            with self.assertRaisesRegex(OSError, 'injected interruption'):
                T.publish_legacy(self.root, self.journal, self.mutation, verify=lambda _: None)
        return (self.root / self.journal).read_bytes()

    def test_prepared_data_and_file_accessors_cannot_mutate_replay(self):
        expected = self.mutation.to_bytes()
        exposed = self.mutation.to_data()
        exposed['baseline']['typed'][0] = False
        exposed['receipt']['before']['snapshot_id'] = 'changed'
        files = self.mutation.files
        files[0]['after'] = b'changed'
        self.assertEqual(self.mutation.to_bytes(), expected)
        self.assertEqual(T.PreparedMutation.from_bytes(expected).to_bytes(), expected)

    def test_recovery_reuses_exact_original_evidence_and_bytes(self):
        raw = self.interrupt_after_archive()
        seen = []
        result = T.recover_legacy(self.root, self.journal, verify=seen.append)
        self.assertEqual(result.to_bytes(), raw)
        self.assertEqual(seen, [self.mutation.to_data()])
        self.assertEqual(self.record.read_bytes(), self.after)
        self.assertEqual(self.archive.read_bytes(), b'new archive\n')
        self.assertFalse((self.root / self.journal).exists())

    def test_rollback_removes_created_file_and_restores_exact_before(self):
        self.interrupt_after_archive()
        T.recover_legacy(self.root, self.journal, verify=lambda _: None, direction='before')
        self.assertFalse(self.archive.exists())
        self.assertEqual(self.record.read_bytes(), self.before)

    def test_recovery_verifier_refusal_preserves_journal_and_partial_state(self):
        raw = self.interrupt_after_archive()
        def refuse(_):
            raise H.HistoryError('receipt_no_longer_valid')
        with self.assertRaisesRegex(H.HistoryError, 'receipt_no_longer_valid'):
            T.recover_legacy(self.root, self.journal, verify=refuse)
        self.assertEqual((self.root / self.journal).read_bytes(), raw)
        self.assertEqual(self.record.read_bytes(), self.before)
        self.assertEqual(self.archive.read_bytes(), b'new archive\n')

    def test_recovery_rejects_unrelated_edit_in_every_member_before_applying(self):
        for changed in ('record', 'archive'):
            with self.subTest(changed=changed):
                self.record.write_bytes(self.before)
                self.archive.unlink(missing_ok=True)
                raw = self.interrupt_after_archive()
                path = self.record if changed == 'record' else self.archive
                path.write_bytes(b'unrelated concurrent edit')
                states = (self.record.read_bytes(), self.archive.read_bytes())
                for direction in ('before', 'after'):
                    with self.assertRaisesRegex(H.HistoryError, 'concurrent_edit'):
                        T.recover_legacy(self.root, self.journal, verify=lambda _: None,
                                         direction=direction)
                    self.assertEqual((self.record.read_bytes(), self.archive.read_bytes()), states)
                    self.assertEqual((self.root / self.journal).read_bytes(), raw)
                (self.root / self.journal).unlink()

    def test_outer_digest_detects_authority_baseline_operation_and_receipt_tampering(self):
        for field in ('authority', 'baseline', 'operation', 'receipt'):
            value = self.mutation.to_data()
            if field == 'authority':
                value[field]['record_id'] = 'other-record'
            elif field == 'baseline':
                value[field]['typed'][0] = 1
            elif field == 'operation':
                value[field] = 'fresh-operation'
            else:
                value[field]['after']['snapshot_id'] = identity('different')
            with self.subTest(field=field), self.assertRaises(H.HistoryError):
                T.PreparedMutation.from_bytes(json_bytes(_encode(value)))

    def test_corrupt_blob_is_rejected_even_with_resealed_outer_digest(self):
        value = self.mutation.to_data()
        value['files'][0]['after']['data'] = 'Y29ycnVwdA=='
        value['digest'] = identity({k: v for k, v in value.items() if k != 'digest'})
        with self.assertRaisesRegex(H.HistoryError, 'byte_hash_mismatch'):
            T.PreparedMutation.from_bytes(json_bytes(_encode(value)))

    def test_existing_journal_blocks_new_operation_before_verification_or_write(self):
        raw = self.interrupt_after_archive()
        verify = mock.Mock()
        with self.assertRaisesRegex(H.HistoryError, 'recovery_required'):
            T.publish_legacy(self.root, self.journal, self.mutation, verify=verify)
        verify.assert_not_called()
        self.assertEqual((self.root / self.journal).read_bytes(), raw)
        self.assertEqual(self.record.read_bytes(), self.before)


class HistoryPreparationConformance(unittest.TestCase):
    def setUp(self):
        obj = claim()
        raw = H.encode_document(obj)
        self.marker = H.authority(record_id='record-1', authority='history', generation=1)
        self.baseline = baseline(obj)
        self.receipt = receipt()
        self.commit = H.make_commit(marker=self.marker, operation='op-1', parents={},
                                    baseline=self.baseline, objects=[(obj, raw)],
                                    receipt=self.receipt, view=b'exact view')
        self.files = [
            {'path': 'GROUNDING.yaml', 'role': 'record', 'before': b'old view', 'after': b'exact view'},
            {'path': '.kpopper/history/' + obj['subject'] + '/' + obj['id'] + '.yaml',
             'role': 'history_object', 'before': None, 'after': raw},
            {'path': '.kpopper/history-commits/op-1.yaml', 'role': 'history_commit',
             'before': None, 'after': H.encode_document(self.commit)}]

    def prepare(self, **changes):
        args = {'operation': 'op-1', 'authority': self.marker, 'baseline': self.baseline,
                'receipt': self.receipt, 'files': self.files}
        args.update(changes)
        return T.PreparedMutation(**args)

    def test_history_envelope_roundtrip_preserves_bound_commit_and_view(self):
        prepared = self.prepare()
        self.assertEqual(T.PreparedMutation.from_bytes(prepared.to_bytes()).to_bytes(),
                         prepared.to_bytes())

    def test_changed_baseline_or_receipt_cannot_reuse_old_commit(self):
        changed_baseline = {**self.baseline, 'committed_set_digest': identity('changed set')}
        changed_receipt = T.semantic_receipt(profile='core/v1', capabilities={},
                                             before={}, after={})
        for changes in ({'baseline': changed_baseline}, {'receipt': changed_receipt}):
            with self.subTest(field=next(iter(changes))), self.assertRaisesRegex(H.HistoryError, 'commit_mismatch'):
                self.prepare(**changes)

    def test_changed_view_or_missing_inventory_member_is_refused(self):
        changed = copy.deepcopy(self.files)
        changed[0]['after'] = b'other view'
        with self.assertRaisesRegex(H.HistoryError, 'commit_view_mismatch'):
            self.prepare(files=changed)
        with self.assertRaisesRegex(H.HistoryError, 'commit_inventory_mismatch'):
            self.prepare(files=[self.files[0], self.files[2]])

    def test_valid_bytes_cannot_be_redirected_with_a_false_file_role(self):
        for index in range(len(self.files)):
            files = copy.deepcopy(self.files)
            files[index]['path'] = 'unrelated.yaml'
            with self.subTest(role=files[index]['role']), self.assertRaisesRegex(H.HistoryError, 'role_path_mismatch'):
                self.prepare(files=files)

    def test_history_commit_cannot_be_replayed_by_legacy_publisher(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            with self.assertRaisesRegex(H.HistoryError, 'invalid_authority'):
                T.publish_legacy(root, '.kpopper/.transaction.json', self.prepare(),
                                 verify=lambda _: None)
            self.assertEqual(list(root.iterdir()), [])


if __name__ == '__main__':
    unittest.main()
