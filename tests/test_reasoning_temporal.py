"""Temporal applicability uses retained exact snapshots and one evaluator."""
import copy
import unittest
from unittest import mock

from scripts import history_adapter as HA, history_authoring as A, history_contract as HC
from scripts.pending_grounding import identity
from scripts.reasoning import context as Context
from scripts.reasoning import history_assessment as V3
from scripts.reasoning.contract import digest
from scripts.reasoning.evaluate import Evaluator
from scripts.reasoning.snapshot import Snapshot
from tests import test_history_authoring as authoring_fixtures


class TemporalApplicability(unittest.TestCase):
    template = {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
                                      'requires': ['arithmetic/v1']}}}
    def fixture(self):
        fixture = authoring_fixtures.Authoring(
            'test_set_creates_claim_and_explicit_accept_without_mutating_original')
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        return fixture

    @staticmethod
    def publish(fixture, action):
        mutation = A.prepare(fixture.entry, action, by='writer')
        A.commit(fixture.entry, mutation, verify=lambda data: None)
        return mutation

    def history(self, applicability):
        fixture = self.fixture()
        first = self.publish(fixture, {'kind': 'add', 'id': 'p.ready', 'into': 'judgments',
            'as_of': None, 'body': {'verdict': 'ready', 'rests_on': ['p.input'],
                'wrong_if': {'expr': 'p.input > 5'},
                'temporal': {'version': 1, 'applicability': applicability}}})
        self.publish(fixture, {'kind': 'set', 'id': 'p.input', 'value': 10,
                               'as_of': '2026-09-18'})
        self.publish(fixture, {'kind': 'set', 'id': 'p.input', 'value': 1,
                               'as_of': '2026-09-19'})
        return fixture, first

    def test_current_recovers_but_retains_exact_counterexample_and_roundtrips(self):
        fixture, first = self.history('current')
        adapted = HA.from_store_capture(fixture.store.capture())
        self.assertIn(HC.TEMPORAL_APPLICABILITY, adapted.projection['requires'])
        initial = first.to_data()['receipt']['after']['temporal_replay']
        self.assertIsNone(__import__('scripts.reasoning.snapshot', fromlist=['Snapshot'])
                          .Snapshot.from_json(initial['snapshot']).to_data()['as_of'])
        context = Context.CapturedAssessment.from_snapshot(adapted.snapshot(as_of='2026-09-20'))
        with mock.patch.object(V3.base_assessment, 'assess',
                               side_effect=AssertionError('native replayed during deserialize')):
            restored = Context.CapturedAssessment.from_json(context.to_json())
        temporal = restored.assessment['nodes']['p.ready']['temporal']
        self.assertEqual(temporal['status'], 'recovered')
        self.assertTrue(any(item['outcome'] == 'counterexample'
                            and item['verification'] == 'verified'
                            and item['as_of'] is None
                            for item in temporal['episodes']))
        self.assertTrue(all(item['anchors']['at'] is None
                            and item['anchors']['applies'] is None
                            and isinstance(item['anchors']['on'], str)
                            for item in temporal['episodes']))
        projected = restored.view['nodes']['p.ready']['status']['temporal']
        self.assertEqual(projected['status'], 'recovered')

    def test_anchored_and_general_keep_counterexample_attention(self):
        for applicability in ('anchored', 'general'):
            with self.subTest(applicability=applicability):
                fixture, _ = self.history(applicability)
                report = V3.assess(HA.from_store_capture(fixture.store.capture()).snapshot())
                temporal = report['nodes']['p.ready']['temporal']
                self.assertEqual(temporal['status'], 'counterexample')
                self.assertEqual(report['nodes']['p.ready']['acceptance']['status'], 'accepted')
                self.assertEqual(report['nodes']['p.ready']['state']['falsifier']['status'],
                                 'does_not_hold')
                self.assertTrue(temporal['counterexample_claim_ids'])
                self.assertIn('historical_counterexample', {
                    reason['code'] for action in report['nodes']['p.ready']['attention']
                    for reason in action['reasons']})

    def test_missing_replay_is_unknown_and_malformed_metadata_refuses(self):
        fixture = self.fixture()
        malformed = {'verdict': 'ready', 'rests_on': ['p.input'],
                     'wrong_if': {'expr': 'p.input > 5'},
                     'temporal': {'version': 2, 'applicability': 'current'}}
        with self.assertRaisesRegex(HC.HistoryError, 'invalid_temporal_metadata'):
            HC.make_object(subject='p.bad', kind='judgment', by='writer', on='unknown',
                operation='bad', body=malformed, pins={'p.input': fixture.original['id']},
                authored={'collection': 'judgments', 'fields': {
                    'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
                    'profile': 'core/v1'})
        self.publish(fixture, {'kind': 'add', 'id': 'p.ready', 'into': 'judgments',
            'body': {'verdict': 'ready', 'rests_on': ['p.input'],
                'wrong_if': {'expr': 'p.input > 5'},
                'temporal': {'version': 1, 'applicability': 'anchored'}}})
        adapted = HA.from_store_capture(fixture.store.capture())
        projection = adapted.projection
        projection['temporal'] = {'version': 1, 'complete': False, 'observations': [],
            'findings': [{'code': 'temporal_replay_unavailable', 'subject': 'p.ready',
                          'object_id': projection['subjects']['p.ready']['heads'][0],
                          'detail': 'claim has no exact retained Snapshot context'}]}
        projection['coverage']['complete'] = False
        projection['integrity']['complete'] = False
        projection['integrity']['findings'] = copy.deepcopy(projection['temporal']['findings'])
        detached = HA.capture_history(fixture.store.capture().objects, projection,
                                      document=self.template)
        report = V3.assess(detached.snapshot())
        self.assertEqual(report['nodes']['p.ready']['temporal']['status'], 'unknown')

    def test_legacy_receipt_shape_and_capabilities_are_unchanged(self):
        fixture = self.fixture()
        mutation = A.prepare(fixture.entry, {'kind': 'set', 'id': 'p.input', 'value': 2},
                             by='writer', operation='legacy-shape')
        data = mutation.to_data()
        self.assertNotIn('temporal_replay', data['receipt']['before'])
        self.assertNotIn('temporal_replay', data['receipt']['after'])
        manifest = HC.decode_document(next(item['after'] for item in mutation.files
                                           if item['role'] == 'history_commit'))
        self.assertNotIn(HC.TEMPORAL_APPLICABILITY, manifest.get('requires', []))
        self.assertEqual(type(mutation).from_bytes(mutation.to_bytes()).to_bytes(),
                         mutation.to_bytes())

    def test_generic_writer_advertises_capability_and_missing_replay_is_unknown(self):
        fixture = self.fixture()
        temporal = authoring_fixtures.claim('p.imported', kind='judgment', op='imported',
            body={'verdict': 'ready', 'rests_on': ['p.input'],
                  'wrong_if': {'expr': 'p.input > 5'},
                  'temporal': {'version': 1, 'applicability': 'general'}},
            pins={'p.input': fixture.original['id']})
        fixture.fixture.publish([temporal], op='generic-temporal-import')
        captured = fixture.store.capture()
        manifest = HC.decode_document(captured.commits['generic-temporal-import'])
        self.assertIn(HC.TEMPORAL_APPLICABILITY, manifest['requires'])
        adapted = HA.from_store_capture(captured)
        report = V3.assess(adapted.snapshot())
        self.assertTrue(adapted.projection['temporal']['complete'])
        temporal_state = report['nodes']['p.imported']['temporal']
        self.assertEqual(temporal_state['status'], 'clear')
        self.assertEqual({item['evidence_kind'] for item in temporal_state['episodes']},
                         {'reconstructed_committed_world'})

    def test_generic_commit_after_activation_reconstructs_exact_world(self):
        fixture, _ = self.history('anchored')
        current = fixture.store.capture()
        old = current.objects[current.state['subjects']['p.input']['head']]
        new = authoring_fixtures.claim('p.input', value=10, op='generic-reading',
                                      saw=sorted(version for version, obj in current.objects.items()
                                                 if obj['subject'] == 'p.input'))
        accept = __import__('tests.test_history_store', fromlist=['act']).act(
            new, op='generic-accept', over=[old['id']], saw=sorted([*new['saw'], new['id']]))
        fixture.fixture.publish([new, accept], op='generic-after-temporal')
        adapted = HA.from_store_capture(fixture.store.capture())
        report = V3.assess(adapted.snapshot())
        temporal = report['nodes']['p.ready']['temporal']
        self.assertEqual(temporal['status'], 'counterexample')
        self.assertIn('reconstructed_committed_world', {
            item['evidence_kind'] for item in temporal['episodes']
            if item['outcome'] == 'counterexample'})

    def test_never_accepted_temporal_proposal_does_not_poison_coverage(self):
        fixture = self.fixture()
        proposal = A.prepare_proposal(fixture.entry, 'p.proposed', {
            'verdict': 'proposal', 'rests_on': ['p.input'],
            'wrong_if': {'expr': 'p.input > 5'},
            'temporal': {'version': 1, 'applicability': 'general'}},
            'judgments', because='proposal only', by='writer')
        A.commit(fixture.entry, proposal, verify=lambda data: None)
        adapted = HA.from_store_capture(fixture.store.capture())
        self.assertTrue(adapted.projection['temporal']['complete'])
        self.assertEqual(adapted.projection['temporal']['observations'], [])
        report = V3.assess(adapted.snapshot())
        self.assertEqual(report['history_subjects']['p.proposed']['acceptance'], 'proposed')
        self.assertNotIn('temporal', report['history_subjects']['p.proposed'])

    def test_narrow_selection_roundtrip_retains_unselected_temporal_evidence(self):
        fixture, _ = self.history('anchored')
        snapshot = HA.from_store_capture(fixture.store.capture()).snapshot()
        context = Context.CapturedAssessment.from_snapshot(snapshot, selection=['p.input'])
        self.assertNotIn('p.ready', context.assessment['nodes'])
        self.assertIn('temporal', context.assessment['history_subjects']['p.ready'])
        with mock.patch.object(V3.base_assessment, 'assess',
                               side_effect=AssertionError('native replayed during deserialize')):
            restored = Context.CapturedAssessment.from_json(context.to_json())
        self.assertEqual(restored.assessment, context.assessment)

    def test_rehashed_reconstructed_result_for_other_expression_refuses_purely(self):
        fixture, _ = self.history('anchored')
        current = fixture.store.capture()
        old = current.objects[current.state['subjects']['p.input']['head']]
        new = authoring_fixtures.claim('p.input', value=10, op='generic-forgery-reading',
                                      saw=sorted(version for version, obj in current.objects.items()
                                                 if obj['subject'] == 'p.input'))
        accept = __import__('tests.test_history_store', fromlist=['act']).act(
            new, op='generic-forgery-accept', over=[old['id']],
            saw=sorted([*new['saw'], new['id']]))
        fixture.fixture.publish([new, accept], op='generic-forgery')
        adapted = HA.from_store_capture(fixture.store.capture())
        current_snapshot = adapted.snapshot()
        context = Context.CapturedAssessment.from_snapshot(current_snapshot)
        forged = context.assessment
        raw = next(item for item in adapted.projection['temporal']['observations']
                   if item['operation'] == 'generic-forgery' and item['phase'] == 'after')
        historical = Snapshot.from_json(raw['snapshot'])
        expression = {'expr': 'p.input < 5'}
        computation = Evaluator(historical).evaluate(expression, declared=['p.input'])
        replacement = {'status': 'does_not_hold', 'expression': expression,
                       'reads': [item['id'] for item in computation['executed_reads']],
                       'computation': computation}
        for holder in (forged['nodes']['p.ready']['temporal'],
                       forged['history_subjects']['p.ready']['temporal']):
            episode = next(item for item in holder['episodes']
                           if item['operation'] == 'generic-forgery'
                           and item['phase'] == 'after')
            episode['result'] = copy.deepcopy(replacement)
            episode['outcome'] = 'not_counterexample'
        forged['findings_revision'] = digest(V3._findings_preimage(forged))
        forged['envelope_revision'] = digest({key: value for key, value in forged.items()
                                              if key != 'envelope_revision'})
        with mock.patch.object(V3.base_assessment, 'assess',
                               side_effect=AssertionError('native replayed during deserialize')):
            with self.assertRaisesRegex(ValueError, 'expression does not match immutable claim'):
                Context.CapturedAssessment(current_snapshot, forged)

    def test_rehashed_forgery_is_not_verified(self):
        fixture, _ = self.history('anchored')
        adapted = HA.from_store_capture(fixture.store.capture())
        projection = adapted.projection
        observation = next(item for item in projection['temporal']['observations']
                           if item['assessment']['nodes']['p.ready']['state']['falsifier']['status'] == 'holds')
        falsifier = observation['assessment']['nodes']['p.ready']['state']['falsifier']
        falsifier['status'] = 'does_not_hold'
        observation['assessment']['assessment_revision'] = digest({
            key: value for key, value in observation['assessment'].items()
            if key != 'assessment_revision'})
        detached = HA.capture_history(fixture.store.capture().objects, projection,
                                      document=self.template)
        report = V3.assess(detached.snapshot())
        episodes = report['nodes']['p.ready']['temporal']['episodes']
        self.assertTrue(any(item['verification'] == 'unknown'
                            and 'replay result mismatch' in item['finding'] for item in episodes))

    def test_receipt_claim_map_is_bound_to_its_causal_frontier(self):
        fixture, _ = self.history('current')
        captured = fixture.store.capture()
        operation, raw = next((operation, raw) for operation, raw in captured.commits.items()
                              if b'temporal_replay:' in raw)
        manifest = HC.decode_document(raw)
        manifest['receipt']['after']['temporal_replay']['claims']['p.ready'] = fixture.original['id']
        manifest['receipt']['digest'] = identity({key: value for key, value in manifest['receipt'].items()
                                                  if key != 'digest'})
        captured.commits[operation] = HC.encode_document(manifest)
        with self.assertRaisesRegex(HC.HistoryError,
                                    'temporal_causal_frontier_mismatch|temporal_claim_mismatch'):
            HA.from_store_capture(captured)

    def test_same_body_new_claim_does_not_inherit_old_counterexample(self):
        fixture, _ = self.history('anchored')
        body = copy.deepcopy(fixture.store.state()['subjects']['p.ready']['body'])
        body.pop('seen')
        old = fixture.store.state()['subjects']['p.ready']['head']
        proposal = A.prepare_proposal(fixture.entry, 'p.ready', body, 'judgments',
                                      because='fresh explicit claim', by='writer')
        A.commit(fixture.entry, proposal, verify=lambda data: None)
        proposal_id = proposal.to_data()['receipt']['after']['authoring']['proposal']
        act = A.prepare_act(fixture.entry, {'kind': 'accept', 'id': 'p.ready',
                            'of': proposal_id, 'over': [old],
                            'because': 'new claim version'}, by='writer')
        A.commit(fixture.entry, act, verify=lambda data: None)
        adapted = HA.from_store_capture(fixture.store.capture())
        report = V3.assess(adapted.snapshot())
        temporal = report['nodes']['p.ready']['temporal']
        new = fixture.store.state()['subjects']['p.ready']['head']
        self.assertNotEqual(old, new)
        self.assertEqual(temporal['status'], 'clear')
        self.assertNotIn(old, temporal['counterexample_claim_ids'])


if __name__ == '__main__':
    unittest.main()
