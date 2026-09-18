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


class BranchAdoption(unittest.TestCase):
    setUp = BranchCapture.setUp
    capture = BranchCapture.capture
    def target(self, value=9):
        original = self.base / 'target-legacy'
        original.mkdir()
        (original / 'GROUNDING.yaml').write_text('known: {p.value: {v: ' + str(value) + '}}\n')
        target = self.base / 'target'
        M.prepare(original / 'GROUNDING.yaml', operation='target-import', recorded_at='2026-09-17',
                  record_id='target-record').publish(target)
        return target / 'GROUNDING.yaml'

    def prepared(self, target, envelope, choices):
        return B.prepare_adoption(target, envelope, choices=choices, by='explicit adopter',
                                  operation='branch-adopt', recorded_at='2026-09-17T12:00:00Z')

    def test_explicit_adoption_preserves_ids_and_source_binding_without_ledger(self):
        envelope = self.capture()
        source = B.validate(envelope)
        target = self.target()
        before = H.Store(target).capture()
        preview = B.preview_adoption(before, envelope, entry=target)
        self.assertEqual(preview['source_revision'], envelope['revision'])
        self.assertEqual(preview['source']['commit'], self.sha)
        self.assertTrue(preview['subjects']['p.value']['requires_choice'])
        with self.assertRaisesRegex(ValueError, 'adoption_choice_required'):
            self.prepared(target, envelope, {})
        selected = source.state['subjects']['p.value']['head']
        mutation = self.prepared(target, envelope, {'p.value': selected})
        self.assertEqual(mutation.to_bytes(), self.prepared(target, envelope, {'p.value': selected}).to_bytes())
        binding = mutation.to_data()['receipt']['after']['history_branch_adoption']
        self.assertEqual(binding['source']['files'], envelope['manifest']['files'])
        self.assertEqual(binding['source']['git']['commit'], self.sha)
        self.assertNotIn('scope', binding['source'])
        shutil.rmtree(self.repo)
        with mock.patch.object(B, '_git', side_effect=AssertionError('no source read')):
            B.commit_adoption(target, mutation, envelope, verify=lambda data: None)
            B.commit_adoption(target, mutation, envelope, verify=lambda data: None)
        after = H.Store(target).capture()
        self.assertEqual(after.document['known']['p.value']['v'], 1)
        for version, obj in source.objects.items():
            self.assertEqual(after.objects[version], obj)
            self.assertEqual(after.object_bytes[(obj['subject'], version)], source.object_bytes[(obj['subject'], version)])
        self.assertEqual(len(after.commits), len(before.commits) + 1)

    def evidence_source(self):
        raw = self.base / 'evidence-legacy'
        raw.mkdir()
        (raw / 'evidence').mkdir()
        (raw / 'evidence/note.txt').write_bytes(b'exact source evidence')
        (raw / 'GROUNDING.yaml').write_text('known: {p.value: {v: 1, file: evidence/note.txt}}\n')
        repo = self.base / 'evidence-repo'
        M.prepare(raw / 'GROUNDING.yaml', operation='evidence-import', recorded_at='2026-09-17',
                  record_id='evidence-record').publish(repo)
        G.git(repo, 'init', '-b', 'main')
        G.git(repo, 'config', 'user.name', 'Fixture')
        G.git(repo, 'config', 'user.email', 'fixture@example.test')
        G.git(repo, 'add', '.')
        G.git(repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Evidence')
        return B.capture(repo, 'main')

    def test_missing_conflicting_and_late_changed_target_evidence_refuse(self):
        envelope = self.evidence_source()
        source = B.validate(envelope)
        target = self.target()
        choice = {'p.value': source.state['subjects']['p.value']['head']}
        before = H.Store(target).capture()
        preview = B.preview_adoption(before, envelope, entry=target)
        self.assertEqual(preview['evidence'][0]['status'], 'missing')
        self.assertEqual(preview['evidence'][0]['source_path'], 'evidence/note.txt')
        with self.assertRaisesRegex(ValueError, 'branch_target_evidence_missing'):
            self.prepared(target, envelope, choice)
        path = target.parent / 'evidence/note.txt'
        path.parent.mkdir()
        path.write_bytes(b'wrong target bytes')
        self.assertEqual(B.preview_adoption(before, envelope, entry=target)['evidence'][0]['status'], 'conflicting')
        with self.assertRaisesRegex(ValueError, 'branch_target_evidence_conflicting'):
            self.prepared(target, envelope, choice)
        path.write_bytes(b'exact source evidence')
        mutation = self.prepared(target, envelope, choice)
        def change(data):
            path.write_bytes(b'changed after verifier')
        with self.assertRaisesRegex(ValueError, 'branch_target_evidence_conflicting'):
            B.commit_adoption(target, mutation, envelope, verify=change)
        self.assertEqual(H.Store(target).capture().commits, before.commits)
        self.assertEqual(target.read_bytes(), before.entry_bytes)

    def test_store_builtin_rechecks_evidence_after_arbitrary_caller_verifier(self):
        envelope = self.evidence_source()
        source = B.validate(envelope)
        target = self.target()
        path = target.parent / 'evidence/note.txt'
        path.parent.mkdir()
        path.write_bytes(b'exact source evidence')
        before = H.Store(target).capture()
        mutation = self.prepared(target, envelope, {'p.value': source.state['subjects']['p.value']['head']})
        with self.assertRaisesRegex(ValueError, 'branch_target_evidence_conflicting'):
            H.Store(target).commit(mutation, verify=lambda data: path.write_bytes(b'late arbitrary caller edit'))
        self.assertEqual(H.Store(target).capture().inventory, before.inventory)

    def test_source_free_receipt_replay_requires_actual_captured_target_evidence_bytes(self):
        envelope = self.evidence_source()
        source = B.validate(envelope)
        target = self.target()
        path = target.parent / 'evidence/note.txt'
        path.parent.mkdir()
        path.write_bytes(b'exact source evidence')
        before = H.Store(target).capture()
        mutation = self.prepared(target, envelope, {'p.value': source.state['subjects']['p.value']['head']})
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('offline means no files')):
            replay = B.verify_adoption(target, mutation, envelope, capture=before,
                                      target_evidence={'evidence/note.txt': b'exact source evidence'})
            self.assertEqual(replay.to_bytes(), mutation.to_bytes())
            with self.assertRaisesRegex(ValueError, 'branch_target_evidence_missing'):
                B.verify_adoption(target, mutation, envelope, capture=before, target_evidence={})
        path.unlink()
        with self.assertRaisesRegex(ValueError, 'branch_target_evidence_missing'):
            B.commit_adoption(target, mutation, envelope, verify=lambda data: None)

    def test_physical_hypothesis_is_observed_as_proposal_never_automatically_accepted(self):
        folder = self.repo / '.kpopper/hypotheses'
        folder.mkdir()
        (folder / 'alternative.yaml').write_text('hypothesis: {claim: alternative, folds: never}\nknown: {p.new: {v: 17}}\n')
        G.git(self.repo, 'add', '.')
        G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Physical proposal')
        envelope = self.capture()
        source = B.validate(envelope)
        target = self.target()
        before = H.Store(target).capture()
        preview = B.preview_adoption(before, envelope, entry=target)
        self.assertEqual(preview['named_proposals']['alternative']['kind'], 'physical_observation')
        self.assertEqual(preview['named_proposals']['alternative']['admission'], 'proposed_only')
        self.assertFalse(preview['subjects']['p.new']['requires_choice'])
        mutation = self.prepared(target, envelope, {'p.value': before.state['subjects']['p.value']['head']})
        B.commit_adoption(target, mutation, envelope, verify=lambda data: None)
        after = H.Store(target).capture()
        self.assertNotIn('p.new', P.entries(after.document))
        self.assertEqual(after.state['subjects']['p.new']['acceptance'], 'proposed')
        groups, _ = B.HH.layers(B.A.from_store_capture(after).projection, B.A.from_store_capture(after).document)
        self.assertEqual(groups['alternative']['doc'], {'known': {'p.new': {'v': 17}}})
        observation_ids = mutation.to_data()['receipt']['after']['history_branch_adoption']['observed_proposals']
        self.assertTrue(observation_ids)
        for version in observation_ids:
            self.assertEqual(after.objects[version]['on'], '2026-09-17T12:00:00Z')
            self.assertIsNone(after.objects[version]['by'])

    def commit_source(self):
        G.git(self.repo, 'add', '.')
        G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Source change')
        return self.capture()

    def test_same_authority_branch_source_is_an_explicit_adoption(self):
        envelope = self.capture()
        before = H.Store(self.record).capture()
        chosen = before.state['subjects']['p.value']['head']
        self.assertTrue(B.preview_adoption(before, envelope)['subjects']['p.value']['requires_choice'])
        mutation = self.prepared(self.record, envelope, {'p.value': chosen})
        B.commit_adoption(self.record, mutation, envelope, verify=lambda data: None)
        after = H.Store(self.record).capture()
        self.assertEqual(after.marker, before.marker)
        self.assertEqual(after.state['subjects']['p.value']['head'], chosen)
        self.assertEqual(len(after.commits), len(before.commits) + 1)

    def test_unoverlapped_refuted_and_retired_subjects_keep_source_dispositions(self):
        from scripts import history_authoring as W
        for subject, verb in [('p.refuted', 'refute'), ('p.retired', 'retire')]:
            mutation = W.prepare(self.record, {'kind': 'add', 'id': subject, 'body': {'v': 3}, 'into': 'known'},
                                 by='source writer', operation='add-' + verb, recorded_at='2026-09-17')
            W.commit(self.record, mutation, verify=lambda data: None)
            chosen = H.Store(self.record).capture().state['subjects'][subject]['head']
            mutation = W.prepare_act(self.record, {'kind': verb, 'id': subject, 'of': chosen, 'over': [],
                'because': 'explicit source disposition'}, by='source writer', operation=verb, recorded_at='2026-09-17')
            W.commit(self.record, mutation, verify=lambda data: None)
        envelope = self.commit_source()
        source = B.validate(envelope)
        target = self.target()
        mutation = self.prepared(target, envelope, {'p.value': source.state['subjects']['p.value']['head']})
        B.commit_adoption(target, mutation, envelope, verify=lambda data: None)
        after = H.Store(target).capture()
        for subject in ('p.refuted', 'p.retired'):
            self.assertEqual(after.state['subjects'][subject]['acceptance'], source.state['subjects'][subject]['acceptance'])
            self.assertEqual(after.state['subjects'][subject]['marks'], source.state['subjects'][subject]['marks'])
            self.assertNotIn(subject, P.entries(after.document))

    def test_named_proposal_collision_requires_compatible_interpretation(self):
        from scripts import history_hypotheses as HH
        head = {'claim': 'original source grouping', 'folds': 'never'}
        mutation = HH.prepare(self.record, 'alternative', {'kind': 'add', 'id': 'p.new', 'body': {'v': 17}, 'into': 'known'},
                              head=head, by='writer', operation='named-source', recorded_at='2026-09-17')
        HH.commit(self.record, mutation, verify=lambda data: None)
        envelope = self.commit_source()
        source = B.validate(envelope)
        target = self.target()
        choice = {'p.value': H.Store(target).capture().state['subjects']['p.value']['head']}
        mutation = self.prepared(target, envelope, choice)
        B.commit_adoption(target, mutation, envelope, verify=lambda data: None)
        after = H.Store(target).capture()
        adapted = B.A.from_store_capture(after)
        groups, _ = HH.layers(adapted.projection, adapted.document)
        self.assertEqual(groups['alternative']['head'], head)
        self.assertEqual(groups['alternative']['doc']['known']['p.new'], {'v': 17})
        self.assertEqual(after.state['subjects']['p.new']['acceptance'], 'proposed')
        self.assertTrue(set(source.objects) <= set(after.objects))
        # The same name with incompatible headers on another subject cannot overwrite the existing group.
        mutation = HH.prepare(self.record, 'other', {'kind': 'add', 'id': 'p.other', 'body': {'v': 18}, 'into': 'known'},
                              head={'claim': 'other source'}, by='writer', operation='other-source', recorded_at='2026-09-17')
        HH.commit(self.record, mutation, verify=lambda data: None)
        mutation = HH.prepare(target, 'other', {'kind': 'add', 'id': 'p.target', 'body': {'v': 19}, 'into': 'known'},
                              head={'claim': 'incompatible target'}, by='writer', operation='other-target', recorded_at='2026-09-17')
        HH.commit(target, mutation, verify=lambda data: None)
        changed = self.commit_source()
        before = H.Store(target).capture()
        choices = {'p.value': before.state['subjects']['p.value']['head'],
                   'p.new': source.state['subjects']['p.new']['proposals'][0]}
        with self.assertRaisesRegex(ValueError, 'branch_hypothesis_conflict'):
            B.prepare_adoption(target, changed, choices=choices, by='adopter', operation='second-adoption', recorded_at='2026-09-17')
        self.assertEqual(H.Store(target).capture().inventory, before.inventory)

    def test_source_capsule_is_durable_audit_and_extracts_without_any_source_reads(self):
        from scripts import history_transaction as T
        envelope = self.capture()
        source = B.validate(envelope)
        target = self.target()
        mutation = self.prepared(target, envelope, {'p.value': source.state['subjects']['p.value']['head']})
        binding = mutation.to_data()['receipt']['after']['history_branch_adoption']['source']
        audit = binding['audit']
        self.assertEqual(audit['path'], '.kpopper/evidence/branches/' + envelope['revision'] + '.json')
        raw = B.to_bytes(envelope)
        self.assertEqual(audit['sha256'], C.sha256(raw))
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('offline audit')):
            self.assertEqual(B.audit_evidence(mutation), envelope)
            data = mutation.to_data()
            data['files'] = mutation.files
            self.assertEqual(B.audit_evidence(data), envelope)
            bad = copy.deepcopy(data)
            item = next(item for item in bad['files'] if item['role'] == 'history_evidence')
            item['after'] += b' '
            with self.assertRaisesRegex(ValueError, 'branch_adoption_audit_mismatch'):
                B.audit_evidence(bad)
            bad = copy.deepcopy(data)
            bad['receipt']['after']['history_branch_adoption']['source']['git']['commit'] = 'a' * 40
            with self.assertRaisesRegex(ValueError, 'branch_adoption_audit_mismatch'):
                B.audit_evidence(bad)
        shutil.rmtree(self.repo)
        B.commit_adoption(target, mutation, envelope, verify=lambda data: None)
        self.assertEqual((target.parent / audit['path']).read_bytes(), raw)
        self.assertEqual(B.from_bytes((target.parent / audit['path']).read_bytes()), envelope)
        # A capsule is audit evidence, not a new claim or source authority.
        after = H.Store(target).capture()
        self.assertEqual(after.marker['record_id'], 'target-record')
        self.assertEqual(set(P.entries(after.document)), {'p.value'})
        G.git(target.parent, 'init', '-b', 'main')
        G.git(target.parent, 'config', 'user.name', 'Fixture')
        G.git(target.parent, 'config', 'user.email', 'fixture@example.test')
        G.git(target.parent, 'add', '.')
        G.git(target.parent, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Adopted audit')
        recaptured = B.capture(target.parent, 'main', as_of='2026-09-17')
        self.assertNotIn(audit['path'], recaptured['files'])
        self.assertEqual(recaptured['manifest']['version'], 2)
        self.assertEqual(recaptured['manifest']['audit_coverage']['prior_branch_capsules'], [{
            'path': audit['path'], 'sha256': C.sha256(raw), 'revision': envelope['revision'],
            'availability': 'not_transferred'}])
        self.assertNotIn(audit['path'], B._evidence_requirements(recaptured, B.validate(recaptured)))
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('coverage replay reads')):
            B.validate(recaptured)
        bad = copy.deepcopy(recaptured)
        bad['manifest']['audit_coverage']['prior_branch_capsules'][0]['availability'] = 'transferred'
        bad['revision'] = P.identity(bad['manifest'])
        with self.assertRaisesRegex(ValueError, 'branch_audit_coverage_mismatch'):
            B.validate(bad)
        bad = copy.deepcopy(recaptured)
        bad['manifest'].pop('audit_coverage')
        bad['manifest'].update(version=1, kind=B.KIND)
        bad['revision'] = P.identity(bad['manifest'])
        with self.assertRaisesRegex(ValueError, 'branch_evidence_unavailable'):
            B.validate(bad)

    def test_four_adoption_cycles_do_not_recursively_embed_prior_audit_capsules(self):
        sizes = []
        capsules = []
        for cycle in range(4):
            envelope = self.capture()
            previous = envelope['manifest'].get('audit_coverage', {}).get('prior_branch_capsules', [])
            self.assertEqual(len(previous), cycle)
            self.assertFalse(any('/evidence/branches/' in path for path in envelope['files']))
            raw = B.to_bytes(envelope)
            sizes.append(len(raw))
            source = B.validate(envelope)
            mutation = B.prepare_adoption(self.record, envelope,
                choices={'p.value': source.state['subjects']['p.value']['head']}, by='cycle adopter',
                operation='cycle-' + str(cycle), recorded_at='2026-09-17')
            B.commit_adoption(self.record, mutation, envelope, verify=lambda data: None)
            capsule = self.repo / mutation.to_data()['receipt']['after']['history_branch_adoption']['source']['audit']['path']
            capsules.append(capsule)
            self.assertEqual(capsule.read_bytes(), raw)
            G.git(self.repo, 'add', '.')
            G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Adoption cycle ' + str(cycle))
        self.assertEqual(len(list((self.repo / '.kpopper/evidence/branches').glob('*.json'))), 4)
        self.assertEqual(sum(path.stat().st_size for path in capsules), sum(sizes))
        # Exact raw capsules occur once apiece; manifests/objects still accumulate.
        self.assertLess(sizes[-1], sizes[0] * 8)
        type(self).cycle_accounting = {'capsule_bytes': sizes, 'files': 4, 'total_bytes': sum(sizes)}


class BranchSetAdoption(unittest.TestCase):
    setUp = BranchCapture.setUp
    capture = BranchCapture.capture
    target = BranchAdoption.target
    evidence_source = BranchAdoption.evidence_source
    def second(self):
        fixture = BranchAdoption('run')
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        return fixture

    def test_atomic_set_requires_global_choice_and_commits_once_source_free(self):
        from scripts import history_authoring as W
        other = self.second()
        mutation = W.prepare(other.record, {'kind': 'set', 'id': 'p.value', 'value': 2},
                             by='source writer', operation='source-two', recorded_at='2026-09-17')
        W.commit(other.record, mutation, verify=lambda data: None)
        envelopes = [self.capture(), other.commit_source()]
        target = self.target()
        before = H.Store(target).capture()
        chosen = B.validate(envelopes[1]).state['subjects']['p.value']['head']
        preview = B.preview_adoption_set(before, envelopes, entry=target)
        self.assertTrue(preview['subjects']['p.value']['requires_choice'])
        self.assertIn(chosen, preview['subjects']['p.value']['claims'])
        with self.assertRaisesRegex(ValueError, 'adoption_choice_required'):
            B.prepare_adoption_set(target, envelopes, choices={}, by='adopter', operation='set-fold', recorded_at='2026-09-17')
        self.assertEqual(H.Store(target).capture().inventory, before.inventory)
        kwargs = {'choices': {'p.value': chosen}, 'by': 'adopter', 'operation': 'set-fold', 'recorded_at': '2026-09-17'}
        prepared = B.prepare_adoption_set(target, envelopes, **kwargs)
        self.assertEqual(prepared.to_bytes(), B.prepare_adoption_set(target, list(reversed(envelopes)), **kwargs).to_bytes())
        receipt = prepared.to_data()['receipt']['after']['history_branch_adoption']
        self.assertEqual(receipt['version'], 2)
        self.assertEqual(receipt['source_revisions'], sorted(item['revision'] for item in envelopes))
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('source-free set')):
            self.assertEqual(len(B.audit_evidences(prepared)), 2)
            self.assertEqual(B.verify_adoption_set(target, prepared, envelopes, capture=before).to_bytes(), prepared.to_bytes())
        shutil.rmtree(self.repo)
        shutil.rmtree(other.repo)
        B.commit_adoption_set(target, prepared, envelopes, verify=lambda data: None)
        B.commit_adoption_set(target, prepared, list(reversed(envelopes)), verify=lambda data: None)
        after = H.Store(target).capture()
        self.assertEqual(after.state['subjects']['p.value']['head'], chosen)
        self.assertEqual(len(after.commits), len(before.commits) + 1)
        for envelope in envelopes:
            self.assertTrue(set(B.validate(envelope).objects) <= set(after.objects))
        self.assertEqual(len(list((target.parent / '.kpopper/evidence/branches').glob('*.json'))), 2)

    def test_conflicting_source_evidence_is_a_named_atomic_boundary(self):
        first = self.evidence_source()
        other = self.second()
        other.evidence_source()
        repo = other.base / 'evidence-repo'
        (repo / 'evidence/note.txt').write_bytes(b'different required bytes')
        G.git(repo, 'add', '.')
        G.git(repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Different source evidence')
        second = B.capture(repo, 'main')
        target = self.target()
        path = target.parent / 'evidence/note.txt'
        path.parent.mkdir()
        path.write_bytes(b'exact source evidence')
        before = H.Store(target).capture()
        preview = B.preview_adoption_set(before, [first, second], entry=target)
        self.assertEqual(preview['evidence_conflicts'], ['evidence/note.txt'])
        with self.assertRaisesRegex(ValueError, 'branch_evidence_sources_conflict'):
            B.prepare_adoption_set(target, [first, second], choices={'p.value': before.state['subjects']['p.value']['head']},
                                   by='adopter', operation='set-fold', recorded_at='2026-09-17')
        self.assertEqual(H.Store(target).capture().inventory, before.inventory)

    def test_same_named_group_conflict_across_sources_never_partially_adopts(self):
        from scripts import history_hypotheses as HH
        other = self.second()
        for fixture, subject, claim in [(self, 'p.one', 'first interpretation'), (other, 'p.two', 'second interpretation')]:
            mutation = HH.prepare(fixture.record, 'shared name',
                {'kind': 'add', 'id': subject, 'body': {'v': 17}, 'into': 'known'},
                head={'claim': claim}, by='source writer', operation='named-' + subject, recorded_at='2026-09-17')
            HH.commit(fixture.record, mutation, verify=lambda data: None)
            G.git(fixture.repo, 'add', '.')
            G.git(fixture.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Named source')
        sources = [self.capture(), other.capture()]
        target = self.target()
        before = H.Store(target).capture()
        preview = B.preview_adoption_set(before, sources, entry=target)
        self.assertEqual(len(preview['named_proposals']['shared name']), 2)
        with self.assertRaisesRegex(ValueError, 'branch_hypothesis_conflict'):
            B.prepare_adoption_set(target, sources, choices={'p.value': before.state['subjects']['p.value']['head']},
                                   by='adopter', operation='set-fold', recorded_at='2026-09-17')
        self.assertEqual(H.Store(target).capture().inventory, before.inventory)

    def test_aggregate_source_limit_precedes_each_envelope_validation(self):
        sources = [{'files': {'one': b'123'}}, {'files': {'two': b'456'}}]
        with mock.patch.object(B, 'MAX_BYTES', 5), mock.patch.object(B, 'validate', side_effect=AssertionError('too early')):
            with self.assertRaisesRegex(ValueError, 'branch_capture_limit'):
                B._source_set(sources)
