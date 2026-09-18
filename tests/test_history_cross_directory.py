"""A logical record remains coherent when an authored shard lives elsewhere."""
import contextlib
import copy
import io
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import yaml
from scripts import provenance as P, history_transaction as T


def _hold_record(path, ready, release):
    with P._directory_locked(path):
        ready.set()
        if not release.wait(10):
            raise RuntimeError('test release timed out')


class CrossDirectory(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.entry = self.root / 'project' / 'GROUNDING.yaml'
        self.member = self.root / 'evidence' / 'decisions.yaml'
        self.entry.parent.mkdir()
        self.member.parent.mkdir()
        self.entry.write_text('record: ../evidence/decisions.yaml\nknown:\n  p.input: {v: 2}\n')
        self.member.write_text('decisions:\n  c.ready:\n    verdict: old\n    rests_on: [p.input]\n'
                               '    wrong_if: p.input > 1\n    seen: {p.input: 1}\n')
        self.action = {'kind': 'add', 'id': 'c.ready', 'body': {
            'verdict': 'new', 'rests_on': ['p.input'], 'wrong_if': 'p.input > 3'}}

    def apply(self):
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.entry)], copy.deepcopy(self.action))

    def fail_member(self):
        replace = T._replace
        def fail(path, data):
            if path == self.member:
                raise OSError('interrupted member')
            return replace(path, data)
        with mock.patch.object(T, '_replace', side_effect=fail):
            with self.assertRaisesRegex(OSError, 'interrupted member'):
                self.apply()

    def test_external_member_and_archive_publish_as_one_generation(self):
        self.apply()
        self.assertEqual(P.bodies(P.load([str(self.entry)]))['c.ready']['verdict'], 'new')
        self.assertEqual(P.read_replaced([str(self.entry)])['c.ready'][0]['verdict'], 'old')
        self.assertEqual(list(self.entry.parent.rglob('*.ready')), [])

    def test_absent_record_directory_keeps_the_missing_record_diagnostic(self):
        with self.assertRaisesRegex(SystemExit, 'no record here'):
            P.load([str(self.root / 'missing' / 'GROUNDING.yaml')], read_mode='frozen')

    def test_interrupted_generation_blocks_standalone_member_and_recovers(self):
        self.fail_member()
        for path in (self.entry, self.member):
            with self.subTest(path=path):
                with self.assertRaisesRegex((ValueError, P.Refused), 'recovery_required'):
                    P.load([str(path)])
        P.recover_direct([str(self.entry)])
        self.assertEqual(P.bodies(P.load([str(self.entry)]))['c.ready']['verdict'], 'new')
        self.assertEqual(P.bodies(P.load([str(self.member)]))['c.ready']['verdict'], 'new')

    def test_rollback_restores_exact_bytes_across_directories(self):
        before = self.entry.read_bytes(), self.member.read_bytes()
        self.fail_member()
        P.recover_direct([str(self.entry)], direction='before')
        self.assertEqual(before, (self.entry.read_bytes(), self.member.read_bytes()))
        self.assertFalse(Path(P.replaced_path([str(self.entry)])).exists())

    def test_recovery_never_overwrites_unrelated_external_edit(self):
        self.fail_member()
        newer = self.member.read_bytes() + b'\n# externally edited\n'
        self.member.write_bytes(newer)
        with self.assertRaisesRegex(ValueError, 'concurrent_edit'):
            P.recover_direct([str(self.entry)])
        self.assertEqual(self.member.read_bytes(), newer)

    def test_preparation_can_be_cancelled_without_overwriting_later_member_edit(self):
        with mock.patch.object(T, '_prepare_replicas', side_effect=OSError('guard preparation')):
            with self.assertRaisesRegex(OSError, 'guard preparation'):
                self.apply()
        newer = self.member.read_bytes() + b'\n# newer independent edit\n'
        self.member.write_bytes(newer)
        P.recover_direct([str(self.entry)], direction='before')
        self.assertEqual(self.member.read_bytes(), newer)
        self.assertEqual(P.bodies(P.load([str(self.entry)]))['c.ready']['verdict'], 'old')

    def test_local_guard_contains_only_member_names_and_operation_identity(self):
        self.fail_member()
        paths = list((self.member.parent / '.history-local').glob('*.json'))
        self.assertEqual(len(paths), 1)
        import json
        guard = json.loads(paths[0].read_bytes())
        self.assertEqual(guard['members'], [self.member.name])
        self.assertEqual(set(guard), {'version', 'kind', 'members', 'operation', 'digest'})
        self.assertNotIn(b'verdict', paths[0].read_bytes())

    def test_missing_ready_marker_cannot_discard_a_partially_published_generation(self):
        archive = Path(P.replaced_path([str(self.entry)]))
        replace = T._replace
        def fail(path, data):
            if path == archive:
                raise OSError('archive publication failed')
            return replace(path, data)
        with mock.patch.object(T, '_replace', side_effect=fail):
            with self.assertRaisesRegex(OSError, 'archive publication failed'):
                self.apply()
        self.assertIn('verdict: new', self.member.read_text())
        journal = self.entry.parent / T.journal_for(self.entry)
        mutation = T.PreparedMutation.from_bytes(journal.read_bytes())
        T._ready_path(journal, mutation).unlink()
        with self.assertRaisesRegex(ValueError, 'incomplete_readiness'):
            P.recover_direct([str(self.entry)], direction='before')
        self.assertTrue(journal.exists())

    def test_alias_paths_share_the_same_local_member_guard(self):
        alias = self.root / 'alias'
        alias.symlink_to(self.root / 'project', target_is_directory=True)
        original = self.entry
        self.entry = alias / 'GROUNDING.yaml'
        self.fail_member()
        with self.assertRaisesRegex((ValueError, P.Refused), 'recovery_required'):
            P.load([str(original)])
        P.recover_direct([str(original)])
        self.assertEqual(P.bodies(P.load([str(original)]))['c.ready']['verdict'], 'new')

    def test_opposite_entry_writers_acquire_shared_directories_in_one_order(self):
        import multiprocessing
        context = multiprocessing.get_context('spawn')
        peer = self.member.parent / 'GROUNDING.yaml'
        peer.write_text('record: ../project/GROUNDING.yaml\n')
        first_ready, second_ready = context.Event(), context.Event()
        release_first, release_second = context.Event(), context.Event()
        first = context.Process(target=_hold_record, args=(str(self.entry), first_ready, release_first))
        second = context.Process(target=_hold_record, args=(str(peer), second_ready, release_second))
        first.start()
        try:
            self.assertTrue(first_ready.wait(5))
            second.start()
            self.assertFalse(second_ready.wait(.2))
            release_first.set()
            self.assertTrue(second_ready.wait(5))
            release_second.set()
            first.join(5)
            second.join(5)
            self.assertEqual((first.exitcode, second.exitcode), (0, 0))
        finally:
            release_first.set()
            release_second.set()
            for process in (first, second):
                if process.pid is not None:
                    process.join(1)
                    if process.is_alive():
                        process.terminate()
                        process.join()


if __name__ == '__main__':
    unittest.main()
