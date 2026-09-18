"""CI execution keeps every selected test and rejects empty or unknown plans."""
import importlib.util
import pathlib
import shutil
import tempfile
import unittest
from unittest.mock import patch

import yaml


ROOT = pathlib.Path(__file__).resolve().parents[1]


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / '.github/scripts' / (name + '.py'))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


class TestPlans(unittest.TestCase):
    def test_shared_changes_keep_all_suites_and_documents_select_only_their_suite(self):
        ci = module('ci_selection')
        self.assertEqual(ci.test_suites(ci.select(['scripts/cli.py'])),
                         ['core', 'documents', 'reasoning', 'session', 'other'])
        self.assertEqual(ci.test_suites(ci.select(['scripts/document/layer.js'])), ['documents'])
        self.assertEqual(ci.test_suites(ci.select(['README.md'])), [])

    def test_every_test_file_belongs_to_exactly_one_suite_including_new_tests(self):
        runner = module('ci_execution')
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            names = ['test_core_authoring.py', 'test_history_runtime.py', 'test_documents.py',
                     'test_reasoning_runtime.py', 'test_session.py', 'test_brand_new_area.py']
            for name in names:
                (root / name).write_text('')
            groups = [runner.test_files([suite], root) for suite in runner.SUITES]
            flattened = [p for group in groups for p in group]
            self.assertEqual(sorted(flattened), sorted(root / name for name in names))
            self.assertEqual(len(flattened), len(set(flattened)))

    def test_unknown_empty_and_unmatched_selections_cannot_succeed(self):
        runner = module('ci_execution')
        with tempfile.TemporaryDirectory() as directory:
            for suites in ([], ['typo'], ['documents'], 'documents'):
                with self.subTest(suites=suites), self.assertRaises(ValueError):
                    runner.test_files(suites, pathlib.Path(directory))

    def test_nested_tests_keep_the_full_suite_coverage(self):
        runner = module('ci_execution')
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'nested').mkdir()
            (root / 'nested/__init__.py').write_text('')
            nested = root / 'nested/test_new.py'
            nested.write_text('')
            self.assertEqual(runner.test_files(['other'], root), [nested])

    def test_all_existing_platform_and_python_combinations_remain_required(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/reasoning-runtime.yml').read_text())
        import json
        targets = workflow['jobs']['target']['strategy']['matrix']['include']
        actual = {(row['target'], python) for row in targets for python in json.loads(row['pythons'])}
        expected = {(target, python) for target in ('linux-x86_64', 'linux-aarch64',
                    'darwin-arm64', 'darwin-x86_64', 'windows-x86_64') for python in ('3.9', '3.13')}
        expected.remove(('darwin-arm64', '3.9'))
        self.assertEqual(actual, expected)

    def test_installed_jobs_wait_only_for_their_own_target(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/reasoning-target.yml').read_text())
        self.assertEqual(workflow['jobs']['installed']['needs'], 'build')
        self.assertNotIn('matrix', workflow['jobs']['build'].get('strategy', {}))
        self.assertEqual(workflow['jobs']['installed']['runs-on'], '${{ inputs.runner }}')
        downloads = [s for s in workflow['jobs']['installed']['steps']
                     if s.get('uses', '').startswith('actions/download-artifact@')]
        self.assertEqual(downloads[0]['with']['name'], 'gmp-replacement-${{ inputs.target }}')
        self.assertNotIn('pattern', downloads[0]['with'])

    def test_invalid_committed_bundles_gate_every_expensive_family(self):
        jobs = yaml.safe_load((ROOT / '.github/workflows/check.yml').read_text())['jobs']
        self.assertTrue(any('--check-bundles' in s.get('run', '') for s in jobs['changes']['steps']))
        for name in ('check', 'document-ui', 'session', 'reasoning-runtime'):
            self.assertEqual(set(jobs[name]['needs']), {'changes', 'record'})
        native = yaml.safe_load((ROOT / '.github/workflows/reasoning-runtime.yml').read_text())['jobs']
        self.assertEqual(native['target']['needs'], 'preflight')

    def test_candidate_only_skips_integrity_step_but_keeps_the_build_prerequisite(self):
        jobs = yaml.safe_load((ROOT / '.github/workflows/reasoning-runtime.yml').read_text())['jobs']
        self.assertNotIn('if', jobs['preflight'])
        integrity = next(s for s in jobs['preflight']['steps'] if '--check-bundles' in s.get('run', ''))
        self.assertIn("github.event_name != 'workflow_dispatch'", integrity['if'])
        self.assertIn('!inputs.candidate-only', integrity['if'])
        self.assertIn("github.event_name == 'workflow_dispatch'", jobs['target']['with']['rebuild'])
        self.assertIn('!inputs.candidate-only', jobs['target']['with']['validate-installed'])

    @unittest.skipUnless(importlib.util.find_spec('pytest') and importlib.util.find_spec('xdist'),
                         'the parallel runner is installed in the Python CI job')
    def test_sibling_import_support_does_not_leak_into_child_processes(self):
        runner = module('ci_execution')
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'tests').mkdir()
            (root / 'tests/__init__.py').write_text('')
            shutil.copyfile(ROOT / 'tests/ci_pytest.py', root / 'tests/ci_pytest.py')
            (root / 'isolated').mkdir()
            (root / 'pyproject.toml').write_text('[tool.pytest.ini_options]\n')
            (root / 'tests/test_child.py').write_text('''import json, pathlib, subprocess, sys, unittest
class ChildPath(unittest.TestCase):
    def test_child_has_no_source_tree(self):
        root = pathlib.Path(__file__).resolve().parents[1]
        paths = json.loads(subprocess.check_output([sys.executable, '-c',
            'import json,sys; print(json.dumps(sys.path))'], cwd=root/'isolated', text=True))
        self.assertNotIn(str(root), paths)
        self.assertNotIn(str(root/'tests'), paths)
''')
            with patch.object(runner, '__file__', str(root / '.github/scripts/ci_execution.py')), \
                    patch.object(runner.sys, 'argv', ['ci_execution.py', '--suites', '["other"]',
                        '--workers', '1', '--junitxml', str(root / 'result.xml')]), \
                    patch.dict(runner.os.environ, {'PYTHONPATH': ''}):
                self.assertEqual(runner.main(), 0)
            import json
            manifest = json.loads((root / 'result.manifest.json').read_text())
            self.assertTrue(manifest['consistent'])
            self.assertEqual(manifest['collected'], manifest['executed'])
            self.assertEqual(len(manifest['executed']), 1)


if __name__ == '__main__':
    unittest.main()
