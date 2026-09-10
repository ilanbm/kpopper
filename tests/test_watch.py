"""Branch compatibility must evaluate authored changes without writing either graph."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import yaml
from scripts import watch as W


@unittest.skipIf(os.name == 'nt', 'Watch background state requires POSIX locking')
class WatchFixture:
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.main = self.root / 'main'
        self.main.mkdir()
        self.env = patch.dict(os.environ, {'XDG_STATE_HOME': str(self.root / 'state')})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.git(self.main, 'init', '-b', 'main')
        self.git(self.main, 'config', 'user.email', 'fixture@example.test')
        self.git(self.main, 'config', 'user.name', 'Fixture')
        self.doc = {'known': {'api.timeout': {'v': 5, 'of': '2026-09-01'}, 'other.value': {'v': 1}},
                    'judgments': {'c.positive': {'rests_on': ['api.timeout'], 'seen': {'api.timeout': 5},
                    'verdict': 'Positive', 'wrong_if': 'api.timeout < 0'}}}
        self.save(self.main, self.doc)
        self.commit(self.main)
        self.work = self.root / 'work'
        self.git(self.main, 'worktree', 'add', '-b', 'experiment', str(self.work))
        main = copy.deepcopy(self.doc)
        main['known']['checkout.budget'] = {'v': 10}
        main['known']['other.value']['v'] = 2
        main['judgments']['c.checkout'] = {'rests_on': ['api.timeout', 'checkout.budget'],
            'seen': {'api.timeout': 5, 'checkout.budget': 10}, 'verdict': 'Fits',
            'wrong_if': 'api.timeout > checkout.budget'}
        self.save(self.main, main)
        self.commit(self.main)
        self.watch = W.Watch(self.work)
        self.watch.setup(base_ref='main')

    def git(self, cwd, *args):
        r = subprocess.run(['git', *args], cwd=cwd, text=True, capture_output=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        return r.stdout.strip()

    def commit(self, cwd):
        self.git(cwd, 'add', '.')
        self.git(cwd, 'commit', '-m', 'Fixture')

    def save(self, cwd, doc):
        (cwd / 'PROVENANCE.yaml').write_text(yaml.safe_dump(doc, sort_keys=False))

    def changed(self, value=30):
        doc = copy.deepcopy(self.doc)
        doc['known']['api.timeout'] = {'v': value, 'of': '2026-09-10'}
        self.save(self.work, doc)


class WatchTests(WatchFixture, unittest.TestCase):
    def test_uncommitted_delta_on_new_main_is_read_only(self):
        self.changed()
        before = [(p / 'PROVENANCE.yaml').read_bytes() for p in (self.main, self.work)]
        snap = self.watch.snapshot()
        result = W.compare(snap)
        self.assertTrue(any('c.checkout' in f['reason'] for f in result['findings']))
        self.assertNotIn('other.value', result['changed'])
        self.assertEqual(before, [(p / 'PROVENANCE.yaml').read_bytes() for p in (self.main, self.work)])

    def test_queue_is_async_and_suppresses_stale_results(self):
        self.changed()
        with patch.object(W, 'launch') as launch:
            self.watch.request()
            launch.assert_called_once()
        self.watch.process()
        self.assertEqual(self.watch.status()['state'], 'attention')
        self.changed(6)
        self.assertEqual(self.watch.status()['state'], 'pending')
        self.watch.process()
        self.assertEqual(self.watch.status()['state'], 'clear')

    def test_concurrent_same_id_changes_are_reported(self):
        self.doc['known']['other.value']['v'] = 3
        self.save(self.work, self.doc)
        result = W.compare(self.watch.snapshot())
        self.assertTrue(any(f['kind'] == 'collision' and f['id'] == 'other.value' for f in result['findings']))

    def test_local_deletion_checks_main_dependents(self):
        del self.doc['known']['api.timeout']
        self.save(self.work, self.doc)
        result = W.compare(self.watch.snapshot())
        self.assertTrue(any('api.timeout' in f['reason'] for f in result['findings']))

    def test_notifications_are_per_session_and_new_findings_only(self):
        self.changed()
        self.watch.process()
        self.assertTrue(self.watch.offer('one'))
        self.assertFalse(self.watch.offer('one'))
        self.assertTrue(self.watch.offer('two'))
        self.git(self.main, 'commit', '--allow-empty', '-m', 'Unrelated main advance')
        self.watch.process()
        self.assertFalse(self.watch.offer('one'))

    def test_unknown_base_is_not_clear(self):
        config = self.watch.config()
        config['base_ref'] = 'refs/heads/missing'
        W.I._save(self.watch.config_path, config)
        self.watch.process()
        self.assertEqual(self.watch.status()['state'], 'unavailable')

    def test_cli_and_real_worker(self):
        self.changed()
        r = subprocess.run([sys.executable, str(W.Path(W.__file__).with_name('cli.py')),
            '--workspace', str(self.work), 'watch', 'scan'], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        import time
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline and self.watch.status()['state'] == 'pending':
            time.sleep(.05)
        self.assertEqual(self.watch.status()['state'], 'attention')

    def test_pointer_closure_and_changed_hypothesis_are_checked(self):
        (self.work / 'PROVENANCE.d').mkdir()
        (self.work / 'PROVENANCE.d' / 'timeout.yaml').write_text('hypothesis:\n  claim: Try a larger timeout\n  folds: never\nknown:\n  api.timeout: {v: 30, of: 2026-09-10}\n')
        result = W.compare(self.watch.snapshot())
        self.assertTrue(any('c.checkout' in f['reason'] for f in result['findings']))

    def test_escaping_pointer_is_unavailable_and_never_read(self):
        outside = self.root / 'private.yaml'
        outside.write_text('known:\n  private.secret: {v: sensitive}\n')
        (self.work / 'PROVENANCE.yaml').write_text('record: ../private.yaml\n')
        self.watch.process()
        result = self.watch.status()
        self.assertEqual(result['state'], 'unavailable')
        self.assertNotIn('sensitive', str(result))

    def test_stale_processing_result_cannot_be_delivered(self):
        self.changed()
        original = W.compare
        def race(snapshot):
            self.changed(6)
            return original(snapshot)
        with patch.object(W, 'compare', side_effect=race):
            self.watch.process()
        self.assertEqual(self.watch.status()['state'], 'clear')
        self.assertIsNone(self.watch.offer('session'))

    def test_native_delivery_failure_and_unknown_acceptance(self):
        from scripts import watch_delivery as D
        self.changed(); self.watch.process()
        with patch.dict(os.environ, {'CODEX_SESSION_ID': 'one'}):
            job = D.reserve(self.watch, 'one')
            self.assertFalse(D.reserve(self.watch, 'one')['dispatch_required'])
            with self.assertRaises(ValueError):
                D.reserve(self.watch, 'other')
        self.assertTrue(D.hidden(self.watch, 'one'))
        answer = D.wait(self.watch, job['job_id'], 0)
        self.assertEqual(answer['state'], 'attention')
        self.assertEqual(answer['recipient'], 'one')
        with self.assertRaises(ValueError):
            D.complete(self.watch, job['job_id'], 'wrong', 'sent')
        D.complete(self.watch, job['job_id'], answer['claim_token'], 'unknown')
        self.assertIsNone(self.watch.offer('codex:one'))
        D.resume(self.watch, 'one')
        self.assertIsNotNone(self.watch.offer('codex:one'))

    def test_scan_all_uses_only_registered_matching_worktrees(self):
        with patch.object(W, 'launch'):
            W.Watch(self.main).request()
            result = self.watch.request_all()
        self.assertEqual({r['workspace'] for r in result['worktrees']}, {str(self.main), str(self.work)})

    def test_delivery_batch_does_not_acknowledge_unseen_findings(self):
        findings = [{'kind': 'falsified', 'id': 'c.' + str(i), 'reason': 'Synthetic', 'fingerprint': str(i)} for i in range(19)]
        result = {'state': 'attention', 'findings': findings, 'episode': 'one'}
        with patch.object(self.watch, 'status', return_value=result):
            first = self.watch.offer('one')
            second = self.watch.offer('one')
            third = self.watch.offer('one')
            self.assertEqual([len(x['findings']) for x in (first, second, third)], [8, 8, 3])
            self.assertIsNone(self.watch.offer('one'))
        reduced = {**result, 'findings': findings[1:]}
        with patch.object(self.watch, 'status', return_value=reduced):
            self.assertIsNone(self.watch.offer('one'))
        with patch.object(self.watch, 'status', return_value=result):
            self.assertEqual(self.watch.offer('one')['findings'][0]['fingerprint'], '0')

    def test_missing_configured_record_is_reported_by_hook(self):
        from scripts import watch_hook as H
        (self.work / 'PROVENANCE.yaml').unlink()
        self.watch.process()
        with patch.object(W, 'launch'):
            out, _, _ = H.handle({'cwd': str(self.work), 'session_id': 'one'}, 'codex', 'wait', 0)
        self.assertIn('unavailable', out)

    def test_new_declared_uncheckable_condition_is_not_clear(self):
        self.doc['judgments']['c.pending'] = {'rests_on': ['api.timeout'], 'seen': {'api.timeout': 5},
            'verdict': 'Awaiting source', 'wrong_if': 'api.timeout meets the external condition',
            'blocked_on': 'The provider has not supplied the rule'}
        self.save(self.work, self.doc)
        result = W.compare(self.watch.snapshot())
        self.assertTrue(any(f['kind'] == 'uncheckable' and f['id'] == 'c.pending' for f in result['findings']))

    def test_multifile_pointer_graph_keeps_authored_delta_and_inputs_intact(self):
        for tree in (self.main,self.work):
            record=tree/'PROVENANCE.yaml'
            data=tree/'records';data.mkdir()
            (data/'facts.yaml').write_bytes(record.read_bytes())
            record.write_text('record: records/facts.yaml\n')
        self.commit(self.main)
        local=W.P.yaml.safe_load((self.work/'records/facts.yaml').read_text())
        local['known']['api.timeout']={'v':30,'of':'2026-09-10'}
        (self.work/'records/facts.yaml').write_text(W.P.yaml.safe_dump(local))
        before=[p.read_bytes() for tree in (self.main,self.work) for p in (tree/'PROVENANCE.yaml',tree/'records/facts.yaml')]
        result=W.compare(self.watch.snapshot())
        self.assertTrue(any('c.checkout' in f['reason'] for f in result['findings']))
        self.assertEqual(before,[p.read_bytes() for tree in (self.main,self.work) for p in (tree/'PROVENANCE.yaml',tree/'records/facts.yaml')])

    def test_main_delete_local_modify_is_a_collision(self):
        main=W.P.yaml.safe_load((self.main/'PROVENANCE.yaml').read_text())
        del main['known']['api.timeout'];main.pop('judgments')
        self.save(self.main,main);self.commit(self.main);self.changed()
        result=W.compare(self.watch.snapshot())
        self.assertTrue(any(f['kind']=='collision' and f['id']=='api.timeout' for f in result['findings']))

    def test_unchanged_hypothesis_condition_is_rechecked_on_main(self):
        # A standing hypothesis committed before the branch fork.
        self.git(self.work,'reset','--hard','main')
        folder=self.main/'PROVENANCE.d';folder.mkdir()
        (folder/'standing.yaml').write_text('hypothesis:\n  claim: Timeout remains bounded\n  wrong_if: api.timeout > 20\n  folds: never\n')
        self.commit(self.main)
        self.git(self.work,'reset','--hard','main')
        main=W.P.yaml.safe_load((self.main/'PROVENANCE.yaml').read_text())
        main['known']['api.timeout']={'v':30,'of':'2026-09-10'}
        self.save(self.main,main);self.commit(self.main)
        result=W.compare(self.watch.snapshot())
        self.assertTrue(any(f['id']=='standing' and f['kind']=='falsified' for f in result['findings']))

    def test_waiter_mutex_and_polling_do_not_recompute_snapshots(self):
        from scripts import watch_hook as H
        session='one';lock=self.watch.state/'waiters'/(W.digest(['codex',session])+'.lock')
        with W.try_lock(lock),patch.object(W,'launch'),patch.object(W.Watch,'poll_result',side_effect=AssertionError('another waiter owns polling')):
            self.assertEqual(H.handle({'cwd':str(self.work),'session_id':session},'codex','wait',0),('', '',0))
        with patch.object(W.Watch,'status',side_effect=AssertionError('no result changed; no snapshot')):
            for _ in range(10):self.assertEqual(self.watch.poll_result(),(None,None))
        self.watch.process()
        with patch.object(self.watch,'snapshot',wraps=self.watch.snapshot) as snapshot:
            signature,result=self.watch.poll_result()
            self.assertEqual(result['state'],'clear')
            for _ in range(10):self.assertEqual(self.watch.poll_result(signature),(signature,None))
            self.assertEqual(snapshot.call_count,1)

    def test_missing_optional_pointer_remains_explicitly_unavailable(self):
        self.doc['also']='missing.yaml';self.save(self.work,self.doc)
        self.watch.process()
        self.assertEqual(self.watch.status()['state'],'unavailable')

    def test_setup_without_locking_does_not_change_configuration(self):
        before=self.watch.config_path.read_bytes()
        with patch.dict(sys.modules,{'fcntl':None}):
            with self.assertRaisesRegex(ValueError,'POSIX'):
                self.watch.setup(shared_private=True)
        self.assertEqual(self.watch.config_path.read_bytes(),before)
        self.assertFalse((self.watch.project_state/'shared/PROVENANCE.yaml').exists())
