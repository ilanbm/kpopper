"""Pinned local branch history transport never adopts, publishes or rereads its source."""
import copy
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest import mock

from scripts import history_branch as B, history_migration as M, history_contract as C
from scripts import history_store as H, project_modes as G, pending_grounding as P


class BranchCapture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.legacy = self.base / 'legacy'
        self.legacy.mkdir()
        self.entry = self.legacy / 'GROUNDING.yaml'
        self.entry.write_text('known:\n  p.value: {v: 1}\n')
        self.repo = self.base / 'repo'
        M.prepare(self.entry, operation='import', recorded_at='2026-09-17', record_id='branch-record').publish(self.repo)
        self.record = self.repo / 'GROUNDING.yaml'
        G.git(self.repo, 'init', '-b', 'main')
        G.git(self.repo, 'config', 'user.name', 'Fixture')
        G.git(self.repo, 'config', 'user.email', 'fixture@example.test')
        G.git(self.repo, 'add', '.')
        G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'History fixture')
        self.sha = G.git(self.repo, 'rev-parse', 'HEAD').stdout.decode().strip()

    def capture(self, **kwargs):
        return B.capture(self.repo, 'main', as_of='2026-09-17', **kwargs)

    def test_exact_objects_pinned_sha_profiles_and_scope_absence_survive_offline_replay(self):
        original = H.Store(self.record).capture()
        envelope = self.capture()
        self.assertEqual(envelope['manifest']['source']['commit'], self.sha)
        self.assertEqual(envelope['manifest']['source']['entry'], 'GROUNDING.yaml')
        self.assertNotIn('scope', envelope['manifest'])
        restored = B.validate(envelope)
        self.assertEqual(restored.objects, original.objects)
        for key, raw in original.object_bytes.items():
            self.assertEqual(restored.object_bytes[key], raw)
        self.assertEqual(B.snapshot(envelope).to_data()['document']['known']['p.value'], {'v': 1})
        self.assertNotIn(str(self.repo), str(envelope))
        shutil.rmtree(self.repo)
        with mock.patch.object(B, '_git', side_effect=AssertionError('no Git replay')), \
             mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('no filesystem replay')):
            self.assertEqual(B.snapshot(envelope).snapshot_id, envelope['manifest']['snapshot']['snapshot_id'])

    def test_dirty_checkout_is_irrelevant_and_capture_does_not_write_git_or_pending(self):
        self.record.write_text('known: {p.value: {v: 999}}\n')
        before = {str(path.relative_to(self.repo)): path.read_bytes() for path in self.repo.rglob('*') if path.is_file()}
        envelope = self.capture()
        self.assertEqual(B.snapshot(envelope).to_data()['document']['known']['p.value']['v'], 1)
        after = {str(path.relative_to(self.repo)): path.read_bytes() for path in self.repo.rglob('*') if path.is_file()}
        self.assertEqual(before, after)
        self.assertIsNone(P.Store(self.repo).head())

    def test_ref_is_resolved_once_and_later_branch_movement_does_not_change_capture(self):
        real = B._git
        calls = []
        moved = [False]
        def run(root, *args, **kwargs):
            calls.append(args)
            raw = real(root, *args, **kwargs)
            if args[:2] == ('rev-parse', '--verify') and not moved[0]:
                moved[0] = True
                (self.repo / 'unrelated.txt').write_text('new branch content')
                G.git(self.repo, 'add', 'unrelated.txt')
                G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Later branch')
            return raw
        with mock.patch.object(B, '_git', side_effect=run):
            envelope = self.capture()
        self.assertEqual(envelope['manifest']['source']['commit'], self.sha)
        self.assertEqual(sum(args[:2] == ('rev-parse', '--verify') for args in calls), 1)
        self.assertNotIn('unrelated.txt', envelope['files'])
        self.assertTrue(all(self.sha in args for args in calls if 'ls-tree' in args))

    def test_missing_corrupt_extra_and_rehashed_object_omission_refuse(self):
        original = self.capture()
        object_path = next(path for path in original['files'] if '/history/' in path)
        for corruption in ('missing', 'bytes', 'extra', 'rehashed'):
            envelope = copy.deepcopy(original)
            if corruption in ('missing', 'rehashed'):
                del envelope['files'][object_path]
            elif corruption == 'bytes':
                envelope['files'][object_path] += b'\n'
            else:
                envelope['files']['unrelated.txt'] = b'no scope'
            if corruption == 'rehashed':
                envelope['manifest']['files'].pop(object_path)
                envelope['revision'] = P.identity(envelope['manifest'])
            with self.subTest(corruption=corruption), self.assertRaises(ValueError):
                B.validate(envelope)

    def test_named_physical_hypothesis_and_evidence_are_retained_but_not_accepted(self):
        folder = self.repo / '.kpopper/hypotheses'
        folder.mkdir()
        (folder / 'alternative.yaml').write_text('hypothesis: {claim: alternative, folds: never}\nknown:\n  p.value: {v: 2, file: evidence/note.txt}\n')
        (self.repo / 'evidence').mkdir()
        (self.repo / 'evidence/note.txt').write_bytes(b'exact evidence\r\n')
        G.git(self.repo, 'add', '.')
        G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Hypothesis')
        envelope = self.capture()
        snapshot = B.snapshot(envelope).to_data()
        self.assertEqual(snapshot['document']['known']['p.value']['v'], 1)
        self.assertEqual(snapshot['hypotheses']['alternative']['document']['known']['p.value']['v'], 2)
        self.assertEqual(envelope['files']['evidence/note.txt'], b'exact evidence\r\n')

    def test_private_and_external_evidence_refuse_without_fetching_locations(self):
        folder = self.repo / '.kpopper/hypotheses'
        folder.mkdir()
        path = folder / 'private.yaml'
        for body in ('known: {p.private: {v: 2, private: true}}\n',
                     'known: {p.external: {v: 2, file: ../outside.txt}}\n'):
            path.write_text(body)
            G.git(self.repo, 'add', '.')
            G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Unshareable closure')
            with self.assertRaises(ValueError):
                self.capture()

    def test_symlink_and_size_budget_refuse_before_blob_allocation(self):
        folder = self.repo / '.kpopper/hypotheses'
        folder.mkdir()
        (folder / 'link.yaml').symlink_to(self.entry)
        G.git(self.repo, 'add', '.')
        G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Link')
        with self.assertRaisesRegex(ValueError, 'unsupported_branch_file'):
            self.capture()
        with mock.patch.object(B, 'MAX_BYTES', 1), self.assertRaisesRegex(ValueError, 'branch_capture_limit'):
            B.capture(self.repo, self.sha)

    def test_legacy_source_and_unknown_ref_are_explicit_refusals(self):
        (self.repo / 'legacy.yaml').write_text('known: {p.old: {v: 1}}\n')
        G.git(self.repo, 'add', '.')
        G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Legacy')
        with self.assertRaisesRegex(ValueError, 'branch_history_not_active'):
            self.capture(entry='legacy.yaml')
        with self.assertRaisesRegex(ValueError, 'branch_git_unavailable'):
            B.capture(self.repo, 'missing-local-ref')

    def test_typed_transport_replays_exact_bytes_and_detects_snapshot_tampering(self):
        envelope = self.capture()
        raw = B.to_bytes(envelope)
        shutil.rmtree(self.repo)
        with mock.patch.object(B, '_git', side_effect=AssertionError('offline')):
            self.assertEqual(B.from_bytes(raw), envelope)
        corrupted = copy.deepcopy(envelope)
        corrupted['manifest']['snapshot']['document']['known']['p.value']['v'] = 99
        corrupted['revision'] = P.identity(corrupted['manifest'])
        with self.assertRaises(ValueError):
            B.validate(corrupted)

    def test_imported_named_proposals_and_cancelled_generations_keep_independent_state(self):
        from tests.test_history_generations import Generations
        from scripts import history_activation as X
        fixture = Generations('run')
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        mutation, _ = fixture.cancelled_generation()
        X.recover(fixture.entry, deployment_guard=fixture.guard, direction='before')
        fixture.set_legacy(8)
        folder = Path(M.P.layout(fixture.entry)['hypotheses'])
        folder.mkdir(parents=True, exist_ok=True)
        (folder / 'alternative.yaml').write_text('hypothesis: {claim: alternative, folds: never}\nreadings: {p.input: {v: 13}}\n')
        fixture.publish(fixture.prepare(operation='five'))
        root = fixture.entry.parent
        G.git(root, 'init', '-b', 'main')
        G.git(root, 'config', 'user.name', 'Fixture')
        G.git(root, 'config', 'user.email', 'fixture@example.test')
        G.git(root, 'add', '.')
        G.git(root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Cancelled and named history')
        envelope = B.capture(root, 'HEAD')
        capture = B.validate(envelope)
        self.assertEqual(capture.marker['generation'], 5)
        self.assertEqual(capture.inactive_generations['3']['disposition'], 'cancelled')
        self.assertEqual(capture.cancellation_bytes, H.Store(fixture.entry).capture().cancellation_bytes)
        data = B.snapshot(envelope).to_data()
        self.assertEqual(data['document']['readings']['p.input']['v'], 8)
        self.assertEqual(data['hypotheses']['alternative']['document']['readings']['p.input']['v'], 13)
        self.assertIn('alternative', data['context']['history_hypotheses']['groups'])

    def test_ref_arguments_never_become_options_and_network_replacement_are_disabled(self):
        real = B.subprocess.Popen
        invocations = []
        def run(args, **kwargs):
            invocations.append((args, kwargs.get('env', {})))
            return real(args, **kwargs)
        with mock.patch.object(B.subprocess, 'Popen', side_effect=run):
            envelope = self.capture()
            with self.assertRaises(ValueError):
                B.capture(self.repo, '--help')
        for args, env in invocations:
            self.assertIn('--no-lazy-fetch', args)
            self.assertIn('--no-replace-objects', args)
            self.assertEqual(env['GIT_TERMINAL_PROMPT'], '0')
            if 'ls-tree' in args:
                self.assertIn('--', args)
        bad = next(args for args, _ in invocations if '--help^{commit}' in args)
        self.assertLess(bad.index('--end-of-options'), bad.index('--help^{commit}'))

    def test_stalled_child_stdout_has_a_real_deadline(self):
        import subprocess
        import sys
        import time
        real = subprocess.Popen
        processes = []
        def stalled(args, **kwargs):
            process = real([sys.executable, '-c', 'import time; time.sleep(30)'], **kwargs)
            processes.append(process)
            return process
        started = time.monotonic()
        with mock.patch.object(B, 'GIT_TIMEOUT', 0.15), mock.patch.object(B.subprocess, 'Popen', side_effect=stalled):
            with self.assertRaisesRegex(ValueError, 'branch_git_timeout'):
                B._git(self.repo, 'rev-parse', 'HEAD')
        self.assertLess(time.monotonic() - started, 3)
        self.assertIsNotNone(processes[0].poll())

    def test_excessive_stdout_is_killed_under_the_output_cap(self):
        import subprocess
        import sys
        real = subprocess.Popen
        def excessive(args, **kwargs):
            return real([sys.executable, '-c', 'import sys; sys.stdout.write("x" * 1000000)'], **kwargs)
        with mock.patch.object(B.subprocess, 'Popen', side_effect=excessive):
            with self.assertRaisesRegex(ValueError, 'branch_capture_limit'):
                B._git(self.repo, 'rev-parse', 'HEAD', maximum=64)
