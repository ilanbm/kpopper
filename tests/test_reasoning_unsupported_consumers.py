"""Operational entrypoints route core records; legacy-only helpers still refuse."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from scripts import followups, watch


CORE = '''meta:
  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}
known:
  p.input: {v: 1}
'''


class UnsupportedCoreConsumers(unittest.TestCase):
    def test_remeasure_routes_core_records(self):
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'GROUNDING.yaml'
            record.write_text(CORE, encoding='utf-8')
            commands = [
                [sys.executable, str(root / 'scripts/cli.py'), 'remeasure', str(record)],
            ]
            for command in commands:
                result = subprocess.run(command, capture_output=True, text=True, check=False)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn('nothing to re-measure', result.stdout)

    def test_followups_record_view_refuses_core(self):
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'GROUNDING.yaml'
            record.write_text(CORE, encoding='utf-8')
            with self.assertRaisesRegex(followups.P.Refused,
                                        'unsupported_capability: use core/v1 consumer'):
                followups._record_view(str(record))

    def test_watch_record_capture_retains_core_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record = root / 'GROUNDING.yaml'
            record.write_text(CORE, encoding='utf-8')
            subprocess.run(['git', 'init', '-q'], cwd=root, check=True)
            subprocess.run(['git', 'add', 'GROUNDING.yaml'], cwd=root, check=True)
            subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                            'commit', '-qm', 'fixture'], cwd=root, check=True)
            captured = watch._records(root, 'GROUNDING.yaml')
            self.assertIn('snapshot', captured['core'])
            self.assertEqual(captured['doc']['known']['p.input']['v'], 1)


if __name__ == '__main__':
    unittest.main()
