"""Focused regressions for reviewed history boundaries; no live activation.

After-image equality is evidence of bytes only. It cannot prove which operation
published them: another writer or an ABA sequence may have produced the same data.
"""
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts import history_contract as H, history_transaction as T, versions as V
from scripts.pending_grounding import _encode, json_bytes
from scripts.reasoning.snapshot import Snapshot, SnapshotError
from tests.test_history_contract import baseline, claim, projection, receipt


def legacy_mutation(*, before=b'before', after=b'after', path='GROUNDING.yaml', role='record'):
    return T.PreparedMutation(operation='review-op',
        authority=H.authority(record_id='record-1', authority='legacy', generation=0),
        baseline={}, receipt=receipt(),
        files=[{'path': path, 'role': role, 'before': before, 'after': after}])


class StrictDecoderRegressions(unittest.TestCase):
    def test_small_alias_dag_is_refused_before_detached_traversal(self):
        # Tiny enough to be safe even on the vulnerable implementation.
        raw = b'a: &a [x, x]\nb: &b [*a, *a]\nc: [*b, *b]\n'
        with mock.patch.object(H, 'detached', wraps=H.detached) as detached:
            with self.assertRaises(H.HistoryError) as caught:
                H.decode_document(raw)
            self.assertEqual(caught.exception.code, 'invalid_history_yaml')
            detached.assert_not_called()

    def test_encoder_shared_references_produce_alias_free_roundtrip(self):
        shared = {'v': [1, 2]}
        value = {'first': shared, 'second': shared}
        raw = H.encode_document(value)
        self.assertNotIn(b'&id', raw)
        self.assertNotIn(b'*id', raw)
        self.assertEqual(H.decode_document(raw), value)

    def test_invalid_decoded_value_types_and_depth_are_history_errors(self):
        inputs = [b'a: !!binary YQ==\n', b'a: !!set {x: null}\n',
                  b'a: !!omap [{x: 1}]\n', b'a: ' + b'[' * 140 + b']' * 140 + b'\n']
        for raw in inputs:
            with self.subTest(raw=raw[:30]), self.assertRaises(H.HistoryError):
                H.decode_document(raw)


class ProjectionRegressions(unittest.TestCase):
    def test_all_coverage_cannot_omit_baseline_heads_or_open_acts(self):
        for role in ('heads', 'open_acts'):
            context = projection(claim())
            context['baseline'][role]['p.omitted'] = ['a' * 64]
            with self.subTest(role=role), self.assertRaises(H.HistoryError):
                H.validate_projection(context)

    def test_selected_coverage_can_leave_unselected_baseline_subjects(self):
        context = projection(claim())
        context['coverage']['scope'] = 'selected'
        context['baseline']['heads']['p.omitted'] = ['a' * 64]
        self.assertEqual(H.validate_projection(context), context)

    def test_recorded_witness_requires_its_declared_identity_scheme(self):
        old = V.version('p.old', 'reading', 'source.a', {'v': 1}, on='2026-09-17', op='old')
        for obj, schemes in ((old, [H.ID_SCHEME]), (claim(), [H.LEGACY_SCHEME])):
            context = projection(obj)
            context['identity_schemes'] = schemes
            with self.subTest(schemes=schemes), self.assertRaises(H.HistoryError):
                H.validate_projection(context)
            context['identity_schemes'] = sorted([H.ID_SCHEME, H.LEGACY_SCHEME])
            self.assertEqual(H.validate_projection(context), context)

    def test_frozen_replay_maps_history_failure_to_invalid_history(self):
        context = projection(claim())
        document = {'readings': {'p.input': {'v': 1}}, 'meta': {'history': context['baseline']}}
        data = H.CapturedHistory(document, context).snapshot(as_of='2026-09-17').to_data()
        data['context']['history']['closure_digest'] = 'z' * 64
        for replay, value in ((Snapshot.from_snapshot, data),
                              (Snapshot.from_json, json_bytes(_encode(data)))):
            with self.subTest(replay=replay.__name__):
                with self.assertRaises(SnapshotError) as caught:
                    replay(value)
                self.assertEqual(caught.exception.code, 'invalid_history')


class CommitGraphRegressions(unittest.TestCase):
    def setUp(self):
        self.marker = H.authority(record_id='record-1', authority='history', generation=1)

    def chain(self, size):
        commits = {}
        previous = None
        for index in range(size):
            operation = 'op-' + str(index)
            parents = {} if previous is None else {previous: H.sha256(commits[previous])}
            commits[operation] = H.encode_document(H.make_commit(
                marker=self.marker, operation=operation, parents=parents,
                baseline=baseline(claim()), objects=[], receipt=receipt(), view=b'view'))
            previous = operation
        return commits

    def test_commit_count_is_bounded_before_manifest_decode(self):
        commits = self.chain(3)
        with mock.patch.object(H, 'MAX_OBJECTS', 2), mock.patch.object(H, 'decode_document', wraps=H.decode_document) as decode:
            with self.assertRaises(H.HistoryError) as caught:
                H.committed_objects(self.marker, commits, {})
            self.assertEqual(caught.exception.code, 'history_limit')
            decode.assert_not_called()

    def test_valid_chain_is_independent_of_captured_mapping_order(self):
        commits = self.chain(40)
        self.assertEqual(H.committed_objects(self.marker, dict(reversed(list(commits.items()))), {}), {})

    def test_cycle_is_refused_after_parent_byte_checks(self):
        # A real hash cycle needs a cryptographic fixed point. Mock only hashing
        # to exercise the separate graph check with otherwise valid manifests.
        commits = self.chain(2)
        for operation, parent in (('op-0', 'op-1'), ('op-1', 'op-0')):
            manifest = H.decode_document(commits[operation])
            manifest['parents'] = {parent: 'a' * 64}
            commits[operation] = H.encode_document(manifest)
        with mock.patch.object(H, 'sha256', return_value='a' * 64):
            with self.assertRaises(H.HistoryError) as caught:
                H.committed_objects(self.marker, commits, {})
            self.assertEqual(caught.exception.code, 'cyclic_commits')


class LegacyReferenceRegressions(unittest.TestCase):
    def test_sorted_legacy_duplicates_preserve_original_ids_and_envelopes(self):
        source = V.version('p.input', 'reading', 'source.a', {'v': 1}, on='2026-09-17', op='old')
        duplicate = V.version('p.input', 'reading', 'source.a', {'v': 2}, on='2026-09-17',
                              op='old-2', saw=[source['id'], source['id']])
        act = V.act('p.input', 'reviewer', 'accept', of=source['id'],
                    over=[source['id'], source['id']], on='2026-09-17', op='old-3')
        closure = {obj['id']: obj for obj in (source, duplicate, act)}
        self.assertEqual(H.validate_closure(closure), closure)

    def test_unsorted_legacy_references_still_refuse(self):
        obj = V.version('p.input', 'reading', 'source.a', {'v': 1}, on='2026-09-17', op='old')
        obj['saw'] = ['b' * 40, 'a' * 40]
        obj['id'] = V.ident(obj)
        with self.assertRaisesRegex(H.HistoryError, 'invalid_references'):
            H.validate_object(obj)

    def test_typed_objects_still_refuse_duplicate_saw_and_over(self):
        source = claim()
        for kind in ('reading', 'act'):
            obj = claim() if kind == 'reading' else H.make_object(
                subject='p.input', kind='act', by='reviewer', on='2026-09-17', operation='review',
                body={'act': 'review', 'of': source['id'], 'over': [], 'because': ''})
            if kind == 'reading':
                obj['saw'] = [source['id'], source['id']]
            else:
                obj['body']['over'] = [source['id'], source['id']]
            obj['id'] = H.object_identity(obj)
            with self.subTest(kind=kind), self.assertRaisesRegex(H.HistoryError, 'invalid_references'):
                H.validate_object(obj)


class FilesystemBoundaryRegressions(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name).resolve()
        self.real = self.base / 'real'
        self.real.mkdir()
        link = self.base / 'alias'
        link.symlink_to(self.real, target_is_directory=True)
        self.root = link / 'record'
        self.root.mkdir()
        self.journal = '.kpopper/.transaction.json'

    def test_chosen_root_with_symlink_ancestor_allows_publish_read_and_recovery(self):
        path = self.root / 'GROUNDING.yaml'
        path.write_bytes(b'before')
        mutation = legacy_mutation()
        T.publish_legacy(self.root, self.journal, mutation, verify=lambda _: None)
        with T.reader_guard(self.root, self.journal):
            self.assertEqual(path.read_bytes(), b'after')
        # A fully applied but not yet unlinked journal is a valid recovery state.
        (self.root / self.journal).write_bytes(mutation.to_bytes())
        restored = T.recover_legacy(self.root, self.journal, verify=lambda _: None, direction='before')
        self.assertEqual(restored.to_bytes(), mutation.to_bytes())
        self.assertEqual(path.read_bytes(), b'before')

    def test_immutable_publication_accepts_trusted_root_ancestor_only(self):
        path = self.root / 'objects' / 'one.yaml'
        T.publish_immutable(path, b'object', root=self.root)
        self.assertEqual(path.read_bytes(), b'object')
        outside = self.base / 'outside'
        outside.mkdir()
        (self.root / 'redirect').symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(H.HistoryError, 'symlink_path'):
            T.publish_immutable(self.root / 'redirect' / 'two.yaml', b'object', root=self.root)
        self.assertEqual(list(outside.iterdir()), [])

    def test_descendant_symlink_refuses_before_journal_or_target_changes(self):
        root = self.root.resolve()
        target = self.base / 'unrelated.yaml'
        target.write_bytes(b'before')
        (root / 'GROUNDING.yaml').symlink_to(target)
        with self.assertRaisesRegex(H.HistoryError, 'symlink_path'):
            T.publish_legacy(root, self.journal, legacy_mutation(), verify=lambda _: None)
        self.assertEqual(target.read_bytes(), b'before')
        self.assertFalse((root / '.kpopper').exists())

    def test_missing_recovery_journal_has_explicit_history_error(self):
        with self.assertRaises(H.HistoryError) as caught:
            T.recover_legacy(self.root.resolve(), self.journal, verify=lambda _: None)
        self.assertEqual(caught.exception.code, 'no_recovery_pending')
        self.assertEqual(list(self.root.iterdir()), [])

    def test_journal_ancestry_collisions_refuse_before_any_mutation(self):
        cases = [('GROUNDING.yaml/journal', legacy_mutation(before=None)),
                 ('.kpopper', legacy_mutation(before=None, path='.kpopper/replaced.yaml', role='replaced'))]
        for journal, mutation in cases:
            with self.subTest(journal=journal), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                verify = mock.Mock()
                with self.assertRaises(H.HistoryError) as caught:
                    T.publish_legacy(root, journal, mutation, verify=verify)
                self.assertEqual(caught.exception.code, 'invalid_journal_path')
                verify.assert_not_called()
                self.assertEqual(list(root.iterdir()), [])

    def test_completed_retry_refuses_with_bytes_only_evidence_code(self):
        root = self.root.resolve()
        path = root / 'GROUNDING.yaml'
        path.write_bytes(b'before')
        mutation = legacy_mutation()
        T.publish_legacy(root, self.journal, mutation, verify=lambda _: None)
        verify = mock.Mock()
        with self.assertRaises(H.HistoryError) as caught:
            T.publish_legacy(root, self.journal, mutation, verify=verify)
        self.assertEqual(caught.exception.code, 'after_images_match')
        self.assertEqual(path.read_bytes(), b'after')
        self.assertFalse((root / self.journal).exists())
        verify.assert_not_called()

    def test_matching_after_image_from_another_writer_is_not_success(self):
        root = self.root.resolve()
        path = root / 'GROUNDING.yaml'
        path.write_bytes(b'after')  # This operation has never been published.
        with self.assertRaises(H.HistoryError) as caught:
            T.publish_legacy(root, self.journal, legacy_mutation(), verify=lambda _: None)
        self.assertEqual(caught.exception.code, 'after_images_match')
        self.assertEqual(path.read_bytes(), b'after')
        self.assertFalse((root / '.kpopper').exists())


if __name__ == '__main__':
    unittest.main()
