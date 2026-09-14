"""Public commands, opening and transitions use the same durable boundaries."""
import json
import copy
import contextlib
import io
import multiprocessing
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

from scripts import pending_grounding as G, project_modes as M
from tests import test_pending_publication as F

ROOT = Path(__file__).resolve().parents[1]


def configure_same(root, results):
    try:
        value = M.Project(root).configure()
        results.put(value['mode'])
    except Exception as error:
        results.put(str(error))


class IntegrationTests(unittest.TestCase):
    def setUp(self):
        F.PublicationTests.setUp(self)

    capture = F.PublicationTests.capture
    run_ok = F.PublicationTests.run_ok
    remote_head = F.PublicationTests.remote_head
    merge = F.PublicationTests.merge

    def cli(self, *args):
        return subprocess.run([sys.executable, str(ROOT / 'scripts/cli.py'),
                               '--workspace', str(self.root), *args],
                              capture_output=True, text=True, timeout=15,
                              env={**os.environ, 'KPOPPER_SESSION_DISABLE': '1',
                                   'XDG_STATE_HOME': str(self.base / 'private-state')})

    def test_capture_notifies_only_after_receipt_and_policy_lock_release(self):
        result_queue = multiprocessing.Queue()

        def callback(project):
            self.assertIsNotNone(G.Store(project).head())
            process = multiprocessing.Process(target=configure_same, args=(str(self.root), result_queue))
            process.start()
            process.join(4)
            alive = process.is_alive()
            if alive:
                process.terminate()
                process.join()
            self.assertFalse(alive, 'publisher was triggered under the policy lock')
            self.assertEqual(result_queue.get(timeout=2), 'advanced')
            return {'started': False, 'reason': 'fixture inspected lock boundary'}

        with patch('scripts.pending_publication.trigger_after_capture', side_effect=callback) as trigger:
            receipt = self.store.capture(F.fixture_bundle(), event_id='notify',
                                         contribution_id='limit', shareability='project')
        self.assertEqual(trigger.call_count, 1)
        self.assertEqual(receipt['commit'], self.store.head())
        self.assertIn('fixture inspected', receipt['publication_attempt']['reason'])

    def test_dispatch_failure_never_reverses_durable_capture_success(self):
        with patch('scripts.pending_publication.trigger_after_capture', side_effect=TypeError('fixture failure')):
            receipt = self.store.capture(F.fixture_bundle(), event_id='failed-dispatch',
                                         contribution_id='limit', shareability='project')
        self.assertEqual(receipt['state'], 'captured')
        self.assertEqual(receipt['commit'], self.store.head())
        self.assertEqual(receipt['publication_attempt'], {'started': False, 'reason': 'fixture failure'})

    def test_private_source_markers_are_refused_before_low_level_object_writes(self):
        for marker in ({'visibility': 'private'}, {'privacy': 'unknown'}, {'private': 'yes'}):
            bundle = F.fixture_bundle()
            bundle['manifest']['document']['sources']['s.vendor'].update(marker)
            bundle['revision'] = G.identity(bundle['manifest'])
            with patch.object(self.store, '_write_blob') as write:
                with self.assertRaisesRegex(ValueError, 'private|sharing'):
                    self.store.capture(bundle, event_id='private', contribution_id='limit', shareability='project')
                write.assert_not_called()

    def test_publication_commands_preserve_explicit_authority(self):
        result = self.cli('pending', 'configure', '--revoke', '--json')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(json.loads(result.stdout)['publication']['standing_permission'])
        revision = self.capture()
        result = self.cli('pending', 'publish', '--json')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIsNone(self.remote_head())
        status = json.loads(self.cli('pending', 'status', '--json').stdout)
        self.assertIn(revision, status['states'])
        self.assertFalse(status['verified'])
        self.assertFalse(status['authority']['standing_permission'])

    def test_changed_remote_url_does_not_inherit_old_publication_grant(self):
        other = self.base / 'another-destination.git'
        M.git(self.root, 'init', '--bare', str(other))
        M.git(self.root, 'remote', 'set-url', 'team', str(other))
        result = self.cli('pending', 'configure', '--json')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        scope = json.loads(result.stdout)['publication']
        self.assertEqual(scope['repository'], str(other))
        self.assertFalse(scope['standing_permission'])

    def test_pending_open_retries_but_frozen_open_does_not(self):
        from scripts import session_start as S
        location = {'record': str(self.root / 'GROUNDING.yaml'), 'workspace': str(self.root)}
        result = subprocess.CompletedProcess([], 0, 'read', '')
        with patch('scripts.pending_publication.retry_session') as retry, patch.object(S, '_run', return_value=result):
            with patch.dict(os.environ, {'KPOPPER_READ_MODE': 'live'}):
                S.read_view(location, reader_args=[])
            retry.assert_called_once_with(str(self.root))
            with patch.dict(os.environ, {'KPOPPER_READ_MODE': 'frozen'}):
                S.read_view(location, reader_args=[])
            self.assertEqual(retry.call_count, 1)

    def test_accepted_ledger_can_transition_without_discarding_history(self):
        revision = self.capture()
        self.run_ok()
        self.merge()
        # Both records represent the same current world before mode changes.
        M.git(self.root, 'fetch', 'team', 'trunk')
        M.git(self.root, 'merge', '--ff-only', 'FETCH_HEAD')
        shared = self.base / 'shared' / 'GROUNDING.yaml'
        shared.parent.mkdir()
        shared.write_bytes((self.root / 'GROUNDING.yaml').read_bytes())
        head = self.store.head()
        with patch('scripts.pending_publication.Publisher._provider', return_value=self.provider):
            config = self.project.configure('simple', record=str(shared))
        self.assertEqual(config['mode'], 'simple')
        self.assertEqual(self.store.head(), head)
        self.assertIn(revision, self.store.snapshot()['bundles'])
        self.assertTrue(list((self.project.state / 'mode-history').glob('*.json')))

    def test_capture_during_verified_transition_forces_refusal(self):
        first = self.capture()
        self.publisher.action('withdraw', revisions=[first], reason='fixture explicit decision')
        shared = self.base / 'shared' / 'GROUNDING.yaml'
        shared.parent.mkdir()
        shared.write_bytes((self.root / 'GROUNDING.yaml').read_bytes())
        original = self.publisher.__class__._verify_obligations

        def verify_then_arrive(publisher):
            proof = original(publisher)
            self.capture('new.arrival')
            return proof

        with patch('scripts.pending_publication.Publisher._provider', return_value=self.provider), \
                patch('scripts.pending_publication.Publisher._verify_obligations', verify_then_arrive):
            with self.assertRaisesRegex(ValueError, 'changed|contribution|reconcile'):
                self.project.configure('simple', record=str(shared))
        self.assertEqual(self.project.config()['mode'], 'advanced')
        self.assertEqual(len(self.store.snapshot()['bundles']), 2)

    def test_source_only_schema_closure_keeps_its_role_in_live_view(self):
        doc = F.fixture_bundle()['manifest']['document']
        doc['schema'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        doc['sources']['s.vendor']['labels'] = ['shareable']
        bundle = G.prepare(doc, ['api.limit'], scope={'kind': 'external', 'environment': 'API v2'},
                           shareability='project', evidence={'evidence/vendor.txt': b'The limit is 10.\n'})
        self.store.capture(bundle, event_id='explicit-schema', contribution_id='limit', shareability='project')
        graph = copy.deepcopy(doc)
        graph['judgments'] = {'plan.ready': {'rests_on': ['api.limit'], 'seen': {'api.limit': 10},
                                           'verdict': 'ready', 'wrong_if': 'api.limit < 1'}}
        (self.root / 'GROUNDING.yaml').write_text(G.P.yaml.safe_dump(graph))
        live = G.P.load([str(self.root / 'GROUNDING.yaml')])
        self.assertFalse(live.knowledge_conflicts)

    def test_committed_target_pointers_and_target_only_hypotheses_are_compared(self):
        bundle = F.fixture_bundle()
        self.store.capture(bundle, event_id='target-hypothesis', contribution_id='limit', shareability='project')
        (self.root / 'facts.yaml').write_text(G.P.yaml.safe_dump(bundle['manifest']['document']))
        (self.root / 'GROUNDING.yaml').write_text('record: facts.yaml\n')
        hypothesis = self.root / '.kpopper/hypotheses/alternative.yaml'
        hypothesis.parent.mkdir(parents=True)
        hypothesis.write_text('hypothesis: {claim: "Different limit"}\nknown:\n  api.limit: {v: 30, from: s.vendor}\n')
        M.git(self.root, 'add', '.')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Target split record with proposal')
        M.git(self.root, 'push', 'team', 'trunk')
        hypothesis.unlink()
        (self.root / 'GROUNDING.yaml').write_text(G.P.yaml.safe_dump(bundle['manifest']['document']))
        live = G.P.load([str(self.root / 'GROUNDING.yaml')])
        self.assertIn('api.limit', live.knowledge_conflicts)
        self.assertTrue(any('target:' in label and ':hypothesis:alternative' in label
                            for label, _ in live.knowledge_conflicts['api.limit']))

    def test_private_drafts_are_discoverable_but_frozen_status_excludes_them(self):
        with patch.dict(os.environ, {'KPOPPER_PRIVATE_HOME': str(self.base / 'private-drafts')}):
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                code = G.P.apply([str(self.root / 'GROUNDING.yaml')],
                                 {'kind': 'add', 'id': 'private.limit', 'body': {'v': 71, 'from': 'private note'},
                                  'shareability': 'private', 'event_id': 'retained-private'})
            self.assertEqual(code, 0)
            receipt = json.loads(output.getvalue())
            self.assertTrue(Path(receipt['path']).is_file())
            live = json.loads(self.cli('knowledge', 'status').stdout)
            frozen = json.loads(self.cli('--frozen', 'knowledge', 'status').stdout)
        self.assertEqual(live['private_drafts'][0]['event_id'], 'retained-private')
        self.assertEqual(frozen['private_drafts'], [])
        self.assertIsNone(self.store.head())
