"""Composition adapter boundaries; native semantics have their own runtime tests."""
import copy
import unittest
from unittest.mock import patch

import yaml

from scripts import provenance as P
from scripts.reasoning import authoring, conversion, ingestion
from scripts.reasoning.contract import COMPOSITION_LIMITS, DEFAULT_LIMITS, CapabilityError, admitted, digest
from scripts.reasoning.evaluate import Evaluator
from scripts.reasoning.language import lower, references, literal, required_modules
from scripts.reasoning.snapshot import Snapshot


def document(composed=True):
    return {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
        'requires': ['arithmetic/v1', 'composition/v1'] if composed else ['arithmetic/v1']}},
        'known': {'p.x': {'v': 2}, 'p.out': {'rule': {'expr': 'p.x + 1'}}}}


class CompositionSyntax(unittest.TestCase):
    def test_boolean_if_field_and_exact_opaque_ids(self):
        tree = lower({'expr': 'field({"x.y": [p.x, ref("שדה / one")]}, "x.y") if not a or b and c else []'})
        self.assertEqual(tree['if'], {'op': 'or', 'args': [
            {'op': 'not', 'args': [{'ref': 'a'}]}, {'op': 'and', 'args': [{'ref': 'b'}, {'ref': 'c'}]}]})
        self.assertEqual(references(tree), ['a', 'b', 'c', 'p.x', 'שדה / one'])
        self.assertEqual(tree['then']['key'], 'x.y')
        self.assertEqual(lower({'expr': 'p.x'}), {'ref': 'p.x'})
        self.assertEqual(lower({'expr': 'a and b and c'})['args'][0],
                         {'op': 'and', 'args': [{'ref': 'a'}, {'ref': 'b'}]})

    def test_rejects_host_constructs_and_malformed_tags(self):
        for text in ['field(a, b)', 'a[0]', '[x for x in xs]', '{**x}', '{1: 2}',
                     '{"a": 1, "a": 2}', 'sum([1])', '(a := 1)', 'field(a,"x", extra=1)']:
            with self.subTest(text=text), self.assertRaises((ValueError, SyntaxError)):
                lower({'expr': text})
        for tree in [{'list': {}}, {'record': {1: {'num': '2'}}},
                     {'if': {'bool': True}, 'then': {'num': '2'}},
                     {'op': 'not', 'args': [{'bool': True}, {'bool': False}]}]:
            with self.subTest(tree=tree), self.assertRaises(ValueError):
                lower(tree)

    def test_stored_empty_heterogeneous_and_recursive_literal_bounds(self):
        self.assertEqual(literal({'x': [None, True, 1, '1', [], {}]}),
            {'record': {'x': {'list': [{'null': True}, {'bool': True}, {'num': '1'},
                {'text': '1'}, {'list': []}, {'record': {}}]}}})
        value = []
        value.append(value)
        with self.assertRaises(ValueError):
            literal(value)
        for value in [{1: 'x'}, float('inf')]:
            with self.assertRaises(ValueError):
                literal(value)


class CompositionPreparation(unittest.TestCase):
    def test_scalar_request_and_basis_unchanged_by_unrelated_capability(self):
        original = Snapshot.from_data(document(False))
        extended = document(True)
        extended['known']['other'] = {'rule': {'expr': '[false, {}]'}}
        newer = Snapshot.from_data(extended)
        left, right = Evaluator(original), Evaluator(newer)
        self.assertEqual(left.basis('p.out'), right.basis('p.out'))
        request, before = left.prepare({'ref': 'p.out'}, ['p.out'])
        request2, after = right.prepare({'ref': 'p.out'}, ['p.out'])
        self.assertEqual(request, request2)
        self.assertNotIn('protocol', request)
        self.assertEqual(before['basis'], after['basis'])
        self.assertEqual(after['modules'], ['arithmetic/v1'])
        self.assertEqual(after['resource_profile'], {'version': 'resources/v2', **DEFAULT_LIMITS})
        # The exact existing scalar Merkle recipe is an independent oracle.
        atom = {'status': 'present', 'expression': {'num': '2'}}
        expected = digest({'version': 1, 'recipe': 'merkle-inputs/v1', 'profile': 'core/v1',
            'modules': ['arithmetic/v1'], 'as_of': None, 'kind': 'node', 'id': 'p.x',
            'input': atom, 'dependencies': []})
        self.assertEqual(left.basis('p.x')['fingerprint'], expected)

    def test_potential_closure_selects_kp3_even_unselected_container(self):
        doc = document()
        doc['known']['p.container'] = {'v': []}
        doc['known']['p.mid'] = {'rule': {'expr': 'p.container'}}
        engine = Evaluator(Snapshot.from_data(doc))
        request, result = engine.prepare({'expr': '1 if true else p.mid'}, ['p.mid'])
        self.assertEqual(request['protocol'], 'KP3')
        self.assertEqual(request['limits'], {**DEFAULT_LIMITS, **COMPOSITION_LIMITS})
        self.assertEqual(result['potential_ids'], ['p.container', 'p.mid'])
        self.assertEqual(result['resource_profile']['version'], 'resources/v3')
        self.assertEqual(engine.basis('p.mid')['basis']['modules'], ['arithmetic/v1', 'composition/v1'])

    def test_undeclared_composition_refuses_without_native_launch(self):
        engine = Evaluator(Snapshot.from_data(document(False)))
        with self.assertRaisesRegex(CapabilityError, 'undeclared modules'):
            engine.prepare({'list': []}, [])
        doc = document(False)
        doc['known']['p.x']['v'] = {}
        with self.assertRaisesRegex(CapabilityError, 'undeclared modules'):
            Evaluator(Snapshot.from_data(doc)).prepare({'ref': 'p.out'}, ['p.out'])
        with self.assertRaisesRegex(CapabilityError, 'must be declared'):
            authoring.validate_declared(doc)

    def test_bare_core_list_input_is_captured_without_changing_legacy_discovery(self):
        doc = document()
        doc['known']['bare'] = [None, 1, {}]
        snapshot = Snapshot.from_data(doc)
        self.assertEqual(snapshot.to_data()['nodes']['bare']['body'], [None, 1, {}])
        request, _ = Evaluator(snapshot).prepare({'ref': 'bare'}, ['bare'])
        self.assertEqual(request['protocol'], 'KP3')
        self.assertEqual(request['nodes']['bare'], literal([None, 1, {}]))
        del doc['meta']
        self.assertNotIn('known', P.collections_of(doc))

    def test_contested_composed_member_still_requires_composition(self):
        doc = document()
        snapshot = Snapshot.from_data(doc, context={'conflicts': {
            'p.x': [['alternative', {'v': []}]]}})
        request, result = Evaluator(snapshot).prepare({'ref': 'p.out'}, ['p.out'])
        self.assertEqual(request['protocol'], 'KP3')
        self.assertIn('composition/v1', result['basis']['modules'])

    def test_unrelated_capability_does_not_change_scope_summary_basis(self):
        from scripts.reasoning.assessment import dependency_result
        doc = document(False)
        doc['known']['scope'] = {'collection_scope': {'collection': 'known', 'fields': ['v']}}
        original = Snapshot.from_data(doc)
        doc['meta']['reasoning']['requires'].append('composition/v1')
        newer = Snapshot.from_data(doc)
        before, after = dependency_result(original, 'scope'), dependency_result(newer, 'scope')
        self.assertEqual(before['basis'], after['basis'])
        self.assertEqual(after['modules'], ['arithmetic/v1'])

    def test_destination_metadata_union_and_later_scalar_edit_retention(self):
        doc = document(False)
        doc['known']['p.new'] = {'v': {'k': []}}
        dest = authoring.declare_document(doc)
        self.assertEqual(dest['meta']['reasoning']['requires'], ['arithmetic/v1', 'composition/v1'])
        self.assertEqual(doc['meta']['reasoning']['requires'], ['arithmetic/v1'])
        del dest['known']['p.new']
        self.assertEqual(authoring.declaration(dest)['requires'], ['arithmetic/v1', 'composition/v1'])
        for style in (True, False):
            lines = yaml.safe_dump(doc, default_flow_style=style, sort_keys=False).splitlines()
            authoring.declare(lines, P)
            rewritten = yaml.safe_load('\n'.join(lines))
            self.assertEqual(rewritten['known'], doc['known'])
            self.assertEqual(rewritten['meta']['reasoning']['requires'], ['arithmetic/v1', 'composition/v1'])


class CompositionAdmission(unittest.TestCase):
    def test_dominated_diagnostics_are_not_unblocked_writer_evidence(self):
        world = authoring.World(P, document())
        for status, codes, blocked, allowed in [
            ('ok', [], False, True), ('ok', ['missing_reference'], False, False),
            ('ok', ['missing_reference'], True, True), ('unknown', ['missing_field'], True, True),
            ('ok', ['division_by_zero'], True, False), ('error', ['type_error'], True, False),
            ('ok', ['missing_reference', 'division_by_zero'], True, False),
            ('limit', ['step_limit'], True, False), ('operational_error', ['runtime_timeout'], True, False)]:
            result = {'status': status, 'diagnostics': [{'code': code} for code in codes]}
            with self.subTest(status=status, codes=codes, blocked=blocked):
                self.assertEqual(admitted(result, blocked=blocked), allowed)
                if allowed:
                    self.assertIs(world.require(result, blocked=blocked), result)
                else:
                    with self.assertRaises(P.Refused):
                        world.require(result, blocked=blocked)

    def test_core_dependency_admission_traverses_every_branch_and_opaque_id(self):
        doc = document()
        doc['known']['literal.dot / key'] = {'v': 1}
        world = authoring.World(P, doc)
        ids, jud, fields = P.infer(doc)
        action = {'kind': 'add', 'id': 'c.test', 'body': {'rests_on': ['p.x'],
            'wrong_if': {'expr': 'false and (ref("literal.dot / key") > 2)'}}}
        failures = P._sound_dependencies(action, doc, ids, jud, fields, world.raw)
        self.assertEqual(failures, ['wrong_if reads literal.dot / key, which the judgment does not rest on'])

    def test_conversion_checks_dominated_and_blocked_errors(self):
        for code, blocked, fails in [('missing_field', True, False), ('missing_field', False, True),
                                      ('division_by_zero', True, True)]:
            report = {'blockers': [], 'problems': [], 'comparisons': []}
            conversion._result(report, 'p.x', 'rule', {'status': 'unavailable'},
                {'status': 'ok', 'value': {'type': 'boolean', 'value': False},
                 'diagnostics': [{'code': code}]}, blocked=blocked, required=True)
            self.assertEqual(bool(report['blockers']), fails)

    def test_ingestion_applies_same_admission_to_rules_and_predicates(self):
        for field in ('rule', 'wrong_if'):
            for blocked, code, fails in [(False, 'missing_reference', True),
                (True, 'missing_field', False), (True, 'division_by_zero', True)]:
                body = {field: {'expr': 'false and missing'}}
                if field == 'wrong_if':
                    body['rests_on'] = ['missing']
                if blocked:
                    body['blocked_on'] = 'waiting for missing input'
                result = {'status': 'ok', 'value': {'type': 'boolean', 'value': False},
                          'diagnostics': [{'code': code}]}
                node = {'body': body, 'fields': {'deps': 'rests_on'},
                    'computation': result if field == 'rule' else None,
                    'state': {'integrity': {'issues': []}, 'falsifier': {
                        'status': 'does_not_hold', 'computation': result if field == 'wrong_if' else None}}}
                with self.subTest(field=field, blocked=blocked, code=code):
                    self.assertEqual(bool(ingestion._failures(P, {'nodes': {'p.x': node}})), fails)

    def test_core_container_replacement_keeps_source_admission(self):
        doc = document()
        doc['known']['p.x'] = {'v': [], 'of': '2026-09-17'}
        world = authoring.World(P, doc)
        # Admission bridge test: native result is explicitly supplied, not simulated evaluation.
        world._readings['p.x'] = {'status': 'ok', 'value': {'type': 'list', 'items': []}}
        ids, judgments, fields = P.infer(doc)
        action = {'kind': 'set', 'id': 'p.x', 'value': [1], 'as_of': '2026-09-17'}
        with patch.object(world, 'same_value', return_value=False) as compared:
            disagreement = P._disagreement(action, doc['known']['p.x'], world.raw, ids, judgments, fields)
        compared.assert_called_once_with('p.x', [1])
        self.assertIsNotNone(disagreement)
        self.assertEqual(disagreement[0], 'value')
        self.assertFalse(disagreement[3])

    def test_writer_container_decoding_preserves_native_types(self):
        value = {'type': 'record', 'fields': {'k': {'type': 'list', 'items': [
            {'type': 'boolean', 'value': True}, {'type': 'number', 'numerator': '1', 'denominator': '2'},
            {'type': 'null'}]}}}
        self.assertEqual(authoring.authored_value(value), {'k': [True, {'rational': ['1', '2']}, None]})


if __name__ == '__main__':
    unittest.main()
