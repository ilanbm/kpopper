"""Focused first-generation authoring and publication boundaries."""
import copy
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts import history_adapter as D, history_bootstrap as B
from scripts import history_contract as C, history_store as H, history_transaction as T


ROOT = Path(__file__).resolve().parents[1]


class HistoryBootstrap(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.entry = self.root / 'GROUNDING.yaml'
        self.policy = {'version': 1, 'mode': 'simple', 'generation': 0}

    def prepare(self, action=None):
        return B.prepare(self.entry, action or {
            'kind': 'add', 'id': 'p.value', 'body': {'v': 1},
            'as_of': '2026-09-01', 'hypothesis': None, 'into': None,
            'why': None, 'source': None, 'at': None,
        }, policy=self.policy, operation='bootstrap-test',
            recorded_at='2026-09-19T10:11:12+00:00', record_id='record-test')

    def publish(self, mutation):
        return B.publish(self.entry, mutation, policy=self.policy, verify=lambda data: None)

    def test_first_claim_is_direct_authored_history_not_legacy_import(self):
        mutation = self.prepare()
        self.publish(mutation)
        captured = H.Store(self.entry).capture()
        self.assertEqual(captured.state['subjects']['p.value']['body'], {'v': 1})
        claim = captured.objects[captured.state['subjects']['p.value']['head']]
        self.assertEqual(claim['on'], '2026-09-19T10:11:12+00:00')
        self.assertEqual(captured.document['meta']['updated'], '2026-09-01')
        self.assertEqual(mutation.to_data()['receipt']['before']['authoring']['action']['as_of'],
                         '2026-09-01')
        self.assertNotIn('locator', claim['authored'])
        self.assertNotIn('history_import', captured.document['meta'])
        self.assertNotIn('originals_storage', str(mutation.to_data()))

    def test_first_named_hypothesis_is_proposed_not_accepted(self):
        action = {'kind': 'add', 'id': 'p.option', 'body': {'v': 2},
                  'as_of': '2026-09-01', 'hypothesis': 'candidate', 'into': None,
                  'why': 'explore it', 'source': None, 'at': None}
        self.publish(self.prepare(action))
        captured = H.Store(self.entry).capture()
        subject = captured.state['subjects']['p.option']
        self.assertEqual(subject['acceptance'], 'proposed')
        self.assertNotIn('p.option', captured.document.get('known', {}))
        adapted = D.from_store_capture(captured)
        layers, _ = __import__('scripts.history_hypotheses', fromlist=['layers']).layers(
            adapted.projection, adapted.document)
        self.assertEqual(layers['candidate']['doc']['known']['p.option'], {'v': 2})

    def test_prepared_intent_is_rederived(self):
        mutation = self.prepare()
        mutation._data['baseline']['bootstrap']['action']['body']['v'] = 9
        with self.assertRaisesRegex(C.HistoryError, 'history_bootstrap_intent_mismatch'):
            B.verify_prepared(self.entry, mutation, policy=self.policy)

    def test_interrupted_marker_switch_recovers_exact_generation(self):
        mutation = self.prepare()
        replace = T._replace
        fired = [False]
        def fail_marker(path, data):
            if Path(path).name == 'history.yaml' and not fired[0]:
                fired[0] = True
                raise OSError('marker fault')
            return replace(path, data)
        with mock.patch.object(T, '_replace', side_effect=fail_marker):
            with self.assertRaisesRegex(OSError, 'marker fault'):
                self.publish(mutation)
        self.assertTrue((self.entry.parent / T.journal_for(self.entry)).is_file())
        B.recover(self.entry, policy=self.policy, verify=lambda data: None)
        captured = H.Store(self.entry).capture()
        self.assertEqual(set(captured.commits), {'bootstrap-test'})
        self.assertEqual(captured.state['subjects']['p.value']['body'], {'v': 1})

    def test_interrupted_birth_can_roll_back_without_claim_residue(self):
        mutation = self.prepare()
        replace = T._replace
        fired = [False]
        def fail_marker(path, data):
            if Path(path).name == 'history.yaml' and not fired[0]:
                fired[0] = True
                raise OSError('marker fault')
            return replace(path, data)
        with mock.patch.object(T, '_replace', side_effect=fail_marker):
            with self.assertRaisesRegex(OSError, 'marker fault'):
                self.publish(mutation)
        B.recover(self.entry, policy=self.policy, verify=lambda data: None, direction='before')
        self.assertFalse(self.entry.exists())
        self.assertFalse(Path(H.Store(self.entry).layout['history_authority']).exists())
        self.assertFalse(any(Path(H.Store(self.entry).layout['history_commits']).glob('*.yaml')))
        self.assertFalse(any(Path(H.Store(self.entry).layout['history']).rglob('*.yaml')))
        retry = B.prepare(self.entry, {'kind': 'add', 'id': 'p.retry', 'body': {'v': 2},
            'as_of': '2026-09-01'}, policy=self.policy, operation='bootstrap-retry',
            recorded_at='2026-09-19T10:12:00+00:00', record_id='record-retry')
        self.publish(retry)
        self.assertEqual(H.Store(self.entry).capture().state['subjects']['p.retry']['body'], {'v': 2})

    def test_rollback_cleanup_is_reentrant_while_journal_remains(self):
        mutation = self.prepare()
        replace = T._replace
        with mock.patch.object(T, '_replace', side_effect=lambda path, data:
                (_ for _ in ()).throw(OSError('marker fault'))
                if Path(path).name == 'history.yaml' else replace(path, data)):
            with self.assertRaisesRegex(OSError, 'marker fault'):
                self.publish(mutation)
        journal = self.entry.parent / T.journal_for(self.entry)
        unlink = Path.unlink
        removed = [0]
        def fail_mid_cleanup(path, *args, **kwargs):
            if '/.kpopper/history/' in str(path):
                removed[0] += 1
                if removed[0] == 2:
                    raise OSError('cleanup fault')
            return unlink(path, *args, **kwargs)
        with mock.patch.object(Path, 'unlink', fail_mid_cleanup):
            with self.assertRaisesRegex(OSError, 'cleanup fault'):
                B.recover(self.entry, policy=self.policy, verify=lambda data: None, direction='before')
        self.assertTrue(journal.is_file())
        B.recover(self.entry, policy=self.policy, verify=lambda data: None, direction='before')
        self.assertFalse(journal.exists())
        self.assertFalse(self.entry.exists())

    def test_existing_companion_evidence_refuses_without_modification(self):
        cases = [self.root / '.kpopper' / 'replaced.yaml',
                 self.root / '.kpopper' / 'history-cancellations' / 'old.yaml',
                 self.root / 'PROVENANCE.history.yaml',
                 self.root / 'PROVENANCE.history-commits' / 'old.yaml',
                 self.root / '.kpopper-history-migration' / 'receipt.json']
        for path in cases:
            with self.subTest(path=path):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b'retained evidence')
                with self.assertRaisesRegex(C.HistoryError, 'existing_.*evidence'):
                    self.prepare()
                self.assertEqual(path.read_bytes(), b'retained evidence')
                path.unlink()

    def test_bracketed_workspace_and_symlink_alias_do_not_bypass_guards(self):
        bracketed = self.root / '[workspace]'
        (bracketed / '.kpopper' / 'hypotheses').mkdir(parents=True)
        hypothesis = bracketed / '.kpopper' / 'hypotheses' / 'existing.yaml'
        hypothesis.write_bytes(b'hypothesis: {born: 2026-09-01}\n')
        with self.assertRaisesRegex(C.HistoryError, 'existing_hypothesis_requires_record'):
            B.prepare(bracketed / 'GROUNDING.yaml', {
                'kind': 'add', 'id': 'p.value', 'body': {'v': 1}, 'as_of': '2026-09-01'},
                policy=self.policy)

        real = self.root / 'real'
        real.mkdir()
        alias = self.root / 'alias'
        alias.symlink_to(real, target_is_directory=True)
        entry = alias / 'GROUNDING.yaml'
        mutation = B.prepare(entry, {'kind': 'add', 'id': 'p.value', 'body': {'v': 1},
            'as_of': '2026-09-01'}, policy=self.policy, operation='alias-bootstrap',
            recorded_at='2026-09-19T10:12:00+00:00', record_id='record-alias')
        B.publish(entry, mutation, policy=self.policy, verify=lambda data: None)
        self.assertEqual(H.Store(real / 'GROUNDING.yaml').capture().state['subjects']['p.value']['body'],
                         {'v': 1})

    def test_history_directory_symlink_is_refused_without_following_it(self):
        outside = self.root / 'outside'
        outside.mkdir()
        evidence = outside / 'evidence.yaml'
        evidence.write_bytes(b'preserve me')
        home = self.root / '.kpopper'
        home.mkdir()
        (home / 'history').symlink_to(outside, target_is_directory=True)
        with self.assertRaisesRegex(C.HistoryError, 'invalid_history_path'):
            self.prepare()
        self.assertEqual(evidence.read_bytes(), b'preserve me')

    def test_refused_first_action_leaves_no_visible_newborn(self):
        workspace = self.root / 'workspace'
        workspace.mkdir()
        env = dict(os.environ, XDG_STATE_HOME=str(self.root / 'state'),
                   XDG_CONFIG_HOME=str(self.root / 'config'), KPOPPER_SESSION_DISABLE='1')
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/cli.py'), 'add', 'p.ready',
            'verdict=ready', 'rests_on=[p.missing]', 'wrong_if=p.missing > 0',
            '--as-of', '2026-09-01'], cwd=workspace, env=env, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((workspace / 'GROUNDING.yaml').exists())
        self.assertFalse((workspace / '.kpopper' / 'history.yaml').exists())


if __name__ == '__main__':
    unittest.main()
