"""The normal writer stores executable formulas without rewriting old knowledge."""
import contextlib
import copy
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import yaml
from scripts import ingestion as I
from scripts.session.core import Core

P = I.P


class Authoring(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.core = Core()
        except ValueError:
            if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1':
                raise
            raise unittest.SkipTest('build the core to exercise normal formula authoring')

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / 'GROUNDING.yaml'
        self.doc = {'sources': {'s.report': {'file': 'report.md'}}, 'known': {
            'order.price': {'v': 20, 'from': 's.report'}, 'order.quantity': {'v': 5, 'from': 's.report'},
            'order.code': {'quoted': '80', 'from': 's.report'}, 'order.state': {'v': 'ready', 'from': 's.report'}},
            'judgments': {'c.original': {'rests_on': ['order.price'], 'seen': {'order.price': 10},
                'verdict': 'Within limit', 'wrong_if': 'order.price > 100'}}}
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        (self.path.parent / 'report.md').write_text('Price 20; quantity 5; code text 80; state ready.')

    def add(self, nid, body, **kwargs):
        with contextlib.redirect_stdout(io.StringIO()) as out:
            P.apply([str(self.path)], {'kind': 'add', 'id': nid, 'body': body, **kwargs})
        return out.getvalue()

    def read(self):
        return P.load([str(self.path)])

    def test_rule_and_condition_are_normalized_at_the_normal_write_boundary(self):
        self.add('order.total', {'rule': 'order.price * order.quantity'})
        self.add('c.total', {'rests_on': ['order.total'], 'verdict': 'Fits', 'wrong_if': 'order.total > 150'})
        raw = P.bodies(self.read())
        self.assertEqual(raw['order.total']['rule'], {'expr': 'order.price * order.quantity'})
        self.assertIsInstance(raw['c.total']['wrong_if'], dict)
        self.assertEqual(raw['c.total']['seen']['order.total']['computed']['value'], 100)
        self.assertEqual(raw['c.original'], self.doc['judgments']['c.original'])

    def test_exact_constant_arithmetic_and_unicode_references(self):
        self.add('order.decimal', {'rule': '0.1 + 0.2'})
        doc = yaml.safe_load(self.path.read_text(encoding='utf-8'))
        doc['known']['מחיר'] = {'v': 7, 'from': 's.report'}
        self.path.write_text(yaml.safe_dump(doc, allow_unicode=True, sort_keys=False), encoding='utf-8')
        self.add('order.hebrew', {'rule': 'מחיר * 2'})
        doc = self.read(); ids, jud, fields = P.infer(doc); raw = P.with_builtins(doc, ids, jud, fields)
        self.assertEqual(P.value_of(raw, ids, 'order.decimal'), {'rational': ['3', '10']})
        self.assertEqual(P.value_of(raw, ids, 'order.hebrew'), 14)
        self.assertEqual(P.E.lower(raw['order.hebrew']['rule'])['args'][0], {'ref': 'מחיר'})

    def test_arithmetic_operands_are_evaluated_as_new_formulas(self):
        for nid, pred in [('c.product', 'order.price * order.quantity > 150'),
                          ('c.bound', 'order.price > order.quantity * 5')]:
            self.add(nid, {'rests_on': ['order.price', 'order.quantity'], 'verdict': 'Fits', 'wrong_if': pred})
            doc = self.read(); ids, _, _ = P.infer(doc); raw = P.bodies(doc)
            self.assertIsInstance(raw[nid]['wrong_if'], dict)
            self.assertIs(P.evaluate(raw[nid]['wrong_if'], raw, ids), False)
        self.assertEqual(P.check_lines([str(self.path)])[0], [])
        before = self.path.read_bytes()
        with self.assertRaises(P.Refused):
            self.add('c.broken', {'rests_on': ['order.price', 'order.quantity'], 'verdict': 'Fits',
                                  'wrong_if': 'order.price > order.quantity * 2'})
        self.assertEqual(self.path.read_bytes(), before)

    def test_legacy_coercion_and_ambiguous_literals_remain_explicit(self):
        for nid, pred, deps in [('c.quote', 'order.code > 100', ['order.code']),
                                ('c.bare', 'order.state != ready', ['order.state']),
                                ('c.numeric_text', "order.state == '001'", ['order.state']),
                                ('c.literal', 'order.state == \'blocked"\'', ['order.state']),
                                ('c.parentheses', "order.state == ('blocked')", ['order.state'])]:
            out = self.add(nid, {'rests_on': deps, 'verdict': 'Unchanged', 'wrong_if': pred})
            self.assertEqual(P.bodies(self.read())[nid]['wrong_if'], pred)
            self.assertIn('kept as text', out)

    def test_first_predicate_in_a_qualitative_record_is_normalized_and_validated(self):
        self.doc['judgments'] = {'c.policy': {'rests_on': ['order.price'], 'seen': {'order.price': 20},
                                             'verdict': 'Fits policy', 'reopened_by': 'Policy changes'}}
        for hypothesis in (None, 'proposal'):
            self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
            before = self.path.read_bytes()
            for pred, deps in [('order.price * order.quantity > 50', ['order.price', 'order.quantity']),
                               ({'op': 'gt', 'args': [{'ref': 'order.quantity'}, {'num': '50'}]}, ['order.price'])]:
                with self.assertRaises(P.Refused):
                    self.add('c.broken', {'rests_on': deps, 'verdict': 'Fits', 'wrong_if': pred}, hypothesis=hypothesis)
                self.assertEqual(self.path.read_bytes(), before)
            self.add('c.budget', {'rests_on': ['order.price', 'order.quantity'], 'verdict': 'Fits',
                                  'wrong_if': 'order.price * order.quantity > 150'}, hypothesis=hypothesis)
            doc = self.read()
            raw = doc.hypotheses[hypothesis]['raw'] if hypothesis else P.bodies(doc)
            self.assertIsInstance(raw['c.budget']['wrong_if'], dict)

    def test_division_by_zero_and_cycles_are_refused_without_partial_writes(self):
        for rule in ('order.price / 0', 'order.invalid + 1'):
            before = self.path.read_bytes()
            with self.assertRaises(P.Refused):
                self.add('order.invalid', {'rule': rule})
            self.assertEqual(self.path.read_bytes(), before)

    def test_date_like_or_non_numeric_rules_do_not_become_arithmetic(self):
        for nid, rule in [('order.date', '2026-10-14'), ('order.text', "order.state + ' later'")]:
            self.add(nid, {'rule': rule})
            self.assertEqual(P.bodies(self.read())[nid]['rule'], rule)

    def test_a_hypothesis_uses_its_own_values_and_the_same_normalizer(self):
        before = self.path.read_bytes()
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.path)], {'kind': 'set', 'id': 'order.price', 'value': 7, 'hypothesis': 'proposal'})
        self.add('order.total', {'rule': 'order.price * order.quantity'}, hypothesis='proposal')
        self.assertEqual(self.path.read_bytes(), before)
        doc = self.read()
        self.assertIsInstance(doc.hypotheses['proposal']['raw']['order.total']['rule'], dict)
        under = P.layered(doc, doc.hypotheses['proposal'])
        ids, jud, fields = P.infer(under)
        self.assertEqual(P.value_of(P.with_builtins(under, ids, jud, fields), ids, 'order.total'), 35)

    @unittest.skipUnless(os.name == 'posix', 'durable ingestion requires POSIX locking')
    def test_source_batch_normalizes_rules_and_snapshots_after_its_final_updates(self):
        report = {'date': '2026-09-13', 'source_quote': 'Price 40, quantity 2.', 'record_sha256': I._sha(self.path.read_bytes()),
                  'updates': [{'kind': 'set', 'id': 'order.price', 'value': 40},
                              {'kind': 'set', 'id': 'order.quantity', 'value': 2},
                              {'kind': 'add', 'id': 'order.total', 'body': {'rule': 'order.price * order.quantity'}},
                              {'kind': 'add', 'id': 'c.total', 'body': {'rests_on': ['order.total'], 'verdict': 'Fits', 'wrong_if': 'order.total > 150'}}]}
        state = self.path.parent / 'state'
        event = I.capture(report, self.path, state, start=False)
        result = I.process(self.path, state, event_id=event['event_id'])[0]
        self.assertEqual(result['state'], 'applied', result)
        raw = P.bodies(self.read())
        self.assertIsInstance(raw['order.total']['rule'], dict)
        self.assertEqual(raw['c.total']['seen']['order.total']['computed']['value'], 80)

    def test_missing_core_keeps_text_with_a_diagnostic(self):
        with patch.object(P.E, '_CORE', None), patch.object(P.E, '_core_type', side_effect=ValueError('core is not ready; run kpopper session setup')):
            out = self.add('order.total', {'rule': 'order.price * order.quantity'})
        self.assertEqual(P.bodies(self.read())['order.total']['rule'], 'order.price * order.quantity')
        self.assertIn('kept as text', out)
        self.assertIn('setup', out)

    @unittest.skipUnless(os.name == 'posix', 'durable ingestion requires POSIX locking')
    def test_batch_fallback_diagnostic_is_durable_through_commit_recovery(self):
        report = {'date': '2026-09-13', 'source_quote': 'Double the unit price.',
                  'record_sha256': I._sha(self.path.read_bytes()),
                  'updates': [{'kind': 'add', 'id': 'order.double', 'body': {'rule': 'order.price * 2'}}]}
        state = self.path.parent / 'state'
        with patch.object(P.E, '_CORE', None), patch.object(P.E, '_core_type', side_effect=ValueError('core is not ready; run kpopper session setup')):
            event = I.capture(report, self.path, state, start=False)
            with self.assertRaises(I._CrashAfterCommit):
                I.process(self.path, state, event_id=event['event_id'], _crash_after_commit=True)
            receipt = I.process(self.path, state, event_id=event['event_id'])[0]
        self.assertEqual(receipt['state'], 'applied', receipt)
        self.assertTrue(receipt['recovered'])
        self.assertIn('setup', ' '.join(receipt['diagnostics']))
        self.assertIn('kept as text', ' '.join(receipt['diagnostics']))
        self.assertEqual(receipt['signal_ids'], [])
        self.assertEqual(I.status(event['event_id'], self.path, state), receipt)
