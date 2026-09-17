"""The shared consumer context captures, fails and replays as one unit."""
import copy
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts.reasoning import context as C
from scripts.reasoning.snapshot import Snapshot


HEADERS = {'meta': {'reasoning': {
    'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}}}


class UnavailableRuntime:
    def request_many(self, requests):
        raise OSError('deliberately unavailable')


class ReasoningContextTests(unittest.TestCase):
    def context(self, value=1):
        snapshot = Snapshot.from_data({**HEADERS, 'readings': {'p.input': {'v': value}}})
        return C.CapturedAssessment.from_snapshot(snapshot, runtime=UnavailableRuntime())

    def test_context_binds_snapshot_findings_view_and_session_revision(self):
        context = self.context()
        self.assertEqual(context.snapshot_id, context.assessment['snapshot_id'])
        self.assertEqual(context.findings_revision, context.view['findings_revision'])
        first = context.session_revision({'project': 'one', 'generation': 1})
        self.assertEqual(first, context.session_revision({'project': 'one', 'generation': 1}))
        self.assertNotEqual(first, context.session_revision({'project': 'one', 'generation': 2}))
        self.assertNotEqual(first, self.context(2).session_revision(
            {'project': 'one', 'generation': 1}))

    def test_context_round_trips_without_sources_or_evaluator(self):
        context = self.context()
        serialized = context.to_json()
        with mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('source access')), \
                mock.patch('builtins.open', side_effect=AssertionError('source access')), \
                mock.patch('subprocess.run', side_effect=AssertionError('evaluator')):
            replay = C.CapturedAssessment.from_json(serialized)
        self.assertEqual(replay.snapshot_id, context.snapshot_id)
        self.assertEqual(replay.findings_revision, context.findings_revision)
        self.assertEqual(replay.view, context.view)

    def test_mutated_context_or_view_refuses(self):
        context = self.context()
        payload = context.to_data()
        mutated = copy.deepcopy(payload)
        mutated['assessment']['nodes']['p.input']['acceptance']['status'] = 'accepted'
        with self.assertRaisesRegex(ValueError, 'context|history|revision'):
            C.CapturedAssessment.from_data(mutated)
        mutated = copy.deepcopy(payload)
        mutated['view']['nodes']['p.input']['status_text'] = 'forged'
        mutated['context_revision'] = C.digest({
            key: value for key, value in mutated.items() if key != 'context_revision'})
        with self.assertRaisesRegex(ValueError, 'consumer view'):
            C.CapturedAssessment.from_data(mutated)

    def test_capture_failure_is_versioned_and_contains_no_findings(self):
        first = C.capture_failure(ValueError('missing_history_object: p.input'))
        second = C.capture_failure(ValueError('missing_history_object: p.input'))
        self.assertEqual(first, second)
        self.assertIsNone(first['findings'])
        self.assertEqual(first['code'], 'missing_history_object')
        self.assertEqual(C.validate_failure(first), first)
        forged = copy.deepcopy(first)
        forged['detail'] = 'different'
        with self.assertRaisesRegex(ValueError, 'noncanonical'):
            C.validate_failure(forged)

    def test_disk_capture_failure_raises_the_same_public_envelope(self):
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / 'missing.yaml'
            with mock.patch.object(Snapshot, 'capture', side_effect=ValueError(
                    'missing_history_object: p.input')):
                with self.assertRaises(C.CaptureError) as raised:
                    C.CapturedAssessment.capture([str(missing)])
        self.assertEqual(raised.exception.envelope,
                         C.capture_failure(ValueError('missing_history_object: p.input')))


if __name__ == '__main__':
    unittest.main()
