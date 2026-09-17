"""Source-copy runtime probes only: these are not T7 installed-runtime proof."""
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts import history_runtime as R


class Runtime(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.package = self.root / 'product'
        source = Path(R.__file__).resolve().parent
        for pattern in R.INCLUSION:
            for path in source.glob(pattern):
                target = self.package / path.relative_to(source)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, target)
        for name in ('assessment.schema.json', 'reasoning/assessment.schema.json'):
            shutil.copyfile(source / name, self.package / name)
        shutil.copytree(source / 'reasoning/lean', self.package / 'reasoning/lean')
        native = self.package / 'reasoning/native'
        native.mkdir()
        target = R.R.target_name() + '.zip'
        shutil.copyfile(source / 'reasoning/native' / target, native / target)
        self.nonce = '0123456789abcdef0123456789abcdef'
        self.code = (f'import sys,json; sys.path.insert(0,{str(self.root)!r}); '
                     'from product import history_runtime as runtime; '
                     'nonce=sys.argv[sys.argv.index("--nonce")+1]; value=runtime.describe(nonce); ')
        self.argv = [str(Path(sys.executable).resolve()), '-B', '-c', self.code + 'print(json.dumps(value))']
        self.item = {'id': 'managed', 'argv': self.argv, 'package_root': str(self.package),
                     'executable': str(Path(sys.executable).resolve())}
        self.declaration = self.read()
        self.expected = {'managed': {key: self.declaration[key]['digest']
                                    for key in ('sources', 'schemas', 'native')}}

    def read(self, argv=None):
        raw = subprocess.check_output([*(argv or self.argv), 'history', 'capabilities',
                                       '--nonce', self.nonce, '--json'])
        return json.loads(raw)

    def inventory(self):
        return {str(path.relative_to(self.root)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in self.root.rglob('*') if path.is_file()}

    def changed_launcher(self, suffix):
        return {**self.item, 'argv': [*self.argv[:-1], self.code + suffix]}

    def test_manifest_is_location_independent_and_description_is_read_only(self):
        before = self.inventory()
        value = self.read()
        self.assertEqual(before, self.inventory())
        self.assertEqual(value['sources']['inclusion'], R.INCLUSION)
        self.assertIn('history_activation.py', value['sources']['required'])
        for name in ('history_paths.py', 'history_identity.py', 'history_edits.py'):
            self.assertIn(name, value['sources']['required'])
        self.assertEqual(value['schemas']['history']['prepared_mutation'], [1, 2])
        self.assertEqual(value['schemas']['history']['authoring_receipt'], [1, 2, 3, 4, 5, 6])
        self.assertEqual(value['schemas']['history']['identity_receipt'], [1, 2])
        self.assertEqual(value['schemas']['history']['history_auxiliary'], [1])
        self.assertEqual(value['resolved']['package_root'], str(self.package))
        self.assertEqual(value['resolved']['cli'], str(self.package / 'cli.py'))
        self.assertEqual(value['native']['status'], 'archive_validated')
        self.assertEqual(value['native']['readiness'], 'not_tested')
        second = self.root / 'elsewhere'
        shutil.copytree(self.package, second / 'product')
        code = self.code.replace(repr(str(self.root)), repr(str(second))) + 'print(json.dumps(value))'
        relocated = self.read([*self.argv[:-1], code])
        self.assertEqual(value['sources'], relocated['sources'])
        self.assertEqual(value['schemas'], relocated['schemas'])
        self.assertEqual(value['native'], relocated['native'])
        self.assertFalse(any(path.name == '__pycache__' for path in self.root.rglob('*')))

    def test_valid_probe_binds_selected_inventory_and_fresh_nonce(self):
        proof = R.probe_launchers([self.item], self.expected, self.nonce)
        self.assertTrue(proof['complete'])
        self.assertEqual(proof['scope'], 'selected_managed_launchers')
        self.assertEqual(proof['launchers'][0]['declaration']['nonce'], self.nonce)
        self.assertEqual(proof['launchers'][0]['argv'][-5:],
                         ['history', 'capabilities', '--nonce', self.nonce, '--json'])
        self.assertEqual(proof['unlisted_launchers'], 'not_covered')
        self.assertEqual(proof['deployment_stability'], 'caller_responsibility')

    def test_wrong_nonce_unsupported_endpoint_and_extra_json_refuse_without_writes(self):
        cases = [
            ('value["nonce"]="wrong_nonce_0123456789"; print(json.dumps(value))', 'runtime_nonce_mismatch'),
            ('value["version"]=99; print(json.dumps(value))', 'unsupported_runtime_endpoint'),
            ('print(json.dumps(value)); print(json.dumps(value))', 'invalid_runtime_json'),
            ('print("old launcher: unsupported command")', 'invalid_runtime_json')]
        for suffix, code in cases:
            with self.subTest(code=code):
                before = self.inventory()
                with self.assertRaisesRegex(R.RuntimeDeclarationError, code):
                    R.probe_launchers([self.changed_launcher(suffix)], self.expected, self.nonce)
                self.assertEqual(before, self.inventory())

    def test_changed_sources_fail_preconfigured_digest(self):
        source = self.package / 'history_store.py'
        source.write_bytes(source.read_bytes() + b'\n# different deployment source\n')
        before = self.inventory()
        with self.assertRaisesRegex(R.RuntimeDeclarationError, 'runtime_digest_mismatch'):
            R.probe_launchers([self.item], self.expected, self.nonce)
        self.assertEqual(before, self.inventory())

    def test_wrong_package_or_interpreter_refuses(self):
        wrong = self.root / 'wrong'
        wrong.mkdir()
        for item in ({**self.item, 'package_root': str(wrong)},
                     {**self.item, 'executable': str(self.package / 'cli.py')}):
            with self.assertRaisesRegex(R.RuntimeDeclarationError, 'runtime_root_mismatch'):
                R.probe_launchers([item], self.expected, self.nonce)

    def test_missing_required_source_and_symlink_escape_refuse(self):
        source = self.package / 'history_store.py'
        raw = source.read_bytes()
        source.unlink()
        before = self.inventory()
        with self.assertRaisesRegex(R.RuntimeDeclarationError, 'launcher_probe_failed'):
            R.probe_launchers([self.item], self.expected, self.nonce)
        self.assertEqual(before, self.inventory())
        escaped = self.root / 'other.py'
        escaped.write_bytes(raw)
        source.symlink_to(escaped)
        before = self.inventory()
        with self.assertRaisesRegex(R.RuntimeDeclarationError, 'launcher_probe_failed'):
            R.probe_launchers([self.item], self.expected, self.nonce)
        self.assertEqual(before, self.inventory())

    def test_mixed_loaded_roots_and_bytecode_only_module_refuse(self):
        other = self.root / 'other.py'
        other.write_text('# unrelated source\n')
        for path in (str(other), str(self.package / 'history_store.pyc')):
            code = (f'import types; sys.modules["product.history_store"]=types.SimpleNamespace(__file__={path!r}, '
                    f'__spec__=types.SimpleNamespace(origin={path!r})); ')
            # Inject before describe, not after a declaration already exists.
            payload = self.code.replace('nonce=sys.argv', code + 'nonce=sys.argv') + 'print(json.dumps(value))'
            item = {**self.item, 'argv': [*self.argv[:-1], payload]}
            before = self.inventory()
            with self.assertRaisesRegex(R.RuntimeDeclarationError, 'launcher_probe_failed'):
                R.probe_launchers([item], self.expected, self.nonce)
            self.assertEqual(before, self.inventory())

    def test_absolute_launcher_ignores_old_path_decoy(self):
        binary = self.root / 'bin'
        binary.mkdir()
        called = self.root / 'decoy-called'
        for name in ('python3', 'kpopper'):
            decoy = binary / name
            decoy.write_text('#!/bin/sh\nprintf called > ' + str(called) + '\nexit 1\n')
            decoy.chmod(0o755)
        with mock.patch.dict(os.environ, {'PATH': str(binary)}):
            self.assertTrue(R.probe_launchers([self.item], self.expected, self.nonce)['complete'])
            with self.assertRaisesRegex(R.RuntimeDeclarationError, 'absolute_launcher_required'):
                R.probe_launchers([{**self.item, 'argv': ['kpopper']}], self.expected, self.nonce)
        self.assertFalse(called.exists())

    def test_output_and_time_are_bounded(self):
        commands = [([sys.executable, '-B', '-c', 'import time; time.sleep(5)'], .05),
                    ([sys.executable, '-B', '-c', 'print("x" * 3000000)'], 2)]
        for argv, timeout in commands:
            before = self.inventory()
            with self.assertRaisesRegex(R.RuntimeDeclarationError, 'launcher_probe_failed'):
                R.probe_launchers([{**self.item, 'argv': argv}], self.expected, self.nonce, timeout=timeout)
            self.assertEqual(before, self.inventory())

    def test_describe_does_not_extract_native_cache_and_unknown_schema_refuses(self):
        cache = self.root / 'native-cache-must-stay-absent'
        with mock.patch.dict(os.environ, {'XDG_CACHE_HOME': str(cache)}):
            self.assertTrue(R.probe_launchers([self.item], self.expected, self.nonce)['complete'])
        self.assertFalse(cache.exists())
        value = copy.deepcopy(self.declaration)
        value['schemas']['history']['commit'] = [99]
        value['schemas'] = R._seal({key: item for key, item in value['schemas'].items() if key != 'digest'})
        with self.assertRaisesRegex(R.RuntimeDeclarationError, 'unsupported_runtime_schema'):
            R._validate_declaration(value, self.nonce)

    def test_old_transition_schema_refuses_even_when_expected_digest_matches(self):
        value = copy.deepcopy(self.declaration)
        value['schemas']['history']['prepared_mutation'] = [1]
        value['schemas'] = R._seal({key: item for key, item in value['schemas'].items() if key != 'digest'})
        expected = {'managed': {key: value[key]['digest'] for key in ('sources', 'schemas', 'native')}}
        with mock.patch.object(R.R, '_run_bounded', return_value=json.dumps(value).encode()):
            with self.assertRaisesRegex(R.RuntimeDeclarationError, 'unsupported_runtime_schema'):
                R.probe_launchers([self.item], expected, self.nonce)

    def test_invalid_inventory_is_rejected_before_execution(self):
        with mock.patch.object(R.R, '_run_bounded', side_effect=AssertionError('must not run')):
            with self.assertRaisesRegex(R.RuntimeDeclarationError, 'invalid_expected_digests'):
                R.probe_launchers([self.item], {}, self.nonce)
            with self.assertRaisesRegex(R.RuntimeDeclarationError, 'invalid_launcher_inventory'):
                R.probe_launchers([self.item, self.item], self.expected, self.nonce)


if __name__ == '__main__':
    unittest.main()
