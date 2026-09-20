"""The same temporal findings reach operational checks and default CLI policy."""
import contextlib
import io
import os
import unittest
from unittest import mock

from scripts import provenance as P
from scripts.reasoning import operations
from scripts.reasoning.context import CapturedAssessment
from tests import test_reasoning_temporal as temporal_fixtures


@unittest.skipIf(os.name == 'nt', 'history writer fixtures require POSIX locks')
class TemporalOperationalConsumers(unittest.TestCase):
    def history(self, applicability):
        case = temporal_fixtures.TemporalApplicability()
        self.addCleanup(case.doCleanups)
        return case.history(applicability)[0]

    def test_operational_findings_distinguish_recovery_and_persistence(self):
        for applicability in ('current', 'anchored', 'general'):
            with self.subTest(applicability=applicability):
                fixture = self.history(applicability)
                document = operations.load([str(fixture.entry)], allow_history=True)
                result = operations.findings(operations.world(document).context)
                self.assertEqual(bool(result['falsified']), applicability != 'current', result)
                self.assertEqual(result['holes'], [], result)

    def test_core_check_distinguishes_recovery_and_persistence(self):
        for applicability in ('current', 'anchored', 'general'):
            with self.subTest(applicability=applicability):
                fixture = self.history(applicability)
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    code = P.core_check([str(fixture.entry)])
                self.assertEqual(code, int(applicability != 'current'), output.getvalue())

    def test_persistent_context_replays_without_native_execution(self):
        fixture = self.history('general')
        context = CapturedAssessment.capture([str(fixture.entry)])
        with mock.patch('scripts.reasoning.history_assessment.base_assessment.assess',
                        side_effect=AssertionError('native reassessment during frozen replay')):
            retained = CapturedAssessment.from_json(context.to_json())
        self.assertEqual(retained.assessment['nodes']['p.ready']['temporal']['status'], 'counterexample')


if __name__ == '__main__':
    unittest.main()
