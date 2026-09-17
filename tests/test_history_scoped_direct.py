"""Ordinary scoped actions retain an independent history contribution, not local acts."""
import contextlib
import copy
import io
import json
from pathlib import Path
import shutil
import unittest
from unittest import mock

from scripts import provenance as P, pending_grounding as G, project_modes as M
from scripts import history_bundle as B, history_store as H
from scripts import history_authoring as A, history_transaction as T, history_direct as D
from scripts.reasoning.snapshot import Snapshot
from tests import test_pending_grounding as repositories
from tests import test_history_bundles as histories
from tests import test_history_snapshot_capture as snapshots


class ScopedDirect(unittest.TestCase):
    def setUp(self):
        repositories.Repository.setUp(self)
        history = snapshots.HistorySnapshotCapture()
        history.setUp()
        self.addCleanup(history.doCleanups)
        shutil.copytree(history.root, self.root, dirs_exist_ok=True)
        self.entry = self.root / 'GROUNDING.yaml'
        self.action = {'kind': 'set', 'id': 'p.input', 'value': 7, 'event_id': 'direct-scoped',
                       'scope': histories.SCOPE, 'shareability': 'project', 'as_of': '2026-09-17'}

    def apply(self, action=None):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(P.apply([str(self.entry)], copy.deepcopy(action or self.action)), 0)
        return json.loads(output.getvalue())

    def test_scoped_set_preserves_checkout_and_source_state_survives_replay(self):
        before = H.Store(self.entry).capture()
        receipt = self.apply()
        self.assertEqual(receipt['state'], 'captured')
        after = H.Store(self.entry).capture()
        self.assertEqual(before.objects, after.objects)
        self.assertEqual(before.commits, after.commits)
        bundle = G.Store(M.Project(self.root)).snapshot()['bundles'][receipt['revision']]
        projected = B.adapt(B.from_contribution(bundle))
        self.assertEqual(projected.document['readings']['p.input']['v'], 7)
        self.assertEqual(projected.projection['dispositions']['p.input']['source_state'], 'prepared_candidate')
        snapshot = Snapshot.capture(self.entry, read_mode='live')
        self.assertEqual(Snapshot.from_json(snapshot.to_json()).snapshot_id, snapshot.snapshot_id)
        repeated = self.apply()
        self.assertEqual(repeated['revision'], receipt['revision'])
        self.assertEqual(len(G.Store(M.Project(self.root)).snapshot()['events']), 1)

    def test_same_event_with_different_action_refuses(self):
        self.apply()
        with self.assertRaisesRegex(ValueError, 'event_identity_mismatch'):
            self.apply({**self.action, 'value': 8})

    def test_cli_locator_disclosure_is_explicit_path_and_checksum(self):
        calls = []
        def captured(paths, action, diagnostics=None):
            calls.append(action)
            return 0
        with mock.patch.object(P, 'apply', side_effect=captured):
            P.write_command('set', ['p.input', '7', '--disclose-locator',
                'members/source.yaml=' + 'a' * 64, str(self.entry)])
        self.assertEqual(calls[0]['disclosed_locators'],
                         [{'path': 'members/source.yaml', 'sha256': 'a' * 64}])
        with self.assertRaises((ValueError, P.Refused)):
            P.write_command('set', ['p.input', '7', '--disclose-locator', '../escape=' + 'a' * 64,
                                    str(self.entry)])

    def test_prepared_capture_can_retry_after_process_failure_without_new_operation(self):
        with mock.patch.object(G.Store, 'capture', side_effect=OSError('before pending CAS')):
            with self.assertRaises(OSError):
                self.apply()
        envelope = next((M.Project(self.root).state / 'history-contributions').glob('*.yaml'))
        before = envelope.read_bytes()
        receipt = self.apply()
        self.assertEqual(envelope.read_bytes(), before)
        self.assertEqual(receipt['state'], 'captured')

    def test_cli_preview_and_explicit_adoption_resolve_overlap_and_keep_recovery(self):
        import subprocess
        import sys
        receipt = self.apply()
        cli = Path(P.__file__).with_name('cli.py')
        command = [sys.executable, str(cli), 'history', 'adopt', '--record', str(self.entry),
                   '--revision', receipt['revision'], '--json']
        preview = subprocess.run([*command, '--preview'], capture_output=True, text=True, timeout=20)
        self.assertEqual(preview.returncode, 0, preview.stdout + preview.stderr)
        self.assertIn('p.input', json.loads(preview.stdout)['subjects'])
        bundle = G.Store(M.Project(self.root)).snapshot()['bundles'][receipt['revision']]
        artifact = B.from_contribution(bundle)
        incoming = B.validate(artifact)
        chosen = incoming.state['subjects']['p.input']['head']
        refused = subprocess.run(command, capture_output=True, text=True, timeout=20)
        self.assertNotEqual(refused.returncode, 0)
        chosen_command = [*command, '--choose', 'p.input=' + chosen, '--by', 'fixture-user']
        result = subprocess.run(chosen_command, capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        target = H.Store(self.entry).capture()
        self.assertEqual(target.state['subjects']['p.input']['body']['v'], 7)
        self.assertTrue(B.adopted_by(target, artifact))

    def test_source_manifest_change_without_view_refresh_blocks_pending_capture(self):
        capture = G.Store.capture
        def changed(store, *args, **kwargs):
            mutation = A.prepare(self.entry, {'kind': 'set', 'id': 'p.input', 'value': 9})
            replace = T._replace
            def fail_view(path, raw):
                if path == self.entry:
                    raise OSError('source view interrupted')
                return replace(path, raw)
            with mock.patch.object(T, '_replace', side_effect=fail_view):
                with self.assertRaises(OSError):
                    A.commit(self.entry, mutation, verify=lambda data: None)
            return capture(store, *args, **kwargs)
        with mock.patch.object(G.Store, 'capture', new=changed):
            with self.assertRaisesRegex(ValueError, 'snapshot_changed'):
                self.apply()
        self.assertIsNone(G.Store(M.Project(self.root)).head())

    def test_interrupted_adoption_recovers_same_immutable_operation(self):
        receipt = self.apply()
        bundle = G.Store(M.Project(self.root)).snapshot()['bundles'][receipt['revision']]
        artifact = B.from_contribution(bundle)
        chosen = B.validate(artifact).state['subjects']['p.input']['head']
        replace = T._replace
        def fail_view(path, raw):
            if path == self.entry:
                raise OSError('adoption view interrupted')
            return replace(path, raw)
        with mock.patch.object(T, '_replace', side_effect=fail_view):
            with self.assertRaises(OSError):
                D.adopt([str(self.entry)], receipt['revision'], {'p.input': chosen}, by='fixture-user',
                        project=M.Project(self.root), original_paths=[str(self.entry)])
        before = H.Store(self.entry).capture()
        P.recover_direct([str(self.entry)])
        after = H.Store(self.entry).capture()
        self.assertEqual(before.commits, after.commits)
        self.assertTrue(B.adopted_by(after, artifact))
