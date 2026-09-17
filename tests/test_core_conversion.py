"""Captured profile conversion preserves evidence and reports semantic differences."""
import copy
import datetime
import json
import unittest
from unittest.mock import patch

from scripts import pending_grounding as G, provenance as P
from scripts.reasoning import conversion as C
from scripts.reasoning.snapshot import Snapshot
from scripts.reasoning.runtime import RuntimeUnavailable


def document():
    return {'schema': {'deps': 'rests_on', 'snapshot': 'observed', 'predicate': 'fails_when'},
            'known': {'p.input': {'v': 3}, 'p.output': {'rule': 'p.input / 3'}},
            'judgments': {'j.safe': {'rests_on': ['p.output'], 'fails_when': 'p.output < 0',
                'observed': {'p.output': {'computed': {'value': 1, 'rule': {'expr': 'p.input / 3'}}}},
                'because': 'Keep this original explanation'}}}


def convert(doc):
    return C.convert_snapshot(Snapshot.from_data(doc), reader=P)


class ConversionTests(unittest.TestCase):
    def test_text_conversion_uses_final_world_and_keeps_original_history(self):
        doc = document()
        original = copy.deepcopy(doc)
        result = convert(doc)
        self.assertTrue(result['complete'], result['blockers'])
        self.assertEqual(result['document']['known']['p.output']['rule'], {'expr': 'p.input / 3'})
        self.assertEqual(result['document']['judgments']['j.safe']['fails_when'], {'expr': 'p.output < 0'})
        self.assertEqual(result['document']['judgments']['j.safe']['observed'], original['judgments']['j.safe']['observed'])
        self.assertEqual(doc, original)
        self.assertTrue(any(item['status'] == 'newly_executable' for item in result['comparisons']))

    def test_absent_legacy_runtime_does_not_change_representation_or_prove_equivalence(self):
        doc = document()
        result = convert(doc)
        unavailable = {'values': {}, 'predicate': {'holds_on_current_values': None}, 'error': 'checked core unavailable'}
        with patch.object(P.E, 'compute', return_value=unavailable):
            other = convert(doc)
        self.assertEqual(result['document'], other['document'])
        self.assertEqual(other['legacy_runtime']['status'], 'unavailable')
        self.assertNotIn('equivalent', {item['status'] for item in other['comparisons']})

    def test_shared_explicit_tree_survives_without_legacy_runtime(self):
        doc = document()
        doc['known']['p.output']['rule'] = {'op': 'div', 'args': [{'ref': 'p.input'}, {'num': '3'}]}
        doc['judgments']['j.safe']['fails_when'] = {'op': 'lt', 'args': [{'ref': 'p.output'}, {'num': '0'}]}
        unavailable = {'values': {}, 'predicate': {'holds_on_current_values': None}, 'error': 'checked core unavailable'}
        with patch.object(P.E, 'compute', return_value=unavailable):
            result = convert(doc)
        self.assertTrue(result['complete'], result['blockers'])
        self.assertEqual(result['document']['known'], doc['known'])
        self.assertTrue(any(item['status'] == 'comparison_unavailable' for item in result['comparisons']))

    def test_ambiguous_coercive_and_builtin_fields_block_complete_conversion(self):
        for expression in ('p.input < threshold', 'p.input == "3"', 'graph.entries > 0'):
            doc = document()
            doc['judgments']['j.safe']['fails_when'] = expression
            with self.subTest(expression=expression):
                result = convert(doc)
                self.assertFalse(result['complete'])
                self.assertTrue(result['blockers'])
        doc = document()
        doc['known']['p.input']['v'] = '3'
        self.assertFalse(convert(doc)['complete'])

    def test_qualitative_prose_and_unknown_fields_are_preserved(self):
        doc = document()
        doc['known']['p.output'] = {'rule': 'estimated from the interviews', 'custom': {'untouched': [1, 2]}}
        doc['judgments']['j.safe']['fails_when'] = 'a better explanation emerges'
        doc['judgments']['j.safe']['rests_on'] = []
        result = convert(doc)
        self.assertTrue(result['complete'], result['blockers'])
        self.assertEqual(result['document']['known']['p.output'], doc['known']['p.output'])
        self.assertEqual(len(result['preserved']), 2)

    def test_formula_in_value_is_moved_to_rule(self):
        doc = document()
        doc['known']['p.output'] = {'v': 'p.input / 3'}
        result = convert(doc)
        self.assertTrue(result['complete'], result['blockers'])
        self.assertEqual(result['document']['known']['p.output'], {'rule': {'expr': 'p.input / 3'}})
        self.assertTrue(any(change['field'] == 'v' and change['target'] == 'rule' for change in result['changes']))

    def test_newly_fired_condition_is_explicit_without_refreshing_history(self):
        doc = document()
        doc['judgments']['j.safe']['fails_when'] = 'p.output > 0'
        result = convert(doc)
        self.assertTrue(result['complete'], result['blockers'])
        self.assertEqual(result['fired'], ['j.safe'])
        self.assertEqual(result['document']['judgments']['j.safe']['observed'], doc['judgments']['j.safe']['observed'])

    def test_known_values_cannot_change_type_value_or_availability(self):
        for before, after in ((True, 1), ('3', 3), (3, 4)):
            source = {'known': {'p.value': {'v': before}}}
            candidate = copy.deepcopy(source)
            candidate['meta'] = {'reasoning': copy.deepcopy(C.DECLARATION)}
            candidate['known']['p.value']['v'] = after
            result = C.compare_snapshots(Snapshot.from_data(source), Snapshot.from_data(candidate), reader=P)
            with self.subTest(before=before, after=after):
                self.assertFalse(result['complete'])
                self.assertIn('result_changed', {item['code'] for item in result['blockers']})

    def test_legacy_null_quote_fallback_is_not_silently_lost(self):
        result = convert({'known': {'p.value': {'v': None, 'quoted': 'recorded quote'}}})
        self.assertFalse(result['complete'])
        self.assertIn('result_changed', {item['code'] for item in result['blockers']})

    def test_missing_input_requires_explicit_hole(self):
        doc = {'known': {'p.value': {'rule': {'ref': 'p.absent'}}}}
        self.assertFalse(convert(doc)['complete'])
        doc['known']['p.value']['blocked_on'] = 'Await p.absent measurement'
        self.assertTrue(convert(doc)['complete'])

    def test_broken_core_runtime_is_a_blocker_without_prose_fallback(self):
        with patch('scripts.reasoning.evaluate.Runtime.request_many', side_effect=RuntimeUnavailable('test unavailable')):
            result = convert(document())
        self.assertFalse(result['complete'])
        self.assertEqual(result['document']['known']['p.output']['rule'], {'expr': 'p.input / 3'})
        self.assertIn('operational_error', {item['code'] for item in result['blockers']})

    def test_typed_export_preserves_dates_and_detaches_input(self):
        doc = document()
        doc['sources'] = {'s.measured': {'read': datetime.date(2026, 9, 16), 'custom': None}}
        result = convert(doc)
        restored = G._decode(json.loads(C.report_to_json(result)))
        self.assertEqual(restored, result)
        self.assertIsInstance(restored['document']['sources']['s.measured']['read'], datetime.date)

    def test_required_missing_judgment_dependency_needs_declared_hole(self):
        doc = {'judgments': {'j.wait': {'rests_on': ['p.absent'],
            'seen': {}, 'wrong_if': 'a better explanation emerges'}}}
        self.assertFalse(convert(doc)['complete'])
        doc['judgments']['j.wait']['blocked_on'] = 'Await measurement'
        self.assertTrue(convert(doc)['complete'])

    def test_comparison_refuses_changed_history_and_field_roles(self):
        source = Snapshot.from_data(document())
        candidate = convert(document())['document']
        candidate['judgments']['j.safe']['observed'] = {}
        result = C.compare_snapshots(source, Snapshot.from_data(candidate), reader=P)
        self.assertIn('history_changed', {item['code'] for item in result['blockers']})
        candidate = convert(document())['document']
        candidate['schema']['snapshot'] = 'other'
        result = C.compare_snapshots(source, Snapshot.from_data(candidate), reader=P)
        self.assertIn('field_roles_changed', {item['code'] for item in result['blockers']})

    def test_known_legacy_condition_change_blocks_and_preserves_false(self):
        doc = {'known': {'p.input': {'v': 1}}, 'judgments': {'j.test': {
            'rests_on': ['p.input'], 'seen': {'p.input': 1}, 'wrong_if': 'p.input > 2'}}}
        source = Snapshot.from_data(doc)
        candidate = convert(doc)['document']
        candidate['judgments']['j.test']['wrong_if'] = {'expr': 'p.input < 2'}
        result = C.compare_snapshots(source, Snapshot.from_data(candidate), reader=P)
        self.assertFalse(result['complete'])
        comparison = next(item for item in result['comparisons'] if item['field'] == 'wrong_if')
        self.assertEqual(comparison['before']['value'], {'type': 'boolean', 'value': False})

    def test_zero_boolean_null_and_text_remain_distinct(self):
        doc = {'known': {'p.zero': {'v': 0}, 'p.false': {'v': False},
                         'p.null': {'v': None}, 'p.text': {'v': '0'}}}
        result = convert(doc)
        self.assertTrue(result['complete'], result['blockers'])
        values = {item['id']: item['after']['value'] for item in result['comparisons']}
        self.assertEqual(len({G.identity(value) for value in values.values()}), 4)

    def test_invalid_scope_and_unsupported_literal_block_complete_conversion(self):
        docs = [
            {'scopes': {'scope.items': {'collection_scope': {'collection': 'missing', 'fields': []}}}},
            {'known': {'p.list': {'v': [1, 2]}}},
        ]
        for doc in docs:
            with self.subTest(doc=doc):
                self.assertFalse(convert(doc)['complete'])

    def test_supported_scope_conversion_captures_typed_summary(self):
        doc = {'items': {}, 'scopes': {'scope.items': {'collection_scope': {'collection': 'items', 'fields': []}}}}
        result = convert(doc)
        self.assertTrue(result['complete'], result['blockers'])
        comparison = next(item for item in result['comparisons'] if item['id'] == 'scope.items')
        self.assertEqual(comparison['after']['value']['fields']['member_count']['numerator'], '0')

    def test_shared_structured_native_comparison_is_exact_when_available(self):
        doc = {'known': {'p.input': {'v': 10}, 'p.output': {'rule': {
            'op': 'div', 'args': [{'ref': 'p.input'}, {'num': '3'}]}}}}
        result = convert(doc)
        self.assertTrue(result['complete'], result['blockers'])
        output = next(item for item in result['comparisons'] if item['id'] == 'p.output')
        self.assertEqual(output['after']['value'], {'type': 'number', 'numerator': '10', 'denominator': '3'})
        if result['legacy_runtime']['status'] == 'available':
            self.assertEqual(output['status'], 'same')
            self.assertEqual(output['before']['value'], output['after']['value'])
        else:
            self.assertEqual(output['status'], 'comparison_unavailable')

    def test_core_scope_with_dependency_role_compares_its_captured_summary(self):
        from scripts.reasoning.assessment import assess, dependency_result
        doc = {'meta': {'reasoning': {**C.DECLARATION, 'version': 1}},
            'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
            'items': {'p.one': {'v': 1}}, 'scopes': {'scope.items': {
                'collection_scope': {'collection': 'items', 'fields': ['v']}, 'rests_on': []}}}
        source = Snapshot.from_data(doc)
        self.assertIsNone(assess(source)['nodes']['scope.items']['computation'])
        self.assertEqual(dependency_result(source, 'scope.items')['status'], 'ok')
        result = C.convert_snapshot(source, reader=P)
        self.assertTrue(result['complete'], result['blockers'])
        scope = next(item for item in result['comparisons'] if item['id'] == 'scope.items')
        self.assertEqual(scope['status'], 'same')
        self.assertEqual(scope['before']['value'], scope['after']['value'])
        self.assertEqual(result['document']['meta']['reasoning']['version'], 2)

    def test_generated_scope_history_stays_exact_and_requires_its_declared_format(self):
        from scripts.reasoning.authoring import World
        doc = {'meta': {'reasoning': copy.deepcopy(C.DECLARATION)}, 'items': {},
            'scopes': {'scope.items': {'collection_scope': {'collection': 'items', 'fields': []}}}}
        history = World(P, doc).history('scope.items', {}, None)
        doc['judgments'] = {'j.review': {'rests_on': ['scope.items'], 'seen': {'scope.items': history},
                                       'wrong_if': 'new evidence emerges'}}
        result = convert(doc)
        self.assertTrue(result['complete'], result['blockers'])
        self.assertEqual(result['document']['judgments']['j.review']['seen'], {'scope.items': history})
        doc['meta']['reasoning']['version'] = 1
        invalid = convert(doc)
        self.assertFalse(invalid['complete'])
        self.assertIn('invalid_capability', {item['code'] for item in invalid['blockers']})

    def test_dates_are_preserved_as_source_metadata_but_not_retyped_scalar_inputs(self):
        date = datetime.date(2026, 9, 16)
        source = convert({'sources': {'s.read': {'read': date}}})
        self.assertTrue(source['complete'], source['blockers'])
        self.assertEqual(source['document']['sources']['s.read']['read'], date)
        scalar = convert({'known': {'p.date': {'v': date}}})
        self.assertFalse(scalar['complete'])
        self.assertEqual(scalar['document']['known']['p.date']['v'], date)
        self.assertIn('unsupported_value', {item['code'] for item in scalar['blockers']})

    def test_no_live_recapture_is_allowed(self):
        snapshot = Snapshot.from_data(document())
        with patch.object(Snapshot, 'capture', side_effect=AssertionError('recapture')):
            result = C.convert_snapshot(snapshot, reader=P)
        self.assertTrue(result['complete'], result['blockers'])

    def test_unknown_capability_and_unsupported_tree_are_blockers(self):
        for declaration, rule in (({'version': 9, 'profile': 'future/v1', 'requires': []}, {'num': '1'}),
                                  (None, {'op': 'future', 'args': []})):
            doc = {'known': {'p.value': {'rule': rule}}}
            if declaration:
                doc['meta'] = {'reasoning': declaration}
            self.assertFalse(convert(doc)['complete'])


if __name__ == '__main__':
    unittest.main()
