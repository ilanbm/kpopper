"""Legacy-only consumers fail closed instead of interpreting core records."""
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
    def test_consolidate_and_remeasure_name_the_core_consumer_boundary(self):
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'GROUNDING.yaml'
            record.write_text(CORE, encoding='utf-8')
            commands = [
                [sys.executable, str(root / 'scripts/cli.py'), 'consolidate',
                 '--dry-run', str(record)],
                [sys.executable, str(root / 'scripts/cli.py'), 'remeasure', str(record)],
            ]
            for command in commands:
                result = subprocess.run(command, capture_output=True, text=True, check=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('unsupported_capability: use core/v1 consumer',
                              result.stdout + result.stderr)

    def test_followups_record_view_refuses_core(self):
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'GROUNDING.yaml'
            record.write_text(CORE, encoding='utf-8')
            with self.assertRaisesRegex(followups.P.Refused,
                                        'unsupported_capability: use core/v1 consumer'):
                followups._record_view(str(record))

    def test_watch_record_capture_refuses_dormant_core(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record = root / 'GROUNDING.yaml'
            record.write_text(CORE, encoding='utf-8')
            subprocess.run(['git', 'init', '-q'], cwd=root, check=True)
            subprocess.run(['git', 'add', 'GROUNDING.yaml'], cwd=root, check=True)
            subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                            'commit', '-qm', 'fixture'], cwd=root, check=True)
            with self.assertRaisesRegex(watch.P.Refused,
                                        'unsupported_capability: use core/v1 consumer'):
                watch._records(root, 'GROUNDING.yaml')


if __name__ == '__main__':
    unittest.main()
