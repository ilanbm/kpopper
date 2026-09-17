"""Atomic core batches validate the complete world and retain recovery evidence."""
import copy
import contextlib
import datetime
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import yaml
from scripts import ingestion as I


@unittest.skipUnless(os.name == 'posix', 'ingestion requires POSIX locking')
class CoreIngestion(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.record = self.root / 'GROUNDING.yaml'
        self.state = self.root / 'state'
        self.doc = {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
            'sources': {'s.old': {'file': 'old.md', 'read': '2026-09-01'}},
            'known': {'p.price': {'v': 10, 'from': 's.old', 'of': '2026-09-01'}},
            'judgments': {'c.price': {'rests_on': ['p.price'], 'verdict': 'Fits',
                'wrong_if': {'expr': 'p.price > 15'}, 'seen': {'p.price': 10}}}}
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        (self.root / 'old.md').write_text('Price 10.')

    def report(self, operations=None, **extra):
        return {'source_quote': 'Updated calculation inputs.', 'date': '2026-09-16',
            'record_sha256': I._sha(self.record.read_bytes()),
            'updates': operations or [{'kind': 'set', 'id': 'p.price', 'value': 20}], **extra}

    def test_core_batch_never_enters_legacy_gates_and_reports_new_falsification(self):
        with patch.object(I.P, 'mark', side_effect=AssertionError('legacy mark')), \
                patch.object(I.P, 'gate', side_effect=AssertionError('legacy gate')), \
                patch.object(I.P.E, 'compute', side_effect=AssertionError('legacy computation')):
            result = I.update(self.report(), self.record, self.state)
        self.assertEqual(result['state'], 'applied', result)
        self.assertEqual(result['newly_fired_judgments'], ['c.price'])
        self.assertEqual(yaml.safe_load(self.record.read_text())['judgments'], self.doc['judgments'])

    def test_forward_references_and_history_use_final_world(self):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            result = I.update(self.report([
                {'kind': 'add', 'id': 'c.total', 'body': {'rests_on': ['p.total'], 'verdict': 'Fits', 'wrong_if': 'p.total > 50'}},
                {'kind': 'add', 'id': 'p.total', 'body': {'rule': 'p.price * p.quantity'}},
                {'kind': 'add', 'id': 'p.quantity', 'body': {'v': 3}},
                {'kind': 'set', 'id': 'p.price', 'value': 12},
            ]), self.record, self.state)
        self.assertEqual(result['state'], 'applied', result)
        self.assertEqual(output.getvalue(), '')
        raw = I.P.bodies(yaml.safe_load(self.record.read_text()))
        value = raw['c.total']['seen']['p.total']['computed']['value']
        self.assertEqual(value, {'type': 'number', 'numerator': '36', 'denominator': '1'})

    def test_introduced_failure_refuses_whole_batch(self):
        before = self.record.read_bytes()
        result = I.update(self.report([{'kind': 'set', 'id': 'p.price', 'value': 12},
            {'kind': 'add', 'id': 'p.bad', 'body': {'rule': {'expr': 'p.price / 0'}}}]), self.record, self.state)
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('division_by_zero', result['reason'])
        self.assertEqual(self.record.read_bytes(), before)

    def test_recovery_checks_core_gate_and_does_not_rewrite(self):
        event = I.capture(self.report(), self.record, self.state, start=False)
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.record, self.state, event['event_id'], _crash_after_commit=True)
        before = self.record.read_bytes()
        result = I.process(self.record, self.state, event['event_id'])[0]
        self.assertEqual(result['state'], 'applied', result)
        self.assertTrue(result['recovered'])
        self.assertEqual(before, self.record.read_bytes())

    def test_explicit_profile_can_promote_scalar_only_batch(self):
        self.doc.pop('meta')
        self.doc.pop('judgments')
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        result = I.update(self.report(profile='core/v1'), self.record, self.state)
        self.assertEqual(result['state'], 'applied', result)
        self.assertEqual(yaml.safe_load(self.record.read_text())['meta']['reasoning']['version'], 2)

    def test_refused_promotion_finishes_event_and_continues_queue(self):
        self.doc.pop('meta')
        self.doc.pop('judgments')
        self.doc['known']['p.old'] = {'rule': 'p.price > 10'}
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        before = self.record.read_bytes()
        first = I.capture(self.report(profile='core/v1'), self.record, self.state, start=False)
        second = I.capture(self.report(), self.record, self.state, start=False)
        prepare = I._prepare

        def check_unchanged(*args, **kwargs):
            self.assertEqual(self.record.read_bytes(), before)
            self.assertEqual(args[2]['event_id'], second['event_id'])
            return prepare(*args, **kwargs)

        with patch.object(I, '_prepare', side_effect=check_unchanged) as prepared:
            results = I.process(self.record, self.state)
        self.assertEqual([result['state'] for result in results], ['needs_primary', 'applied'])
        self.assertIn('requires explicit migration', results[0]['reason'])
        self.assertEqual(prepared.call_count, 1)
        self.assertEqual(I.status(first['event_id'], self.record, self.state)['state'], 'needs_primary')
        self.assertEqual(I.status(second['event_id'], self.record, self.state)['state'], 'applied')

    def test_final_promotion_refusal_preserves_record_and_prepared_journal(self):
        event = I.capture(self.report(), self.record, self.state, start=False)
        before = self.record.read_bytes()
        authoring = I.P._peer('reasoning.authoring')
        prepare = authoring.prepare

        def refuse_after_preparation(*args, **kwargs):
            if (self.state / 'journals' / (event['event_id'] + '.json')).exists():
                raise I.P.Refused('pending_profile_reconciliation_required: changed overlay')
            return prepare(*args, **kwargs)

        with patch.object(authoring, 'prepare', side_effect=refuse_after_preparation):
            result = I.process(self.record, self.state)[0]
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('pending_profile_reconciliation_required', result['reason'])
        self.assertEqual(self.record.read_bytes(), before)
        journal = json.loads((self.state / 'journals' / (event['event_id'] + '.json')).read_text())
        self.assertEqual(journal['phase'], 'prepared')

    def test_recovery_assessment_refusal_retains_committed_record(self):
        event = I.capture(self.report(), self.record, self.state, start=False)
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.record, self.state, event['event_id'], _crash_after_commit=True)
        before = self.record.read_bytes()
        with patch.object(I, '_graph', side_effect=I.P.Refused('unsupported_capability: changed overlay')):
            result = I.process(self.record, self.state)[0]
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('unsupported_capability', result['reason'])
        self.assertTrue(result['record_committed'])
        self.assertEqual(self.record.read_bytes(), before)

    def test_post_commit_assessment_refusal_reports_commit_without_reapplying(self):
        event = I.capture(self.report(), self.record, self.state, start=False)
        journal_path = self.state / 'journals' / (event['event_id'] + '.json')
        graph = I._graph

        def refuse_after_commit(*args, **kwargs):
            if journal_path.exists() and json.loads(journal_path.read_text())['phase'] == 'record_committed':
                raise I.P.Refused('unsupported_capability: changed overlay')
            return graph(*args, **kwargs)

        with patch.object(I, '_graph', side_effect=refuse_after_commit):
            result = I.process(self.record, self.state)[0]
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertTrue(result['record_committed'])
        self.assertIn('unsupported_capability', result['reason'])
        self.assertEqual(yaml.safe_load(self.record.read_text())['known']['p.price']['v'], 20)
        committed = self.record.read_bytes()
        self.assertEqual(I.process(self.record, self.state), [])
        self.assertEqual(self.record.read_bytes(), committed)

    def test_assessment_does_not_swallow_unrelated_process_exit(self):
        I.capture(self.report(), self.record, self.state, start=False)
        before = self.record.read_bytes()
        with patch.object(I, '_graph', side_effect=SystemExit(42)):
            with self.assertRaises(SystemExit) as raised:
                I.process(self.record, self.state)
        self.assertEqual(raised.exception.code, 42)
        self.assertEqual(self.record.read_bytes(), before)

    def test_native_dates_survive_journal_and_recovery(self):
        self.doc['sources']['s.old']['read'] = datetime.date(2026, 9, 1)
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        event = I.capture(self.report(), self.record, self.state, start=False)
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.record, self.state, event['event_id'], _crash_after_commit=True)
        result = I.process(self.record, self.state, event['event_id'])[0]
        self.assertEqual(result['state'], 'applied', result)
        self.assertEqual(yaml.safe_load(self.record.read_text())['sources']['s.old']['read'], datetime.date(2026, 9, 1))

    def test_recovery_refuses_substituted_gate_evidence(self):
        event = I.capture(self.report(), self.record, self.state, start=False)
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.record, self.state, event['event_id'], _crash_after_commit=True)
        journal_path = self.state / 'journals' / (event['event_id'] + '.json')
        journal = json.loads(journal_path.read_text())
        journal['core_gate']['after'] = journal['core_gate']['before']
        journal_path.write_text(json.dumps(journal))
        before = self.record.read_bytes()
        result = I.process(self.record, self.state, event['event_id'])[0]
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('invalid core writer gate evidence', result['reason'])
        self.assertEqual(self.record.read_bytes(), before)

    def test_core_batch_text_input_is_not_a_legacy_inline_formula(self):
        self.doc['known']['p.note'] = {'v': 'p.price + 1', 'from': 's.old'}
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        result = I.update(self.report([{'kind': 'set', 'id': 'p.note', 'value': 'p.price + 2'}]), self.record, self.state)
        self.assertEqual(result['state'], 'applied', result)
        self.assertEqual(yaml.safe_load(self.record.read_text())['known']['p.note']['v'], 'p.price + 2')

    def test_shared_legacy_record_adapter_remains_dormant(self):
        with self.assertRaisesRegex(I.P.Refused, 'core/v1 consumer'):
            I._record_world(self.record)
        with self.assertRaisesRegex(I.P.Refused, 'core/v1 consumer'):
            I._target(self.record, 'p.price')


if __name__ == '__main__':
    unittest.main()
