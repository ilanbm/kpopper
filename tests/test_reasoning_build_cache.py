"""A cache hit can replace GMP compilation only for intact, matching inputs."""
import importlib.util
import pathlib
import tempfile
import unittest
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('cache_builder', ROOT / 'scripts/reasoning/build_runtime.py')
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class GmpCache(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.archive = self.root / 'source.tar.xz'
        self.archive.write_bytes(b'verified source fixture')
        self.patch = patch.object(builder, 'GMP_SHA256', builder.sha256(self.archive))
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def fake_build(self, archive, directory, target, *, replacement_probe=False):
        prefix = pathlib.Path(directory) / 'install'
        prefix.mkdir(parents=True, exist_ok=True)
        (prefix / 'library').write_bytes(b'replacement' if replacement_probe else b'original')
        return prefix

    def test_matching_hit_avoids_rebuild_and_corruption_rebuilds(self):
        with patch.object(builder, 'gmp_tool_versions', return_value={'cc': 'fixture 1'}), \
                patch.object(builder, 'build_gmp', side_effect=self.fake_build) as build:
            args = (self.archive, self.root / 'work', 'linux-x86_64')
            cached = builder.cached_gmp(*args, cache_dir=self.root / 'cache')
            self.assertEqual(build.call_count, 1)
            self.assertEqual(builder.cached_gmp(*args, cache_dir=self.root / 'cache'), cached)
            self.assertEqual(build.call_count, 1)
            (cached / 'library').write_bytes(b'corrupt')
            builder.cached_gmp(*args, cache_dir=self.root / 'cache')
            self.assertEqual(build.call_count, 2)
            self.assertEqual((cached / 'library').read_bytes(), b'original')

    def test_toolchain_and_replacement_variant_cannot_share_a_cached_prefix(self):
        with patch.object(builder, 'gmp_tool_versions', return_value={'cc': 'fixture 1'}) as versions, \
                patch.object(builder, 'build_gmp', side_effect=self.fake_build) as build:
            args = (self.archive, self.root / 'work', 'linux-x86_64')
            one = builder.cached_gmp(*args, cache_dir=self.root / 'cache')
            replacement = builder.cached_gmp(*args, cache_dir=self.root / 'cache', replacement_probe=True)
            versions.return_value = {'cc': 'fixture 2'}
            two = builder.cached_gmp(*args, cache_dir=self.root / 'cache')
            self.assertEqual(len({one, replacement, two}), 3)
            self.assertEqual(build.call_count, 3)
            self.assertEqual((replacement / 'library').read_bytes(), b'replacement')

    def test_failed_build_publishes_no_cache_receipt(self):
        with patch.object(builder, 'gmp_tool_versions', return_value={'cc': 'fixture'}), \
                patch.object(builder, 'build_gmp', side_effect=RuntimeError('make check failed')):
            with self.assertRaisesRegex(RuntimeError, 'make check failed'):
                builder.cached_gmp(self.archive, self.root / 'work', 'linux-x86_64',
                                   cache_dir=self.root / 'cache')
        self.assertEqual(list((self.root / 'cache').rglob('receipt.json')), [])

    def test_unverified_source_is_rejected_before_using_cache_or_building(self):
        self.archive.write_bytes(b'wrong source')
        with patch.object(builder, 'build_gmp') as build:
            with self.assertRaisesRegex(ValueError, 'source hash'):
                builder.cached_gmp(self.archive, self.root / 'work', 'linux-x86_64',
                                   cache_dir=self.root / 'cache')
            build.assert_not_called()


if __name__ == '__main__':
    unittest.main()
