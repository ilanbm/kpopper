"""Ordinary direct commands publish and recover immutable history operations."""
import contextlib
import io
import os
from pathlib import Path
import subprocess
import sys
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

    def test_fold_preview_retains_captured_assessment_without_writing(self):
        HH = D.P._peer('history_hypotheses')
        mutation = HH.prepare(self.entry, 'alternative',
                              {'kind': 'set', 'id': 'p.input', 'value': 2}, operation='proposal')
        HH.commit(self.entry, mutation, verify=lambda data: None)
        before = H.Store(self.entry).capture()
        result = D.finish_hypotheses([str(self.entry)], ['alternative'], kind='fold',
                                     because='preview', dry=True)
        self.assertEqual(result['state'], 'prepared')
        assessment = result['assessment']
        self.assertNotEqual(assessment['source_snapshot'], assessment['candidate_snapshot'])
        self.assertEqual(assessment['candidate']['holes'], [])
        self.assertEqual(before.inventory, H.Store(self.entry).capture().inventory)
        self.assertFalse((self.entry.parent / D.journal(self.entry)).exists())

    def test_legacy_history_preview_keeps_its_existing_result(self):
        from tests import test_history_store as storage
        fixture = storage.Storage()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        entry = fixture.entry
        document = D.C.decode_document(entry.read_bytes())
        document['meta'].pop('reasoning')
        entry.write_bytes(D.C.encode_document(document))
        reading = fixtures.claim()
        reading['authored']['profile'] = 'ordinary-reader/v1'
        reading['id'] = D.C.object_identity(reading)
        judgment = fixtures.claim('d.ready', kind='judgment', operation='legacy-judgment',
            body={'verdict': 'ready', 'rests_on': ['p.input'], 'seen': {'p.input': 1},
                  'wrong_if': 'p.input > 5'}, pins={'p.input': reading['id']})
        judgment['authored']['profile'] = 'ordinary-reader/v1'
        judgment['authored']['collection'] = 'judgments'
        judgment['id'] = D.C.object_identity(judgment)
        fixture.publish([reading, judgment], op='legacy-bootstrap')
        HH = D.P._peer('history_hypotheses')
        mutation = HH.prepare(entry, 'alternative',
                              {'kind': 'set', 'id': 'p.input', 'value': 2}, operation='proposal')
        HH.commit(entry, mutation, verify=lambda data: None)
        result = D.finish_hypotheses([str(entry)], ['alternative'], kind='fold',
                                     because='preview', dry=True)
        self.assertEqual(result, {'state': 'prepared', 'hypotheses': ['alternative'], 'action': 'fold'})

    @unittest.skipIf(os.name == 'nt', 'History fixture publication requires POSIX locking')
    def test_public_cli_shows_captured_history_preview(self):
        HH = D.P._peer('history_hypotheses')
        mutation = HH.prepare(self.entry, 'alternative',
                              {'kind': 'set', 'id': 'p.input', 'value': 2}, operation='proposal')
        HH.commit(self.entry, mutation, verify=lambda data: None)
        before = H.Store(self.entry).capture()
        result = subprocess.run([sys.executable, str(Path(D.__file__).with_name('cli.py')),
            '--frozen', 'consolidate', '--dry-run', str(self.entry)],
            capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('captured history preview:', result.stdout)
        self.assertIn('candidate: 0 falsified, 0 holes', result.stdout)
        self.assertEqual(before.inventory, H.Store(self.entry).capture().inventory)

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

    def test_cli_explicit_refutation_and_return_preserve_claim_body(self):
        import json
        import subprocess
        import sys
        cli = Path(P.__file__).with_name('cli.py')
        for operation, expected in [('refute', 'refuted'), ('accept', 'accepted')]:
            result = subprocess.run([sys.executable, str(cli), 'history', operation,
                '--record', str(self.entry), '--subject', 'p.input', '--of', self.source['id'],
                '--because', 'explicit fixture decision', '--json'], capture_output=True, text=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(json.loads(result.stdout)['state'], 'committed')
            captured = H.Store(self.entry).capture()
            self.assertEqual(captured.state['subjects']['p.input']['acceptance'], expected)
            self.assertEqual(captured.objects[self.source['id']], self.source)

    def test_cli_named_hypothesis_and_consolidation_are_history_operations(self):
        import subprocess
        import sys
        cli = Path(P.__file__).with_name('cli.py')
        def run(*args):
            result = subprocess.run([sys.executable, str(cli), *args, str(self.entry)],
                                    capture_output=True, text=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return result
        run('set', 'p.input', '5', '--hypothesis', 'alternative')
        data = Snapshot.capture(self.entry).to_data()
        self.assertEqual(data['nodes']['p.input']['body']['v'], 1)
        self.assertEqual(data['hypotheses']['alternative']['document']['readings']['p.input']['v'], 5)
        before = H.Store(self.entry).capture().commits
        run('consolidate', '--dry-run', 'alternative')
        self.assertEqual(before, H.Store(self.entry).capture().commits)
        run('consolidate', 'alternative')
        self.assertEqual(Snapshot.capture(self.entry).to_data()['nodes']['p.input']['body']['v'], 5)

    def test_cli_records_edited_view_as_proposal_and_retains_original_text(self):
        import json
        import subprocess
        import sys
        from scripts import history_contract as C, history_edits as E
        document = C.decode_document(self.entry.read_bytes())
        document['readings']['p.input']['v'] = 8
        edited = C.encode_document(document) + b'# authored edit note\n'
        self.entry.write_bytes(edited)
        cli = Path(P.__file__).with_name('cli.py')
        result = subprocess.run([sys.executable, str(cli), 'history', 'reconcile', '--record', str(self.entry),
            '--record-proposals', '--because', 'explicitly retain this edit', '--json'],
            capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        operation = json.loads(result.stdout)['operation']
        captured = H.Store(self.entry).capture()
        self.assertEqual(captured.state['subjects']['p.input']['body']['v'], 1)
        manifest = C.decode_document(captured.commits[operation])
        self.assertEqual(E.raw_edit_evidence(manifest['receipt']), edited)

    def test_cli_same_and_distinct_use_immutable_identity_operations(self):
        import subprocess
        import sys
        from tests import test_history_identity as identity
        fixture = identity.Identity()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        before = fixture.store.capture()
        cli = Path(P.__file__).with_name('cli.py')
        result = subprocess.run([sys.executable, str(cli), 'same', 'p.input', 'p.other', str(fixture.entry)],
                                capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        after = fixture.store.capture()
        self.assertEqual(after.state['subjects']['p.other']['acceptance'], 'retired')
        self.assertEqual(after.objects[fixture.other['id']], fixture.other)
        self.assertEqual(len(after.commits), len(before.commits) + 1)
        fixture.publish(fixtures.claim('p.third', operation='third'), operation='third')
        result = subprocess.run([sys.executable, str(cli), 'distinct', 'p.input', 'p.third',
                                 'different measured subjects', str(fixture.entry)],
                                capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(fixture.store.capture().document['readings']['p.input']['distinct_from'], 'p.third')


if __name__ == '__main__':
    unittest.main()
