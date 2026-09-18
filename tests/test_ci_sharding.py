"""Parallel machines may accelerate tests but cannot drop or duplicate coverage."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('ci_runner', ROOT / '.github/scripts/ci_execution.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class ExecutionEvidence(unittest.TestCase):
    def fixture(self, directory):
        root = Path(directory)
        matrix = {'include': []}
        for python in ('3.9', '3.13'):
            for group, selected in ((1, ['a', 'b']), (2, ['c', 'd'])):
                matrix['include'].append({'python': python, 'group': group, 'splits': 2, 'workers': 2})
                (root / ('%s-%s.manifest.json' % (python, group))).write_text(json.dumps({
                    'python': python, 'group': group, 'splits': 2, 'consistent': True,
                    'collected': ['a', 'b', 'c', 'd'], 'selected': selected, 'executed': selected}))
        return matrix

    def test_every_interpreter_executes_the_complete_partition(self):
        with tempfile.TemporaryDirectory() as directory:
            matrix = self.fixture(directory)
            self.assertEqual(runner.verify_results(directory, matrix), {'3.9': 4, '3.13': 4})

    def test_missing_duplicate_disagreed_and_unexecuted_tests_fail_the_gate(self):
        for fault in ('missing_shard', 'duplicate_test', 'duplicate_execution',
                      'discovery_drift', 'unexecuted', 'worker_disagrees'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                matrix = self.fixture(directory)
                path = Path(directory) / '3.9-2.manifest.json'
                data = json.loads(path.read_text())
                if fault == 'missing_shard':
                    path.unlink()
                else:
                    if fault == 'duplicate_test':
                        data['selected'] = data['executed'] = ['a', 'c', 'd']
                    elif fault == 'duplicate_execution':
                        data['executed'].append('c')
                    elif fault == 'discovery_drift':
                        data['collected'].append('e')
                    elif fault == 'unexecuted':
                        data['executed'] = ['c']
                    else:
                        data['consistent'] = False
                    path.write_text(json.dumps(data))
                with self.assertRaises(ValueError):
                    runner.verify_results(directory, matrix)

    @unittest.skipUnless(importlib.util.find_spec('pytest_split'), 'split plugin is a Python CI dependency')
    def test_duration_split_covers_untimed_tests_once_and_balances_the_groups(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'tests').mkdir()
            (root / 'pyproject.toml').write_text('[tool.pytest.ini_options]\n')
            (root / 'tests/test_example.py').write_text('import unittest\nclass Cases(unittest.TestCase):\n' +
                ''.join('    def test_case_%02d(self): pass\n' % i for i in range(13)))
            durations = {'tests/test_example.py::Cases::test_case_%02d' % i: i + 1 for i in range(12)}
            path = root / 'durations.json'
            path.write_text(json.dumps(durations))
            base = [sys.executable, '-m', 'pytest', '--collect-only', '-q', 'tests']
            def collect(extra):
                p = subprocess.run(base + extra, cwd=root, capture_output=True, text=True, check=True)
                return {line for line in p.stdout.splitlines() if line.startswith('tests/') and '::' in line}
            all_tests = collect([])
            groups = [collect(['--splits', '2', '--group', str(group), '--splitting-algorithm',
                               'least_duration', '--durations-path', str(path)]) for group in (1, 2)]
            self.assertEqual(len(all_tests), 13)
            self.assertEqual(set.union(*groups), all_tests)
            self.assertFalse(set.intersection(*groups))
            estimates = [sum(durations.get(test, 6.5) for test in group) for group in groups]
            self.assertLessEqual(abs(estimates[0] - estimates[1]), 12)


if __name__ == '__main__':
    unittest.main()
