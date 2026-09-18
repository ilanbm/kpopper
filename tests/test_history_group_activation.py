"""Explicit two-worktree groups; source/deployment exclusion is fixture-owned."""
import contextlib
import copy
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts import history_group_activation as G, history_activation as A
from scripts import history_contract as C, history_transaction as T, history_store as H
from scripts import history_runtime as R, provenance as P
from scripts.reasoning.snapshot import Snapshot


class GroupActivation(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name).resolve()
        self.root = self.base / 'main'
        self.root.mkdir()
        def git(*args):
            subprocess.run(['git', '-C', str(self.root), *args], check=True,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        git('init', '-q')
        document = {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
                    'known': {'p.input': {'v': 2}}}
        (self.root / 'GROUNDING.yaml').write_bytes(C.encode_document(document))
        git('add', 'GROUNDING.yaml')
        git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
        self.other = self.base / 'worktree'
        git('worktree', 'add', '-q', '-b', 'other', str(self.other))
        self.entries = [self.root / 'GROUNDING.yaml', self.other / 'GROUNDING.yaml']
        self.originals = {str(path): path.read_bytes() for path in self.entries}
        scripts = Path(A.__file__).resolve().parent
        executable = str(Path(sys.executable).resolve())
        self.inventory = [{'id': 'fixture', 'argv': [sys.executable, '-B', str(scripts / 'kpopper')],
                           'package_root': str(scripts), 'executable': executable}]
        declaration = R.describe('group_fixture_nonce_0123456789')
        self.expected = {'fixture': {key: declaration[key]['digest'] for key in ('sources', 'schemas', 'native')}}
        self.guarded = False
        self.guard_count = 0
        # The actual runtime probing path is independently covered by Activation.
        # This fixture declaration isolates multi-record persistence and recovery.
        def probe(inventory, expected):
            self.assertTrue(self.guarded)
            if expected != self.expected:
                raise C.HistoryError('runtime_digest_mismatch')
            return {'complete': True, 'launchers': [{'declaration': {'schemas': {
                'history': {'prepared_mutation': [1, 2]}}}}]}
        self.addCleanup(mock.patch.stopall)
        mock.patch.object(A, '_probe', side_effect=probe).start()

    @contextlib.contextmanager
    def guard(self, inventory, expected):
        self.assertFalse(self.guarded)
        self.guarded = True
        self.guard_count += 1
        try:
            yield
        finally:
            self.guarded = False

    def prepare(self, **kwargs):
        return G.prepare_group(self.entries, inventory=self.inventory, expected_digests=self.expected,
                               deployment_guard=self.guard, **kwargs)

    def publish(self, group):
        return G.publish(group, deployment_guard=self.guard)

    def blocked(self):
        for entry in self.entries:
            with self.assertRaisesRegex(ValueError, 'recovery_required'):
                Snapshot.capture([str(entry)], read_mode='frozen')
            with self.assertRaisesRegex(C.HistoryError, 'group_recovery_required'):
                A.recover(entry, deployment_guard=self.guard)

    def interrupt_first_marker(self, group):
        original = T._replace
        first = Path(P.layout(group.entries[0])['history_authority'])
        def fail(path, raw):
            original(path, raw)
            if Path(path) == first:
                raise OSError('first marker changed')
        with mock.patch.object(T, '_replace', side_effect=fail):
            with self.assertRaisesRegex(OSError, 'first marker changed'):
                self.publish(group)

    def test_exact_selected_group_activation_inverse_and_pending_unchanged(self):
        from tests.test_pending_grounding import fixture_bundle
        ledger = A.G.Store(A.V.project_for([str(self.entries[0])]))
        ledger._capture(fixture_bundle(), event_id='group-pending', contribution_id='group-pending',
                        shareability='project')
        self.assertTrue(ledger.snapshot()['events'])
        pending = {str(entry): A._pending(A.V.project_for([str(entry)])) for entry in self.entries}
        group = self.prepare(operation='group-activate', recorded_at='2026-09-17')
        self.assertEqual({str(p): p.read_bytes() for p in self.entries}, self.originals)
        replay = G.GroupPrepared.from_bytes(group.to_bytes())
        self.assertEqual(group.to_bytes(), replay.to_bytes())
        self.assertEqual(self.publish(replay)['state'], 'activated')
        for entry in self.entries:
            self.assertEqual(H.Store(entry).capture().marker['authority'], 'history')
            self.assertEqual(A._pending(A.V.project_for([str(entry)])), pending[str(entry)])
        inverse = self.prepare(activations=replay)
        self.assertEqual(self.publish(inverse)['state'], 'deactivated')
        for entry in self.entries:
            self.assertEqual(entry.read_bytes(), self.originals[str(entry)])
            self.assertEqual(A._pending(A.V.project_for([str(entry)])), pending[str(entry)])
        self.assertEqual(self.guard_count, 4)

    def test_after_first_marker_both_readers_block_exact_forward_retry(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        self.blocked()
        result = G.recover(G.journal_for(group), deployment_guard=self.guard)
        self.assertEqual(result['operation'], group.to_data()['operation'])
        for entry in self.entries:
            Snapshot.capture([str(entry)], read_mode='frozen')
            self.assertEqual(H.Store(entry).capture().marker['authority'], 'history')
        # The durable completion receipt makes an exact retry idempotent.
        self.publish(G.GroupPrepared.from_bytes(group.to_bytes()))

    def test_after_first_marker_rollback_restores_both_keeps_immutable_objects(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        self.blocked()
        staged = {str(entry): {path: A._tree(entry.parent, path) for path in A._trees(entry)} for entry in self.entries}
        G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        for entry in self.entries:
            self.assertEqual(entry.read_bytes(), self.originals[str(entry)])
            current = {path: A._tree(entry.parent, path) for path in A._trees(entry)}
            for tree, old in staged[str(entry)].items():
                self.assertTrue(old.items() <= current[tree].items())
            marker = C.decode_document(Path(P.layout(entry)['history_authority']).read_bytes())
            self.assertEqual((marker['authority'], marker['generation']), ('legacy', 2))
            self.assertIn('1', marker['cancellations'])
            Snapshot.capture([str(entry)], read_mode='frozen')

    def test_reversed_selection_uses_same_canonical_lock_and_guard_order(self):
        seen = []
        lock = T._lock
        @contextlib.contextmanager
        def capture(root, exclusive):
            seen.append(str(Path(root).resolve()))
            with lock(root, exclusive):
                yield
        with mock.patch.object(T, '_lock', side_effect=capture):
            group = G.prepare_group(list(reversed(self.entries)), inventory=self.inventory,
                expected_digests=self.expected, deployment_guard=self.guard)
        self.assertEqual(group.entries, sorted(map(str, self.entries)))
        first_root = seen.index(str(self.root))
        second_root = seen.index(str(self.other))
        self.assertLess(first_root, second_root)
        self.assertEqual(self.guard_count, 1)

    def test_scope_source_runtime_and_missing_guard_refusals_do_not_change_records(self):
        with self.assertRaisesRegex(C.HistoryError, 'invalid_group_entries'):
            G.prepare_group([self.entries[0]], inventory=self.inventory, expected_digests=self.expected,
                            deployment_guard=self.guard)
        with self.assertRaisesRegex(C.HistoryError, 'deployment_guard_required'):
            G.prepare_group(self.entries, inventory=self.inventory, expected_digests=self.expected, deployment_guard=None)
        bad = copy.deepcopy(self.expected)
        bad['fixture']['sources'] = 'f' * 64
        with self.assertRaisesRegex(C.HistoryError, 'runtime_digest_mismatch'):
            G.prepare_group(self.entries, inventory=self.inventory, expected_digests=bad, deployment_guard=self.guard)
        group = self.prepare()
        self.entries[1].write_bytes(self.originals[str(self.entries[1])] + b'# concurrent\n')
        with self.assertRaisesRegex(C.HistoryError, 'concurrent_edit|transition_source_changed'):
            self.publish(group)
        self.assertFalse(G.journal_for(group).exists())
        for entry in self.entries:
            self.assertFalse(Path(P.layout(entry)['history_authority']).exists())

    def test_pre_ready_cancellation_preserves_later_unguarded_write(self):
        group = self.prepare()
        target = T._target(self.entries[1].parent, T.journal_for(self.entries[1]))
        publish = T.publish_immutable
        def fail(path, raw, **kwargs):
            if Path(path) == target:
                raise OSError('guard installation crash')
            return publish(path, raw, **kwargs)
        with mock.patch.object(T, 'publish_immutable', side_effect=fail):
            with self.assertRaisesRegex(OSError, 'guard installation crash'):
                self.publish(group)
        changed = self.originals[str(self.entries[1])] + b'# later independent edit\n'
        self.entries[1].write_bytes(changed)
        G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        self.assertEqual(self.entries[1].read_bytes(), changed)
        self.assertEqual(self.entries[0].read_bytes(), self.originals[str(self.entries[0])])

    def test_missing_ready_after_publication_is_not_preparation_cancellation(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        G._group_paths(group)[1].unlink()
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_group_readiness'):
            G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        self.blocked()

    def test_recovery_refuses_unrelated_changed_member_without_partial_repair(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        self.entries[1].write_bytes(self.originals[str(self.entries[1])] + b'# unrelated\n')
        first = self.entries[0].read_bytes()
        with self.assertRaisesRegex(C.HistoryError, 'concurrent_edit|transition_source_changed'):
            G.recover(G.journal_for(group), deployment_guard=self.guard)
        self.assertEqual(self.entries[0].read_bytes(), first)
        self.blocked()

    def test_new_unselected_worktree_and_nonlive_context_are_refused(self):
        group = self.prepare()
        third = self.base / 'third'
        subprocess.run(['git', '-C', str(self.root), 'worktree', 'add', '-q', '-b', 'third', str(third)],
                       check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        with self.assertRaisesRegex(C.HistoryError, 'group_inventory_mismatch'):
            self.publish(group)
        self.assertFalse(G.journal_for(group).exists())
        for entry in self.entries:
            self.assertEqual(entry.read_bytes(), self.originals[str(entry)])
        context = G._GroupContext(group.entries, {}, 'fake', self.inventory, self.expected)
        context.active = True
        with self.assertRaisesRegex(C.HistoryError, 'inactive_group_context'):
            context.receipt()

    def test_small_guards_do_not_copy_other_private_bodies_and_forgery_stays_blocked(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        for path, (_, raw) in G._local_guards(group).items():
            self.assertLess(len(raw), 2000)
            self.assertNotIn(b'p.input', raw)
        path = T._target(self.entries[1].parent, T.journal_for(self.entries[1]))
        path.write_bytes(b'forged group guard')
        with self.assertRaisesRegex(C.HistoryError, 'group_guard_mismatch'):
            G.recover(G.journal_for(group), deployment_guard=self.guard)
        for entry in self.entries:
            with self.assertRaisesRegex(ValueError, 'recovery_required'):
                Snapshot.capture([str(entry)], read_mode='frozen')

    def test_forged_cancellation_completion_cannot_expose_partially_changed_authorities(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        completed = G._group_paths(group)[2]
        completed.write_bytes(G._complete_bytes(group, 'before', cancelled=True))
        with self.assertRaisesRegex(C.HistoryError, 'incomplete_group_readiness'):
            G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        self.blocked()

    def test_cleanup_crash_rechecks_durable_completion_before_removing_guards(self):
        group = self.prepare()
        with mock.patch.object(G, '_cleanup', side_effect=OSError('cleanup crash')):
            with self.assertRaisesRegex(OSError, 'cleanup crash'):
                self.publish(group)
        self.blocked()
        G.recover(G.journal_for(group), deployment_guard=self.guard)
        for entry in self.entries:
            self.assertEqual(H.Store(entry).capture().marker['authority'], 'history')

    def test_low_level_single_transition_cannot_publish_group_member(self):
        group = self.prepare()
        entry = Path(group.entries[0])
        mutation = group.mutations[str(entry)]
        with self.assertRaisesRegex(C.HistoryError, 'group_recovery_required'):
            T.publish_transition(entry.parent, T.journal_for(entry), mutation, verify=lambda unused: None)
        primary = T._target(entry.parent, T.journal_for(entry))
        primary.parent.mkdir(parents=True, exist_ok=True)
        primary.write_bytes(mutation.to_bytes())
        with self.assertRaisesRegex(C.HistoryError, 'group_recovery_required'):
            T.recover_transition(entry.parent, T.journal_for(entry), verify=lambda unused: None)
        for entry in self.entries:
            self.assertEqual(entry.read_bytes(), self.originals[str(entry)])

    def test_project_identity_recheck_refuses_changed_git_common_directory(self):
        with G._guard(self.entries, 'identity-check', self.inventory, self.expected, self.guard) as context:
            entry = str(self.entries[0])
            project = context.projects[entry]
            state = A._project_state(Path(entry), project)
            changed = copy.copy(project)
            changed.common = self.base / 'different-git-common'
            with mock.patch.object(G.V, 'project_for', return_value=changed):
                with self.assertRaisesRegex(C.HistoryError, 'group_project_changed'):
                    context.validate_state(state)
        self.assertEqual({str(path): path.read_bytes() for path in self.entries}, self.originals)

    def test_cancelled_generation_is_fenced_before_group_reactivation(self):
        first = self.prepare()
        self.publish(first)
        inverse = self.prepare(activations=first)
        self.publish(inverse)
        cancelled = self.prepare()
        self.interrupt_first_marker(cancelled)
        G.recover(G.journal_for(cancelled), deployment_guard=self.guard, direction='before')
        for entry in self.entries:
            marker = C.decode_document(Path(P.layout(entry)['history_authority']).read_bytes())
            self.assertEqual((marker['authority'], marker['generation']), ('legacy', 4))
            self.assertIn('3', marker['cancellations'])
            self.assertEqual(entry.read_bytes(), self.originals[str(entry)])
        latest = self.prepare()
        self.publish(latest)
        for entry in self.entries:
            captured = H.Store(entry).capture()
            self.assertEqual(captured.marker['generation'], 5)
            self.assertEqual({C.decode_document(raw)['authority_generation'] for raw in captured.commits.values()}, {5})
            for image in A.cancellation_plan(entry, cancelled.mutations[str(entry)])['immutable_images']:
                self.assertEqual((entry.parent / image['path']).read_bytes(), image['after'])

    def test_compensation_interruption_requires_same_direction_and_exact_terminal_proof(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        original = A.apply_cancellation
        first = group.entries[0]
        def crash(entry, mutation, plan):
            original(entry, mutation, plan)
            if str(entry) == first:
                raise OSError('first compensation durable')
        with mock.patch.object(A, 'apply_cancellation', side_effect=crash):
            with self.assertRaisesRegex(OSError, 'first compensation durable'):
                G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        self.blocked()
        with self.assertRaisesRegex(C.HistoryError, 'group_cancellation_in_progress'):
            G.recover(G.journal_for(group), deployment_guard=self.guard, direction='after')
        G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        for entry in self.entries:
            self.assertEqual(entry.read_bytes(), self.originals[str(entry)])
            self.assertEqual(C.decode_document(Path(P.layout(entry)['history_authority']).read_bytes())['generation'], 2)
        self.publish(self.prepare())
        self.assertTrue(all(H.Store(entry).capture().marker['generation'] == 3 for entry in self.entries))

    def test_compensation_checks_all_sources_and_runtime_before_first_fence(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        before = {entry: Path(P.layout(entry)['history_authority']).read_bytes()
                  if Path(P.layout(entry)['history_authority']).exists() else None for entry in self.entries}
        self.entries[1].write_bytes(self.originals[str(self.entries[1])] + b'# unrelated\n')
        with self.assertRaisesRegex(C.HistoryError, 'transition_source_changed|cancellation_entry_mismatch'):
            G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        self.entries[1].write_bytes(self.originals[str(self.entries[1])])
        old_expected = self.expected
        self.expected = {'drift': {}}
        try:
            with self.assertRaisesRegex(C.HistoryError, 'runtime_digest_mismatch'):
                G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        finally:
            self.expected = old_expected
        for entry in self.entries:
            self.assertEqual(T._read(Path(P.layout(entry)['history_authority'])), before[entry])
            plan = A.cancellation_plan(entry, group.mutations[str(entry)])
            self.assertFalse((entry.parent / plan['receipt_path']).exists())
        self.assertFalse(G.journal_for(group).with_suffix('.cancel').exists())

    def test_compensated_completion_is_revalidated_after_cleanup_interruption(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        with mock.patch.object(G, '_cleanup', side_effect=OSError('compensated cleanup crash')):
            with self.assertRaisesRegex(OSError, 'compensated cleanup crash'):
                G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        self.blocked()
        G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        for entry in self.entries:
            self.assertEqual(entry.read_bytes(), self.originals[str(entry)])
            self.assertEqual(C.decode_document(Path(P.layout(entry)['history_authority']).read_bytes())['generation'], 2)

    def test_generic_recovery_cannot_restore_old_epoch_after_cancellation_fence(self):
        entry = self.base / 'single' / 'GROUNDING.yaml'
        entry.parent.mkdir()
        entry.write_bytes(next(iter(self.originals.values())))
        mutation = A.prepare_activation(entry, inventory=self.inventory, expected_digests=self.expected,
                                        deployment_guard=self.guard)
        replace = T._replace
        def crash(path, raw):
            replace(path, raw)
            if Path(path) == entry:
                raise OSError('single publication crash')
        with mock.patch.object(T, '_replace', side_effect=crash):
            with self.assertRaisesRegex(OSError, 'single publication crash'):
                A.publish(entry, mutation, deployment_guard=self.guard)
        apply = A.apply_cancellation
        def compensated(*args):
            apply(*args)
            raise OSError('single compensation crash')
        with mock.patch.object(A, 'apply_cancellation', side_effect=compensated):
            with self.assertRaisesRegex(OSError, 'single compensation crash'):
                A.recover(entry, deployment_guard=self.guard, direction='before')
        marker = Path(P.layout(entry)['history_authority']).read_bytes()
        for direction in ('before', 'after'):
            with self.assertRaisesRegex(C.HistoryError, 'cancellation_recovery_required'):
                T.recover_transition(entry.parent, T.journal_for(entry), direction=direction,
                                     verify=lambda data: None)
            self.assertEqual(Path(P.layout(entry)['history_authority']).read_bytes(), marker)

    def test_forged_compensated_completion_cannot_clear_partial_group_guards(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        G._group_paths(group)[2].write_bytes(G._complete_bytes(group, 'before', compensated=True))
        with self.assertRaisesRegex(C.HistoryError, 'transition_unfinalized'):
            G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        self.blocked()

    def test_interrupted_deactivation_rollback_restores_exact_history_authorities(self):
        activation = self.prepare()
        self.publish(activation)
        before = {entry: (entry.read_bytes(), Path(P.layout(entry)['history_authority']).read_bytes())
                  for entry in self.entries}
        inverse = self.prepare(activations=activation)
        self.interrupt_first_marker(inverse)
        self.blocked()
        G.recover(G.journal_for(inverse), deployment_guard=self.guard, direction='before')
        for entry, (record, marker) in before.items():
            self.assertEqual(entry.read_bytes(), record)
            self.assertEqual(Path(P.layout(entry)['history_authority']).read_bytes(), marker)
            self.assertEqual(H.Store(entry).capture().marker['generation'], 1)

    def test_interrupted_compensation_names_exact_manual_restore_bytes(self):
        group = self.prepare()
        self.interrupt_first_marker(group)
        apply = A.apply_cancellation
        def interrupt(entry, mutation, plan):
            apply(entry, mutation, plan)
            if str(entry) == group.entries[0]:
                raise OSError('first compensated')
        with mock.patch.object(A, 'apply_cancellation', side_effect=interrupt):
            with self.assertRaisesRegex(OSError, 'first compensated'):
                G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        other = Path(group.entries[1])
        divergent = self.originals[str(other)] + b'# independent edit to retain\n'
        other.write_bytes(divergent)
        with self.assertRaises(C.HistoryError) as caught:
            G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        self.assertIn(str(other), str(caught.exception))
        self.assertIn(C.sha256(self.originals[str(other)]), str(caught.exception))
        self.assertIn('save the divergent bytes separately', str(caught.exception))
        self.blocked()
        backup = self.base / 'independent-edit.txt'
        backup.write_bytes(divergent)
        retained = next(item['before'] for item in group.mutations[str(other)].files if item['role'] == 'record')
        other.write_bytes(retained)
        G.recover(G.journal_for(group), deployment_guard=self.guard, direction='before')
        self.assertEqual(backup.read_bytes(), divergent)
        for entry in self.entries:
            Snapshot.capture([str(entry)], read_mode='frozen')

    def test_final_verification_failure_reports_unfinalized_and_keeps_every_guard(self):
        group = self.prepare()
        verify = G._verify
        calls = []
        def fail(current, context):
            calls.append(True)
            if len(calls) == 2:
                raise C.HistoryError('fixture_final_verification')
            return verify(current, context)
        with mock.patch.object(G, '_verify', side_effect=fail):
            with self.assertRaisesRegex(C.HistoryError, 'transition_unfinalized.*fixture_final_verification'):
                self.publish(group)
        self.blocked()
        G.recover(G.journal_for(group), deployment_guard=self.guard)
        for entry in self.entries:
            self.assertEqual(H.Store(entry).capture().marker['generation'], 1)
