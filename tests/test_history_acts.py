"""Explicit history acts resolve named versions without temporal or review inference."""
import copy
from pathlib import Path
import unittest
from unittest import mock

from scripts import history_authoring as A, history_contract as C, history_transaction as T
from scripts import history_store as H, provenance as P, versions as V
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_store as fixtures
from tests.test_history_authoring import claim


class Acts(unittest.TestCase):
    def setUp(self):
        fixture = fixtures.Storage()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture, self.entry, self.store = fixture, fixture.entry, fixture.store
        self.first = claim(body={'v': 1}, op='first')
        self.second = claim(body={'v': 2}, op='second')
        fixture.publish([self.first, self.second], op='disputed-bootstrap')

    def action(self, kind, target=None, over=(), because='explicit tested decision'):
        return {'kind': kind, 'id': 'p.input', 'of': (target or self.first)['id'],
                'over': [obj['id'] for obj in over], 'because': because}

    def prepare(self, action):
        return A.prepare_act(self.entry, action, by='named reviewer')

    def publish(self, action):
        mutation = self.prepare(action)
        A.commit(self.entry, mutation, verify=lambda data: None)
        return mutation

    def test_choose_contested_version_adds_only_act_and_preserves_original_bytes(self):
        before = self.store.capture()
        self.assertEqual(before.state['subjects']['p.input']['acceptance'], 'contested')
        mutation = self.prepare(self.action('accept', self.second, [self.first]))
        self.assertEqual(self.store.capture().inventory, before.inventory)
        with mock.patch.object(V, '_evaluate', side_effect=AssertionError('prototype evaluator used')):
            A.commit(self.entry, mutation, verify=lambda data: None)
        after = self.store.capture()
        self.assertEqual(after.state['subjects']['p.input']['head'], self.second['id'])
        self.assertEqual(after.state['subjects']['p.input']['acceptance'], 'accepted')
        additions = set(after.objects) - set(before.objects)
        self.assertEqual(len(additions), 1)
        act = after.objects[additions.pop()]
        self.assertEqual(act['kind'], 'act')
        self.assertEqual(act['by'], 'named reviewer')
        self.assertEqual(act['body']['over'], [self.first['id']])
        for key, raw in before.object_bytes.items():
            self.assertEqual(after.object_bytes[key], raw)
        self.assertEqual(mutation.to_data()['receipt']['before']['authoring']['version'], 4)

    def test_refute_replacement_never_revives_previous_but_explicit_return_can(self):
        self.publish(self.action('accept', self.second, [self.first]))
        self.publish(self.action('refute', self.second))
        state = self.store.state()['subjects']['p.input']
        self.assertEqual(state['heads'], [])
        self.assertNotEqual(state['acceptance'], 'accepted')
        self.publish(self.action('accept', self.first, [self.second], 'explicit return to first version'))
        self.assertEqual(self.store.state()['subjects']['p.input']['head'], self.first['id'])

    def test_correct_names_replaced_version_and_refute_contested_alternative(self):
        self.publish(self.action('refute', self.second))
        self.assertEqual(self.store.state()['subjects']['p.input']['head'], self.first['id'])
        self.publish(self.action('correct', self.second, [self.first]))
        state = self.store.state()['subjects']['p.input']
        self.assertEqual(state['head'], self.second['id'])
        self.assertEqual(state['marks'][self.first['id']], 'corrected')

    def test_source_free_capture_replay_retains_explicit_act_evidence(self):
        mutation = self.publish(self.action('accept', self.second, [self.first]))
        snapshot = Snapshot.capture([str(self.entry)], read_mode='frozen')
        raw = snapshot.to_json()
        with mock.patch.object(H.Store, 'capture', side_effect=AssertionError('live lookup')):
            replay = Snapshot.from_json(raw)
        self.assertEqual(replay.snapshot_id, snapshot.snapshot_id)
        projection = replay.to_data()['context']['history']
        self.assertEqual(projection['subjects']['p.input']['acceptance'], 'accepted')
        self.assertEqual(projection['subjects']['p.input']['heads'], [self.second['id']])
        self.assertTrue(projection['subjects']['p.input']['open_acts'])

    def test_exact_retry_after_manifest_crash_and_new_operation_identity(self):
        action = self.action('accept', self.second, [self.first])
        mutation = self.prepare(action)
        with mock.patch.object(T, '_replace', side_effect=OSError('crash before view refresh')):
            with self.assertRaisesRegex(OSError, 'crash before view refresh'):
                A.commit(self.entry, mutation, verify=lambda data: None)
        self.assertIn(mutation.to_data()['operation'], self.store.capture().commits)
        restored = T.PreparedMutation.from_bytes(mutation.to_bytes())
        A.commit(self.entry, restored, verify=lambda data: None)
        A.commit(self.entry, restored, verify=lambda data: None)
        self.assertEqual(len(self.store.capture().commits), 2)
        fresh = self.prepare(action)
        self.assertNotEqual(fresh.to_data()['operation'], mutation.to_data()['operation'])
        self.assertNotEqual(fresh.to_bytes(), mutation.to_bytes())

    def test_crash_before_manifest_leaves_dispute_until_retry(self):
        mutation = self.prepare(self.action('accept', self.second, [self.first]))
        publish = T.publish_immutable
        def fail(path, *args, **kwargs):
            if Path(path).parent == Path(self.store.layout['history_commits']):
                raise OSError('manifest interrupted')
            return publish(path, *args, **kwargs)
        with mock.patch.object(T, 'publish_immutable', side_effect=fail):
            with self.assertRaisesRegex(OSError, 'manifest interrupted'):
                A.commit(self.entry, mutation, verify=lambda data: None)
        self.assertEqual(self.store.state()['subjects']['p.input']['acceptance'], 'contested')
        A.commit(self.entry, T.PreparedMutation.from_bytes(mutation.to_bytes()), verify=lambda data: None)
        self.assertEqual(self.store.state()['subjects']['p.input']['head'], self.second['id'])

    def test_explicit_acceptance_retains_failed_and_unknown_conditions_independently(self):
        self.publish(self.action('accept', self.second, [self.first]))
        for subject, predicate, expected in [('d.failed', {'expr': 'p.input > 0'}, 'holds'),
                                             ('d.unknown', 'condition cannot yet be measured', 'unknown')]:
            with self.subTest(subject=subject):
                body = {'verdict': 'a chosen judgment', 'rests_on': ['p.input'],
                        'seen': {'p.input': 'untouched old evidence'}}
                if predicate is not None:
                    body['wrong_if'] = predicate
                else:
                    body['blocked_on'] = 'condition has not been specified'
                obj = claim(subject, kind='judgment', op=subject, body=body,
                            pins={'p.input': self.second['id']})
                self.fixture.publish([obj], op=subject + '-bootstrap')
                action = {'kind': 'accept', 'id': subject, 'of': obj['id'], 'over': [],
                          'because': 'explicit acceptance with recorded reservation'}
                mutation = self.publish(action)
                captured = self.store.capture()
                self.assertEqual(captured.state['subjects'][subject]['acceptance'], 'accepted')
                self.assertEqual(captured.objects[obj['id']]['body'], body)
                status = mutation.to_data()['receipt']['after']['assessment']['nodes'][subject]['state']['falsifier']['status']
                self.assertEqual(status, expected)

    def test_operational_failure_refuses_without_float_fallback_or_writes(self):
        before = self.store.capture().inventory
        with mock.patch.object(A.authoring.World, 'assessment', side_effect=P.Refused('operational_error')), \
             mock.patch.object(V, '_evaluate', side_effect=AssertionError('fallback')):
            with self.assertRaisesRegex(P.Refused, 'operational_error'):
                self.prepare(self.action('accept', self.first, [self.second]))
        self.assertEqual(self.store.capture().inventory, before)

    def test_wrong_subject_missing_target_act_target_and_empty_reason_refuse(self):
        other = claim('other.input', op='other')
        self.fixture.publish([other], op='other-bootstrap')
        before = self.store.capture().inventory
        actions = [self.action('accept', other), self.action('accept', self.first, [other]),
                   {**self.action('accept'), 'of': 'f' * 64}, self.action('accept', because='  '),
                   {key: val for key, val in self.action('accept').items() if key != 'over'},
                   self.action('refute', self.first, [self.second])]
        for action in actions:
            with self.subTest(action=action):
                with self.assertRaises(C.HistoryError):
                    self.prepare(action)
        self.assertEqual(self.store.capture().inventory, before)
        mutation = self.publish(self.action('accept', self.first, [self.second]))
        act = next(C.decode_document(item['after']) for item in mutation.files if item['role'] == 'history_object')
        with self.assertRaisesRegex(C.HistoryError, 'act_target_must_be_claim'):
            self.prepare({**self.action('accept'), 'of': act['id']})

    def test_unrelated_edit_and_stale_concurrent_act_refuse(self):
        first = self.prepare(self.action('accept', self.first, [self.second]))
        second = self.prepare(self.action('accept', self.second, [self.first]))
        A.commit(self.entry, first, verify=lambda data: None)
        with self.assertRaisesRegex(C.HistoryError, 'stale_baseline'):
            A.commit(self.entry, second, verify=lambda data: None)
        mutation = self.prepare(self.action('refute', self.first))
        self.entry.write_bytes(self.entry.read_bytes() + b'# unrelated user edit\n')
        with self.assertRaisesRegex(C.HistoryError, 'concurrent_edit'):
            A.commit(self.entry, mutation, verify=lambda data: None)


if __name__ == '__main__':
    unittest.main()
