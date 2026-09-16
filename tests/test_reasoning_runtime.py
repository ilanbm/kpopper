"""A packaged executable is immutable; LGPL shared libraries remain replaceable."""
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from scripts.reasoning.runtime import Runtime, RuntimeUnavailable, source_hash, target_name

ROOT = Path(__file__).resolve().parents[1] / 'scripts/reasoning'


class RuntimeBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.env = patch.dict(os.environ, {'XDG_CACHE_HOME': str(self.root / 'cache')})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.addCleanup(self.temp.cleanup)

    def archive(self, *, source=None, extra=None):
        files = {'evaluator': b'original executable', 'library': b'original shared library',
                 'licenses/NOTICE': b'redistribution notices'}
        manifest = {'version': 1, 'protocol': 'KP2', 'target': target_name(), 'lean_version': '4.33.1',
                    'source_sha256': source or source_hash(ROOT / 'lean'), 'modules': ['arithmetic/v1'],
                    'executable': 'evaluator', 'libraries': ['library'],
                    'files': {name: hashlib.sha256(data).hexdigest() for name, data in files.items()}}
        path = self.root / 'runtime.zip'
        with zipfile.ZipFile(path, 'w') as output:
            for name, data in files.items():
                output.writestr(name, data)
            output.writestr('manifest.json', json.dumps(manifest))
            if extra:
                output.writestr(extra, b'escape')
        return path

    def test_shared_library_replacement_is_allowed_and_identity_bound(self):
        archive = self.archive()
        before = Runtime(archive)
        executable_hash = before.implementation['binary_sha256']
        (before.root / 'library').write_bytes(b'user-modified library')
        after = Runtime(archive)
        self.assertEqual(after.implementation['binary_sha256'], executable_hash)
        self.assertEqual(after.implementation['modified_libraries'], ['library'])
        self.assertNotEqual(before.implementation['libraries'], after.implementation['libraries'])

    def test_executable_replacement_is_not_mislabeled_as_verified_source(self):
        archive = self.archive()
        runtime = Runtime(archive)
        runtime.binary.write_bytes(b'different program')
        with self.assertRaises(RuntimeUnavailable):
            Runtime(archive)

    def test_stale_source_archive_refuses_before_running_anything(self):
        with self.assertRaises(RuntimeUnavailable):
            Runtime(self.archive(source='0' * 64))

    def test_archive_cannot_write_outside_its_cache(self):
        for path in ('../outside', 'C:/outside', 'licenses/NOTICE:stream'):
            with self.subTest(path=path), self.assertRaises(RuntimeUnavailable):
                Runtime(self.archive(extra=path))
        self.assertFalse((self.root / 'outside').exists())


if __name__ == '__main__':
    unittest.main()
