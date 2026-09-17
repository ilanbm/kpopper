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
        manifest = {'version': 2, 'protocols': ['KP2', 'KP3'], 'target': target_name(),
                    'min_os': 'test fixture', 'lean_version': '4.33.1',
                    'source_sha256': source or source_hash(ROOT / 'lean'),
                    'modules': ['arithmetic/v1', 'composition/v1'],
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

    def test_implementation_audit_is_bound_per_request_without_changing_scalar_base(self):
        runtime = Runtime(self.archive())
        scalar = {'nodes': {}, 'declared': [], 'expression': {'num': '1'}}
        composed = {'protocol': 'KP3', 'nodes': {}, 'declared': [],
                    'expression': {'list': [{'num': '1'}]}}
        self.assertEqual(runtime.implementation['protocol'], 'KP2')
        self.assertEqual(runtime.implementation_for(scalar), runtime.implementation)
        implementation = runtime.implementation_for(composed)
        self.assertEqual(implementation['protocol'], 'KP3')
        self.assertEqual({key: value for key, value in implementation.items() if key != 'protocol'},
                         {key: value for key, value in runtime.implementation.items() if key != 'protocol'})

    def test_requests_and_responses_are_bound_to_their_protocols(self):
        runtime = Runtime(self.archive())
        scalar = {'nodes': {}, 'declared': [], 'expression': {'num': '1'}}
        composed = {'protocol': 'KP3', 'nodes': {}, 'declared': [], 'limits': {},
                    'expression': {'list': [{'num': '1'}]}}
        response = lambda protocol: '\t'.join((protocol, 'ok', 'n', '1', '1',
                                                '0', '0', '0', '1', '1', '0'))
        with patch('scripts.reasoning.runtime._run_bounded',
                   return_value=(response('KR2') + '\n' + response('KR3') + '\n').encode()):
            results = runtime.request_many([scalar, composed])
        self.assertEqual([result['value']['numerator'] for result in results], ['1', '1'])
        with patch('scripts.reasoning.runtime._run_bounded',
                   return_value=(response('KR3') + '\n').encode()):
            with self.assertRaisesRegex(RuntimeUnavailable, 'protocol does not match'):
                runtime.request(scalar)

    def test_unsupported_or_ambiguous_protocol_refuses_before_encoding(self):
        runtime = Runtime(self.archive())
        for protocol in ('KP2', 'KP4'):
            request = {'protocol': protocol, 'nodes': {}, 'declared': [], 'expression': {'num': '1'}}
            with self.subTest(protocol=protocol), \
                    patch('scripts.reasoning.transport.encode_request', side_effect=AssertionError('encoded')):
                with self.assertRaises(RuntimeUnavailable):
                    runtime.request(request)

    def test_executable_replacement_is_not_mislabeled_as_verified_source(self):
        archive = self.archive()
        runtime = Runtime(archive)
        runtime.binary.write_bytes(b'different program')
        with self.assertRaises(RuntimeUnavailable):
            Runtime(archive)

    def test_stale_source_archive_refuses_before_running_anything(self):
        with self.assertRaises(RuntimeUnavailable):
            Runtime(self.archive(source='0' * 64))

    def test_legacy_scalar_only_manifest_is_not_mislabeled_as_composition_capable(self):
        archive = self.archive()
        with zipfile.ZipFile(archive) as source:
            members = {name: source.read(name) for name in source.namelist()}
        manifest = json.loads(members['manifest.json'])
        manifest['protocols'] = ['KP2']
        manifest['modules'] = ['arithmetic/v1']
        members['manifest.json'] = json.dumps(manifest).encode()
        with zipfile.ZipFile(archive, 'w') as output:
            for name, data in members.items():
                output.writestr(name, data)
        with self.assertRaisesRegex(RuntimeUnavailable, 'manifest'):
            Runtime(archive)

    def test_archive_cannot_write_outside_its_cache(self):
        for path in ('../outside', 'C:/outside', 'licenses/NOTICE:stream'):
            with self.subTest(path=path), self.assertRaises(RuntimeUnavailable):
                Runtime(self.archive(extra=path))
        self.assertFalse((self.root / 'outside').exists())


if __name__ == '__main__':
    unittest.main()
