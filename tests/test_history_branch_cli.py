"""History branch reads pin committed evidence and never perform an adoption."""
import contextlib
import datetime
import io
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

from scripts import consolidate as S, history_authoring as A, history_hypotheses as HH
from scripts import history_store as H, history_contract as C, project_modes as G, pending_grounding as PG
from tests import test_history_branch as fixtures


class BranchCLI(unittest.TestCase):
    def setUp(self):
        fixture = fixtures.BranchCapture(); fixture.setUp(); self.addCleanup(fixture.doCleanups)
        self.fixture, self.repo, self.entry = fixture, fixture.repo, fixture.record

    def inventory(self):
        return {p.relative_to(self.repo).as_posix(): p.read_bytes() for p in self.repo.rglob('*') if p.is_file()}

    def call(self, *args):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = S.main([*args, str(self.entry)])
        return code, output.getvalue()

    def commit_tree(self, message):
        G.git(self.repo, 'add', '.')
        G.git(self.repo, '-c', 'commit.gpgsign=false', 'commit', '-m', message)
        return G.git(self.repo, 'rev-parse', 'HEAD').stdout.decode().strip()

    def test_pull_names_pinned_branch_and_current_standing_without_mutating_any_repo_bytes(self):
        original = H.Store(self.entry).capture()
        branch_head = original.state['subjects']['p.value']['head']
        mutation = A.prepare(self.entry, {'kind': 'set', 'id': 'p.value', 'value': 3})
        A.commit(self.entry, mutation, verify=lambda data: None)
        current_head = H.Store(self.entry).state()['subjects']['p.value']['head']
        before = self.inventory()
        with mock.patch.object(S.P, 'load', side_effect=AssertionError('legacy flattening')), \
             mock.patch.object(S.P, '_keep', side_effect=AssertionError('parser cache write')):
            code, text = self.call('pull', 'p.value', '--from', 'main')
        self.assertEqual(code, 0)
        self.assertIn('BRANCH OBSERVATION', text)
        self.assertIn('no target acceptance or adoption', text)
        self.assertIn('source commit: ' + self.fixture.sha, text)
        self.assertIn(branch_head, text)
        self.assertIn(current_head, text)
        self.assertIn('v: 1', text)
        self.assertIn('v: 3', text)
        self.assertEqual(self.inventory(), before)
        self.assertIsNone(PG.Store(self.repo).head())

    def test_unicode_named_proposals_preserve_original_head_dates_seen_and_body(self):
        subject = 'מדידה.טמפרטורה'
        mutation = A.prepare_proposal(self.entry, subject, {'v': 22, 'note': 'נתון מקורי'}, 'known',
            because='explicit alternative', hypothesis={'version': 1, 'name': 'חלופה א',
                'head': {'claim': 'תיאור מקורי', 'born': datetime.date(2026, 9, 17), 'folds': 'never'}})
        A.commit(self.entry, mutation, verify=lambda data: None)
        pinned = self.commit_tree('Named Unicode alternative')
        before = self.inventory()
        code, text = self.call('pull', subject, '--from', 'main')
        self.assertEqual(code, 0)
        for phrase in (subject, 'חלופה א', 'נתון מקורי', 'תיאור מקורי', '2026-09-17', 'folds: never', pinned):
            self.assertIn(phrase, text)
        self.assertIn('standing: proposed', text)
        self.assertIn('named_proposals:', text)
        self.assertEqual(self.inventory(), before)

    def test_physical_branch_and_current_proposals_are_read_without_cache_or_acceptance(self):
        path = Path(S.P.layout(self.entry)['hypotheses']) / 'scenario.yaml'
        path.parent.mkdir(parents=True)
        path.write_text('hypothesis: {claim: Physical scenario, folds: never}\nknown: {p.value: {v: 8, seen: {p.value: 1}}}\n')
        self.commit_tree('Physical scenario')
        before = self.inventory()
        with mock.patch.object(S.P, 'parse', side_effect=AssertionError('cached parser')):
            code, text = self.call('pull', 'p.value', '--from', 'main')
        self.assertEqual(code, 0)
        self.assertIn('Physical scenario', text)
        self.assertIn('v: 8', text)
        self.assertIn('seen:', text)
        self.assertEqual(H.Store(self.entry).state()['subjects']['p.value']['body']['v'], 1)
        self.assertEqual(self.inventory(), before)

    def test_budget_reports_omitted_subjects_and_unknown_seed(self):
        A.commit(self.entry, A.prepare(self.entry, {'kind': 'add', 'id': 'p.extra', 'body': {'v': 2}}), verify=lambda data: None)
        self.commit_tree('Extra subject')
        code, text = self.call('pull', 'p', 'missing', '--from', 'main', '--budget', '1')
        self.assertEqual(code, 0)
        self.assertIn('subjects shown: 1; omitted by budget: 1', text)
        self.assertIn('unmatched seeds: missing', text)
        self.assertEqual(text.count('SUBJECT '), 1)

    def test_branch_ref_is_captured_once_even_if_it_moves_during_comparison(self):
        branch = S.P._peer('history_branch')
        original = branch.capture
        calls = []
        def moving(*args, **kwargs):
            envelope = original(*args, **kwargs)
            calls.append(envelope['manifest']['source']['commit'])
            (self.repo / 'unrelated.txt').write_text('later commit')
            self.commit_tree('Later unrelated branch')
            return envelope
        with mock.patch.object(branch, 'capture', side_effect=moving):
            _, text = self.call('pull', 'p.value', '--from', 'main')
        self.assertEqual(calls, [self.fixture.sha])
        self.assertIn('source commit: ' + self.fixture.sha, text)

    def test_changed_routing_during_read_refuses_without_accepting_another_target(self):
        views, branch = S.P._peer('knowledge_views'), S.P._peer('history_branch')
        original_routes, original_capture = views.write_paths, branch.capture
        changed = [False]
        def routes(paths):
            return [str(self.repo / 'other.yaml')] if changed[0] else original_routes(paths)
        def capture(*args, **kwargs):
            envelope = original_capture(*args, **kwargs)
            changed[0] = True
            return envelope
        before = self.inventory()
        with mock.patch.object(views, 'write_paths', side_effect=routes),              mock.patch.object(branch, 'capture', side_effect=capture):
            with self.assertRaisesRegex(S.P.Refused, 'history_routing_changed'):
                self.call('pull', 'p.value', '--from', 'main')
        self.assertEqual(self.inventory(), before)

    def test_branch_choice_parser_rejects_duplicate_and_invalid_choices_before_writes(self):
        version = H.Store(self.entry).state()['subjects']['p.value']['head']
        before = self.inventory()
        for extra, reason in [(['--choose', 'p.value=' + version, '--choose', 'p.value=' + version], 'duplicate --choose'),
                              (['--choose', 'p.value=not-a-version'], 'exact history version'),
                              (['--by', 'one', '--by', 'two'], 'one nonempty actor')]:
            with self.subTest(reason=reason), self.assertRaisesRegex(S.P.Refused, reason):
                self.call('--from', 'main', *extra)
        self.assertEqual(self.inventory(), before)

    def test_duplicate_branch_sources_and_mixed_choice_modes_refuse_atomically(self):
        version = H.Store(self.entry).state()['subjects']['p.value']['head']
        before = self.inventory()
        with self.assertRaisesRegex(S.P.Refused, 'one --from'):
            self.call('pull', 'p.value', '--from', 'main', '--from', 'HEAD')
        with self.assertRaisesRegex(S.P.Refused, 'duplicate_branch_source'):
            self.call('--from', 'main', '--from', 'HEAD', '--by', 'reviewer', '--choose', 'p.value=' + version)
        with self.assertRaisesRegex(S.P.Refused, 'history_branch_choices_required'):
            self.call('--from', 'main', '--by', 'reviewer', '--choose', 'p.value=' + version, '--take', 'p.value')
        with self.assertRaisesRegex(S.P.Refused, '--by ACTOR'):
            self.call('--from', 'main', '--choose', 'p.value=' + version)
        self.assertEqual(self.inventory(), before)

    def test_actual_cli_adopts_two_pinned_sources_in_one_manifest(self):
        from tests import test_history_branch_direct as direct
        fixture = direct.BranchDirect()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        selected, _ = fixture.second_source()
        command = [sys.executable, '-B', str(Path(S.__file__).with_name('cli.py')), 'consolidate',
                   '--from', 'incoming', '--from', 'incoming-two', str(fixture.entry)]
        preview_run = subprocess.run([*command, '--dry-run'], capture_output=True, text=True, timeout=30)
        self.assertEqual(preview_run.returncode, 0, preview_run.stdout + preview_run.stderr)
        preview = S.yaml.safe_load(preview_run.stdout.split('\n', 1)[1])
        self.assertEqual(len(preview['source_revisions']), 2)
        result = subprocess.run([*command, '--by', 'fixture operator', '--choose', 'p.value=' + selected,
                                 '--source-revision', preview['source_set_revision']],
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        captured = H.Store(fixture.entry).capture()
        self.assertEqual(captured.state['subjects']['p.value']['head'], selected)
        self.assertEqual(len(captured.commits), len(fixture.before.commits) + 1)
        self.assertIsNone(PG.Store(fixture.repo).head())

    def test_actual_preview_then_pinned_choice_adopts_one_branch_without_git_or_ledger_publication(self):
        branch_head = H.Store(self.entry).state()['subjects']['p.value']['head']
        G.git(self.repo, 'tag', 'source-observation', self.fixture.sha)
        A.commit(self.entry, A.prepare(self.entry, {'kind': 'set', 'id': 'p.value', 'value': 3}), verify=lambda data: None)
        self.commit_tree('Target reading three')
        old_target = H.Store(self.entry).state()['subjects']['p.value']['head']
        before_preview = self.inventory()
        code, output = self.call('--from', 'source-observation', '--dry-run')
        self.assertEqual(code, 0)
        self.assertIn('BRANCH ADOPTION PREVIEW', output)
        preview = S.yaml.safe_load(output.split('\n', 1)[1])
        self.assertEqual(preview['source']['commit'], self.fixture.sha)
        self.assertTrue(preview['subjects']['p.value']['requires_choice'])
        self.assertEqual(self.inventory(), before_preview)
        git_before = G.git(self.repo, 'rev-parse', 'HEAD').stdout
        code, output = self.call('--from', 'source-observation', '--by', 'reviewer',
            '--choose', 'p.value=' + branch_head, '--source-revision', preview['source_revision'])
        self.assertEqual(code, 0)
        self.assertIn('BRANCH ADOPTED', output)
        capture = H.Store(self.entry).capture()
        self.assertEqual(capture.state['subjects']['p.value']['head'], branch_head)
        self.assertIn(old_target, capture.objects)
        self.assertEqual(G.git(self.repo, 'rev-parse', 'HEAD').stdout, git_before)
        self.assertIsNone(PG.Store(self.repo).head())

    def test_changed_preview_source_binding_refuses_and_choice_parser_preserves_unicode(self):
        _, output = self.call('--from', 'main', '--dry-run')
        preview = S.yaml.safe_load(output.split('\n', 1)[1])
        (self.repo / 'later.txt').write_text('later source commit')
        self.commit_tree('Changed source ref')
        before = self.inventory()
        version = H.Store(self.entry).state()['subjects']['p.value']['head']
        with self.assertRaisesRegex(S.P.Refused, 'branch_source_changed'):
            self.call('--from', 'main', '--by', 'reviewer', '--choose', 'p.value=' + version,
                      '--source-revision', preview['source_revision'])
        self.assertEqual(self.inventory(), before)
        direct = S.P._peer('history_direct')
        with mock.patch.object(direct, 'adopt_branch', return_value={'state': 'prepared'}) as adopt:
            self.call('--from', 'main', '--dry-run', '--by', 'אדם', '--choose', 'שם=חלק=' + version)
        self.assertEqual(adopt.call_args.kwargs['choices'], {'שם=חלק': version})
        self.assertEqual(adopt.call_args.kwargs['by'], 'אדם')

    def test_actual_cli_routes_history_pull_and_preserves_git_and_ledger(self):
        before = self.inventory()
        script = Path(S.__file__).parent / 'kpopper'
        environment = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', KPOPPER_NO_CACHE='1')
        result = subprocess.run([sys.executable, '-B', str(script), 'pull', 'p.value', '--from', 'main', str(self.entry)],
            cwd=self.repo, env=environment, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(self.fixture.sha, result.stdout)
        self.assertIn('BRANCH OBSERVATION', result.stdout)
        self.assertEqual(self.inventory(), before)

    def test_legacy_pull_path_still_uses_existing_hypothesis_projection(self):
        with mock.patch.object(S.P._peer('history_direct'), 'active', return_value=False), \
             mock.patch.object(S, 'from_ref', return_value=[]) as from_ref, \
             mock.patch.object(S.P, 'load', return_value=S.P.Record({'known': {'p.value': {'v': 1}}})), \
             mock.patch.object(S.P, 'pull', return_value=7) as pull:
            self.assertEqual(S.pull_from([str(self.entry)], 'main', ['p.value']), 7)
        from_ref.assert_called_once()
        pull.assert_called_once()


if __name__ == '__main__': unittest.main()
