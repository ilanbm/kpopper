"""Input identities preserve complete computational dependencies without rescans."""
import copy
import unittest
from unittest.mock import patch

from scripts.reasoning.basis import InputBasis
from scripts.reasoning import language


def num(value):
    return {'num': str(value)}


def ref(node):
    return {'ref': node}


def op(name, left, right):
    return {'op': name, 'args': [left, right]}


def data(bodies, *, conflicts=None, as_of=None):
    return {'document': {'meta': {'reasoning': {
        'version': 1, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}}},
        'nodes': {key: {'body': copy.deepcopy(body), 'collection': 'inputs',
                        'fields': {'deps': 'based_on', 'snapshot': 'reviewed_values',
                                   'predicate': 'reject_if'}} for key, body in bodies.items()},
        'context': {'conflicts': copy.deepcopy(conflicts or {})}, 'as_of': as_of}


class InputBasisTests(unittest.TestCase):
    def test_cancelling_inputs_and_changed_rule_change_identity(self):
        original = data({'a': {'v': 1}, 'b': {'v': 2},
                         'sum': {'rule': op('add', ref('a'), ref('b'))}})
        changed = copy.deepcopy(original)
        changed['nodes']['a']['body']['v'] = 2
        changed['nodes']['b']['body']['v'] = 1
        first, second = InputBasis(original), InputBasis(changed)
        self.assertNotEqual(first.fingerprint('sum'), second.fingerprint('sum'))
        changed['nodes']['sum']['body']['rule'] = op('sub', ref('a'), ref('b'))
        self.assertNotEqual(second.fingerprint('sum'), InputBasis(changed).fingerprint('sum'))
        self.assertEqual([x['id'] for x in first.dependencies(['sum'])], ['a', 'b', 'sum'])

    def test_review_support_assessment_and_incidental_dates_are_not_inputs(self):
        original = data({'a': {'v': 4}, 'out': {'rule': ref('a')}})
        changed = copy.deepcopy(original)
        changed['nodes']['a']['body'].update({
            'seen': {'a': 99}, 'reviewed_values': {'a': 100}, 'based_on': ['unrelated'],
            'current_assessment': {'status': 'changed'}, 'read': '2099-01-01',
            'observed_at': '2099-01-01T00:00:00Z'})
        changed['nodes']['a']['assessment'] = {'status': 'anything'}
        self.assertEqual(InputBasis(original).fingerprint('out'), InputBasis(changed).fingerprint('out'))

    def test_transitive_fingerprints_and_unrelated_component_stability(self):
        original = data({'leaf': {'v': 1}, 'middle': {'rule': ref('leaf')},
                         'out': {'rule': ref('middle')}, 'other': {'v': 5}})
        first = InputBasis(original)
        changed = copy.deepcopy(original)
        changed['nodes']['leaf']['body']['v'] = 2
        second = InputBasis(changed)
        for node in ['leaf', 'middle', 'out']:
            self.assertNotEqual(first.fingerprint(node), second.fingerprint(node))
        self.assertEqual(first.fingerprint('other'), second.fingerprint('other'))
        extended = copy.deepcopy(original)
        extended['nodes']['unrelated'] = {'body': {'rule': ref('unrelated')}}
        self.assertEqual(first.fingerprint('out'), InputBasis(extended).fingerprint('out'))
        witnesses = {x['id']: x['fingerprint'] for x in first.dependencies(['out'])}
        self.assertEqual(witnesses['middle'], first.fingerprint('middle'))

    def test_missing_inputs_and_descendant_unavailability_are_explicit(self):
        index = InputBasis(data({'out': {'rule': ref('missing')}, 'healthy': {'v': 2}}))
        basis = index.basis('out')
        self.assertEqual(basis['status'], 'unavailable')
        self.assertIn('missing_reference', basis['diagnostics'])
        self.assertEqual([x['id'] for x in basis['dependencies']], ['missing', 'out'])
        self.assertEqual(index.basis('healthy')['status'], 'available')
        self.assertEqual(index.basis('not-in-graph')['status'], 'unavailable')
        fixed = InputBasis(data({'out': {'rule': ref('missing')}, 'missing': {'v': None}, 'healthy': {'v': 2}}))
        self.assertEqual(fixed.basis('out')['status'], 'available')
        self.assertNotEqual(index.fingerprint('out'), fixed.fingerprint('out'))

    def test_contested_variants_keep_base_and_variant_reference_targets(self):
        bodies = {'a': {'v': 1}, 'b': {'v': 2}, 'claim': {'rule': ref('a')},
                  'out': {'rule': ref('claim')}}
        conflicts = {'claim': [['right', {'rule': ref('b')}], ['left', {'v': 9}]]}
        first = InputBasis(data(bodies, conflicts=conflicts))
        self.assertEqual([x['id'] for x in first.dependencies(['out'])], ['a', 'b', 'claim', 'out'])
        self.assertEqual(first.basis('out')['status'], 'unavailable')
        self.assertIn('contested', first.basis('out')['diagnostics'])
        reordered = {'claim': list(reversed(conflicts['claim']))}
        self.assertEqual(first.fingerprint('out'), InputBasis(data(bodies, conflicts=reordered)).fingerprint('out'))
        bodies['b']['v'] = 3
        self.assertNotEqual(first.fingerprint('out'), InputBasis(data(bodies, conflicts=conflicts)).fingerprint('out'))
        conflicts['claim'][0][1]['seen'] = {'b': 3}
        changed = InputBasis(data(bodies, conflicts=conflicts))
        del conflicts['claim'][0][1]['seen']
        self.assertEqual(changed.fingerprint('out'), InputBasis(data(bodies, conflicts=conflicts)).fingerprint('out'))

    def test_conflicted_entry_without_a_base_retains_alternative_inputs(self):
        bodies = {'source': {'v': 1}, 'out': {'rule': ref('new')}}
        conflicts = {'new': [['one', {'rule': ref('source')}], ['two', {'v': 4}]]}
        first = InputBasis(data(bodies, conflicts=conflicts))
        self.assertEqual([x['id'] for x in first.dependencies(['out'])], ['new', 'out', 'source'])
        self.assertEqual(first.basis('new')['diagnostics'], ['contested', 'missing_reference'])
        bodies['source']['v'] = 2
        self.assertNotEqual(first.fingerprint('out'), InputBasis(data(bodies, conflicts=conflicts)).fingerprint('out'))

    def test_cycles_are_canonical_and_unavailable_without_breaking_an_edge(self):
        bodies = {'a': {'rule': op('add', ref('b'), ref('external'))},
                  'b': {'rule': ref('a')}, 'external': {'v': 1},
                  'out': {'rule': ref('a')}, 'healthy': {'v': 7}}
        first = InputBasis(data(bodies))
        second = InputBasis(data(dict(reversed(list(bodies.items())))))
        for node in ['a', 'b', 'out']:
            self.assertEqual(first.fingerprint(node), second.fingerprint(node))
            self.assertEqual(first.basis(node)['status'], 'unavailable')
            self.assertIn('cyclic_reference', first.basis(node)['diagnostics'])
        self.assertNotEqual(first.fingerprint('a'), first.fingerprint('b'))
        self.assertEqual(first.dependencies(['b', 'a']), first.dependencies(['a', 'b', 'a']))
        self.assertEqual([x['id'] for x in first.dependencies(['out'])], ['a', 'b', 'external', 'out'])
        bodies['external']['v'] = 2
        changed = InputBasis(data(bodies))
        self.assertNotEqual(first.fingerprint('a'), changed.fingerprint('a'))
        self.assertNotEqual(first.fingerprint('out'), changed.fingerprint('out'))
        self.assertEqual(first.fingerprint('healthy'), changed.fingerprint('healthy'))
        self_cycle = InputBasis(data({'self': {'rule': ref('self')}}))
        self.assertIn('cyclic_reference', self_cycle.basis('self')['diagnostics'])

    def test_invalid_rule_is_local_and_changes_are_bound(self):
        first = InputBasis(data({'bad': {'rule': {'garbage': 1}}, 'out': {'rule': ref('bad')}, 'ok': {'v': 1}}))
        second = InputBasis(data({'bad': {'rule': {'garbage': 2}}, 'out': {'rule': ref('bad')}, 'ok': {'v': 1}}))
        self.assertEqual(first.basis('out')['status'], 'unavailable')
        self.assertIn('invalid_expression', first.basis('out')['diagnostics'])
        self.assertNotEqual(first.fingerprint('out'), second.fingerprint('out'))
        self.assertEqual(first.fingerprint('ok'), second.fingerprint('ok'))

    def test_input_and_returned_values_are_detached(self):
        original = data({'a': {'v': 1}, 'out': {'rule': ref('a')}})
        index = InputBasis(original)
        expected = index.fingerprint('out')
        original['nodes']['a']['body']['v'] = 999
        basis = index.basis('out')
        basis['dependencies'][0]['fingerprint'] = 'forged'
        basis['basis']['modules'].append('forged/v1')
        self.assertEqual(index.fingerprint('out'), expected)
        self.assertNotEqual(index.dependencies(['out'])[0]['fingerprint'], 'forged')
        self.assertEqual(index.basis('out')['basis']['modules'], ['arithmetic/v1'])

    def test_as_of_and_recipe_are_bound(self):
        first = InputBasis(data({'a': {'v': 1}}))
        second = InputBasis(data({'a': {'v': 1}}, as_of='2026-09-15'))
        self.assertNotEqual(first.fingerprint('a'), second.fingerprint('a'))
        self.assertEqual(first.basis('a')['basis']['recipe'], 'merkle-inputs/v1')
        self.assertEqual(first.basis('a')['basis']['profile'], 'core/v1')

    def test_chain_indexes_each_local_expression_once_and_reads_do_not_rescan(self):
        nodes = {'n0': {'v': 1}, **{f'n{i}': {'rule': ref(f'n{i-1}')} for i in range(1, 1500)}}
        with patch.object(language, 'node_expression', wraps=language.node_expression) as normalize:
            index = InputBasis(data(nodes))
            count = normalize.call_count
            self.assertEqual(count, len(nodes))
            for node in nodes:
                self.assertEqual(len(index.fingerprint(node)), 64)
            self.assertEqual(normalize.call_count, count)
            self.assertEqual(len(index.dependencies(['n1499'])), 1500)
            self.assertEqual(normalize.call_count, count)
        self.assertEqual(index.basis('n1499')['status'], 'available')
