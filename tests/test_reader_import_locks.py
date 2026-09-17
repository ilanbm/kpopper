"""Package writers and captured readers share one reentrant directory lock."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class ReaderImportLocks(unittest.TestCase):
    def test_ingestion_writer_lock_allows_a_nested_snapshot_read(self):
        with tempfile.TemporaryDirectory() as directory:
            entry = Path(directory) / 'GROUNDING.yaml'
            entry.write_text('known: {p.input: {v: 1}}\n', encoding='utf-8')
            code = '''
import sys
from scripts import ingestion
from scripts.reasoning.snapshot import Snapshot
with ingestion.P._directory_locked(sys.argv[1]):
    captured = Snapshot.capture(sys.argv[1], read_mode='frozen')
    assert captured.to_data()['nodes']['p.input']['body']['v'] == 1
print('captured under writer lock')
'''
            environment = dict(os.environ, PYTHONDONTWRITEBYTECODE='1')
            try:
                result = subprocess.run([sys.executable, '-B', '-c', code, str(entry)],
                    cwd=Path(__file__).resolve().parents[1], env=environment,
                    capture_output=True, text=True, timeout=10)
            except subprocess.TimeoutExpired:
                self.fail('captured reader deadlocked inside the ingestion writer lock')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stdout.strip(), 'captured under writer lock')


if __name__ == '__main__':
    unittest.main()
