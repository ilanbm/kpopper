"""Every supported core consumer preserves the versioned no-findings failure."""
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts import export_graph as E, search as S, workspace_cli as W
from scripts.reasoning import context as C


class CoreCaptureFailures(unittest.TestCase):
    def failure(self):
        return C.CaptureError(C.capture_failure(
            ValueError('missing_history_object: p.input')))

    def test_open_json_keeps_the_failure_envelope(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'GROUNDING.yaml'
            path.write_text('placeholder\n', encoding='utf-8')
            output = io.StringIO()
            with mock.patch.object(W.W, 'locate', return_value={
                    'workspace': directory, 'record': str(path), 'status': 'found',
                    'key': 'capture-failure'}), \
                    mock.patch.object(C.CapturedAssessment, 'capture', side_effect=self.failure()), \
                    contextlib.redirect_stdout(output):
                code = W.open_context(['--json', '--profile', 'core/v1', str(path)])
        result = json.loads(output.getvalue())
        self.assertEqual(code, 2)
        self.assertEqual(result['kind'], 'assessment_capture_failure')
        self.assertRegex(result['failure_revision'], r'^[0-9a-f]{64}$')
        self.assertIsNone(result['findings'])

    def test_export_and_search_keep_the_failure_revision(self):
        failure = self.failure()
        with mock.patch.object(C.CapturedAssessment, 'capture', side_effect=failure):
            stderr = io.StringIO()
            with self.assertRaises(SystemExit), contextlib.redirect_stderr(stderr):
                E.main(['p.input', '--profile', 'core/v1', '--record', 'missing.yaml'])
            self.assertIn(failure.envelope['failure_revision'], stderr.getvalue())
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                code = S.main(['input', '--json', '--profile', 'core/v1',
                               '--record', 'missing.yaml'])
            self.assertEqual(code, 2)
            result = json.loads(stderr.getvalue())
            self.assertEqual(result['capture_failure'], failure.envelope)

    def test_page_cli_prints_the_same_failure_envelope(self):
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / 'missing.yaml'
            result = subprocess.run([
                sys.executable, str(Path(__file__).resolve().parents[1] /
                                    'scripts' / 'render_page.py'),
                '--profile', 'core/v1', str(missing)],
                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 2)
        failure = json.loads(result.stderr)
        self.assertEqual(failure['kind'], 'assessment_capture_failure')
        self.assertRegex(failure['failure_revision'], r'^[0-9a-f]{64}$')
        self.assertIsNone(failure['findings'])


if __name__ == '__main__':
    unittest.main()
