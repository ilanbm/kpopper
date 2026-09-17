"""The shared consumer context captures, fails and replays as one unit."""
import copy
import datetime
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts.reasoning import context as C
from scripts.reasoning.snapshot import Snapshot
from scripts.pending_grounding import _decode, _encode
from scripts.reasoning import history_assessment as V3
from scripts.reasoning.projection import project_findings


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
        decoded = _decode(mutated['payload'])
        decoded['assessment']['nodes']['p.input']['acceptance']['status'] = 'accepted'
        mutated['payload'] = _encode(decoded)
        with self.assertRaisesRegex(ValueError, 'context|history|revision'):
            C.CapturedAssessment.from_data(mutated)
        mutated = copy.deepcopy(payload)
        decoded = _decode(mutated['payload'])
        decoded['view']['nodes']['p.input']['status_text'] = 'forged'
        mutated['payload'] = _encode(decoded)
        mutated['context_revision'] = C.digest(decoded)
        with self.assertRaisesRegex(ValueError, 'consumer view'):
            C.CapturedAssessment.from_data(mutated)

    def test_date_bearing_context_is_typed_json_safe(self):
        snapshot = Snapshot.from_data({**HEADERS, 'readings': {
            'p.input': {'v': 1, 'of': datetime.date(2026, 9, 17)}}})
        context = C.CapturedAssessment.from_snapshot(
            snapshot, runtime=UnavailableRuntime())
        replay = C.CapturedAssessment.from_json(context.to_json())
        self.assertEqual(replay.snapshot.to_data()['nodes']['p.input']['body']['of'],
                         datetime.date(2026, 9, 17))
        self.assertEqual(replay.findings_revision, context.findings_revision)

    def test_replay_cannot_rebind_valid_shaped_findings_to_another_snapshot_body(self):
        context = self.context()
        payload = context.to_data()
        decoded = _decode(payload['payload'])
        report = decoded['assessment']
        report['nodes']['p.input']['body']['v'] = 999
        base = {
            'schema_version': 2, 'assessment_profile': report['assessment_profile'],
            'attention_policy': report['attention_policy'], 'snapshot_id': report['snapshot_id'],
            'as_of': report['as_of'], 'scope': copy.deepcopy(report['scope']),
            'selection': list(report['assessment_selection']),
            'nodes': {identifier: {key: copy.deepcopy(node[key]) for key in (
                'body', 'fields', 'state', 'attention', 'computation')}
                for identifier, node in report['nodes'].items()},
            'operational_limits': copy.deepcopy(report['operational_limits']),
        }
        base['assessment_revision'] = C.digest(base)
        report['base_assessment_revision'] = base['assessment_revision']
        report['findings_revision'] = C.digest(V3._findings_preimage(report))
        report['envelope_revision'] = C.digest({
            key: value for key, value in report.items() if key != 'envelope_revision'})
        decoded['view'] = project_findings(report)
        payload['payload'] = _encode(decoded)
        payload['context_revision'] = C.digest(decoded)
        with self.assertRaisesRegex(ValueError, 'snapshot|assessment_revision|match'):
            C.CapturedAssessment.from_data(payload)

    def test_public_v3_validator_rejects_forged_evidence_shapes(self):
        report = self.context().assessment
        for mutation in ('coverage', 'support', 'head_ids', 'finding', 'assurance', 'history',
                         'reviews', 'disposition', 'selection'):
            forged = copy.deepcopy(report)
            if mutation == 'coverage':
                forged['nodes']['p.input']['coverage']['complete'] = 'yes'
            elif mutation == 'support':
                forged['nodes']['p.input']['support'] = {'arbitrary': 'shape'}
            elif mutation == 'head_ids':
                forged['nodes']['p.input']['acceptance']['head_ids'] = 'not-a-list'
            elif mutation == 'finding':
                forged['nodes']['p.input']['coverage']['findings'] = [1]
            elif mutation == 'assurance':
                forged['nodes']['p.input']['assurance']['actual']['node'] = 'not-an-object'
            elif mutation == 'history':
                forged['nodes']['p.input']['history']['proposals'] = 'not-a-list'
            elif mutation == 'reviews':
                forged['nodes']['p.input']['history']['reviews'] = [1]
            elif mutation == 'disposition':
                forged['nodes']['p.input']['history']['disposition_marks'] = {'v': 'bogus'}
            else:
                forged['assessment_selection'] = 'p.input'
            forged['findings_revision'] = C.digest(V3._findings_preimage(forged))
            forged['envelope_revision'] = C.digest({
                key: value for key, value in forged.items() if key != 'envelope_revision'})
            with self.subTest(mutation=mutation), self.assertRaisesRegex(
                    ValueError, 'coverage|support|head|finding|assurance|history|selection'):
                V3.validate(forged)

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
