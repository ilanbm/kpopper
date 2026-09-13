"""Real temporary bare Git transport with an explicitly simulated PR provider."""
import copy
import multiprocessing
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from scripts import pending_grounding as G, pending_publication as C, project_modes as M
from scripts.publication_provider import UnknownOutcome
from tests.test_pending_grounding import fixture_bundle


class ProviderFixture:
    """Provider state only is simulated; branch and target writes are real Git."""
    def __init__(self, repository):
        self.repository = repository
        self.rows = []
        self.creates = 0
        self.updates = 0
        self.offline = False
        self.uncertain_create = False
        self.uncertain_before_create = False
        self.on_list = None

    def list(self, scope):
        assert scope['repository'] == str(self.repository)
        if self.offline:
            raise UnknownOutcome('provider fixture offline')
        if self.on_list:
            callback, self.on_list = self.on_list, None
            callback()
        return copy.deepcopy(self.rows)

    def create(self, scope, *, title, body, request_id):
        self.creates += 1
        if self.uncertain_before_create:
            raise UnknownOutcome('fixture disconnected without a confirmed create')
        if any(row['state'] == 'open' for row in self.rows):
            raise AssertionError('duplicate active fixture PR')
        head = M.git(self.repository, 'rev-parse', 'refs/heads/' + scope['branch']).stdout.decode().strip()
        row = dict(id=str(len(self.rows) + 1), state='open', body=body, url='fixture://pr/' + str(len(self.rows) + 1), head=head)
        self.rows.append(row)
        if self.uncertain_create:
            self.uncertain_create = False
            raise UnknownOutcome('fixture lost create response after remote side effect')
        return copy.deepcopy(row)

    def update(self, scope, pr_id, *, title, body):
        self.updates += 1
        row = next(row for row in self.rows if row['id'] == str(pr_id))
        row['body'] = body
        row['head'] = M.git(self.repository, 'rev-parse', 'refs/heads/' + scope['branch']).stdout.decode().strip()
        return copy.deepcopy(row)


REAL_TRIGGER = C.trigger_after_capture


class PublicationTests(unittest.TestCase):
    def setUp(self):
        trigger = patch.object(C, 'trigger_after_capture', return_value={'started': False, 'reason': 'provider fixture'})
        trigger.start()
        self.addCleanup(trigger.stop)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.root = self.base / 'source'
        self.root.mkdir()
        self.remote = self.base / 'provider-fixture.git'
        M.git(self.root, 'init', '-b', 'trunk')
        M.git(self.root, 'config', 'user.name', 'Fixture')
        M.git(self.root, 'config', 'user.email', 'fixture@example.test')
        (self.root / 'app.txt').write_text('target code\n')
        (self.root / 'GROUNDING.yaml').write_text('{}\n')
        M.git(self.root, 'add', '.')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Target')
        M.git(self.root, 'init', '--bare', str(self.remote))
        M.git(self.root, 'remote', 'add', 'team', str(self.remote))
        M.git(self.root, 'push', 'team', 'trunk')
        self.project = M.Project(self.root)
        self.project.publication_config('team', 'trunk', grant=True)
        self.store = G.Store(self.project)
        self.provider = ProviderFixture(self.remote)
        self.publisher = C.Publisher(self.project, self.provider)
        self.counter = 0

    def capture(self, name=None, value=10, contribution_id='limit'):
        self.counter += 1
        bundle = fixture_bundle(name or 'api.limit' + str(self.counter), value)
        self.store.capture(bundle, event_id='event-' + str(self.counter), contribution_id=contribution_id,
                           shareability='project')
        return bundle['revision']

    def run_ok(self):
        result = self.publisher.run(force_retry=True)
        self.assertNotIn(result['outcome'], ('attention', 'unknown'), result)
        return result

    def remote_head(self, branch='pending_grounding'):
        proc = M.git(self.remote, 'rev-parse', '--verify', 'refs/heads/' + branch, check=False)
        return proc.stdout.decode().strip() if proc.returncode == 0 else None

    def merge(self, method='squash', delete=True, tree=None):
        row = next(row for row in self.provider.rows if row['state'] == 'open')
        branch, old = self.remote_head(), self.remote_head('trunk')
        tree = tree or M.git(self.remote, 'rev-parse', branch + '^{tree}').stdout.decode().strip()
        env = os.environ.copy()
        for role in ('AUTHOR', 'COMMITTER'):
            env['GIT_' + role + '_NAME'] = 'Provider fixture'
            env['GIT_' + role + '_EMAIL'] = 'fixture@example.test'
        parents = ['-p', old] + (['-p', branch] if method == 'merge' else [])
        commit = M.git(self.remote, 'commit-tree', tree, *parents,
                       data=(method + ' target acceptance\n').encode(), env=env).stdout.decode().strip()
        M.git(self.remote, 'update-ref', 'refs/heads/trunk', commit, old)
        row['state'] = 'merged'
        if delete:
            M.git(self.remote, 'update-ref', '-d', 'refs/heads/pending_grounding', branch)
        return commit

    def test_two_cycles_same_branch_cumulative_pr_target_only_and_seen_unchanged(self):
        (self.root / 'app.txt').write_text('unmerged feature code\n')
        M.git(self.root, 'add', 'app.txt')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Feature work')
        first = self.capture()
        ledger1 = self.store.head()
        self.run_ok()
        self.assertEqual(M.git(self.remote, 'show', 'pending_grounding:app.txt').stdout, b'target code\n')
        second = self.capture()
        self.run_ok()
        self.assertEqual(self.provider.creates, 1)
        merged = self.merge('squash')
        self.assertNotEqual(M.git(self.root, 'merge-base', '--is-ancestor', self.publisher.status()['expected_head'],
                                  merged, check=False).returncode, 0)
        third = self.capture()
        result = self.run_ok()
        self.assertEqual(result['states'][first], 'accepted')
        self.assertEqual(result['states'][second], 'accepted')
        self.assertEqual(result['states'][third], 'proposed')
        self.assertEqual(self.provider.creates, 2)
        self.assertIsNotNone(self.remote_head())
        self.merge('rebase')
        proof = self.publisher.verify_obligations()
        self.assertFalse(proof['unresolved'])
        self.assertEqual(set(proof['terminal']), {first, second, third})
        self.assertEqual(M.git(self.root, 'merge-base', '--is-ancestor', ledger1, self.store.head()).returncode, 0)
        doc = G.P.yaml.safe_load(M.git(self.remote, 'show', 'trunk:GROUNDING.yaml').stdout)
        self.assertEqual(doc['sources']['s.vendor']['read'], '2026-09-14')

    def test_merge_ancestry_does_not_accept_missing_content(self):
        revision = self.capture()
        self.run_ok()
        oldtree = M.git(self.remote, 'rev-parse', 'trunk^{tree}').stdout.decode().strip()
        self.merge('merge', tree=oldtree)
        result = self.run_ok()
        self.assertEqual(result['states'][revision], 'proposed')
        self.assertEqual(self.provider.creates, 2)

    def test_feature_pr_cherry_pick_content_accepts_without_knowledge_pr(self):
        revision = self.capture()
        snap = self.store.snapshot()
        target = self.remote_head('trunk')
        files = self.publisher._files(target)
        commit = self.publisher._commit(target, files, self.publisher._graph(files), snap, [revision])
        M.git(self.root, 'push', 'team', commit + ':refs/heads/trunk')
        result = self.run_ok()
        self.assertEqual(result['states'][revision], 'accepted')
        self.assertEqual(self.provider.creates, 0)

    def test_acceptance_rechecks_source_body_and_evidence(self):
        revision = self.capture()
        self.run_ok()
        self.merge()
        self.assertFalse(self.publisher.verify_obligations()['unresolved'])
        target = self.remote_head('trunk')
        M.git(self.root, 'fetch', 'team', 'trunk')
        files = self.publisher._files(target)
        doc = self.publisher._graph(files)
        doc['sources']['s.vendor']['read'] = '2026-09-15'
        bad = self._replace_target({'GROUNDING.yaml': G.P.yaml.safe_dump(doc).encode()})
        self.assertTrue(self.publisher.verify_obligations()['unresolved'])
        self.assertFalse(self.publisher.status()['verified'])
        self.assertEqual(self.publisher.status()['last_verified']['target'], bad)

    def _replace_target(self, changes):
        target = self.remote_head('trunk')
        M.git(self.root, 'fetch', 'team', 'trunk')
        index = self.base / 'fixture-index'
        env = os.environ.copy()
        env['GIT_INDEX_FILE'] = str(index)
        M.git(self.root, 'read-tree', target, env=env)
        for path, data in changes.items():
            blob = self.store._write_blob(data)
            M.git(self.root, 'update-index', '--add', '--cacheinfo', '100644,' + blob + ',' + path, env=env)
        tree = M.git(self.root, 'write-tree', env=env).stdout.decode().strip()
        commit = M.git(self.root, '-c', 'commit.gpgsign=false', 'commit-tree', tree, '-p', target,
                       data=b'Fixture target update\n').stdout.decode().strip()
        M.git(self.root, 'push', 'team', commit + ':refs/heads/trunk')
        return commit

    def test_evidence_bytes_required_for_acceptance(self):
        revision = self.capture()
        self.run_ok()
        self.merge()
        self._replace_target({'evidence/vendor.txt': b'changed evidence'})
        proof = self.publisher.verify_obligations()
        self.assertEqual(proof['unresolved'], [revision])

    def test_new_arrival_during_merge_remains_captured_then_next_cycle(self):
        first = self.capture()
        self.run_ok()
        self.merge()
        arrival = []
        self.provider.on_list = lambda: arrival.append(self.capture())
        result = self.run_ok()
        self.assertEqual(result['states'][first], 'accepted')
        self.assertEqual(result['states'][arrival[0]], 'captured')
        self.assertNotEqual(result['last_verified']['ledger_ref'], self.store.head())
        result = self.run_ok()
        self.assertEqual(result['states'][arrival[0]], 'proposed')
        self.assertEqual(self.provider.creates, 2)

    def test_closed_and_rejected_revisions_do_not_republish(self):
        first = self.capture()
        self.run_ok()
        self.provider.rows[-1]['state'] = 'closed'
        result = self.run_ok()
        self.assertEqual(result['states'][first], 'closed')
        self.assertEqual(self.provider.creates, 1)
        second = self.capture()
        result = self.run_ok()
        self.assertEqual(self.provider.creates, 2)
        doc = G.P.yaml.safe_load(M.git(self.remote, 'show', 'pending_grounding:GROUNDING.yaml').stdout)
        self.assertNotIn('api.limit1', doc['known'])
        self.assertIn('api.limit2', doc['known'])
        self.provider.rows[-1]['state'] = 'rejected'
        self.assertEqual(self.run_ok()['states'][second], 'rejected')
        self.assertEqual(self.provider.creates, 2)

    def test_uncertain_create_after_side_effect_reconciles_without_duplicate(self):
        revision = self.capture()
        self.provider.uncertain_create = True
        self.assertEqual(self.publisher.run()['outcome'], 'unknown')
        self.assertEqual(self.publisher.status()['intent']['kind'], 'create')
        self.assertEqual(self.run_ok()['states'][revision], 'proposed')
        self.assertEqual(self.provider.creates, 1)

    def test_uncertain_create_without_visible_pr_never_resends(self):
        self.capture()
        self.provider.uncertain_before_create = True
        self.assertEqual(self.publisher.run()['outcome'], 'unknown')
        for _ in range(8):
            self.publisher.run(force_retry=True)
        self.assertEqual(self.provider.creates, 1)
        self.assertEqual(self.publisher.run()['outcome'], 'backoff')

    def test_unknown_push_after_side_effect_is_reconciled(self):
        self.capture()
        original = M.git
        once = [True]
        def uncertain(cwd, *args, **kwargs):
            result = original(cwd, *args, **kwargs)
            if args and args[0] == 'push' and once[0]:
                once[0] = False
                raise subprocess.TimeoutExpired('git fixture after push', 30)
            return result
        with patch.object(M, 'git', uncertain):
            self.assertEqual(self.publisher.run()['outcome'], 'unknown')
        self.assertEqual(self.publisher.status()['intent']['kind'], 'push')
        self.run_ok()
        self.assertEqual(self.provider.creates, 1)

    def test_unexpected_remote_writer_never_overwritten(self):
        self.capture()
        self.run_ok()
        expected = self.remote_head()
        target = self.remote_head('trunk')
        M.git(self.remote, 'update-ref', 'refs/heads/pending_grounding', target, expected)
        self.capture()
        result = self.publisher.run(force_retry=True)
        self.assertEqual(result['outcome'], 'attention')
        self.assertEqual(self.remote_head(), target)
        self.assertEqual(self.provider.creates, 1)

    def test_expected_head_cas_rejects_race_after_reconciliation(self):
        self.capture()
        original = self.publisher._commit
        def race(*args):
            commit = original(*args)
            M.git(self.remote, 'update-ref', 'refs/heads/pending_grounding', self.remote_head('trunk'))
            return commit
        with patch.object(self.publisher, '_commit', race):
            self.assertEqual(self.publisher.run()['outcome'], 'unknown')
        target = self.remote_head('trunk')
        self.assertEqual(self.remote_head(), target)
        self.assertEqual(self.publisher.run(force_retry=True)['outcome'], 'attention')
        self.assertEqual(self.remote_head(), target)
        self.assertEqual(self.provider.creates, 0)

    def test_no_grant_and_scope_change_cannot_publish(self):
        self.project.publication_config('team', 'trunk', grant=False)
        self.capture()
        self.run_ok()
        self.assertIsNone(self.remote_head())
        self.assertEqual(self.provider.creates, 0)
        result = self.publisher.run(authorized=True)
        self.assertEqual(result['outcome'], 'proposed')
        M.git(self.root, 'remote', 'set-url', '--push', 'team', str(self.base / 'elsewhere.git'))
        self.assertEqual(self.publisher.run()['outcome'], 'attention')

    def test_capture_works_while_publisher_lock_is_owned_and_dead_owner_recovers(self):
        with self.publisher.lock():
            revision = self.capture()
            self.assertEqual(C.Publisher(self.root, self.provider).run()['outcome'], 'busy')
        self.assertEqual(self.run_ok()['states'][revision], 'proposed')

    def test_pause_withdraw_supersede_and_explicit_resume(self):
        first = self.capture(name='api.limit', value=10)
        second = self.capture(name='api.limit', value=11)
        self.assertEqual(self.publisher.run()['outcome'], 'attention')
        self.assertIsNone(self.remote_head())
        self.publisher.action('pause')
        self.assertEqual(self.run_ok()['outcome'], 'paused')
        self.publisher.action('supersede', revisions=[first], replacement=second, reason='Reviewed correction')
        self.publisher.action('resume')
        result = self.run_ok()
        self.assertEqual(result['states'][first], 'superseded')
        self.assertEqual(result['states'][second], 'proposed')
        self.publisher.action('withdraw', revisions=[second], reason='Do not publish this reading')
        self.assertEqual(self.publisher.status()['states'][second], 'withdrawn')
        self.publisher.action('resume', revisions=[second])
        self.assertEqual(self.publisher.status()['states'][second], 'captured')

    def test_offline_bounded_backoff_preserves_local_capture(self):
        revision = self.capture()
        self.provider.offline = True
        result = self.publisher.run()
        self.assertEqual(result['outcome'], 'unknown')
        self.assertEqual(self.publisher.run()['outcome'], 'backoff')
        for _ in range(C.MAX_FAILURES):
            self.publisher.run(force_retry=True)
        self.assertEqual(self.publisher.run()['outcome'], 'backoff')
        self.assertEqual(self.store.snapshot()['bundles'][revision]['revision'], revision)
        self.provider.offline = False
        self.publisher.action('retry')
        self.run_ok()

    def test_stale_accepted_receipt_cannot_verify_offline(self):
        self.capture()
        self.run_ok()
        self.merge()
        self.publisher.verify_obligations()
        self.provider.offline = True
        with self.assertRaises(UnknownOutcome):
            self.publisher.verify_obligations()

    def test_hook_returns_without_waiting_and_never_grants_authority(self):
        self.project.publication_config('team', 'trunk', grant=False)
        with patch.object(C.subprocess, 'Popen') as popen:
            self.assertFalse(REAL_TRIGGER(self.project)['started'])
            popen.assert_not_called()
        self.project.publication_config('team', 'trunk', grant=True)
        with patch.object(C.subprocess, 'Popen') as popen:
            popen.return_value.pid = 123
            self.assertTrue(REAL_TRIGGER(self.project)['started'])
            self.assertNotIn('authorized', str(popen.call_args))
            self.assertTrue(popen.call_args.kwargs['start_new_session'])


    def test_actual_rebase_preserves_accepted_closure_and_target_code(self):
        revision = self.capture()
        self.run_ok()
        proposed = self.remote_head()
        advanced = self._replace_target({'app.txt': b'new target code\n'})
        checkout = self.base / 'rebase-fixture'
        M.git(self.root, 'clone', str(self.remote), str(checkout))
        M.git(checkout, 'config', 'user.name', 'Fixture')
        M.git(checkout, 'config', 'user.email', 'fixture@example.test')
        M.git(checkout, 'checkout', '-b', 'fixture-rebased', 'origin/pending_grounding')
        M.git(checkout, '-c', 'commit.gpgsign=false', 'rebase', 'origin/trunk')
        rebased = M.git(checkout, 'rev-parse', 'HEAD').stdout.decode().strip()
        self.assertNotEqual(rebased, proposed)
        self.assertEqual(M.git(checkout, 'rev-parse', 'HEAD^').stdout.decode().strip(), advanced)
        M.git(checkout, 'push', 'origin', 'HEAD:trunk')
        self.provider.rows[-1]['state'] = 'merged'
        M.git(self.remote, 'update-ref', '-d', 'refs/heads/pending_grounding', proposed)
        self.assertEqual(self.run_ok()['states'][revision], 'accepted')
        self.assertEqual(M.git(self.remote, 'show', 'trunk:app.txt').stdout, b'new target code\n')

    def test_nested_custom_record_and_relative_evidence(self):
        # Configure explicitly before capture; the remote target also carries it.
        cfg = self.project.config()
        cfg['record'] = 'knowledge/facts.yaml'
        M.I._save(self.project.config_path, cfg)
        self._replace_target({'knowledge/facts.yaml': b'{}\n'})
        revision = self.capture()
        self.run_ok()
        self.assertEqual(M.git(self.remote, 'show', 'pending_grounding:knowledge/evidence/vendor.txt').stdout,
                         b'The limit is 10.\n')
        self.merge()
        self.assertEqual(self.publisher.verify_obligations()['terminal'][revision], 'accepted')

    def test_inherited_shared_subset_with_local_extras_accepts_through_feature(self):
        revision = self.capture()
        self.run_ok()
        head = self.remote_head()
        doc = G.P.yaml.safe_load(M.git(self.remote, 'show', head + ':GROUNDING.yaml').stdout)
        doc['known']['feature.extra'] = {'v': 'local feature observation'}
        self._replace_target({'GROUNDING.yaml': G.P.yaml.safe_dump(doc).encode(),
                              'evidence/vendor.txt': b'The limit is 10.\n'})
        result = self.run_ok()
        self.assertEqual(result['states'][revision], 'accepted')
        self.assertEqual(self.provider.creates, 1)

    def test_offline_explicit_terminal_proof_and_guard_retains_decision_lock(self):
        revision = self.capture()
        self.publisher.action('withdraw', revisions=[revision], reason='Explicitly withdrawn')
        self.provider.offline = True
        with self.publisher.transition_guard() as proof:
            self.assertEqual(proof['terminal'], {revision: 'withdrawn'})
            self.assertFalse(proof['unresolved'])
            self.assertIsNone(proof['target'])
            with self.assertRaises(C.Busy):
                C.Publisher(self.project, self.provider).action('resume', revisions=[revision])
            # Capture remains independent; caller must reject this stale proof.
            self.capture()
            self.assertNotEqual(proof['ledger_ref'], self.store.head())

    def test_simple_mode_disables_run_and_hook_without_erasing_ledger(self):
        revision = self.capture()
        head = self.store.head()
        cfg = self.project.config()
        cfg['mode'] = 'simple'
        M.I._save(self.project.config_path, cfg)
        self.assertEqual(self.publisher.run()['outcome'], 'attention')
        with patch.object(C.subprocess, 'Popen') as popen:
            self.assertFalse(REAL_TRIGGER(self.project)['started'])
            popen.assert_not_called()
        self.assertEqual(self.store.head(), head)
        self.assertIn(revision, self.store.snapshot()['bundles'])

    def test_split_target_fails_explicitly_without_partial_acceptance(self):
        self.capture()
        self._replace_target({'GROUNDING.yaml': b'also: other.yaml\n', 'other.yaml': b'{}\n'})
        result = self.publisher.run()
        self.assertEqual(result['outcome'], 'attention')
        self.assertIn('split', result['detail'])
        self.assertIsNone(self.remote_head())

    def test_url_is_rechecked_immediately_before_push(self):
        self.capture()
        original = self.publisher._commit
        def redirect(*args):
            commit = original(*args)
            M.git(self.root, 'remote', 'set-url', '--push', 'team', str(self.base / 'wrong.git'))
            return commit
        with patch.object(self.publisher, '_commit', redirect):
            self.assertEqual(self.publisher.run()['outcome'], 'attention')
        self.assertIsNone(self.remote_head())

    def test_dead_process_publisher_lock_recovers(self):
        # A separate process takes the real flock and exits without unlocking.
        script = 'from scripts.pending_publication import Publisher; import os; p=Publisher(' + repr(str(self.root)) + '); lock=p.lock(); lock.__enter__(); os._exit(0)'
        result = subprocess.run([sys.executable, '-c', script], cwd=str(Path(__file__).resolve().parents[1]), timeout=10)
        self.assertEqual(result.returncode, 0)
        revision = self.capture()
        self.assertEqual(self.run_ok()['states'][revision], 'proposed')



    def test_withdrawn_revision_is_removed_from_active_proposal(self):
        first = self.capture()
        second = self.capture()
        self.run_ok()
        self.publisher.action('withdraw', revisions=[first], reason='Withdraw this proposal')
        self.run_ok()
        doc = G.P.yaml.safe_load(M.git(self.remote, 'show', 'pending_grounding:GROUNDING.yaml').stdout)
        self.assertNotIn('api.limit1', doc['known'])
        self.assertIn('api.limit2', doc['known'])
        self.publisher.action('withdraw', revisions=[second], reason='Withdraw remaining proposal')
        self.run_ok()
        doc = G.P.yaml.safe_load(M.git(self.remote, 'show', 'pending_grounding:GROUNDING.yaml').stdout)
        self.assertFalse(doc.get('known'))
        self.assertEqual(self.provider.creates, 1)

    def test_unknown_create_cannot_be_abandoned_by_local_withdrawal(self):
        revision = self.capture()
        self.provider.uncertain_before_create = True
        self.assertEqual(self.publisher.run()['outcome'], 'unknown')
        self.publisher.action('withdraw', revisions=[revision], reason='Withdraw the reading')
        with self.assertRaises(UnknownOutcome):
            self.publisher.verify_obligations()
        self.assertEqual(self.provider.creates, 1)


if __name__ == '__main__':
    unittest.main()
