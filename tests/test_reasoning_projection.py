"""Focused findings and scoped module reads do not expose unrelated evidence."""
import copy
import json
import unittest
from unittest.mock import patch

from scripts.reasoning.assessment import assess
from scripts.reasoning import contract
from scripts.reasoning.projection import (
    computation_truth, predicate_truth, project_findings, project_impacts,
    project_node_status, project_witnesses, render_expression, render_value,
)
from scripts.reasoning.snapshot import Snapshot
try:
    from test_reasoning_assessment import UnavailableRuntime
except ImportError:  # Support both discovery -s tests and package-style targeting.
    from tests.test_reasoning_assessment import UnavailableRuntime


class ProjectionTests(unittest.TestCase):
    def test_scope_read_projects_alternative_bodies_to_granted_fields(self):
        doc = {'items': {'a': {'v': 0, 'secret': 'base-private'}},
               'scopes': {'scope.items': {'collection_scope': {'collection': 'items', 'fields': ['v']}}}}
        variants = [['one', {'v': 1, 'secret': 'hidden-one'}],
                    ['two', {'v': 2, 'secret': 'hidden-two'}]]
        snapshot = Snapshot.from_data(doc, context={'conflicts': {'a': variants}})
        rows = snapshot.capture_scope('scope.items').view().read_scope('scope.items', 'v')
        self.assertNotIn('secret', json.dumps(rows))
        self.assertEqual(snapshot.to_data()['context']['conflicts']['a'], variants)
        self.assertEqual([body['v'] for _, body in rows[0]['alternatives']], [1, 2])

    def test_selected_assessment_binds_but_does_not_print_unrelated_bodies(self):
        doc = {'items': {'a': {'v': 1}, 'b': {'v': 2}}}
        secret = 'unrelated-private-value'
        context = {'conflicts': {'b': [['other', {'v': secret}]]},
                   'pending': {'ref': 'ledger', 'bundles': {
                       'revision': {'manifest': {'document': {'items': {'b': {'v': secret}}},
                                                'roots': ['b'], 'scope': {'kind': 'project'}},
                                    'files': {'private.txt': secret}}}}}
        hypotheses = {'h': {'doc': {'items': {'b': {'v': secret}}}, 'head': {'claim': secret}}}
        snapshot = Snapshot.from_data(doc, context=context, hypotheses=hypotheses)
        report = assess(snapshot, ['a'], runtime=UnavailableRuntime())
        self.assertNotIn(secret, json.dumps(report))
        self.assertEqual(report['snapshot_id'], snapshot.snapshot_id)
        self.assertIn(secret, json.dumps(snapshot.to_data()))

    def test_generic_typed_values_delegate_to_the_contract(self):
        values = [
            {'type': 'null'},
            {'type': 'boolean', 'value': False},
            {'type': 'text', 'value': 'שלום "world"'},
            {'type': 'list', 'items': [
                {'type': 'number', 'numerator': '1', 'denominator': '3'},
                {'type': 'boolean', 'value': True},
                {'type': 'null'},
            ]},
            {'type': 'record', 'fields': {
                'z': {'type': 'text', 'value': 'last'},
                'a': {'type': 'list', 'items': []},
            }},
        ]
        for value in values:
            with self.subTest(value=value):
                self.assertEqual(render_value(value), contract.render_value(value))
        self.assertEqual(render_value(values[-1]), '{"a": [], "z": "last"}')

    def test_scalar_and_composition_expressions_render_without_evaluation(self):
        binary = {
            'add': '+', 'sub': '-', 'mul': '*', 'div': '/',
            'eq': '==', 'ne': '!=', 'lt': '<', 'le': '<=', 'gt': '>', 'ge': '>=',
            'and': 'and', 'or': 'or',
        }
        for operator, symbol in binary.items():
            with self.subTest(operator=operator):
                left = {'bool': True} if operator in ('and', 'or') else {'num': '1'}
                right = {'bool': False} if operator in ('and', 'or') else {'num': '2'}
                self.assertEqual(render_expression({'op': operator, 'args': [left, right]}),
                                 '(' + render_expression(left) + ' ' + symbol + ' ' +
                                 render_expression(right) + ')')
        cases = [
            ({'num': '1.25'}, '1.25'),
            ({'ref': 'price.current'}, 'price.current'),
            ({'ref': 'item-with-hyphen'}, 'ref("item-with-hyphen")'),
            ({'ref': 'if'}, 'ref("if")'),
            ({'ref': 'None'}, 'ref("None")'),
            ({'op': 'add', 'args': [{'num': '1'}, {'ref': 'x'}]}, '(1 + x)'),
            ({'op': 'ge', 'args': [{'ref': 'x'}, {'num': '2'}]}, '(x >= 2)'),
            ({'op': 'and', 'args': [
                {'bool': True}, {'op': 'not', 'args': [{'bool': False}]},
            ]}, '(true and (not false))'),
            ({'if': {'ref': 'ready'}, 'then': {'text': 'yes'}, 'else': {'null': True}},
             '("yes" if ready else null)'),
            ({'field': {'record': {'b': {'num': '2'}, 'a': {'list': [
                {'null': True}, {'bool': False}, {'text': 'x'},
            ]}}}, 'key': 'a'},
             'field({"a": [null, false, "x"], "b": 2}, "a")'),
            ({'expr': ' [value, {"ok": true}] if enabled else null '},
             '[value, {"ok": true}] if enabled else null'),
        ]
        for expression, expected in cases:
            with self.subTest(expression=expression):
                self.assertEqual(render_expression(expression), expected)

    def test_unknown_and_error_are_never_projected_as_false(self):
        self.assertIs(predicate_truth('holds'), True)
        self.assertIs(predicate_truth('does_not_hold'), False)
        for status in ('unknown', 'error', 'not_declared', 'not_applicable'):
            self.assertIsNone(predicate_truth(status))
        for status in ('unknown', 'error', 'operational_error'):
            self.assertIsNone(computation_truth({'status': status, 'value': None}))
        self.assertIs(computation_truth({'status': 'ok', 'value': {
            'type': 'boolean', 'value': False}}), False)

        node = {'acceptance': {'status': 'accepted'}, 'state': {
            'basis': {'status': 'assessed'},
            'falsifier': {'status': 'error'},
            'contention': {'status': 'none_detected'},
            'integrity': {'status': 'assessed'},
        }, 'coverage': {'status': 'complete'}, 'computation': {
            'status': 'unknown', 'value': None,
            'assurance': {'kind': 'computed'},
        }}
        status = project_node_status(node)
        self.assertIsNone(status['falsifier']['holds'])
        self.assertIsNone(status['computation']['truth'])
        self.assertEqual(status['acceptance'], 'accepted')

    def test_status_projection_tolerates_canonical_or_state_nested_dimensions(self):
        canonical = {'acceptance': {'status': 'accepted'},
                     'coverage': {'complete': False},
                     'assurance': {'kind': 'computed'},
                     'computation': {'status': 'unknown', 'value': None},
                     'state': {'falsifier': {'status': 'unknown'}}}
        nested = {'state': {
            'acceptance': {'status': 'accepted'},
            'coverage': {'complete': False},
            'assurance': {'kind': 'computed'},
            'computation': {'status': 'unknown', 'value': None},
            'falsifier': {'status': 'unknown'},
        }}
        self.assertEqual(project_node_status(canonical), project_node_status(nested))
        self.assertEqual(project_node_status(canonical)['coverage'], 'incomplete')

    def test_status_projection_reads_canonical_v3_coverage_and_assurance(self):
        node = {'acceptance': {'status': 'accepted', 'head_ids': ['h'], 'open_act_ids': []},
                'coverage': {'included': True, 'complete': True, 'findings': []},
                'assurance': {
                    'actual': {
                        'node': None,
                        'falsifier': {'kind': 'computed', 'formal_scope': []},
                        'dependencies': {'p.input': {'kind': 'computed', 'formal_scope': []}},
                    },
                    'recorded_evidence_kinds': ['review', 'pin', 'review'],
                },
                'state': {'falsifier': {'status': 'does_not_hold'}}}
        status = project_node_status(node)
        self.assertEqual(status['coverage'], 'complete')
        self.assertIs(status['coverage_included'], True)
        self.assertEqual(status['assurance'], 'computed')
        self.assertEqual(status['recorded_evidence_kinds'], ['pin', 'review'])

    def test_potential_and_executed_witnesses_remain_distinct(self):
        computation = {
            'potential_dependencies': [
                {'id': 'chosen', 'fingerprint': 'a'},
                {'id': 'unchosen', 'fingerprint': 'b'},
            ],
            'potential_ids': ['chosen', 'unchosen'],
            'executed_reads': [{'id': 'chosen', 'fingerprint': 'a'}],
        }
        projected = project_witnesses(computation)
        self.assertEqual(projected['executed'], ['chosen'])
        self.assertEqual(projected['unexecuted'], ['unchosen'])
        self.assertEqual(projected['dependencies'], [
            {'id': 'chosen', 'classification': 'executed'},
            {'id': 'unchosen', 'classification': 'potential'},
        ])

        nodes = {'decision': {'computation': {
            'potential_ids': ['decision'], 'executed_reads': ['decision']},
            'state': {'falsifier': {
                'status': 'does_not_hold', 'computation': computation}}}}
        self.assertEqual(project_impacts(nodes), [
            {'from': 'chosen', 'to': 'decision', 'classification': 'executed',
             'witnesses': [{'context': 'falsifier', 'classification': 'executed'}]},
            {'from': 'unchosen', 'to': 'decision', 'classification': 'potential',
             'witnesses': [{'context': 'falsifier', 'classification': 'potential'}]},
        ])

    def test_projection_uses_findings_not_clipping_or_attention(self):
        computation = {'status': 'ok', 'value': {'type': 'boolean', 'value': True},
                       'potential_ids': ['a', 'b'], 'executed_reads': ['a'],
                       'assurance': {'kind': 'computed'}}
        node = {'acceptance': {'status': 'accepted'}, 'coverage': {'status': 'complete'},
                'computation': computation, 'state': {
                    'basis': {'status': 'assessed'},
                    'falsifier': {'status': 'unknown'},
                    'contention': {'status': 'none_detected'},
                    'integrity': {'status': 'assessed'},
                }}
        first = {'snapshot_id': 'snapshot', 'findings_revision': 'findings',
                 'display_selection': ['node'], 'nodes': {'node': {
                     **node, 'attention': [{'action': 'review'}]}}}
        clipped = {'snapshot_id': 'snapshot', 'findings_revision': 'findings',
                   'display_selection': [], 'clipped': True, 'nodes': {'node': {
                       **node, 'attention': []}}}
        self.assertEqual(project_findings(first), project_findings(clipped))
        self.assertEqual(project_findings(first)['nodes']['node']['dependencies'], [
            {'id': 'a', 'classification': 'executed',
             'witnesses': [{'context': 'value', 'classification': 'executed'}]},
            {'id': 'b', 'classification': 'potential',
             'witnesses': [{'context': 'value', 'classification': 'potential'}]},
        ])

    def test_projection_is_source_free_and_does_not_mutate_findings(self):
        report = {'snapshot_id': 'snapshot', 'findings_revision': 'findings', 'nodes': {
            'node': {'computation': {'status': 'unknown', 'value': None,
                                     'potential_ids': [], 'executed_reads': []},
                     'state': {'falsifier': {'status': 'error'}}}}}
        before = copy.deepcopy(report)
        with patch('builtins.open', side_effect=AssertionError('projection must not read sources')):
            project_findings(report)
            render_expression({'op': 'add', 'args': [{'num': '1'}, {'num': '2'}]})
        self.assertEqual(report, before)


if __name__ == '__main__':
    unittest.main()
