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
