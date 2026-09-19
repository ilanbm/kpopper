"""Captured what-ifs compute consequences without claiming history admission."""
import copy
import datetime
import json
import unittest
from unittest import mock

from scripts.reasoning import scenario
from scripts.reasoning.snapshot import Snapshot


class ComputationalScenario(unittest.TestCase):
    def source(self, *, value=3, head=None, as_of=None):
        document = {
            'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
                                    'requires': ['arithmetic/v1']}},
            'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
            'known': {'p.input': {'v': 1}},
            'judgments': {'d.limit': {'verdict': 'within bound', 'rests_on': ['p.input'],
                'seen': {'p.input': 1}, 'wrong_if': {'expr': 'p.input > 2'}}}}
        hypotheses = {'future': {'doc': {'meta': copy.deepcopy(document['meta']),
                                        'known': {'p.input': {'v': value}}},
            'head': head or {'folds': 'never', 'wrong_if': {'expr': 'p.input > 2'}}}}
        return Snapshot.from_data(document, hypotheses=hypotheses, as_of=as_of)

    def test_never_folding_scenario_retains_source_and_evaluates_both_conditions(self):
        source = self.source()
        before = source.to_json()
        candidate = scenario.build(source, {'future': ['p.input']})
        result = scenario.assess(candidate)
        self.assertTrue(any(line.startswith('d.limit:') for line in result['findings']['falsified']))
        self.assertIs(result['heads'][0]['truth'], True)
        self.assertEqual(result['heads'][0]['computation']['value'], {'type': 'boolean', 'value': True})
        self.assertEqual(source.to_json(), before)
        self.assertNotIn('history', candidate.to_data()['context'])
        self.assertNotIn('acceptance', result)
        self.assertEqual(Snapshot.from_json(candidate.to_json()).to_data(), candidate.to_data())

    def test_head_only_selection_does_not_replay_stale_alternative_value(self):
        source = self.source(value=0, head={'wrong_if': {'expr': 'p.input > 0'}})
        result = scenario.assess(scenario.build(source, {'future': []}))
        self.assertIs(result['heads'][0]['truth'], True)
        self.assertEqual(Snapshot.from_json(result['snapshot']).to_data()['document']['known']['p.input']['v'], 1)

    def test_replay_rederives_documents_selections_and_overlay_digests(self):
        candidate = scenario.build(self.source(), {'future': ['p.input']})
        for target in ('document', 'selection', 'overlay', 'clock'):
            data = candidate.to_data()
            if target == 'document':
                data['document']['known']['p.input']['v'] = 9
            elif target == 'selection':
                data['context']['scenario']['selection']['future'] = []
            elif target == 'overlay':
                data['context']['scenario']['overlays'][0]['digest'] = '0' * 64
            else:
                data['as_of'] = '2026-09-01'
            with self.subTest(target=target), self.assertRaises(ValueError):
                Snapshot.from_snapshot(data)

    def test_nested_or_unknown_selections_are_refused(self):
        source = self.source()
        candidate = scenario.build(source, {'future': []})
        with self.assertRaisesRegex(ValueError, 'nested'):
            scenario.build(candidate, {})
        for selected in ({'missing': []}, {'future': ['missing']}, {'future': ['p.input', 'p.input']}):
            with self.subTest(selected=selected), self.assertRaises(ValueError):
                scenario.build(source, selected)

    def test_none_and_explicit_temporal_basis_are_preserved(self):
        for as_of in (None, '2026-09-01'):
            candidate = scenario.build(self.source(as_of=as_of), {'future': []})
            self.assertEqual(candidate.to_data()['as_of'], as_of)
            self.assertEqual(Snapshot.from_json(candidate.to_json()).to_data()['as_of'], as_of)

    def test_shared_collision_never_overwrites_observed_or_hypothetical_value(self):
        source = self.source(value=9)
        data = source.to_data()
        data['document']['known']['p.input']['v'] = 7
        shared = Snapshot.from_data(data['document'])
        candidate = scenario.build(source, {'future': ['p.input']}, shared=shared)
        self.assertEqual(candidate.to_data()['context']['scenario']['collisions'], ['p.input'])
        self.assertEqual(candidate.to_data()['document']['known']['p.input']['v'], 1)

    def test_new_shared_value_is_available_to_a_hypothesis_head(self):
        source = self.source(head={'wrong_if': {'expr': 'p.shared > 2'}})
        document = copy.deepcopy(source.to_data()['document'])
        document['known'] = {'p.shared': {'v': 3}}
        document.pop('judgments')
        shared = Snapshot.from_data(document)
        result = scenario.assess(scenario.build(source, {'future': []}, shared=shared))
        self.assertIs(result['heads'][0]['truth'], True)

    def test_literal_legacy_shared_reading_keeps_its_observed_interpretation(self):
        source = self.source(head={'wrong_if': {'expr': 'p.shared > 2'}})
        shared = Snapshot.from_data({'known': {'p.shared': {'v': 3}}})
        candidate = scenario.build(source, {'future': []}, shared=shared)
        self.assertNotIn('meta', candidate.to_data()['context']['scenario']['shared']['document'])
        self.assertIs(scenario.assess(candidate)['heads'][0]['truth'], True)

    def test_executable_legacy_shared_meaning_requires_explicit_interpretation(self):
        source = self.source()
        shared = Snapshot.from_data({'known': {'p.shared': {'rule': 'p.input + 1'}}})
        with self.assertRaisesRegex(ValueError, 'shared legacy executable'):
            scenario.build(source, {}, shared=shared)

    def test_shared_core_field_roles_cannot_change_silently(self):
        source = self.source()
        document = copy.deepcopy(source.to_data()['document'])
        document['schema'] = {'deps': 'uses', 'snapshot': 'saw', 'predicate': 'bad_if'}
        document['known'] = {'p.shared': {'v': 3}}
        document['judgments'] = {'d.shared': {'verdict': 'bad', 'uses': ['p.shared'],
            'saw': {'p.shared': 3}, 'bad_if': {'expr': 'p.shared > 2'}}}
        with self.assertRaisesRegex(ValueError, 'shared field roles'):
            scenario.build(source, {}, shared=Snapshot.from_data(document))

    def test_legacy_shared_judgment_is_not_reclassified_as_a_literal(self):
        shared = Snapshot.from_data({
            'schema': {'deps': 'uses', 'snapshot': 'saw', 'predicate': 'bad_if'},
            'known': {'p.shared': {'v': 3}},
            'judgments': {'d.shared': {'verdict': 'x', 'uses': ['p.shared'], 'saw': {'p.shared': 3}}}})
        with self.assertRaisesRegex(ValueError, 'not a literal reading'):
            scenario.build(self.source(), {}, shared=shared)

    def test_date_valued_scenario_result_is_json_safe_and_replayable(self):
        data = self.source().to_data()
        day = datetime.date(2026, 9, 1)
        data['document']['known']['p.date'] = {'v': day}
        source = Snapshot.from_data(data['document'], hypotheses=data['hypotheses'])
        result = scenario.assess(scenario.build(source, {'future': []}))
        persisted = json.loads(json.dumps(result))
        replay = Snapshot.from_json(persisted['snapshot'])
        self.assertEqual(replay.to_data()['document']['known']['p.date']['v'], day)

    def test_cyclic_or_deep_replay_is_rejected_before_budget_walk(self):
        candidate = scenario.build(self.source(), {'future': []})
        cyclic = candidate.to_data()
        cyclic['context']['scenario']['cycle'] = cyclic
        deep = candidate.to_data()
        cursor = deep['context']['scenario']
        for _ in range(130):
            cursor['deeper'] = {}
            cursor = cursor['deeper']
        for data in (cyclic, deep):
            with self.subTest(cyclic=data is cyclic), \
                 mock.patch.object(scenario.OutputBudget, 'add', side_effect=AssertionError('budget walked')), \
                 self.assertRaises(ValueError):
                Snapshot.from_snapshot(data)

    def test_full_scenario_must_fit_transport_before_return(self):
        source = self.source(value='x' * 4000)
        # Evidence alone fits this boundary; document and nodes repeat the value.
        with mock.patch('scripts.reasoning.snapshot.MAX_REQUEST_BYTES', 10000):
            with self.assertRaises(ValueError):
                scenario.build(source, {'future': ['p.input']})

    def test_constructor_and_replay_do_not_read_sources_or_allocate_time(self):
        source = self.source()
        with mock.patch('pathlib.Path.read_bytes', side_effect=AssertionError('source read')), \
             mock.patch('scripts.history_store.Store.capture', side_effect=AssertionError('capture')):
            candidate = scenario.build(source, {'future': ['p.input']})
            self.assertEqual(Snapshot.from_snapshot(candidate.to_data()).to_data(), candidate.to_data())

    def test_executable_errors_remain_unknown(self):
        source = self.source(head={'wrong_if': {'expr': '1 / 0 > 2'}})
        result = scenario.assess(scenario.build(source, {'future': []}))
        self.assertIsNone(result['heads'][0]['truth'])
        self.assertNotEqual(result['heads'][0]['computation']['status'], 'ok')

    def test_uninterpreted_physical_expressions_are_not_promoted(self):
        data = self.source().to_data()
        data['hypotheses']['future']['document']['known']['p.input'] = {'rule': 'p.other + 1'}
        source = Snapshot.from_data(data['document'], hypotheses=data['hypotheses'])
        with self.assertRaisesRegex(ValueError, 'uninterpreted'):
            scenario.build(source, {'future': ['p.input']})

    def test_undeclared_structured_hypothesis_body_or_head_is_not_promoted(self):
        for body, head in (({'rule': {'expr': 'p.input + 1'}}, {}),
                           ({'v': 3}, {'wrong_if': {'expr': 'p.input > 2'}})):
            data = self.source().to_data()
            data['hypotheses']['future']['document'].pop('meta')
            data['hypotheses']['future']['document']['known']['p.input'] = body
            data['hypotheses']['future']['head'] = head
            source = Snapshot.from_data(data['document'], hypotheses=data['hypotheses'])
            with self.subTest(body=body), self.assertRaisesRegex(ValueError, 'legacy hypothesis'):
                scenario.build(source, {'future': ['p.input']})

    def test_undeclared_nonexecutable_hypothesis_judgment_is_not_reclassified(self):
        data = self.source().to_data()
        data['hypotheses'] = {'legacy': {'doc': {'judgments': {
            'd.extra': {'verdict': 'safe', 'rests_on': ['p.input'], 'seen': {'p.input': 1}}}}, 'head': {}}}
        source = Snapshot.from_data(data['document'], hypotheses=data['hypotheses'])
        with self.assertRaisesRegex(ValueError, 'not a literal reading'):
            scenario.build(source, {'legacy': ['d.extra']})


if __name__ == '__main__':
    unittest.main()
