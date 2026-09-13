"""Process-isolated imports and UTF-8 streams must not depend on test order or locale."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ProcessBoundaries(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.record = self.work / 'GROUNDING.yaml'
        self.record.write_text('sources:\n  s.original: {file: original.md}\nknown:\n  מחיר: {v: 10, from: s.original}\n  price: {v: 10, from: s.original}\n', encoding='utf-8')
        (self.work / 'original.md').write_text('המחיר 10', encoding='utf-8')
        self.env = dict(os.environ, PYTHONIOENCODING='cp1252', XDG_STATE_HOME=str(self.work / 'state'), TMPDIR=str(self.work))

    def run_python(self, script, *args, **kwargs):
        return subprocess.run([sys.executable, str(ROOT / 'scripts' / script), *args],
            cwd=self.work, env=self.env, text=True, encoding='utf-8', capture_output=True, timeout=30, **kwargs)

    @unittest.skipUnless(os.name == 'posix', 'ingestion writes require POSIX locking')
    def test_package_update_without_a_brief_in_a_fresh_process(self):
        code = '''import sys,json
sys.path.insert(0,sys.argv[1])
from scripts import ingestion as I
report={'source_quote':'Price 20','date':'2026-09-13','updates':[{'kind':'set','id':'price','value':20}]}
result=I.update(report,'GROUNDING.yaml')
assert result['state']=='applied',result
'''
        result = subprocess.run([sys.executable, '-I', '-c', code, str(ROOT)], cwd=self.work,
            env=self.env, text=True, encoding='utf-8', capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_direct_reader_and_json_wrapper_preserve_hebrew_output(self):
        direct = self.run_python('provenance.py', 'pull', 'מחיר', str(self.record))
        self.assertEqual(direct.returncode, 0, direct.stderr)
        self.assertIn('מחיר', direct.stdout)
        wrapped = self.run_python('cli.py', '--json', 'pull', 'מחיר')
        self.assertEqual(wrapped.returncode, 0, wrapped.stderr)
        self.assertIn('מחיר', json.loads(wrapped.stdout)['output'])

    def test_direct_source_search_uses_utf8(self):
        result = self.run_python('search.py', 'המחיר', '--record', str(self.record), '--json')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('המחיר', result.stdout)

    @unittest.skipUnless(os.name == 'posix', 'ingestion writes require POSIX locking')
    def test_direct_capture_preserves_utf8_input(self):
        report = {'source_quote': 'המחיר המעודכן 20', 'date': '2026-09-13',
                  'updates': [{'kind': 'set', 'id': 'price', 'value': 20}]}
        result = self.run_python('ingestion.py', 'capture', '--file', '-', '--record', str(self.record),
            '--no-start', input=json.dumps(report, ensure_ascii=False))
        self.assertEqual(result.returncode, 0, result.stderr)
        files = list((self.work / 'state').glob('kpopper/ingestion/*/sources/*.txt'))
        self.assertEqual(len(files), 1)
        self.assertEqual(files[0].read_text(encoding='utf-8'), report['source_quote'])

    @unittest.skipUnless(os.name == 'posix', 'shell hook requires sh')
    def test_stop_hook_bounces_for_a_hebrew_failure_under_ansi_stdio(self):
        mark = self.work / 'kpopper-base-utf8-test'
        self.assertEqual(self.run_python('provenance.py', 'mark', str(mark), str(self.record)).returncode, 0)
        with self.record.open('a', encoding='utf-8') as stream:
            stream.write('judgments:\n  c.בדיקה:\n    rests_on: [מחיר]\n    seen: {מחיר: 10}\n    verdict: התקציב מתאים\n    wrong_if: מחיר > 5\n')
        result = subprocess.run(['sh', str(ROOT / 'scripts/session_gate.sh')],
            input=json.dumps({'session_id':'utf8-test','cwd':str(self.work)}, ensure_ascii=False),
            cwd=self.work, env=self.env, text=True, encoding='utf-8', capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn('c.בדיקה', result.stderr)
