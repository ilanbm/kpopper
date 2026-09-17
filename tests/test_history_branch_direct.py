"""Local branch adoption preserves routing, explicit choices and exact crash recovery."""
import contextlib
import io
from pathlib import Path
import unittest
from unittest import mock

from scripts import history_authoring as A, history_branch as Branch, history_direct as D
from scripts import history_store as H, history_transaction as T, history_contract as C, project_modes as G
from scripts import pending_grounding as PG, provenance as P
from tests import test_history_branch as fixtures


class BranchDirect(unittest.TestCase):
    def setUp(self):
        fixture = fixtures.BranchCapture()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture, self.repo, self.entry = fixture, fixture.repo, fixture.record
        G.git(self.repo, 'checkout', '-b', 'incoming')
        with contextlib.redirect_stdout(io.StringIO()):
            mutation = A.prepare(self.entry, {'kind': 'set', 'id': 'p.value', 'value': 2})
        A.commit(self.entry, mutation, verify=lambda data: None)
        self.incoming = H.Store(self.entry).state()['subjects']['p.value']['head']
        G.git(self.repo, 'add', '.')
        G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Incoming observation')
        self.source_commit = G.git(self.repo, 'rev-parse', 'HEAD').stdout.decode().strip()
        G.git(self.repo, 'checkout', 'main')
        self.before = H.Store(self.entry).capture()

    def adopt(self, **options):
        return D.adopt_branch([str(self.entry)], 'incoming',
                             choices={'p.value': self.incoming}, by='fixture operator', **options)

    def second_source(self):
        G.git(self.repo, 'checkout', '-b', 'incoming-two')
        mutation = A.prepare(self.entry, {'kind': 'set', 'id': 'p.value', 'value': 3})
        A.commit(self.entry, mutation, verify=lambda data: None)
        head = H.Store(self.entry).state()['subjects']['p.value']['head']
        G.git(self.repo, 'add', '.')
        G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Second incoming observation')
        commit = G.git(self.repo, 'rev-parse', 'HEAD').stdout.decode().strip()
        G.git(self.repo, 'checkout', 'main')
        return head, commit

    def adopt_set(self, choices, **options):
        return D.adopt_branches([str(self.entry)], ['incoming', 'incoming-two'], choices=choices,
                                by='fixture operator', **options)

    def test_preview_and_same_authority_adoption_do_not_publish_to_pending(self):
        preview = D.adopt_branch([str(self.entry)], 'incoming', preview=True)
        self.assertEqual(preview['source']['commit'], self.source_commit)
        self.assertTrue(preview['subjects']['p.value']['requires_choice'])
        self.assertEqual(H.Store(self.entry).capture().inventory, self.before.inventory)
        result = self.adopt(expected_source_revision=preview['source_revision'])
        current = H.Store(self.entry).capture()
        self.assertEqual(current.state['subjects']['p.value']['body']['v'], 2)
        self.assertEqual(current.state['subjects']['p.value']['head'], self.incoming)
        self.assertEqual(len(current.commits), len(self.before.commits) + 1)
        self.assertEqual(result['source_commit'], self.source_commit)
        self.assertEqual(G.git(self.repo, 'rev-parse', 'incoming').stdout.decode().strip(), self.source_commit)
        self.assertIsNone(PG.Store(G.Project(self.repo)).head())
        for version, obj in self.before.objects.items():
            self.assertEqual(current.objects[version], obj)
        manifest = C.decode_document(current.commits[result['operation']])
        audit = manifest['receipt']['after']['history_branch_adoption']['source']['audit']
        capsule = (self.entry.parent / audit['path']).read_bytes()
        self.assertEqual(C.sha256(capsule), audit['sha256'])
        with mock.patch.object(Branch, '_git', side_effect=AssertionError('source lookup')):
            retained = Branch.from_bytes(capsule)
            self.assertEqual(Branch.validate(retained).state['subjects']['p.value']['head'], self.incoming)

    def test_forward_recovery_uses_retained_source_even_after_the_ref_is_removed(self):
        replace = T._replace
        def interrupt(path, raw):
            if Path(path).resolve() == self.entry.resolve():
                raise OSError('branch view interrupted')
            return replace(path, raw)
        with mock.patch.object(T, '_replace', side_effect=interrupt), self.assertRaisesRegex(
                OSError, 'branch view interrupted'):
            self.adopt()
        pending = self.entry.parent / D.journal(self.entry)
        self.assertTrue(pending.is_file())
        count = len(H.Store(self.entry).capture().commits)
        G.git(self.repo, 'branch', '-D', 'incoming')
        with mock.patch.object(Branch, '_git', side_effect=AssertionError('source Git reread')), \
             mock.patch.object(Branch, 'capture', side_effect=AssertionError('source recapture')):
            mutation = P.recover_direct([str(self.entry)])
        current = H.Store(self.entry).capture()
        self.assertEqual(len(current.commits), count)
        self.assertIn(mutation.to_data()['operation'], current.commits)
        self.assertEqual(current.state['subjects']['p.value']['head'], self.incoming)
        self.assertFalse(pending.exists())
        self.assertIsNone(PG.Store(G.Project(self.repo)).head())

    def test_rollback_before_manifest_cancels_only_the_retained_operation(self):
        publish = T.publish_immutable
        commits = Path(H.Store(self.entry).layout['history_commits']).resolve()
        def interrupt(path, raw, **options):
            if Path(path).resolve().parent == commits:
                raise OSError('branch manifest interrupted')
            return publish(path, raw, **options)
        with mock.patch.object(T, 'publish_immutable', side_effect=interrupt), self.assertRaises(OSError):
            self.adopt()
        P.recover_direct([str(self.entry)], direction='before')
        current = H.Store(self.entry).capture()
        self.assertEqual(current.entry_bytes, self.before.entry_bytes)
        self.assertEqual(current.commits, self.before.commits)
        self.assertEqual(current.objects, self.before.objects)
        self.assertFalse((self.entry.parent / D.journal(self.entry)).exists())

    def test_missing_choices_changed_preview_and_uncommitted_target_refuse(self):
        with self.assertRaisesRegex(ValueError, 'adoption_choice_required'):
            D.adopt_branch([str(self.entry)], 'incoming', by='fixture operator')
        with self.assertRaisesRegex(ValueError, 'branch_source_changed'):
            self.adopt(expected_source_revision='0' * 64)
        A.commit(self.entry, A.prepare(self.entry, {'kind': 'set', 'id': 'p.value', 'value': 3}),
                 verify=lambda data: None)
        captured = H.Store(self.entry).capture()
        with self.assertRaisesRegex(ValueError, 'branch_target_uncommitted'):
            self.adopt()
        self.assertEqual(H.Store(self.entry).capture().inventory, captured.inventory)
        self.assertFalse((self.entry.parent / D.journal(self.entry)).exists())

    def test_committed_entry_does_not_hide_uncommitted_target_history(self):
        A.commit(self.entry, A.prepare(self.entry, {'kind': 'set', 'id': 'p.value', 'value': 3}),
                 verify=lambda data: None)
        G.git(self.repo, 'add', 'GROUNDING.yaml')
        G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Entry without its history')
        captured = H.Store(self.entry).capture()
        with self.assertRaisesRegex(ValueError, 'branch_target_uncommitted'):
            self.adopt()
        self.assertEqual(H.Store(self.entry).capture().inventory, captured.inventory)
        self.assertFalse((self.entry.parent / D.journal(self.entry)).exists())

    def test_two_divergent_refs_commit_once_with_one_global_choice(self):
        second, second_commit = self.second_source()
        preview = D.adopt_branches([str(self.entry)], ['incoming', 'incoming-two'], preview=True)
        self.assertTrue(preview['subjects']['p.value']['requires_choice'])
        self.assertEqual(len(preview['source_revisions']), 2)
        result = self.adopt_set({'p.value': second}, expected_source_revision=preview['source_set_revision'])
        current = H.Store(self.entry).capture()
        self.assertEqual(len(current.commits), len(self.before.commits) + 1)
        self.assertEqual(current.state['subjects']['p.value']['head'], second)
        self.assertEqual(set(result['source_commits']), {self.source_commit, second_commit})
        manifest = C.decode_document(current.commits[result['operation']])
        self.assertEqual(manifest['receipt']['after']['history_branch_adoption']['version'], 2)
        self.assertIsNone(PG.Store(G.Project(self.repo)).head())

    def test_set_recovery_uses_retained_sources_after_refs_are_removed(self):
        second, _ = self.second_source()
        replace = T._replace
        def interrupt(path, raw):
            if Path(path).resolve() == self.entry.resolve():
                raise OSError('branch set view interrupted')
            return replace(path, raw)
        with mock.patch.object(T, '_replace', side_effect=interrupt), self.assertRaisesRegex(
                OSError, 'branch set view interrupted'):
            self.adopt_set({'p.value': second})
        pending = self.entry.parent / D.journal(self.entry)
        self.assertTrue(pending.is_file())
        G.git(self.repo, 'branch', '-D', 'incoming')
        G.git(self.repo, 'branch', '-D', 'incoming-two')
        with mock.patch.object(Branch, '_git', side_effect=AssertionError('source Git reread')), \
             mock.patch.object(Branch, 'capture', side_effect=AssertionError('source recapture')):
            mutation = P.recover_direct([str(self.entry)])
        current = H.Store(self.entry).capture()
        self.assertIn(mutation.to_data()['operation'], current.commits)
        self.assertEqual(current.state['subjects']['p.value']['head'], second)
        self.assertFalse(pending.exists())
        self.assertIsNone(PG.Store(G.Project(self.repo)).head())

    def test_set_preview_binding_stales_and_single_ref_stays_identical(self):
        second, _ = self.second_source()
        preview = D.adopt_branches([str(self.entry)], ['incoming', 'incoming-two'], preview=True)
        with self.assertRaisesRegex(ValueError, 'branch_source_changed'):
            self.adopt_set({'p.value': second}, expected_source_revision='0' * 64)
        self.assertEqual(H.Store(self.entry).capture().inventory, self.before.inventory)
        single = D.adopt_branches([str(self.entry)], ['incoming'], choices={'p.value': self.incoming},
                                  by='fixture operator')
        self.assertEqual(single['source_commit'], self.source_commit)

    def test_set_manifest_failure_can_cancel_without_partial_knowledge(self):
        second, _ = self.second_source()
        publish = T.publish_immutable
        commits = Path(H.Store(self.entry).layout['history_commits']).resolve()
        def interrupt(path, raw, **options):
            if Path(path).resolve().parent == commits:
                raise OSError('branch set manifest interrupted')
            return publish(path, raw, **options)
        with mock.patch.object(T, 'publish_immutable', side_effect=interrupt), self.assertRaisesRegex(
                OSError, 'branch set manifest interrupted'):
            self.adopt_set({'p.value': second})
        P.recover_direct([str(self.entry)], direction='before')
        current = H.Store(self.entry).capture()
        self.assertEqual(current.entry_bytes, self.before.entry_bytes)
        self.assertEqual(current.commits, self.before.commits)
        self.assertEqual(current.objects, self.before.objects)
        self.assertFalse((self.entry.parent / D.journal(self.entry)).exists())

    def test_set_rollback_refuses_after_its_manifest_is_committed(self):
        second, _ = self.second_source()
        replace = T._replace
        def interrupt(path, raw):
            if Path(path).resolve() == self.entry.resolve():
                raise OSError('branch set view interrupted')
            return replace(path, raw)
        with mock.patch.object(T, '_replace', side_effect=interrupt), self.assertRaisesRegex(
                OSError, 'branch set view interrupted'):
            self.adopt_set({'p.value': second})
        with self.assertRaisesRegex(ValueError, 'history_already_committed'):
            P.recover_direct([str(self.entry)], direction='before')
        P.recover_direct([str(self.entry)])


if __name__ == '__main__':
    unittest.main()
