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
        if suite_for(path) in suites:
            files.append(path)
    if not files:
        raise ValueError('the selected test suites contain no tests')
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--suites', default=json.dumps(list(SUITES)))
    parser.add_argument('--workers', type=int, default=min(4, os.cpu_count() or 1))
    parser.add_argument('--junitxml', default='test-results/python.xml')
    args = parser.parse_args()
    if args.workers < 1:
        parser.error('--workers must be positive')
    root = Path(__file__).resolve().parents[2]
    suites = json.loads(args.suites)
    files = test_files(suites, root / 'tests')
    print(json.dumps({'suites': suites, 'test_files': [p.relative_to(root).as_posix() for p in files],
                      'workers': args.workers}, indent=2), flush=True)
    # Match unittest's sibling-fixture imports without exporting a source-tree
    # PYTHONPATH into the child processes that test isolated installations.
    return subprocess.call([sys.executable, '-m', 'pytest', '-q', '-n', str(args.workers),
                            '--dist', 'loadfile', '--durations=30', '--durations-min=1',
                            '--junitxml=' + args.junitxml, '-o', 'python_functions=',
                            '-o', 'python_classes=', '-o', 'pythonpath=tests',
                            *map(str, files)], cwd=root)


if __name__ == '__main__':
    raise SystemExit(main())
