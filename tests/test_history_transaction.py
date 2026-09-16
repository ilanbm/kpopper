"""Interruption, replay and concurrent edits at the prepared publication boundary."""
import tempfile
import multiprocessing
import unittest
from pathlib import Path
from unittest import mock

from scripts import history_contract as H, history_transaction as T
from tests.test_history_contract import receipt


def _fork_reader(root, journal, connection):
    connection.send('started')
    with T.reader_guard(root, journal):
        connection.send('entered')
    connection.close()


class PreparedWrites(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.journal = '.kpopper/.transaction.json'
        (self.root / '.kpopper').mkdir()
        (self.root / 'GROUNDING.yaml').write_bytes(b'record-before')
        (self.root / '.kpopper/replaced.yaml').write_bytes(b'archive-before')
        self.mutation = T.PreparedMutation(operation='op-1',
            authority=H.authority(record_id='record-1', authority='legacy', generation=0),
            baseline={'revision': 'captured-before'}, receipt=receipt(), files=[
                {'path': 'GROUNDING.yaml', 'role': 'record', 'before': b'record-before', 'after': b'record-after'},
                {'path': '.kpopper/replaced.yaml', 'role': 'replaced', 'before': b'archive-before', 'after': b'archive-after'}])

    def fail_after_archive(self):
        original = T._replace
        def injected(path, data):
            if path.name == 'GROUNDING.yaml':
                raise OSError('injected record publication failure')
            return original(path, data)
        with mock.patch.object(T, '_replace', side_effect=injected):
            with self.assertRaisesRegex(OSError, 'injected'):
                T.publish_legacy(self.root, self.journal, self.mutation, verify=lambda _: None)

    def test_roundtrip_retains_every_file_and_evidence(self):
        replay = T.PreparedMutation.from_bytes(self.mutation.to_bytes())
        self.assertEqual(replay.files, self.mutation.files)
        self.assertEqual(replay.to_bytes(), self.mutation.to_bytes())

    def test_normal_publication_is_observable_after_guard(self):
        T.publish_legacy(self.root, self.journal, self.mutation, verify=lambda _: None)
        with T.reader_guard(self.root, self.journal):
            self.assertEqual((self.root / 'GROUNDING.yaml').read_bytes(), b'record-after')
            self.assertEqual((self.root / '.kpopper/replaced.yaml').read_bytes(), b'archive-after')
        self.assertFalse((self.root / self.journal).exists())

    def test_writer_lock_allows_nested_preparation_and_readback(self):
        # Same lock as ordinary provenance writers: nested publication must not hang.
        from scripts import provenance as P
        with P._directory_locked(self.root / 'GROUNDING.yaml'):
            T.publish_legacy(self.root, self.journal, self.mutation, verify=lambda _: None)
            with T.reader_guard(self.root, self.journal):
                self.assertEqual((self.root / 'GROUNDING.yaml').read_bytes(), b'record-after')

    def test_read_lock_cannot_silently_upgrade_to_writer(self):
        with T.reader_guard(self.root, self.journal):
            with self.assertRaisesRegex(H.HistoryError, 'lock_upgrade_refused'):
                T.publish_legacy(self.root, self.journal, self.mutation, verify=lambda _: None)

    @unittest.skipUnless('fork' in multiprocessing.get_all_start_methods(), 'POSIX fork required')
    def test_fork_does_not_inherit_reentrant_lock_authority(self):
        context = multiprocessing.get_context('fork')
        parent, child = context.Pipe()
        process = context.Process(target=_fork_reader, args=(self.root, self.journal, child))
        try:
            with T.writer_guard(self.root):
                process.start()
                self.assertTrue(parent.poll(5))
                self.assertEqual(parent.recv(), 'started')
                self.assertFalse(parent.poll(.15), 'child bypassed parent directory lock')
            self.assertTrue(parent.poll(5))
            self.assertEqual(parent.recv(), 'entered')
            process.join(5)
            self.assertEqual(process.exitcode, 0)
        finally:
            if process.is_alive():
                process.terminate()
                process.join(5)
            parent.close()
            child.close()

    def test_interrupted_multi_file_state_refuses_read_then_recovers_same_operation(self):
        self.fail_after_archive()
        self.assertEqual((self.root / '.kpopper/replaced.yaml').read_bytes(), b'archive-after')
        self.assertEqual((self.root / 'GROUNDING.yaml').read_bytes(), b'record-before')
        with self.assertRaisesRegex(H.HistoryError, 'recovery_required'):
            with T.reader_guard(self.root, self.journal):
                self.fail('mixed generation exposed')
        recovered = T.recover_legacy(self.root, self.journal, verify=lambda _: None)
        self.assertEqual(recovered.to_bytes(), self.mutation.to_bytes())
        self.assertEqual((self.root / 'GROUNDING.yaml').read_bytes(), b'record-after')

    def test_interrupted_mutation_can_restore_exact_before_bytes(self):
        self.fail_after_archive()
        T.recover_legacy(self.root, self.journal, verify=lambda _: None, direction='before')
        self.assertEqual((self.root / '.kpopper/replaced.yaml').read_bytes(), b'archive-before')
        self.assertEqual((self.root / 'GROUNDING.yaml').read_bytes(), b'record-before')

    def test_concurrent_unrelated_edit_blocks_recovery_without_more_mutation(self):
        self.fail_after_archive()
        (self.root / 'GROUNDING.yaml').write_bytes(b'unrelated edit')
        saved = (self.root / self.journal).read_bytes()
        for direction in ('before', 'after'):
            with self.assertRaisesRegex(H.HistoryError, 'concurrent_edit'):
                T.recover_legacy(self.root, self.journal, verify=lambda _: None, direction=direction)
            self.assertEqual((self.root / self.journal).read_bytes(), saved)
            self.assertEqual((self.root / '.kpopper/replaced.yaml').read_bytes(), b'archive-after')

    def test_verifier_failure_changes_nothing(self):
        def refused(_):
            raise H.HistoryError('capability_changed')
        with self.assertRaisesRegex(H.HistoryError, 'capability_changed'):
            T.publish_legacy(self.root, self.journal, self.mutation, verify=refused)
        self.assertFalse((self.root / self.journal).exists())
        self.assertEqual((self.root / '.kpopper/replaced.yaml').read_bytes(), b'archive-before')

    def test_immutable_replay_is_byte_exact(self):
        path = self.root / 'object.yaml'
        T.publish_immutable(path, b'first')
        T.publish_immutable(path, b'first')
        with self.assertRaisesRegex(H.HistoryError, 'immutable_collision'):
            T.publish_immutable(path, b'second')
        self.assertEqual(path.read_bytes(), b'first')

    def test_symlink_cannot_redirect_prepared_write(self):
        (self.root / 'GROUNDING.yaml').unlink()
        target = self.root / 'unrelated.yaml'
        target.write_bytes(b'record-before')
        (self.root / 'GROUNDING.yaml').symlink_to(target)
        with self.assertRaisesRegex(H.HistoryError, 'symlink_path'):
            T.publish_legacy(self.root, self.journal, self.mutation, verify=lambda _: None)
        self.assertEqual(target.read_bytes(), b'record-before')
        self.assertFalse((self.root / self.journal).exists())

    def test_altered_journal_never_executes(self):
        raw = self.mutation.to_bytes().replace(b'captured-before', b'captured-changed')
        with self.assertRaisesRegex(H.HistoryError, 'invalid_journal'):
            T.PreparedMutation.from_bytes(raw)

    def test_preflight_checks_all_paths_before_any_write(self):
        (self.root / 'GROUNDING.yaml').write_bytes(b'changed')
        with self.assertRaisesRegex(H.HistoryError, 'concurrent_edit'):
            T.publish_legacy(self.root, self.journal, self.mutation, verify=lambda _: None)
        self.assertEqual((self.root / '.kpopper/replaced.yaml').read_bytes(), b'archive-before')
        self.assertFalse((self.root / self.journal).exists())


if __name__ == '__main__':
    unittest.main()
