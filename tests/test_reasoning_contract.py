"""Portable computation keeps snapshot identity, types and historical evidence."""
import copy
import unittest

from scripts.reasoning.snapshot import Snapshot
from scripts.reasoning.contract import CapabilityError, capabilities, render_value
from scripts.reasoning.language import lower


def record(value=10):
    return {
        'meta': {'reasoning': {'version': 1, 'profile': 'core/v1',
                               'requires': ['arithmetic/v1']}},
        'parameters': {'p.price': {'v': value}, 'p.quantity': {'v': 3}},
        'calculations': {'m.total': {'rule': {'expr': 'p.price * p.quantity'}}},
        'decisions': {'d.order': {'rests_on': ['m.total'],
                                  'wrong_if': {'expr': 'm.total > 100'},
                                  'seen': {'m.total': {'computed': {
                                      'value': 30, 'rule': {'expr': 'p.price * p.quantity'}}}}}},
    }


class SnapshotContractTests(unittest.TestCase):
    def test_snapshot_is_detached_from_input_and_public_copy(self):
        doc = record()
        captured = Snapshot.from_data(doc)
        before = captured.snapshot_id
        doc['parameters']['p.price']['v'] = 100
        exported = captured.to_data()
        exported['document']['parameters']['p.price']['v'] = 200
        self.assertEqual(captured.snapshot_id, before)
        self.assertEqual(captured.to_data()['document']['parameters']['p.price']['v'], 10)

    def test_equal_looking_scalar_types_do_not_share_identity(self):
        ids = {Snapshot.from_data(record(v)).snapshot_id
               for v in [False, 0, 0.0, '0', None]}
        absent = record()
        del absent['parameters']['p.price']['v']
        ids.add(Snapshot.from_data(absent).snapshot_id)
        self.assertEqual(len(ids), 6)

    def test_map_order_is_irrelevant_but_sequence_order_is_not(self):
        doc = record()
        shuffled = dict(reversed(list(doc.items())))
        self.assertEqual(Snapshot.from_data(doc).snapshot_id,
                         Snapshot.from_data(shuffled).snapshot_id)
        self.assertNotEqual(Snapshot.from_data(record([1, 2])).snapshot_id,
                            Snapshot.from_data(record([2, 1])).snapshot_id)

    def test_historical_review_is_never_substituted_for_current_value(self):
        doc = record(100)
        snapshot = Snapshot.from_data(doc).to_data()
        self.assertEqual(snapshot['document']['parameters']['p.price']['v'], 100)
        self.assertEqual(snapshot['document']['decisions']['d.order']['seen']
                         ['m.total']['computed']['value'], 30)

    def test_as_of_is_explicit_normalized_and_bound(self):
        a = Snapshot.from_data(record(), as_of='2026-09-14T12:00:00+03:00')
        b = Snapshot.from_data(record(), as_of='2026-09-14T09:00:00Z')
        self.assertEqual(a.snapshot_id, b.snapshot_id)
        self.assertNotEqual(a.snapshot_id, Snapshot.from_data(record()).snapshot_id)
        with self.assertRaises(ValueError):
            Snapshot.from_data(record(), as_of='2026-09-14T09:00:00')

    def test_nonfinite_or_recursive_input_is_rejected(self):
        for value in (float('nan'), float('inf'), float('-inf')):
            with self.assertRaises(ValueError):
                Snapshot.from_data(record(value))
        recursive = []
        recursive.append(recursive)
        with self.assertRaises(ValueError):
            Snapshot.from_data(record(recursive))


class CapabilityContractTests(unittest.TestCase):
    def test_absent_metadata_keeps_legacy_profile(self):
        doc = record()
        del doc['meta']['reasoning']
        self.assertEqual(capabilities(doc)['profile'], 'ordinary-reader/v1')
        for head in (None, 'legacy notes'):
            self.assertEqual(capabilities({'meta': head})['profile'], 'ordinary-reader/v1')

    def test_unsupported_requirements_fail_without_modifying_input(self):
        doc = record()
        doc['meta']['reasoning']['requires'].append('untrusted.example/v3')
        before = copy.deepcopy(doc)
        with self.assertRaises(CapabilityError):
            capabilities(doc)
        self.assertEqual(doc, before)

    def test_rendering_is_by_type_and_preserves_exactness(self):
        self.assertEqual(render_value({'type': 'number', 'numerator': '1', 'denominator': '3'}), '1/3')
        self.assertEqual(render_value({'type': 'boolean', 'value': False}), 'false')
        self.assertEqual(render_value({'type': 'null'}), 'null')
        self.assertEqual(render_value({'type': 'text', 'value': 'p.price'}), '"p.price"')


class ScalarGrammarTests(unittest.TestCase):
    def test_core_nesting_does_not_inherit_legacy_64_limit(self):
        expression = {'num': '0'}
        for _ in range(80):
            expression = {'op': 'add', 'args': [expression, {'num': '1'}]}
        self.assertEqual(lower(expression), expression)
        for _ in range(49):
            expression = {'op': 'add', 'args': [expression, {'num': '1'}]}
        with self.assertRaises(ValueError):
            lower(expression)

    def test_null_comparison_keeps_explicit_type(self):
        expected = {'op': 'eq', 'args': [{'ref': 'p.empty'}, {'null': True}]}
        self.assertEqual(lower({'expr': 'p.empty == null'}), expected)
        self.assertEqual(lower(expected), expected)

    def test_unicode_reference_spelling_is_not_normalized_by_python(self):
        self.assertEqual(lower({'expr': 'K.value + 0.100'}),
                         {'op': 'add', 'args': [{'ref': 'K.value'}, {'num': '0.100'}]})

    def test_host_calls_and_cyclic_trees_are_refused(self):
        with self.assertRaises(ValueError):
            lower({'expr': '__import__("os").getcwd()'})
        cycle = {'op': 'add', 'args': []}
        cycle['args'] = [cycle, {'num': '1'}]
        with self.assertRaises(ValueError):
            lower(cycle)


if __name__ == '__main__':
    unittest.main()
