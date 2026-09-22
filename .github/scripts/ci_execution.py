"""Execute an explicit CI test plan, keeping unittest discovery boundaries."""
import argparse
import fnmatch
import json
import os
from pathlib import Path
import subprocess
import sys

SUITES = ('core', 'documents', 'reasoning', 'session', 'other')
PATTERNS = {
    'core': ('test_core*.py', 'test_history*.py', 'test_mode*.py', 'test_contribution*.py'),
    'documents': ('test_document*.py',),
    'reasoning': ('test_reasoning*.py',),
    'session': ('test_session*.py', 'test_structured*.py', 'test_export*.py',
                'test_predicate_literals.py', 'test_expression_authoring.py',
                'test_readable_expressions.py', 'test_process_boundaries.py', 'test_assessment.py'),
}


# The record job runs these on every pull request. Leaving them out of the shards keeps the
# files only they read (the README, community documents, release scripts) out of the Python
# lane's inputs, and runs each once.
RECORD_JOB_MODULES = ('test_skills.py', 'test_release.py', 'test_ci_selection.py',
                      'test_ci_execution.py', 'test_ci_sharding.py', 'test_ci_audit.py')


def suite_for(path):
    for suite, patterns in PATTERNS.items():
        if any(fnmatch.fnmatch(path.name, pattern) for pattern in patterns):
            return suite
    # New tests are covered before someone classifies them more precisely.
    return 'other'


def test_files(suites, root):
    if not isinstance(suites, list) or not suites or any(s not in SUITES for s in suites):
        raise ValueError('expected a nonempty list of known test suites')
    root = Path(root)
    files = []
    for path in sorted(root.rglob('test_*.py')):
        # unittest only descends into importable test packages, not fixture trees.
        if any(not (parent / '__init__.py').is_file()
               for parent in path.parents if parent != root and root in parent.parents):
            continue
        if path.name in RECORD_JOB_MODULES and path.parent == root:
            continue
        if suite_for(path) in suites:
            files.append(path)
    if not files:
        raise ValueError('the selected test suites contain no tests')
    return files


def verify_results(directory, matrix):
    """Every interpreter must execute a disjoint, complete partition of its tests."""
    expected = {(row['python'], row['group']): row['splits'] for row in matrix['include']}
    if not expected or len(expected) != len(matrix['include']):
        raise ValueError('invalid expected test matrix')
    manifests = {}
    for path in Path(directory).rglob('*.manifest.json'):
        data = json.loads(path.read_text(encoding='utf-8'))
        key = (data['python'], data['group'])
        if key not in expected or key in manifests or data['splits'] != expected[key]:
            raise ValueError('unexpected or duplicate shard manifest: ' + str(key))
        for field in ('collected', 'selected', 'executed'):
            values = data[field]
            if not isinstance(values, list) or not values or len(values) != len(set(values)):
                raise ValueError('empty or duplicate test identities: ' + field)
        if not data['consistent'] or set(data['executed']) != set(data['selected']):
            raise ValueError('a worker omitted or added selected tests: ' + str(key))
        manifests[key] = data
    if set(manifests) != set(expected):
        raise ValueError('missing required shard manifests')
    counts = {}
    for python in sorted({key[0] for key in expected}):
        groups = [data for (version, _), data in manifests.items() if version == python]
        collected, executed = set(groups[0]['collected']), set()
        for data in groups:
            selected = set(data['selected'])
            if set(data['collected']) != collected or not selected <= collected or executed & selected:
                raise ValueError('shards disagree about discovery or duplicate tests: ' + python)
            executed |= selected
        if executed != collected:
            raise ValueError('shards did not cover every collected test: ' + python)
        counts[python] = len(collected)
    return counts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--suites', default=json.dumps(list(SUITES)))
    parser.add_argument('--workers', type=int, default=min(4, os.cpu_count() or 1))
    parser.add_argument('--junitxml', default='test-results/python.xml')
    parser.add_argument('--splits', type=int, default=1)
    parser.add_argument('--group', type=int, default=1)
    parser.add_argument('--verify-results', type=Path)
    parser.add_argument('--matrix')
    args = parser.parse_args()
    if args.verify_results:
        print(json.dumps({'tests_executed_exactly_once':
                          verify_results(args.verify_results, json.loads(args.matrix))}, sort_keys=True))
        return 0
    if args.workers < 1:
        parser.error('--workers must be positive')
    if not 1 <= args.group <= args.splits:
        parser.error('--group must be between 1 and --splits')
    root = Path(__file__).resolve().parents[2]
    suites = json.loads(args.suites)
    files = test_files(suites, root / 'tests')
    print(json.dumps({'suites': suites, 'test_files': [p.relative_to(root).as_posix() for p in files],
                      'workers': args.workers, 'group': args.group, 'splits': args.splits}, indent=2), flush=True)
    # Match unittest's sibling-fixture imports without exporting a source-tree
    # PYTHONPATH into the child processes that test isolated installations.
    split_args = (['--splits', str(args.splits), '--group', str(args.group),
                   '--splitting-algorithm', 'least_duration',
                   '--durations-path', str(root / '.github/test-durations.json')]
                  if args.splits > 1 else [])
    return subprocess.call([sys.executable, '-m', 'pytest', '-vv', '-n', str(args.workers),
                            '--dist', 'worksteal', '--durations=30', '--durations-min=1',
                            '-p', 'tests.ci_pytest', '--ci-manifest',
                            str(Path(args.junitxml).with_suffix('.manifest.json')),
                            '--junitxml=' + args.junitxml, '-o', 'python_functions=',
                            '-o', 'python_classes=', '-o', 'pythonpath=tests',
                            *split_args, *map(str, files)], cwd=root)


if __name__ == '__main__':
    raise SystemExit(main())
