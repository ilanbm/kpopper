"""The read audit maps what a lane opened to tracked files and reports what it does not declare."""
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / '.github/scripts'))
spec = importlib.util.spec_from_file_location('ci_audit', ROOT / '.github/scripts/ci_audit.py')
AUDIT = importlib.util.module_from_spec(spec)
spec.loader.exec_module(AUDIT)
import ci_selection as CI  # noqa: E402


def repository(files):
    directory = tempfile.TemporaryDirectory()
    root = Path(directory.name)
    for name in files:
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text('x')
    subprocess.check_call(['git', 'init', '-q'], cwd=root)
    subprocess.check_call(['git', 'add', '-A'], cwd=root)
    return directory, str(root.resolve())


class Mapping(unittest.TestCase):
    def test_checkout_files_and_bytecode_map_to_tracked_sources(self):
        cases = {
            '/work/repo/tests/test_ci_audit.py': 'tests/test_ci_audit.py',
            '/work/repo/tests/__pycache__/test_x.cpython-313.pyc': 'tests/test_x.py',
            '/work/repo': '',
            '/work/repo/native/src': 'native/src',
            '/work/repository/native/src/main.rs': None,
            '/tmp/plugin-stage/scripts/hook.sh': None,
        }
        for opened, expected in cases.items():
            with self.subTest(opened=opened):
                self.assertEqual(AUDIT.repository_path(opened, '/work/repo'), expected)


class Undeclared(unittest.TestCase):
    lanes = {'demo': CI.Lane(reads=('scripts/*', 'tests/*'), lists=('scripts*',))}

    def test_reads_and_listings_outside_the_declaration_are_reported(self):
        directory, root = repository(['scripts/hook.sh', 'tests/test_cli.py', 'docs/guide.md',
                                      'assets/logo.png', 'untracked-later/x'])
        with directory:
            (Path(root) / 'build').mkdir()
            (Path(root) / 'build/output.txt').write_text('generated')
            opened = [root + '/scripts/hook.sh', root + '/tests/test_cli.py', root + '/docs/guide.md',
                      root + '/scripts', root + '/assets', root, root + '/build/output.txt',
                      root + '/.git/index', '/usr/lib/python3/os.py']
            files, directories = AUDIT.undeclared('demo', opened, root, lanes=self.lanes)
            self.assertEqual(files, ['docs/guide.md'])
            self.assertEqual(directories, ['', 'assets'])

    def test_record_job_files_are_undeclared_for_every_lane(self):
        directory, root = repository(['tests/test_release.py', 'tests/test_other.py'])
        with directory:
            files, _ = AUDIT.undeclared('demo', [root + '/tests/test_release.py', root + '/tests/test_other.py'],
                                        root, lanes=self.lanes)
            self.assertEqual(files, ['tests/test_release.py'])

    def test_the_workflows_audit_exactly_the_lanes_declared_audited(self):
        runs = []
        for path in (ROOT / '.github/workflows').glob('*.yml'):
            for job in yaml.safe_load(path.read_text())['jobs'].values():
                runs += [step.get('run', '') for step in job.get('steps', [])]
        finished = set()
        for run in runs:
            for argument in re.findall(r'ci_audit\.py finish --lane (\S+(?: [^}]*}})?)', run):
                finished |= set(re.findall(r"'(\w+)'", argument)) - {'true'} or {argument}
        started = sum('ci_audit.py start' in run for run in runs)
        self.assertEqual(finished, {name for name, lane in CI.LANES.items() if lane.audited})
        self.assertEqual(started, sum('ci_audit.py finish' in run for run in runs))


class Verdicts(unittest.TestCase):
    def run_check(self, data, canary=None, enforced=True):
        directory, root = repository(['scripts/hook.sh', 'docs/guide.md'])
        with directory, tempfile.TemporaryDirectory() as state:
            trace = Path(state) / 'reads.json'
            paths = [p.replace('ROOT', root) for p in data.pop('paths')]
            trace.write_text(json.dumps(dict(data, paths=paths)))
            return AUDIT.check('demo', trace, root, canary and canary.replace('ROOT', root),
                               lanes={'demo': CI.Lane(reads=('scripts/*',), enforced=enforced)})

    def test_a_lane_in_report_mode_warns_without_failing(self):
        self.assertEqual(self.run_check({'overflow': False, 'events': 1, 'paths': ['ROOT/docs/guide.md']},
                                        enforced=False), 0)

    def test_declared_reads_pass(self):
        self.assertEqual(self.run_check({'overflow': False, 'events': 1, 'paths': ['ROOT/scripts/hook.sh']}), 0)

    def test_undeclared_read_fails(self):
        self.assertEqual(self.run_check({'overflow': False, 'events': 1, 'paths': ['ROOT/docs/guide.md']}), 1)

    def test_lost_events_and_a_blind_recorder_fail_closed(self):
        self.assertEqual(self.run_check({'overflow': True, 'events': 1, 'paths': ['ROOT/scripts/hook.sh']}), 1)
        self.assertEqual(self.run_check({'overflow': False, 'events': 1, 'paths': ['ROOT/scripts/hook.sh']},
                                        canary='ROOT/docs/guide.md'), 1)
        self.assertEqual(self.run_check({'overflow': False, 'events': 1,
                                         'paths': ['ROOT/scripts/hook.sh', 'ROOT/docs/guide.md']},
                                        canary='ROOT/docs/guide.md'), 0)


@unittest.skipUnless(sys.platform.startswith('linux') and hasattr(os, 'geteuid') and os.geteuid() == 0,
                     'fanotify needs root on Linux')
class Recorder(unittest.TestCase):
    def test_records_file_reads_and_directory_listings_until_stopped(self):
        with tempfile.TemporaryDirectory(dir=str(ROOT)) as watched:
            (Path(watched) / 'sub').mkdir()
            (Path(watched) / 'sub/file.txt').write_text('content')
            output, ready = Path(watched) / 'reads.json', Path(watched) / 'ready'
            # A prefix that does not exist yet, like a directory the job creates later.
            later = str(Path(watched) / 'not-created-yet/payload')
            process = subprocess.Popen([sys.executable, str(ROOT / '.github/scripts/ci_audit.py'), 'record',
                                        '--output', str(output), '--ready', str(ready), '--prefix', watched,
                                        '--prefix', later])
            deadline = time.monotonic() + 30
            while not (ready.exists() and ready.read_text().strip()):
                self.assertLess(time.monotonic(), deadline)
                time.sleep(0.1)
            subprocess.check_call(['cat', str(Path(watched) / 'sub/file.txt')], stdout=subprocess.DEVNULL)
            subprocess.check_call(['ls', str(Path(watched) / 'sub')], stdout=subprocess.DEVNULL)
            process.terminate()
            self.assertEqual(process.wait(timeout=30), 0)
            data = json.loads(output.read_text())
            self.assertFalse(data['overflow'])
            self.assertIn(str(Path(watched).resolve() / 'sub/file.txt'), data['paths'])
            self.assertIn(str(Path(watched).resolve() / 'sub'), data['paths'])


if __name__ == '__main__':
    unittest.main()
