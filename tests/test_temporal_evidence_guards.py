"""Frozen temporal evidence cannot borrow truth or witnesses from another run."""
import copy
import unittest
from unittest import mock

from scripts import history_adapter as HA
from scripts.reasoning import history_assessment as V3
from scripts.reasoning.context import CapturedAssessment
from scripts.reasoning.contract import digest
from tests import test_history_authoring as fixtures
from tests.test_history_store import act
from tests import test_reasoning_temporal as temporal_fixtures


class TemporalEvidenceGuards(unittest.TestCase):
    def context(self):
        case = temporal_fixtures.TemporalApplicability()
        self.addCleanup(case.doCleanups)
        fixture, _ = case.history('anchored')
        current = fixture.store.capture()
        old = current.state['subjects']['p.input']['head']
        reading = fixtures.claim('p.input', value=1, op='guard-source',
            saw=sorted(version for version, obj in current.objects.items() if obj['subject'] == 'p.input'))
        accepted = act(reading, op='guard-accept', over=[old], saw=sorted([*reading['saw'], reading['id']]))
        fixture.fixture.publish([reading, accepted], op='guard-world')
        return CapturedAssessment.from_snapshot(HA.from_store_capture(fixture.store.capture()).snapshot())

    def forged(self, context, mutate):
        report = context.assessment
        for holder in (report['nodes']['p.ready']['temporal'], report['history_subjects']['p.ready']['temporal']):
            episode = next(item for item in holder['episodes']
                           if item['operation'] == 'guard-world' and item['phase'] == 'after')
            mutate(episode)
        report['findings_revision'] = digest(V3._findings_preimage(report))
        report['envelope_revision'] = digest({key: value for key, value in report.items()
                                            if key != 'envelope_revision'})
        return report

    def test_verified_boolean_needs_a_matching_computation(self):
        context = self.context()
        def mutate(episode):
            episode['result']['status'] = 'holds'
            episode['result']['computation'] = None
            episode['result']['reads'] = []
            episode['outcome'] = 'counterexample'
        report = self.forged(context, mutate)
        with mock.patch.object(V3.base_assessment, 'assess', side_effect=AssertionError('native replay')), \
             self.assertRaisesRegex(ValueError, 'computed boolean'):
            CapturedAssessment(context.snapshot, report)

    def test_rehashed_basis_must_match_the_actual_source_witnesses(self):
        context = self.context()
        def mutate(episode):
            computation = episode['result']['computation']
            for key in ('potential_dependencies', 'executed_reads'):
                for item in computation[key]:
                    item['fingerprint'] = '0' * 64
            computation['basis']['dependencies'] = copy.deepcopy(computation['potential_dependencies'])
            computation['basis']['digest'] = digest({key: value for key, value in computation['basis'].items()
                                                      if key != 'digest'})
        report = self.forged(context, mutate)
        with mock.patch.object(V3.base_assessment, 'assess', side_effect=AssertionError('native replay')), \
             self.assertRaisesRegex(ValueError, 'basis does not match'):
            CapturedAssessment(context.snapshot, report)


if __name__ == '__main__':
    unittest.main()
