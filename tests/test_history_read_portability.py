"""Read-only installations do not acquire a POSIX write requirement."""
import builtins
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import history_contract as C, history_transaction as T, provenance as P


class ReadPortability(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.entry = self.root / 'GROUNDING.yaml'
        self.entry.write_text('readings:\n  p.input: {v: 1}\n')
        original = builtins.__import__
        def without_fcntl(name, *args, **kwargs):
            if name == 'fcntl':
                raise ImportError('simulated read-only Windows installation')
            return original(name, *args, **kwargs)
        self.patch = mock.patch('builtins.__import__', side_effect=without_fcntl)

    def test_ordinary_frozen_read_remains_available_without_fcntl(self):
        before = sorted(path.relative_to(self.root) for path in self.root.rglob('*'))
        with self.patch:
            self.assertEqual(P.load([str(self.entry)], read_mode='frozen')['readings']['p.input']['v'], 1)
        self.assertEqual(sorted(path.relative_to(self.root) for path in self.root.rglob('*')), before)

    def test_missing_posix_lock_never_permits_a_writer(self):
        with self.patch, self.assertRaisesRegex(C.HistoryError, 'locking_unavailable'):
            with T.writer_guard(self.root):
                self.fail('write lock was silently bypassed')

    def test_pending_journal_still_blocks_portable_reader(self):
        journal = self.root / T.journal_for(self.entry)
        journal.parent.mkdir(parents=True)
        journal.write_bytes(b'pending')
        with self.patch, self.assertRaisesRegex(C.HistoryError, 'recovery_required'):
            P.load([str(self.entry)], read_mode='frozen')

    def test_journal_appearing_during_unlocked_read_refuses(self):
        relative = T.journal_for(self.entry)
        journal = self.root / relative
        with self.patch, self.assertRaisesRegex(C.HistoryError, 'recovery_required'):
            with T.reader_guard(self.root, relative):
                journal.parent.mkdir(parents=True)
                journal.write_bytes(b'pending')


if __name__ == '__main__':
    unittest.main()
