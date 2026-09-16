"""Core writes preserve executable intent and typed review evidence."""
import contextlib
import copy
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import yaml
from scripts import provenance as P
from scripts.reasoning.assessment import assess
from scripts.reasoning.snapshot import Snapshot


@unittest.skipUnless(os.name == 'posix', 'record writers require POSIX locks')
class CoreAuthoring(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name) / 'GROUNDING.yaml'
        self.doc = {'sources': {'s.report': {'file': 'report.md'}}, 'known': {
            'p.price': {'v': 20, 'from': 's.report'},
            'p.quantity': {'v': 5, 'from': 's.report'}}}
        self.save(self.doc)
        (self.path.parent / 'report.md').write_text('Price 20, quantity 5.')

    def save(self, doc):
        self.path.write_text(yaml.safe_dump(doc, sort_keys=False), encoding='utf-8')

    def read(self):
        return yaml.safe_load(self.path.read_text(encoding='utf-8'))

    def write(self, kind, nid, **values):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            P.apply([str(self.path)], {'kind': kind, 'id': nid, **values})
        return output.getvalue()

    def test_explicit_profile_stores_intent_without_legacy_executor(self):
        with patch.object(P.E, 'compute', side_effect=AssertionError('legacy evaluation')):
            self.write('add', 'p.total', body={'rule': 'p.price * p.quantity'}, profile='core/v1')
        doc = self.read()
        self.assertEqual(doc['meta']['reasoning'], {
            'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']})
        self.assertEqual(P.bodies(doc)['p.total']['rule'], {'expr': 'p.price * p.quantity'})
        with self.assertRaisesRegex(P.Refused, 'core/v1 consumer'):
            P.load([str(self.path)])

    def test_declared_profile_reviews_typed_basis_and_uses_recording_date_only(self):
        self.write('add', 'p.ratio', body={'rule': '1 / 3'}, profile='core/v1')
        with patch.object(P.E, 'compute', side_effect=AssertionError('legacy evaluation')):
            self.write('add', 'c.ratio', body={'rests_on': ['p.ratio'], 'verdict': 'Fits',
                'wrong_if': 'p.ratio > 1'}, as_of='2026-09-16')
            first = copy.deepcopy(P.bodies(self.read())['c.ratio']['seen'])
            self.write('review', 'c.ratio', as_of='2026-09-17')
        seen = P.bodies(self.read())['c.ratio']['seen']
        self.assertEqual(first, seen)
        history = seen['p.ratio']['computed']
        self.assertEqual(history['version'], 2)
        self.assertEqual(history['value'], {'type': 'number', 'numerator': '1', 'denominator': '3'})
        self.assertIsNone(history['basis']['as_of'])
        report = assess(Snapshot.capture([str(self.path)], read_mode='frozen'), ['c.ratio'])
        dep = report['nodes']['c.ratio']['state']['basis']['dependencies']['p.ratio']
        self.assertEqual((dep['comparison'], dep['basis_comparison']), ('same', 'same'))

    def test_runtime_failure_refuses_without_storing_prose(self):
        from scripts.reasoning.runtime import RuntimeUnavailable
        before = self.path.read_bytes()
        with patch('scripts.reasoning.runtime.Runtime.request_many', side_effect=RuntimeUnavailable('broken package')):
            with self.assertRaisesRegex(P.Refused, 'runtime_unavailable'):
                self.write('add', 'p.total', body={'rule': 'p.price * p.quantity'}, profile='core/v1')
        self.assertEqual(before, self.path.read_bytes())

    def test_promotion_refuses_legacy_executable_fields_without_changes(self):
        for field in ('rule', 'wrong_if'):
            doc = copy.deepcopy(self.doc)
            doc['known']['p.old'] = {field: 'p.price > 10'}
            self.save(doc)
            before = self.path.read_bytes()
            with self.assertRaisesRegex(P.Refused, 'requires.*migration'):
                self.write('add', 'p.total', body={'rule': 'p.price * p.quantity'}, profile='core/v1')
            self.assertEqual(before, self.path.read_bytes())

    def test_null_zero_false_have_distinct_successful_history(self):
        self.write('add', 'p.null', body={'rule': {'null': True}}, profile='core/v1')
        self.write('add', 'p.zero', body={'v': 0})
        self.write('add', 'p.false', body={'v': False})
        self.write('add', 'c.values', body={'rests_on': ['p.null', 'p.zero', 'p.false'],
                                          'verdict': 'Recorded', 'reopened_by': 'New evidence'})
        seen = P.bodies(self.read())['c.values']['seen']
        self.assertEqual([seen[n]['computed']['value']['type'] for n in ('p.null', 'p.zero', 'p.false')],
                         ['null', 'number', 'boolean'])

    def test_core_builtin_and_born_broken_refuse_before_mutation(self):
        self.write('add', 'p.ratio', body={'rule': '1 / 3'}, profile='core/v1')
        for body, reason in [({'rule': {'expr': 'graph.flagged + 1'}}, 'unsupported_core_builtin'),
                ({'rests_on': ['p.ratio'], 'verdict': 'Too large', 'wrong_if': 'p.ratio < 1'}, 'born broken')]:
            before = self.path.read_bytes()
            with self.assertRaisesRegex(P.Refused, reason):
                self.write('add', 'c.invalid', body=body)
            self.assertEqual(before, self.path.read_bytes())

    def test_cli_profile_is_an_action_option(self):
        with contextlib.redirect_stdout(io.StringIO()):
            P.write_command('add', ['p.total', 'rule=p.price * p.quantity', '--profile', 'core/v1', str(self.path)])
        self.assertEqual(self.read()['meta']['reasoning']['profile'], 'core/v1')

    def test_scope_review_binds_membership_even_without_fields(self):
        self.write('add', 'scope.inputs', body={'collection_scope': {'collection': 'known', 'fields': []}},
                   profile='core/v1')
        self.write('add', 'c.scope', body={
            'rests_on': ['scope.inputs'], 'verdict': 'Reviewed inputs', 'reopened_by': 'Membership changes'})
        before = self.read()
        snapshot = P.bodies(before)['c.scope']['seen']['scope.inputs']['computed']
        self.assertEqual(snapshot['basis']['recipe'], 'scope-inputs/v2')
        after = copy.deepcopy(before)
        after['known']['p.replacement'] = after['known'].pop('p.quantity')
        report = assess(Snapshot.from_data(after), ['c.scope'])
        dep = report['nodes']['c.scope']['state']['basis']['dependencies']['scope.inputs']
        self.assertEqual(dep['comparison'], 'same')
        self.assertEqual(dep['basis_comparison'], 'changed')

    def test_named_hypothesis_has_its_own_core_world(self):
        self.write('add', 'p.total', body={'rule': 'p.price * p.quantity'}, profile='core/v1')
        before = self.path.read_bytes()
        self.write('set', 'p.price', value=7, hypothesis='proposal')
        self.write('add', 'c.total', body={'rests_on': ['p.total'], 'verdict': 'Fits',
            'wrong_if': 'p.total > 50'}, hypothesis='proposal')
        with contextlib.redirect_stdout(io.StringIO()):
            from scripts.reasoning.authoring import load
            doc = load(P, [str(self.path)])
        hyp = doc.hypotheses['proposal']
        history = P.bodies(hyp['doc'])['c.total']['seen']['p.total']['computed']
        self.assertEqual(history['value'], {'type': 'number', 'numerator': '35', 'denominator': '1'})
        self.assertEqual(self.path.read_bytes(), before)

    def test_unknown_shard_requirement_cannot_be_masked(self):
        self.doc['meta'] = {'reasoning': {'version': 99, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}}
        self.doc['also'] = 'later.yaml'
        self.save(self.doc)
        (self.path.parent / 'later.yaml').write_text(yaml.safe_dump({'meta': {'reasoning': {
            'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}}}))
        before = self.path.read_bytes()
        with self.assertRaisesRegex(P.Refused, 'unsupported_capability'):
            self.write('add', 'p.total', body={'rule': 'p.price * p.quantity'}, profile='core/v1')
        self.assertEqual(before, self.path.read_bytes())
        self.assertFalse(P._CORE_READS.get())

    def test_unrelated_write_keeps_versionless_history_unchanged(self):
        self.doc['meta'] = {'reasoning': {'version': 1, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}}
        self.doc['judgments'] = {'c.old': {'rests_on': ['p.price'], 'verdict': 'Known',
            'seen': {'p.price': 20}, 'wrong_if': {'expr': 'p.price > 100'}}}
        self.save(self.doc)
        old = copy.deepcopy(self.doc['judgments'])
        self.write('add', 'p.total', body={'rule': 'p.price * p.quantity'})
        self.assertEqual(self.read()['judgments'], old)

    def test_set_preserves_null_and_numeric_looking_text_types(self):
        self.write('add', 'p.null', body={'v': None}, profile='core/v1')
        self.write('set', 'p.null', value=0)
        self.write('add', 'p.text', body={'v': '1'})
        self.write('set', 'p.text', value=1)
        raw = P.bodies(self.read())
        self.assertIs(type(raw['p.null']['v']), int)
        self.assertIs(type(raw['p.text']['v']), int)

    def test_history_budget_refusal_keeps_original_bytes(self):
        self.write('add', 'p.total', body={'rule': 'p.price * p.quantity'}, profile='core/v1')
        before = self.path.read_bytes()
        from scripts.reasoning.authoring import World
        snapshot = World.history
        def oversized(world, *args, **kwargs):
            result = snapshot(world, *args, **kwargs)
            world.bounds['output_bytes'] = 1
            return result
        with patch.object(World, 'history', oversized):
            with self.assertRaisesRegex(ValueError, 'output_limit'):
                self.write('add', 'c.total', body={'rests_on': ['p.total'], 'verdict': 'Fits',
                    'wrong_if': 'p.total > 200'})
        self.assertEqual(before, self.path.read_bytes())


if __name__ == '__main__':
    unittest.main()
