"""The focused exporter and computed rules share reader semantics after integration."""
import copy
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import export_graph as X


class ExportExpressions(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.record = Path(self.tmp.name) / 'GROUNDING.yaml'
        self.rule = {'expr': 'order.price * order.quantity'}
        self.doc = {'sources': {'s.report': {'file': 'report.md'}}, 'known': {
            'order.price': {'v': 20, 'from': 's.report'},
            'order.quantity': {'v': 5, 'from': 's.report'},
            'order.total': {'rule': copy.deepcopy(self.rule)}}, 'judgments': {
                'c.budget': {'rests_on': ['order.total'], 'verdict': 'Fits',
                    'wrong_if': {'expr': 'order.total > 150'},
                    'seen': {'order.total': {'computed': {'value': 100, 'rule': copy.deepcopy(self.rule)}}}}}}
        self.save()

    def save(self):
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False), encoding='utf-8')

    def packet(self, *ids, **kwargs):
        return X.project([str(self.record)], list(ids), **kwargs)

    def require_core(self):
        try: X.P.E._core_type()()
        except (ValueError, ImportError):
            if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1': raise
            self.skipTest('structured calculation checks need the Lean core')

    def test_readable_and_ast_rules_keep_support_and_impact_links(self):
        for rule in (self.rule, X.P.E.lower(self.rule)):
            with self.subTest(rule=rule):
                self.doc['known']['order.total']['rule'] = rule
                self.doc['known']['order.label'] = {'rule': {'expr': '"order.price"'}}
                self.save()
                support = self.packet('order.total')
                self.assertIn(('order.total', 'rule_reads', 'order.price'), support['edges'])
                self.assertIn(('order.total', 'rule_reads', 'order.quantity'), support['edges'])
                impact = self.packet('order.price', direction='impact', depth=2)
                self.assertIn('c.budget', impact['nodes'])
                self.assertNotIn('order.label', impact['nodes'])

    def test_unavailable_calculation_exports_unknown_without_throwing_or_rewriting(self):
        before = self.record.read_bytes()
        with patch.object(X.P.E, '_CORE', None), patch.object(X.P.E, '_core_type', side_effect=ValueError('core is not ready')):
            packet = self.packet('c.budget')
            text = X.render_markdown(packet)
        row = packet['nodes']['c.budget']['readings'][0]
        self.assertEqual(row['current_status'], 'unavailable')
        self.assertIsNone(packet['nodes']['c.budget']['condition']['result'])
        self.assertIn('UNKNOWN:', text)
        self.assertIn('not evaluated', text)
        self.assertEqual(self.record.read_bytes(), before)

    def test_explicit_missing_rule_input_remains_an_edge_or_boundary(self):
        expression = {'expr': 'missing.input + 1'}
        self.doc['judgments'] = {}
        for rule in (expression, X.P.E.lower(expression)):
            with self.subTest(rule=rule):
                self.doc['known']['order.total']['rule'] = rule
                self.save()
                packet = self.packet('order.total')
                self.assertTrue(packet['nodes']['missing.input']['missing'])
                self.assertIn(('order.total', 'rule_reads', 'missing.input'), packet['edges'])
                self.assertIn('MISSING', X.render_mermaid(packet))
                bounded = self.packet('order.total', depth=0)
                self.assertEqual(bounded['frontier_edges'], 1)
                self.assertNotIn('missing.input', bounded['nodes'])

    def test_computed_history_and_current_result_are_distinct_and_comparable(self):
        self.require_core()
        self.doc['known']['order.quantity']['v'] = 10
        self.save(); before = self.record.read_bytes()
        packet = self.packet('c.budget', depth=2)
        row = packet['nodes']['c.budget']['readings'][0]
        self.assertEqual(row['at_review'], self.doc['judgments']['c.budget']['seen']['order.total'])
        self.assertEqual(row['current'], 200)
        self.assertEqual(row['comparison'], 'crossed')
        text = X.render_markdown(packet)
        self.assertIn('100 (calculated)', text)
        self.assertIn('200 (calculated)', text)
        self.assertIn('order.price \\* order.quantity', text)
        self.assertNotIn('"expr":', text)
        self.assertIn('"expr":', X.render_markdown(packet, details=True))
        self.assertEqual(self.record.read_bytes(), before)

    def test_formula_change_with_same_value_is_not_reported_unchanged(self):
        self.require_core()
        self.doc['known']['order.total']['rule'] = {'expr': 'order.price * 5'}
        self.save()
        packet = self.packet('c.budget')
        self.assertEqual(packet['nodes']['c.budget']['readings'][0]['comparison'], 'moved')
        self.assertIn('MOVED', X.render_markdown(packet))
        self.assertIn('formula changed', X.render_markdown(packet))

    def test_unchanged_exact_computed_snapshot_compares_equal(self):
        self.require_core()
        self.doc['known']['order.price']['v'] = 0.1
        self.doc['known']['order.quantity']['v'] = 3
        self.doc['judgments']['c.budget']['seen']['order.total']['computed']['value'] = {'rational': ['3', '10']}
        self.save()
        packet = self.packet('c.budget')
        row = packet['nodes']['c.budget']['readings'][0]
        self.assertEqual(row['comparison'], 'same')
        self.assertEqual(row['current'], {'rational': ['3', '10']})
        self.assertIn('3/10 (calculated)', X.render_markdown(packet))

    def test_legacy_formula_history_is_not_a_historical_numeric_reading(self):
        self.require_core()
        self.doc['judgments']['c.budget']['seen']['order.total'] = 'order.price * order.quantity'
        self.save()
        packet = self.packet('c.budget')
        row = packet['nodes']['c.budget']['readings'][0]
        self.assertEqual(row['at_review'], 'order.price * order.quantity')
        self.assertEqual(row['comparison'], 'formula_only')
        self.assertIn('no historical result', X.render_markdown(packet))

    def test_changed_legacy_formula_keeps_movement_and_missing_numeric_history(self):
        self.require_core()
        self.doc['judgments']['c.budget']['seen']['order.total'] = 'order.price * order.quantity'
        self.doc['known']['order.total']['rule'] = {'expr': 'order.price * 5'}
        self.save()
        packet = self.packet('c.budget')
        row = packet['nodes']['c.budget']['readings'][0]
        self.assertEqual(row['comparison'], 'moved')
        text = X.render_markdown(packet)
        self.assertIn('formula changed', text)
        self.assertIn('no historical result', text)

    def test_depth_zero_does_not_leak_computed_current_values(self):
        self.require_core()
        self.doc['known']['order.quantity']['v'] = 98765
        self.save()
        packet = self.packet('c.budget', depth=0)
        row = packet['nodes']['c.budget']['readings'][0]
        self.assertEqual(row['current_status'], 'omitted')
        self.assertNotIn('current', row)
        self.assertNotIn('1975300', X.render_markdown(packet))


if __name__ == '__main__': unittest.main()
