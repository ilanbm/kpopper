"""Exact opaque subjects have portable keys; legacy bytes remain explicit inputs."""
import hashlib
from pathlib import Path
import tempfile
import unicodedata
import unittest
from unittest import mock

from scripts import history_paths as H
from scripts.reasoning.snapshot import Snapshot


class HistorySubjectPaths(unittest.TestCase):
    def test_supported_snapshot_subjects_keep_exact_text_and_distinct_paths(self):
        subjects = ['p.עלות', 'p.🧪', 'a/b', r'a\b', '.', '..', '', ' A ', 'nul\x00id',
                    'p.A', 'p.a', 'Caf\u00e9', 'Cafe\u0301', 'CON', 'aux', '~' + 'a' * 64]
        paths = []
        version = '1' * 64
        for subject in subjects:
            self.assertIn(subject, Snapshot.from_data({'known': {subject: {'v': 1}}}).to_data()['nodes'])
            self.assertEqual(H.validate_subject(subject), subject)
            path = H.object_path(subject, version)
            parsed = H.parse_object_path(path)
            self.assertEqual(parsed.scheme, H.HASHED)
            self.assertIsNone(parsed.legacy_subject)
            self.assertEqual(H.validate_object_path(path, subject, version), H.HASHED)
            self.assertEqual(len(path.split('/')), 2)
            self.assertEqual(len(parsed.directory), 65)
            paths.append(path)
        # Simulate the equivalences of case-insensitive, normalized filesystems.
        self.assertEqual(len(set(paths)), len(subjects))
        self.assertEqual(len({unicodedata.normalize('NFC', path).casefold() for path in paths}), len(subjects))
        self.assertEqual(len({unicodedata.normalize('NFD', path).casefold() for path in paths}), len(subjects))

    def test_new_paths_are_domain_separated_hashes_even_for_ascii(self):
        name = 'p.input'
        expected = '~' + hashlib.sha256(b'kpopper-history-subject-path/v1\x00' + name.encode()).hexdigest()
        self.assertEqual(H.subject_directory(name), expected)
        self.assertNotEqual(H.subject_directory(name), H.subject_directory(name, scheme=H.LEGACY))
        self.assertIsNone(H.LEGACY_DIRECTORY.fullmatch(expected))

    def test_paths_stay_under_root_for_traversal_and_absolute_subject_data(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            for index, subject in enumerate(('../../outside', '/absolute', 'C:\\Windows', 'a/../b', '\n../', '\x00')):
                relative = H.object_path(subject, format(index, '064x'))
                path = root / relative
                self.assertTrue(path.resolve().is_relative_to(root))
                path.parent.mkdir()
                path.write_bytes(subject.encode())
                self.assertEqual(path.read_bytes(), subject.encode())

    def test_existing_raw_paths_and_both_immutable_id_schemes_remain_exact(self):
        for version in ('a' * 40, 'b' * 64):
            original = 'p.Input/' + version + '.yaml'
            self.assertEqual(H.validate_object_path(original, 'p.Input', version), H.LEGACY)
            retained = H.parse_object_path(original)
            self.assertEqual(H.object_path('p.Input', version, scheme=retained.scheme), original)
            self.assertEqual(retained.legacy_subject, 'p.Input')
            self.assertEqual(H.resolve_object_path({original}, 'p.Input', version), original)
            H.validate_path_capability([original], [])

    def test_invalid_native_forms_or_subject_object_mismatches_refuse(self):
        version = 'a' * 64
        proper = H.object_path('p.עלות', version)
        for path in ('/' + proper, './' + proper, proper + '/', 'x/../' + proper,
                     proper.replace('/', '//'), proper.replace('/', '\\'),
                     proper.upper(), 'unknown/' + version + '.yml', '../' + version + '.yaml'):
            with self.subTest(path=path), self.assertRaises(H.HistoryPathError):
                H.parse_object_path(path)
        for subject, oid in [('p.other', version), ('p.עלות', 'b' * 64)]:
            with self.assertRaisesRegex(H.HistoryPathError, 'history_object_path_mismatch'):
                H.validate_object_path(proper, subject, oid)
        with self.assertRaisesRegex(H.HistoryPathError, 'history_object_path_mismatch'):
            H.validate_object_path('p.Input/' + version + '.yaml', 'p.input', version)

    def test_manifest_resolution_does_not_decode_unknown_orphan_bytes(self):
        version = 'a' * 64
        committed = H.object_path('p.עלות', version)
        orphan = H.object_path('orphan', 'b' * 64)
        storage = {committed: b'committed bytes', orphan: b'broken yaml : ['}
        with mock.patch('builtins.open', side_effect=AssertionError('not a filesystem reader')):
            self.assertEqual(H.resolve_object_path(storage, 'p.עלות', version), committed)
        self.assertEqual(storage[orphan], b'broken yaml : [')
        legacy = H.object_path('p.input', version, scheme=H.LEGACY)
        with H.replay_layout({}):
            self.assertEqual(H.resolve_object_path({legacy: b'old'}, 'p.input', version), legacy)
        self.assertTrue(H.object_path('p.input', version).startswith('~'))
        for requires in (['unknown/v1'], [H.CAPABILITY, H.CAPABILITY]):
            with self.assertRaisesRegex(H.HistoryPathError, 'invalid_history_path_capabilities'):
                with H.replay_layout({'requires': requires}):
                    self.fail('unsupported replay layout')

    def test_writer_replay_context_is_task_local_and_restored_on_error(self):
        from concurrent.futures import ThreadPoolExecutor
        version = 'a' * 64
        with self.assertRaisesRegex(RuntimeError, 'abort'):
            with H.replay_layout({}):
                self.assertEqual(H.object_path('p.input', version), 'p.input/' + version + '.yaml')
                self.assertIsNone(H.commit_requires())
                with ThreadPoolExecutor(max_workers=1) as pool:
                    self.assertTrue(pool.submit(H.object_path, 'p.input', version).result().startswith('~'))
                raise RuntimeError('abort')
        self.assertTrue(H.object_path('p.input', version).startswith('~'))

    def test_duplicate_physical_representations_fail_closed(self):
        version = 'a' * 64
        paths = {H.object_path('p.input', version), H.object_path('p.input', version, scheme=H.LEGACY)}
        with self.assertRaisesRegex(H.HistoryPathError, 'duplicate_history_object_path'):
            H.resolve_object_path(paths, 'p.input', version)
        with self.assertRaisesRegex(H.HistoryPathError, 'missing_history_object_path'):
            H.resolve_object_path(set(), 'p.input', version)

    def test_new_layout_requires_explicit_capability(self):
        from scripts import history_runtime as R
        self.assertIn('history_paths.py', R.REQUIRED)
        self.assertIn(H.CAPABILITY, R._history_schemas()['commit_capabilities'])
        self.assertIn(H.CAPABILITY, R._history_schemas()['bundle_capabilities'])
        paths = [H.object_path('p.input', 'a' * 64)]
        with self.assertRaisesRegex(H.HistoryPathError, 'subject_path_capability_required'):
            H.validate_path_capability(paths, [])
        H.validate_path_capability(paths, [H.CAPABILITY])
        with self.assertRaisesRegex(H.HistoryPathError, 'invalid_legacy_subject_path'):
            H.object_path('p.עלות', 'a' * 64, scheme=H.LEGACY)

    def test_invalid_text_and_bounded_encoding_refuse_without_coercion(self):
        for subject in (None, 1, b'text', [], {}):
            with self.assertRaisesRegex(H.HistoryPathError, 'invalid_history_subject'):
                H.subject_directory(subject)
        with self.assertRaisesRegex(H.HistoryPathError, 'invalid_subject_encoding'):
            H.subject_directory('\ud800')
        with mock.patch.object(H, 'MAX_SUBJECT_BYTES', 4):
            H.subject_directory('éé')
            with self.assertRaisesRegex(H.HistoryPathError, 'history_subject_limit'):
                H.subject_directory('ééa')


if __name__ == '__main__':
    unittest.main()
