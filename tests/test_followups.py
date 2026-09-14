"""Behavioral boundaries for durable followups and host-owned daily reviews."""
import datetime as dt
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import yaml

from scripts import followups as F
from scripts import followup_daily as D

ROOT = Path(__file__).resolve().parents[1]
UTC = dt.timezone.utc


class Followups(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.workspace = self.root / 'project'
        self.workspace.mkdir()
        self.record = self.workspace / 'PROVENANCE.yaml'
        self.doc = {
            'meta': {'name': 'Followup fixture', 'updated': '2026-09-10'},
            'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
            'known': {
                'facts.count': {'name': 'Count', 'v': 1},
                'facts.other': {'name': 'Other count', 'v': 10},
            },
            'judgments': {
                'c.acceptable': {'rests_on': ['facts.count'], 'verdict': 'Count is acceptable.',
                                 'wrong_if': 'facts.count < 0', 'seen': {'facts.count': 1}},
            },
        }
        self.write_record()
        self.clock = dt.datetime(2026, 9, 10, 12, tzinfo=UTC)
        self.addCleanup(mock.patch.stopall)
        mock.patch.dict(os.environ, {'XDG_STATE_HOME': str(self.root / 'state')}).start()
        self.store = F.Store(self.workspace, now=lambda: self.clock)
        self.store.setup(timezone='Asia/Jerusalem')

    def write_record(self):
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False), encoding='utf-8')

    def change_count(self, value):
        self.doc['known']['facts.count']['v'] = value
        self.write_record()

    def spec(self, identity='task', when=None, **changes):
        result = {'id': identity, 'title': 'Review the count', 'why': 'Keep the decision current.',
                  'how': 'Inspect the count and its evidence.', 'scope': 'Read the count and report findings.',
                  'related': ['facts.count'], 'when': when or {'at': '2026-09-10'}}
        result.update(changes)
        return result

    def add(self, identity='task', when=None, **changes):
        return self.store.add(self.spec(identity, when, **changes))

    def row(self, identity='task'):
        return next(row for row in self.store.scan(limit=100)['items'] if row['id'] == identity)

    def claim(self, identity='task', **kwargs):
        return self.store.claim(identity, self.row(identity)['occurrence'], 'test-session', **kwargs)['claim']

    def advance(self, **kwargs):
        self.clock += dt.timedelta(**kwargs)

    def observe(self, ref, value, when=None, evidence='Read the canonical provider record.'):
        return self.store.observe({'ref': ref, 'value': value, 'observed_at': when or F.stamp(self.clock),
                                   'evidence': evidence})

    def test_capture_date_claim_checked_retry_done_retains_history(self):
        item = self.add(when={'at': '2026-09-11'})
        task = Path(item['task'])
        self.assertTrue(task.is_file())
        self.assertIn('**When**', task.read_text())
        self.assertIn('**Routine**', task.read_text())
        self.assertNotIn(str(self.workspace), str(task))
        row = self.row()
        self.assertEqual(row['state'], 'waiting')
        self.assertEqual(row['wake_hint'], '2026-09-10T21:00:00Z')
        self.advance(hours=9)
        first = self.claim()
        with self.assertRaises(F.Refused):
            self.store.finish('task', first['token'], 'checked', 'Checked without a next time.')
        future = F.stamp(self.clock + dt.timedelta(hours=2))
        result = self.store.finish('task', first['token'], 'checked', 'Checked; retry after the data release.', future)
        self.assertEqual(result['state'], 'waiting')
        self.assertEqual(self.row()['state'], 'waiting')
        self.advance(hours=2)
        second = self.claim()
        self.assertNotEqual(first['token'], second['token'])
        self.assertEqual(self.store.finish('task', second['token'], 'done', 'Verified the final result.')['state'], 'done')
        saved = self.store.load()['items']['task']
        self.assertEqual([entry['outcome'] for entry in saved['attempts']], ['checked', 'done'])
        self.assertEqual(saved['attempts'][0]['evidence'], 'Checked; retry after the data release.')
        self.assertTrue(self.store.finish('task', second['token'], 'done', 'Verified the final result.')['already_recorded'])
        self.assertEqual(len(self.store.load()['items']['task']['attempts']), 2)
        with self.assertRaises(F.Refused):
            self.store.finish('task', second['token'], 'cancelled', 'Different result.')

    def test_changed_revert_stale_occurrence_and_read_only_scan(self):
        self.add(when={'changed': 'facts.count'})
        self.assertEqual(self.row()['state'], 'waiting')
        self.change_count(2)
        ready = self.row()
        self.assertEqual(ready['state'], 'ready')
        before_record = self.record.read_bytes()
        before_ledger = self.store.path.read_bytes()
        self.store.scan()
        self.assertEqual(self.record.read_bytes(), before_record)
        self.assertEqual(self.store.path.read_bytes(), before_ledger)
        self.change_count(1)
        self.assertEqual(self.row()['state'], 'waiting')
        with self.assertRaises(F.Refused):
            self.store.claim('task', ready['occurrence'], 'stale-reader')
        self.change_count(3)
        with self.assertRaises(F.Refused):
            self.store.claim('task', ready['occurrence'], 'stale-reader')
        run = self.claim()
        before_record = self.record.read_bytes()
        self.store.finish('task', run['token'], 'done', 'Verified the updated count.')
        self.assertEqual(self.record.read_bytes(), before_record)
        self.assertEqual(yaml.safe_load(before_record)['judgments']['c.acceptable']['seen'], {'facts.count': 1})

    def test_unrelated_edits_and_yaml_formatting_preserve_occurrence(self):
        self.add(when={'condition': {'id': 'facts.count', 'op': '>=', 'value': 1}})
        initial = self.row()['occurrence']
        self.doc['known']['facts.other']['v'] = 99
        self.write_record()
        self.record.write_text('# harmless formatting\n' + self.record.read_text(), encoding='utf-8')
        self.assertEqual(self.row()['occurrence'], initial)
        self.assertEqual(self.row()['state'], 'ready')

    def test_changed_check_rearms_at_next_time_without_another_change(self):
        self.add(when={'changed': 'facts.count'})
        self.change_count(2)
        run = self.claim()
        self.store.finish('task', run['token'], 'checked', 'Recheck the pending release tomorrow.',
                          F.stamp(self.clock + dt.timedelta(days=1)))
        self.assertEqual(self.row()['state'], 'waiting')
        self.advance(days=1)
        self.assertEqual(self.row()['state'], 'ready')
        again = self.claim()
        self.store.finish('task', again['token'], 'done', 'The later review resolved the task.')
        self.assertEqual(self.row()['state'], 'done')

    def test_new_relevant_change_can_fire_before_the_next_scheduled_check(self):
        self.add(when={'changed': 'facts.count'})
        self.change_count(2)
        run = self.claim()
        self.store.finish('task', run['token'], 'checked', 'Recheck tomorrow unless evidence changes.',
                          F.stamp(self.clock + dt.timedelta(days=1)))
        self.change_count(3)
        self.assertEqual(self.row()['state'], 'ready')

    def test_finish_after_inputs_change_parks_result_without_accepting_unseen_baseline(self):
        self.add(when={'changed': 'facts.count'})
        self.change_count(2)
        run = self.claim()
        self.change_count(3)
        before = self.record.read_bytes()
        result = self.store.finish('task', run['token'], 'done', 'Completed using the count observed at claim.')
        self.assertEqual(result['state'], 'needs_user')
        self.assertTrue(result['inputs_changed'])
        self.assertEqual(self.record.read_bytes(), before)
        item = self.store.load()['items']['task']
        self.assertEqual(item['baseline']['facts.count'], 1)
        self.assertEqual(item['attempts'][0]['run']['baseline']['facts.count'], 2)
        self.assertEqual(item['attempts'][0]['outcome'], 'needs_user')

    def test_missing_graph_or_related_entry_stays_unknown(self):
        self.add()
        del self.doc['known']['facts.count']
        self.write_record()
        self.assertEqual(self.row()['state'], 'unknown')
        self.record.unlink()
        scan = self.store.scan()
        self.assertEqual(scan['items'][0]['state'], 'unknown')
        self.assertTrue(scan['graph_error'])
        self.assertEqual(scan['record'], str(self.record))
        self.assertFalse(self.record.exists())

    def test_unavailable_context_does_not_override_trigger_logic(self):
        missing = {'condition': {'id': 'facts.other', 'op': '>', 'value': 0}}
        known = {'condition': {'id': 'facts.count', 'op': '>', 'value': 0}}
        due = {'at': '2026-09-10'}
        cases = ((due, 'ready'), ({'at': '2026-09-11'}, 'waiting'), (known, 'ready'),
                 (missing, 'unknown'), ({'changed': 'facts.other'}, 'unknown'),
                 ({'any': [due, missing]}, 'ready'), ({'all': [due, missing]}, 'unknown'),
                 ({'manual': 'Confirm authority'}, 'unknown'))
        values, flags, error = F.graph(str(self.record))
        values['facts.other'] = {'unavailable': 'The calculation has no result'}
        with mock.patch.object(F, 'graph', return_value=(values, flags, error)):
            for index, (trigger, expected) in enumerate(cases):
                with self.subTest(trigger=trigger):
                    key = 'task' + str(index)
                    self.add(key, trigger, related=['facts.count', 'facts.other'])
                    row = self.row(key)
                    self.assertEqual(row['state'], expected)
                    self.assertTrue(any('facts.other' in reason and 'unavailable' in reason
                                        for reason in row['reasons']))

    def test_unavailable_context_preserves_execution_boundaries(self):
        values, flags, error = F.graph(str(self.record))
        values['facts.count'] = {'unavailable': 'Install the evaluator to check this reading'}
        with mock.patch.object(F, 'graph', return_value=(values, flags, error)):
            local = self.add('local', title='Diagnose the missing evaluator')
            self.add('remote', task='https://tasks.example.test/items/repair')
            self.add('owned', executor='another-routine')
            self.assertEqual(self.row('local')['state'], 'ready')
            self.assertEqual(self.row('remote')['state'], 'unknown')
            self.assertEqual(self.row('owned')['state'], 'delegated')
            for key in ('remote', 'owned'):
                with self.assertRaises(F.Refused):
                    self.claim(key)
            claim = self.claim('local')
            self.assertEqual(self.row('local')['state'], 'claimed')
            self.store.finish('local', claim['token'], 'needs_user', 'Installation requires a user decision.')
            self.assertEqual(self.row('local')['state'], 'needs_user')
            with self.assertRaises(F.Refused):
                self.claim('local')
            self.store.resume('local', 'The owner authorized the existing diagnostic scope.')
            Path(local['task']).write_text('A different task scope')
            self.assertEqual(self.row('local')['state'], 'unknown')
            with self.assertRaises(F.Refused):
                self.claim('local')

    def test_retry_does_not_override_unknown_trigger_or_baseline(self):
        for key, trigger in (('changed', {'changed': 'facts.count'}),
                             ('condition', {'condition': {'id': 'facts.count', 'op': '>', 'value': 0}})):
            self.add(key, trigger)
        self.change_count(2)
        for key in ('changed', 'condition'):
            run = self.claim(key)
            self.store.finish(key, run['token'], 'checked', 'The report is due tomorrow.',
                              F.stamp(self.clock + dt.timedelta(days=1)))
        self.advance(days=1)
        # Unknown can come from the baseline or from a nonnumeric comparison,
        # even when every current related value is available.
        with self.store.transaction() as data:
            data['items']['changed']['baseline']['facts.count'] = {'unavailable': 'No earlier calculation'}
        self.change_count('pending')
        for key in ('changed', 'condition'):
            self.assertEqual(self.row(key)['state'], 'unknown')
            with self.assertRaises(F.Refused):
                self.claim(key)

    def test_edited_and_missing_canonical_file_require_attention(self):
        canonical = self.root / 'existing-task.md'
        canonical.write_text('# Existing task\nOriginal scope.\n')
        supplied = self.spec(task=str(canonical))
        self.store.add(supplied)
        original = self.row()['occurrence']
        canonical.write_text('# Existing task\nChanged scope.\n')
        self.assertEqual(self.row()['state'], 'unknown')
        with self.assertRaises(F.Refused):
            self.store.claim('task', original, 'stale-owner')
        self.store.refresh('task', supplied, 'Reread and accept the revised scope.')
        self.assertEqual(self.row()['state'], 'ready')
        self.assertEqual(self.store.load()['items']['task']['task'], str(canonical))
        canonical.unlink()
        self.assertEqual(self.row()['state'], 'unknown')
        self.assertFalse(canonical.exists())
        self.assertFalse((self.store.root / 'items').exists())

    def test_remote_task_requires_fresh_observation_and_keeps_canonical_url(self):
        url = 'https://tasks.example.test/items/42'
        self.add(task=url)
        self.assertEqual(self.row()['state'], 'unknown')
        self.observe(url, 'open')
        self.assertEqual(self.row()['state'], 'ready')
        self.advance(hours=24)
        self.assertEqual(self.row()['state'], 'unknown')
        self.assertEqual(self.store.load()['items']['task']['task'], url)
        self.assertFalse((self.store.root / 'items').exists())

    def test_same_external_value_refresh_during_claim_preserves_completion(self):
        for kind in ('trigger', 'canonical_task'):
            with self.subTest(kind=kind):
                identity = 'same-' + kind
                ref = 'event:' + identity if kind == 'trigger' else 'https://tasks.example.test/' + identity
                expected = 'merged' if kind == 'trigger' else 'open'
                when = {'external': {'ref': ref, 'equals': expected}} if kind == 'trigger' else None
                self.add(identity, when, **({'task': ref} if kind == 'canonical_task' else {}))
                self.observe(ref, expected, evidence='Initial provider inspection.')
                run = self.claim(identity)
                self.advance(minutes=1)
                self.observe(ref, expected, evidence='Reopened provider and confirmed the same status.')
                result = self.store.finish(identity, run['token'], 'done', 'Completed after confirming current status.')
                self.assertEqual(result['state'], 'done')
                self.assertFalse(result['inputs_changed'])

    def test_changed_external_value_during_claim_still_parks_completion(self):
        for kind in ('trigger', 'canonical_task'):
            with self.subTest(kind=kind):
                identity = 'changed-' + kind
                ref = 'event:' + identity if kind == 'trigger' else 'https://tasks.example.test/' + identity
                expected = 'merged' if kind == 'trigger' else 'open'
                when = {'external': {'ref': ref, 'equals': expected}} if kind == 'trigger' else None
                self.add(identity, when, **({'task': ref} if kind == 'canonical_task' else {}))
                self.observe(ref, expected, evidence='Initial provider inspection.')
                run = self.claim(identity)
                self.advance(minutes=1)
                self.observe(ref, 'reopened' if kind == 'trigger' else 'closed', evidence='Provider status changed.')
                result = self.store.finish(identity, run['token'], 'done', 'Work used the earlier provider status.')
                self.assertEqual(result['state'], 'needs_user')
                self.assertTrue(result['inputs_changed'])

    def test_external_trigger_freshness_and_completed_prerequisite(self):
        self.add('first')
        self.add('second', {'all': [{'completed': 'first'},
                                 {'external': {'ref': 'pr:42', 'equals': 'merged', 'max_age_hours': 1}}]})
        self.assertEqual(self.row('second')['state'], 'waiting')
        run = self.claim('first')
        self.store.finish('first', run['token'], 'done', 'Finished the prerequisite.')
        self.assertEqual(self.row('second')['state'], 'unknown')
        self.observe('pr:42', 'merged')
        self.assertEqual(self.row('second')['state'], 'ready')
        self.advance(hours=1)
        self.assertEqual(self.row('second')['state'], 'unknown')

    def test_missing_prerequisite_and_cycle_are_rejected(self):
        with self.assertRaises(F.Refused):
            self.add('missing', {'completed': 'no-such-task'})
        self.add('first')
        self.add('second', {'completed': 'first'})
        with self.assertRaises(F.Refused):
            self.store.refresh('first', self.spec('first', {'completed': 'second'}), 'Would create a cycle.')
        self.assertEqual(self.store.load()['items']['first']['spec']['when'], {'at': '2026-09-10'})

    def test_duplicate_id_and_observation_retries_are_idempotent(self):
        first = self.add()
        self.assertEqual(self.add()['task'], first['task'])
        with self.assertRaises(F.Refused):
            self.add(title='A conflicting title')
        self.assertEqual(len(self.store.load()['items']), 1)
        first_report = self.observe('pr:42', 'open')
        self.assertEqual(self.observe('pr:42', 'open'), first_report)
        for value, when, evidence in [('closed', F.stamp(self.clock), 'Conflicting value.'),
                                      ('open', F.stamp(self.clock), 'Conflicting evidence.'),
                                      ('open', F.stamp(self.clock - dt.timedelta(seconds=1)), 'Older report.'),
                                      ('open', F.stamp(self.clock + dt.timedelta(seconds=1)), 'Future report.')]:
            with self.subTest(value=value, when=when, evidence=evidence), self.assertRaises(F.Refused):
                self.observe('pr:42', value, when, evidence)
        self.assertEqual(self.store.load()['observations']['pr:42']['value'], 'open')

    def test_observations_require_offset_timestamps(self):
        for when in ('2026-09-10', '2026-09-10T12:00:00'):
            with self.subTest(when=when), self.assertRaises(ValueError):
                self.observe('pr:42', 'open', when)

    def test_claim_expiry_blocks_steal_late_completion_and_requires_reconciliation(self):
        self.add()
        claim = self.claim()
        with self.assertRaises(F.Refused):
            self.claim()
        with self.assertRaises(F.Refused):
            self.store.finish('task', 'wrong-token', 'done', 'Wrong owner.')
        self.advance(minutes=30)
        self.assertEqual(self.row()['state'], 'interrupted')
        for operation in (lambda: self.claim(),
                          lambda: self.store.finish('task', claim['token'], 'done', 'Too late.'),
                          lambda: self.store.renew('task', claim['token']),
                          lambda: self.store.recover('task', '')):
            with self.assertRaises(F.Refused):
                operation()
        self.store.recover('task', 'Verified no external action occurred during the interrupted run.')
        self.assertEqual(self.row()['state'], 'ready')
        new = self.claim()
        self.assertNotEqual(new['token'], claim['token'])
        self.assertEqual(self.store.load()['items']['task']['attempts'][0]['outcome'], 'recovered')

    def test_live_renewal_extends_expiry_and_release_retains_attempt(self):
        self.add()
        claim = self.claim()
        self.advance(minutes=20)
        renewed = self.store.renew('task', claim['token'])
        self.assertEqual(F.T.parse_time(renewed['expires_at']), self.clock + dt.timedelta(minutes=30))
        self.advance(minutes=15)
        self.assertEqual(self.row()['state'], 'claimed')
        with self.assertRaises(F.Refused):
            self.store.refresh('task', self.spec(), 'Cannot edit a claimed task.')
        self.store.finish('task', claim['token'], 'released', 'No work started; relinquishing ownership.')
        self.assertEqual(self.row()['state'], 'ready')
        self.assertEqual(self.store.load()['items']['task']['attempts'][0]['outcome'], 'released')

    def test_two_concurrent_subprocess_claimers_have_one_winner(self):
        self.add()
        occurrence = self.row()['occurrence']
        program = '''import json, sys
sys.path.insert(0, sys.argv[1])
from scripts import followups as F
store = F.Store(sys.argv[2], now=lambda: F.T.parse_time(sys.argv[3]))
print('READY', flush=True)
sys.stdin.read(1)
try:
    result = store.claim('task', sys.argv[4], sys.argv[5])
    print(json.dumps(result), flush=True)
except F.Refused as error:
    print(json.dumps({'error': str(error)}), flush=True)
    sys.exit(2)
'''
        processes = [subprocess.Popen([sys.executable, '-c', program, str(ROOT), str(self.workspace),
                                      F.stamp(self.clock), occurrence, owner], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                     env=os.environ.copy()) for owner in ('owner-a', 'owner-b')]
        try:
            for process in processes:
                self.assertEqual(process.stdout.readline().strip(), 'READY')
            for process in processes:
                process.stdin.write('x')
                process.stdin.flush()
            outputs = [process.communicate(timeout=20) for process in processes]
            self.assertEqual(sorted(process.returncode for process in processes), [0, 2], outputs)
            results = [json.loads(output[0]) for output in outputs]
            self.assertEqual(sum('claim' in result for result in results), 1)
            winner = next(result['claim'] for result in results if 'claim' in result)
            self.assertEqual(self.store.load()['items']['task']['claim']['token'], winner['token'])
        finally:
            for process in processes:
                if process.poll() is None:
                    process.kill()
                    process.communicate()

    def test_external_executor_remains_delegated(self):
        self.add(executor='existing-host:paused-owner')
        self.assertEqual(self.row()['state'], 'delegated')
        with self.assertRaises(F.Refused):
            self.claim()

    def test_daily_same_local_day_is_idempotent_and_timezone_is_preserved(self):
        self.clock = dt.datetime(2026, 9, 10, 22, tzinfo=UTC)
        plan = D.plan(self.store, '08:15')
        self.assertEqual(plan['timezone'], 'Asia/Jerusalem')
        self.assertEqual(plan['time'], '08:15')
        self.assertEqual(plan['state'], 'proposed')
        self.assertIn(str(self.record), plan['prompt'])
        self.assertIn(str(self.store.path), plan['prompt'])
        review = D.start(self.store, 'daily-owner')
        self.assertEqual(review['claim']['day'], '2026-09-11')
        token = review['claim']['token']
        with self.assertRaises(F.Refused):
            D.start(self.store, 'competing-owner')
        self.assertEqual(D.finish(self.store, token, 'No useful work needed.')['state'], 'complete')
        self.assertEqual(D.finish(self.store, token, 'No useful work needed.')['state'], 'already_completed')
        again = D.start(self.store, 'another-owner')
        self.assertEqual(again['state'], 'already_completed')
        self.assertFalse(again['notification'])
        self.assertEqual(len(self.store.load()['daily']['receipts']), 1)
        self.advance(days=1)
        next_day = D.start(self.store, 'next-day-owner')
        self.assertEqual(next_day['state'], 'running')
        self.assertFalse(next_day['new_attention'])

    def test_daily_next_day_compares_attention_with_completed_review_state(self):
        self.add()
        first = D.start(self.store, 'first-review')
        self.assertTrue(first['new_attention'])
        run = self.claim(daily_token=first['claim']['token'])
        self.store.finish('task', run['token'], 'done', 'Finished the task during daily review.')
        D.finish(self.store, first['claim']['token'], 'Reviewed and completed the only task.')
        self.advance(days=1)
        second = D.start(self.store, 'second-review')
        self.assertEqual(second['packet']['counts']['done'], 1)
        self.assertFalse(second['new_attention'])

    def test_daily_binding_reuses_one_identity_and_reports_paused_or_stale(self):
        report = {'host': 'codex', 'id': 'schedule-1', 'state': 'active', 'evidence': 'Created and read back host schedule.'}
        D.binding(self.store, report)
        self.assertEqual(D.status(self.store)['state'], 'active_reported')
        self.assertEqual(D.plan(self.store)['state'], 'registered')
        with self.assertRaises(F.Refused):
            D.binding(self.store, {**report, 'id': 'schedule-2'})
        D.binding(self.store, {**report, 'state': 'paused', 'evidence': 'Paused and inspected the same schedule.'})
        self.assertEqual(D.status(self.store)['state'], 'paused_reported')
        self.advance(hours=25)
        self.assertEqual(D.status(self.store)['state'], 'unverified')
        self.assertEqual(D.status(self.store)['binding']['id'], 'schedule-1')

    def test_daily_expiry_and_recovery_require_item_reconciliation_first(self):
        self.add()
        daily = D.start(self.store, 'daily-owner')['claim']
        item = self.claim(daily_token=daily['token'])
        self.advance(minutes=30)
        self.assertEqual(D.status(self.store)['run'], 'interrupted')
        for operation in (lambda: D.start(self.store, 'stealing-owner'),
                          lambda: D.finish(self.store, daily['token'], 'Too late.'),
                          lambda: D.renew(self.store, daily['token']),
                          lambda: D.recover(self.store, 'Daily review inspected but item is unresolved.')):
            with self.assertRaises(F.Refused):
                operation()
        self.store.recover('task', 'Reconciled the task; no external effect remains uncertain.')
        D.recover(self.store, 'Reconciled daily work and its item attempts.')
        self.assertEqual(D.status(self.store)['run'], 'idle')
        self.assertEqual(D.status(self.store)['last_review']['outcome'], 'recovered')
        self.assertNotEqual(D.start(self.store, 'recovery-owner')['claim']['token'], daily['token'])

    def test_daily_live_renewal_extends_expiry_and_rejects_other_tokens(self):
        claim = D.start(self.store, 'daily-owner')['claim']
        self.advance(minutes=20)
        with self.assertRaises(F.Refused):
            D.renew(self.store, 'wrong-token')
        renewed = D.renew(self.store, claim['token'])
        self.assertEqual(F.T.parse_time(renewed['expires_at']), self.clock + dt.timedelta(minutes=30))
        self.advance(minutes=15)
        self.assertEqual(D.status(self.store)['run'], 'running')
        D.finish(self.store, claim['token'], 'Review finished without actions.')
        with self.assertRaises(F.Refused):
            D.finish(self.store, claim['token'], 'Conflicting review outcome.')

    def test_daily_action_budget_and_active_item_finish_boundary(self):
        for index in range(4):
            self.add('task{}'.format(index))
        review = D.start(self.store, 'daily-owner')
        token = review['claim']['token']
        self.assertEqual(review['limits'], {'followup_actions': 3, 'maintenance_actions': 1})
        first = self.claim('task0', daily_token=token)
        with self.assertRaises(F.Refused):
            D.finish(self.store, token, 'Cannot finish while task is claimed.')
        self.store.finish('task0', first['token'], 'done', 'Completed task zero.')
        for index in (1, 2):
            run = self.claim('task{}'.format(index), daily_token=token)
            self.store.finish('task{}'.format(index), run['token'], 'done', 'Completed the task.')
        with self.assertRaises(F.Refused):
            self.claim('task3', daily_token=token)
        self.assertEqual(self.row('task3')['state'], 'ready')
        self.assertEqual(len(self.store.load()['daily']['claim']['actions']), 3)
        self.assertEqual(D.finish(self.store, token, 'Three task actions complete.')['state'], 'complete')

    def test_daily_packet_is_bounded_with_honest_omitted_counts(self):
        for index in range(23):
            self.add('task{:02d}'.format(index))
        self.change_count(-1)
        packet = D.start(self.store, 'daily-owner')['packet']
        self.assertEqual(len(packet['items']), 20)
        self.assertEqual(packet['omitted'], 3)
        self.assertEqual(packet['counts']['ready'], 23)
        self.assertLessEqual(len(packet['maintenance']), 1)
        self.assertIn('maintenance_omitted', packet)

    def assert_missing_nested_record_keeps_registry(self, git=False):
        parent = self.root / 'nested-parent'
        nested = parent / 'subproject'
        child = nested / 'src' / 'inner'
        child.mkdir(parents=True)
        (parent / 'PROVENANCE.yaml').write_bytes(self.record.read_bytes())
        record = nested / 'PROVENANCE.yaml'
        record.write_bytes(self.record.read_bytes())
        if git:
            initialized = subprocess.run(['git', '-C', str(parent), 'init', '-q'], text=True,
                                         capture_output=True, timeout=20)
            self.assertEqual(initialized.returncode, 0, initialized.stderr)
        configured = F.Store(nested, now=lambda: self.clock)
        configured.setup(timezone='Asia/Jerusalem')
        configured.add(self.spec())
        before = F.Store(child, now=lambda: self.clock)
        self.assertEqual(before.path, configured.path)
        record.unlink()
        resumed = F.Store(child, now=lambda: self.clock)
        self.assertEqual(resumed.path, configured.path)
        scan = resumed.scan()
        self.assertEqual(scan['record'], str(record))
        self.assertEqual(scan['workspace'], str(nested))
        self.assertEqual(scan['items'][0]['state'], 'unknown')
        self.assertTrue(scan['graph_error'])
        self.assertFalse(record.exists())

    def test_missing_nested_non_git_record_keeps_configured_ledger_from_child(self):
        self.assert_missing_nested_record_keeps_registry()

    def test_missing_git_subproject_record_keeps_configured_ledger_from_child(self):
        self.assert_missing_nested_record_keeps_registry(git=True)

    def test_disappeared_explicit_task_directory_is_not_recreated(self):
        workspace = self.root / 'explicit-store-project'
        workspace.mkdir()
        (workspace / 'PROVENANCE.yaml').write_bytes(self.record.read_bytes())
        destination = self.root / 'user-tasks'
        destination.mkdir()
        store = F.Store(workspace, now=lambda: self.clock)
        store.setup(store=str(destination), timezone='Asia/Jerusalem')
        destination.rmdir()
        with self.assertRaises(F.Refused):
            store.setup(store=str(destination), timezone='Asia/Jerusalem')
        self.assertFalse(destination.exists())
        with self.assertRaises(F.Refused):
            store.add(self.spec())
        self.assertFalse(destination.exists())
        self.assertEqual(store.load()['items'], {})

    def test_refresh_metadata_preserves_external_canonical_task(self):
        destination = self.root / 'user-tasks'
        destination.mkdir()
        canonical = destination / 'canonical.md'
        canonical.write_text('# User-owned task\nCurrent instructions from the owner.\n')
        supplied = self.spec(task=str(canonical))
        self.store.add(supplied)
        canonical.write_text('# User-owned task\nRevised instructions from the owner.\n')
        current = canonical.read_bytes()
        revised = {**supplied, 'title': 'Updated title', 'how': 'Read the revised owner instructions.',
                   'scope': 'Read and summarize the revised instructions.'}
        refreshed = self.store.refresh('task', revised, 'Reread and accept the canonical revised instructions.')
        self.assertEqual(refreshed['task'], str(canonical))
        self.assertEqual(refreshed['spec']['scope'], revised['scope'])
        self.assertEqual(canonical.read_bytes(), current)
        self.assertEqual(self.row()['state'], 'ready')
        self.assertFalse((self.store.root / 'items').exists())

    def test_worktrees_share_state_and_keep_the_original_record_pinned(self):
        def git(*args):
            result = subprocess.run(['git', '-C', str(self.workspace), *args], text=True,
                                    capture_output=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stderr)

        git('init', '-q')
        git('add', 'PROVENANCE.yaml')
        git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test',
            'commit', '-qm', 'Fixture knowledge record')
        self.store = F.Store(self.workspace, now=lambda: self.clock)
        self.store.setup(timezone='Asia/Jerusalem')
        self.add(when={'changed': 'facts.count'})
        secondary = self.root / 'second-checkout'
        git('worktree', 'add', '--detach', str(secondary), 'HEAD')
        other = F.Store(secondary, now=lambda: self.clock)
        self.assertEqual(other.path, self.store.path)
        self.assertEqual(other.load()['config']['record'], str(self.record))
        alternative = yaml.safe_load((secondary / 'PROVENANCE.yaml').read_text())
        alternative['known']['facts.count']['v'] = 99
        (secondary / 'PROVENANCE.yaml').write_text(yaml.safe_dump(alternative))
        self.assertEqual(other.scan()['items'][0]['state'], 'waiting')
        self.change_count(2)
        self.assertEqual(other.scan()['items'][0]['state'], 'ready')
        self.record.unlink()
        result = other.scan()
        self.assertEqual(result['record'], str(self.record))
        self.assertEqual(result['items'][0]['state'], 'unknown')
        self.assertFalse(self.record.exists())

    def test_cli_setup_add_scan_claim_and_finish_emit_json(self):
        workspace = self.root / 'cli-project'
        workspace.mkdir()
        (workspace / 'PROVENANCE.yaml').write_bytes(self.record.read_bytes())

        def cli(*arguments, supplied=None):
            result = subprocess.run([sys.executable, str(ROOT / 'scripts' / 'cli.py'),
                                     '--workspace', str(workspace), 'followups', *arguments],
                                    input=json.dumps(supplied) if supplied is not None else None,
                                    text=True, capture_output=True, env=os.environ.copy(), timeout=20)
            self.assertEqual(result.returncode, 0, result.stderr or result.stdout)
            return json.loads(result.stdout)

        setup = cli('setup', '--timezone', 'Asia/Jerusalem')
        self.assertEqual(setup['timezone'], 'Asia/Jerusalem')
        added = cli('add', '--file', '-', supplied=self.spec(when={'at': '2000-01-01'}))
        self.assertTrue(Path(added['task']).is_file())
        row = cli('scan')['items'][0]
        self.assertEqual(row['state'], 'ready')
        run = cli('claim', 'task', '--occurrence', row['occurrence'], '--owner', 'cli-session')['claim']
        finished = cli('finish', 'task', '--token', run['token'], '--outcome', 'done', '--evidence', 'CLI flow verified.')
        self.assertEqual(finished['state'], 'done')
        self.assertEqual(cli('show', 'task')['attempts'][0]['outcome'], 'done')


if __name__ == '__main__':
    unittest.main()
