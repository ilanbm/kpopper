"""Typed review history and scope evidence share one core dependency boundary."""
import copy
import datetime
import json
from pathlib import Path
import unittest
from unittest import mock

from scripts.reasoning import assessment as A
from scripts.reasoning.contract import CapabilityError, capabilities, digest
from scripts.reasoning.evaluate import Evaluator, compare_basis
from scripts.reasoning.snapshot import Snapshot, SnapshotError


def document(value=3):
    return {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
            'schema': {'deps': 'rests_on', 'snapshot': 'reviewed', 'predicate': 'wrong_if'},
            'items': {'p': {'v': value}, 'q': {'rule': {'expr': 'p / 3'}}},
            'scopes': {'scope.items': {'collection_scope': {'collection': 'items', 'fields': []}}},
            'judgments': {'j': {'rests_on': ['q'], 'reviewed': {}}}}


def reviewed(doc, dep):
    snapshot = Snapshot.from_data(doc)
    result = A.dependency_result(snapshot, dep)
    if result['status'] != 'ok':
        raise AssertionError(result)
    envelope = {'version': 2, 'value': result['value'], 'basis': result['basis']}
    body = snapshot.to_data()['nodes'][dep]['body']
    if isinstance(body, dict) and 'rule' in body:
        envelope['rule'] = copy.deepcopy(body['rule'])
    doc['judgments']['j']['rests_on'] = [dep]
    doc['judgments']['j']['reviewed'] = {dep: {'computed': envelope}}
    return envelope


def finding(doc, dep):
    return A.assess(Snapshot.from_data(doc), ['j'])['nodes']['j']['state']['basis']['dependencies'][dep]


class TypedHistoryTests(unittest.TestCase):
    def test_absent_history_does_not_make_capabilities_validate_legacy_schema(self):
        self.assertEqual(capabilities({'schema': 'legacy source text'})['profile'], 'ordinary-reader/v1')

    def test_scope_history_retains_member_ids_without_projected_fields(self):
        history = reviewed(document(), 'scope.items')
        self.assertEqual(history['basis']['members'], ['p', 'q'])

    def test_metadata_v1_and_v2_keep_same_core_value_and_basis(self):
        doc = document()
        v2 = Evaluator(Snapshot.from_data(doc)).evaluate({'ref': 'q'}, declared=['q'])
        doc['meta']['reasoning']['version'] = 1
        v1 = Evaluator(Snapshot.from_data(doc)).evaluate({'ref': 'q'}, declared=['q'])
        self.assertEqual(v2['status'], 'ok')
        self.assertEqual(v1['value'], v2['value'])
        self.assertEqual(v1['basis'], v2['basis'])

    def test_generated_history_requires_declared_format_two(self):
        for version in (1, None):
            doc = document()
            doc['judgments']['j']['reviewed'] = {'q': {'computed': {
                'version': 2, 'value': {'type': 'null'}, 'basis': {}}}}
            if version is None:
                del doc['meta']
            else:
                doc['meta']['reasoning']['version'] = version
            with self.subTest(version=version), self.assertRaises(CapabilityError):
                capabilities(doc, profile='core/v1')

    def test_scalar_null_boolean_text_zero_and_exact_rational_history(self):
        for value in (None, False, '3', 0, {'rational': ['1', '3']}):
            doc = document()
            dep = 'p'
            if isinstance(value, dict):
                doc['items']['p'] = {'rule': {'expr': '1 / 3'}}
            else:
                doc['items']['p'] = {'v': value}
            with self.subTest(value=value):
                envelope = reviewed(doc, dep)
                result = finding(doc, dep)
                self.assertEqual(result['comparison'], 'same')
                self.assertEqual(result['basis_comparison'], 'same')
                self.assertEqual(result['current']['value'], envelope['value'])

    def test_versionless_legacy_history_still_decodes_exact_rationals(self):
        doc = document()
        doc['items']['q']['rule'] = {'expr': '1 / 3'}
        doc['judgments']['j']['reviewed'] = {'q': {'computed': {
            'value': {'rational': ['1', '3']}, 'rule': {'expr': '1 / 3'}}}}
        result = finding(doc, 'q')
        self.assertEqual(result['comparison'], 'same')
        self.assertEqual(result['basis_comparison'], 'not_recorded')

    def test_unknown_and_malformed_history_are_unavailable(self):
        for change in ({'version': 3}, {'version': True}, {'value': {'type': 'number', 'numerator': '1', 'denominator': '0'}}, {'extra': True}):
            doc = document()
            envelope = reviewed(doc, 'q')
            envelope.update(change)
            with self.subTest(change=change):
                result = finding(doc, 'q')
                self.assertEqual(result['comparison'], 'unknown')
                self.assertEqual(result['at_review']['status'], 'unavailable')
                self.assertEqual(result['basis_comparison'], 'unavailable')

    def test_same_value_transitive_change_retains_complete_basis(self):
        doc = document()
        doc['items']['q']['rule'] = {'expr': 'p * 0'}
        envelope = reviewed(doc, 'q')
        self.assertEqual([item['id'] for item in envelope['basis']['dependencies']], ['p', 'q'])
        doc['items']['p']['v'] = 4
        result = finding(doc, 'q')
        self.assertEqual(result['comparison'], 'same')
        self.assertEqual(result['basis_comparison'], 'changed')
        self.assertEqual(result['at_review']['value']['computed']['basis'], envelope['basis'])

    def test_altered_historical_basis_is_unavailable(self):
        doc = document()
        envelope = reviewed(doc, 'q')
        envelope['basis']['dependencies'] = []
        result = finding(doc, 'q')
        self.assertEqual(result['at_review']['status'], 'unavailable')
        self.assertEqual(result['basis_comparison'], 'unavailable')
        self.assertEqual(result['comparison'], 'unknown')

    def test_malformed_legacy_envelope_is_not_successful_null(self):
        for computed in ({}, [], None):
            doc = document(None)
            doc['judgments']['j']['rests_on'] = ['p']
            doc['judgments']['j']['reviewed'] = {'p': {'computed': computed}}
            result = finding(doc, 'p')
            self.assertEqual(result['comparison'], 'unknown')
            self.assertEqual(result['at_review']['status'], 'unavailable')

    def test_unknown_record_format_is_refused(self):
        doc = document()
        doc['meta']['reasoning']['version'] = 3
        with self.assertRaises(CapabilityError):
            capabilities(doc)

    def test_scope_review_compares_basis_even_when_member_count_is_equal(self):
        doc = document()
        historical = reviewed(doc, 'scope.items')
        self.assertEqual(finding(doc, 'scope.items')['basis_comparison'], 'same')
        doc['items']['replacement'] = doc['items'].pop('q')
        result = finding(doc, 'scope.items')
        self.assertEqual(result['comparison'], 'same')
        self.assertEqual(result['basis_comparison'], 'changed')
        self.assertEqual(historical['basis']['recipe'], 'scope-inputs/v2')
        self.assertEqual(historical['basis']['version'], 1)
        self.assertEqual(historical['basis']['fields'], [])
        self.assertIn('membership_digest', historical['basis']['witness'])

    def test_old_valid_scope_basis_is_retained_but_not_equated(self):
        doc = document()
        envelope = reviewed(doc, 'scope.items')
        old = {key: value for key, value in envelope['basis'].items()
               if key not in ('recipe', 'witness', 'fields', 'digest')}
        old['digest'] = digest(old)
        envelope['basis'] = old
        result = finding(doc, 'scope.items')
        self.assertEqual(result['basis_comparison'], 'changed')
        self.assertEqual(result['at_review']['value']['computed']['basis'], old)

    def test_review_date_does_not_change_default_semantic_as_of(self):
        for dep in ('q', 'scope.items'):
            doc = document()
            envelope = reviewed(doc, dep)
            doc['judgments']['j']['reviewed_at'] = datetime.date(2026, 9, 17)
            self.assertEqual(finding(doc, dep)['basis_comparison'], 'same')
            result = A.dependency_result(Snapshot.from_data(doc, as_of='2026-09-17'), dep)
            self.assertEqual(compare_basis(result['basis'], envelope['basis']), 'changed')

    def test_rule_equality_uses_core_lowering(self):
        doc = document()
        doc['items']['p']['v'] = None
        doc['items']['q']['rule'] = {'expr': 'p == null'}
        reviewed(doc, 'q')
        doc['items']['q']['rule'] = {'op': 'eq', 'args': [{'ref': 'p'}, {'null': True}]}
        with mock.patch.object(A.E, 'same', side_effect=AssertionError('legacy equality')):
            self.assertFalse(finding(doc, 'q')['rule_changed'])

    def test_scope_success_and_failure_use_existing_result_schema(self):
        import jsonschema
        schema = json.loads((Path(__file__).parents[1] / 'scripts/reasoning/assessment.schema.json').read_text())
        doc = document()
        snapshot = Snapshot.from_data(doc)
        with mock.patch.object(Evaluator, 'evaluate', side_effect=AssertionError('arithmetic scope ref')):
            result = A.dependency_result(snapshot, 'scope.items')
        jsonschema.validate(result, {'$defs': schema['$defs'], '$ref': '#/$defs/result'})
        jsonschema.validate(A.assess(snapshot, ['scope.items']), schema)
        for code in ('invalid_scope', 'scope_unavailable', 'limit'):
            with mock.patch.object(Snapshot, 'capture_scope', side_effect=SnapshotError(code)):
                result = A.dependency_result(snapshot, 'scope.items')
            self.assertEqual(result['diagnostics'], [{'code': code, 'related_ids': ['scope.items']}])
            self.assertEqual(result['status'], 'limit' if code == 'limit' else 'error')
            jsonschema.validate(result, {'$defs': schema['$defs'], '$ref': '#/$defs/result'})

    def test_reused_evaluator_cannot_observe_a_different_snapshot(self):
        before = Snapshot.from_data(document(3))
        after = Snapshot.from_data(document(4))
        with self.assertRaises(ValueError):
            A.dependency_result(after, 'q', Evaluator(before))


if __name__ == '__main__':
    unittest.main()
