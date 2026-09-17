"""Ordinary direct commands publish and recover immutable history operations."""
import contextlib
import io
from pathlib import Path
import unittest
from unittest import mock

from scripts import provenance as P, history_direct as D, history_store as H, history_transaction as T
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_snapshot_capture as fixtures


class DirectHistory(unittest.TestCase):
    def setUp(self):
        fixture = fixtures.HistorySnapshotCapture()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.entry = fixture.entry
        self.source = fixture.source

    def apply(self, value=5):
        with contextlib.redirect_stdout(io.StringIO()):
            return P.apply([str(self.entry)], {'kind': 'set', 'id': 'p.input', 'value': value})

    def test_normal_set_reaches_history_and_keeps_original_claim(self):
        self.assertEqual(self.apply(), 0)
        captured = H.Store(self.entry).capture()
        self.assertEqual(captured.objects[self.source['id']]['body'], {'v': 1})
        self.assertEqual(Snapshot.capture(self.entry).to_data()['nodes']['p.input']['body']['v'], 5)
        self.assertFalse((self.entry.parent / D.journal(self.entry)).exists())

    def test_ordinary_scalar_add_keeps_the_original_authored_body(self):
        with contextlib.redirect_stdout(io.StringIO()):
            result = P.apply([str(self.entry)], {'kind': 'add', 'id': 'q.choice',
                                                'body': 'Which source remains current?'})
        self.assertEqual(result, 0)
        captured = H.Store(self.entry).capture()
        head = captured.state['subjects']['q.choice']['head']
        self.assertEqual(captured.objects[head]['body'], 'Which source remains current?')
        self.assertEqual(Snapshot.capture(self.entry).to_data()['nodes']['q.choice']['body'],
                         'Which source remains current?')

    def test_after_manifest_failure_retains_exact_recovery_operation(self):
        replace = T._replace
        def failed(path, raw):
            if path == self.entry:
                raise OSError('interrupted history view')
            return replace(path, raw)
        with mock.patch.object(T, '_replace', side_effect=failed):
            with self.assertRaisesRegex(OSError, 'interrupted history view'):
                self.apply()
        pending = self.entry.parent / D.journal(self.entry)
        mutation, _ = D._read_envelope(pending.read_bytes())
        before = H.Store(self.entry).capture()
        self.assertIn(mutation.to_data()['operation'], before.commits)
        with self.assertRaisesRegex(ValueError, 'recovery_required'):
            self.apply(6)
        P.recover_direct([str(self.entry)])
        after = H.Store(self.entry).capture()
        self.assertEqual(before.commits, after.commits)
        self.assertEqual(before.objects, after.objects)
        self.assertEqual(Snapshot.capture(self.entry).to_data()['nodes']['p.input']['body']['v'], 5)

    def test_committed_history_cannot_be_rolled_back_by_deleting_the_event(self):
        with mock.patch.object(T, '_replace', side_effect=OSError('view failure')):
            with self.assertRaises(OSError):
                self.apply()
        with self.assertRaisesRegex(ValueError, 'history_already_committed'):
            P.recover_direct([str(self.entry)], direction='before')
        self.assertEqual(len(H.Store(self.entry).capture().commits), 2)

    def test_cli_set_status_and_rebuild_use_actual_history(self):
        import json
        import subprocess
        import sys
        cli = Path(P.__file__).with_name('cli.py')
        result = subprocess.run([sys.executable, str(cli), 'set', 'p.input', '7', str(self.entry)],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = subprocess.run([sys.executable, str(cli), 'history', 'status', '--record', str(self.entry), '--json'],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout)['commits'], 2)
        result = subprocess.run([sys.executable, str(cli), 'history', 'rebuild', '--record', str(self.entry), '--json'],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout)['state'], 'rebuilt')
        self.assertEqual(Snapshot.capture(self.entry).to_data()['nodes']['p.input']['body']['v'], 7)

    def test_cli_report_recovery_uses_the_event_lock_without_a_nested_direct_lock(self):
        import json
        import subprocess
        import sys
        from tests import test_history_report_integration as reports
        report = reports.HistoryReports()
        report.setUp()
        self.addCleanup(report.doCleanups)
        report.capture()
        replace = report.T._replace
        def fail_view(path, raw):
            if Path(path).resolve() == report.entry.resolve():
                raise OSError('report view interrupted')
            return replace(path, raw)
        with mock.patch.object(report.T, '_replace', side_effect=fail_view):
            self.assertEqual(report.process()['state'], 'recovery_required')
        cli = Path(P.__file__).with_name('cli.py')
        result = subprocess.run([sys.executable, str(cli), 'recover', '--record', str(report.entry), '--json'],
                                capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt['state'], 'applied')
        self.assertTrue(receipt['recovered'])
        self.assertEqual(len(report.store.capture().commits), 2)


if __name__ == '__main__':
    unittest.main()
