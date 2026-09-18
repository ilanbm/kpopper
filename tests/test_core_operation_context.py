"""Operational readers retain one coherent captured source and typed assessment."""
from pathlib import Path
import contextlib
import copy
import io
import os
import tempfile
import unittest
from unittest import mock

import yaml

from scripts.reasoning.snapshot import capture_source, SnapshotError
from scripts.reasoning.snapshot import Snapshot
from scripts.reasoning.context import CapturedAssessment
from scripts.reasoning import operations as O
from scripts import consolidate as C


class CapturedOperationSource(unittest.TestCase):
    def test_detached_reader_metadata_and_snapshot_share_the_verified_read(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record = root / 'GROUNDING.yaml'
            record.write_text('meta:\n  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  p.x: {v: 1}\n')
            hypotheses = root / '.kpopper' / 'hypotheses'
            hypotheses.mkdir(parents=True)
            hypothesis = hypotheses / 'other.yml'
            original = '# preserve this exact source text\nhypothesis: {claim: alternative}\nknown:\n  p.x: {v: 2}\n'
            hypothesis.write_text(original)
            captured = capture_source([str(record)], read_mode='frozen')
            document = captured.document
            self.assertEqual(dict(document), captured.snapshot.to_data()['document'])
            self.assertEqual(document.hypotheses['other']['path'], str(hypothesis))
            self.assertEqual(captured.files[str(hypothesis)], original.encode())
            document['known']['p.x']['v'] = 999
            document.hypotheses['other']['head']['claim'] = 'changed'
            self.assertEqual(captured.document['known']['p.x']['v'], 1)
            self.assertEqual(captured.document.hypotheses['other']['head']['claim'], 'alternative')
            captured.verify()
            hypothesis.write_text(original + '# later edit\n')
            with self.assertRaises(SnapshotError):
                captured.verify()
            self.assertEqual(captured.files[str(hypothesis)], original.encode())

    def test_supplied_clock_and_pending_context_survive_value_admission(self):
        document = {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
                                          'requires': ['arithmetic/v1']}},
                    'known': {'p.x': {'v': 1}}}
        snapshot = Snapshot.from_data(document, context={'read_mode': 'supplied', 'target': {'status': 'unavailable'}},
                                      as_of='2026-09-18')
        world = O.World(document, snapshot)
        self.assertEqual(world.snapshot.snapshot_id, snapshot.snapshot_id)
        seen = []
        original = O.authoring.Snapshot.from_data
        def retain(*args, **kwargs):
            result = original(*args, **kwargs)
            seen.append(result.to_data())
            return result
        with mock.patch.object(O.authoring.Snapshot, 'from_data', side_effect=retain):
            self.assertTrue(world.same_value('p.x', 1))
        self.assertEqual(seen[-1]['as_of'], snapshot.to_data()['as_of'])
        self.assertEqual(seen[-1]['context'], snapshot.to_data()['context'])

    def test_missing_dependency_attention_retains_the_full_integrity_finding(self):
        snapshot = Snapshot.from_data({'meta': {'reasoning': {
            'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
            'judgments': {'d.wait': {'rests_on': ['p.missing'], 'seen': {},
                'verdict': 'waiting', 'blocked_on': 'source absent',
                'wrong_if': {'expr': 'p.missing > 5'}}}})
        node = CapturedAssessment.from_snapshot(snapshot).assessment['nodes']['d.wait']
        self.assertEqual(node['state']['falsifier']['status'], 'unknown')
        self.assertTrue(any(issue.get('field') == 'rests_on'
                            for issue in node['state']['integrity']['issues']))
        self.assertTrue(node['attention'])
        for action in node['attention']:
            for reason in action['reasons']:
                self.assertEqual(set(reason), {'code', 'related_ids'})

    def test_prospective_snapshot_preserves_inputs_without_claiming_a_live_read(self):
        document = C.P.Record({'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
                                                     'requires': ['arithmetic/v1']}},
                               'known': {'p.x': {'v': 1}}})
        document.hypotheses = {}
        snapshot = Snapshot.from_data(document, context={'read_mode': 'frozen',
                                                          'target': {'status': 'unavailable'}})
        O.bind(document, snapshot)
        hypothesis = C.hypothesis('change', {'known': {'p.x': {'v': 2}}})
        derived = O.derive(C.P.layered(document, hypothesis), document, ['change'], proposals=[hypothesis])
        context = O.world(derived).context
        data = context.snapshot.to_data()
        self.assertEqual(data['context']['read_mode'], 'supplied')
        self.assertEqual(data['context']['target'], {'status': 'unavailable'})
        self.assertEqual(data['context']['operation']['base_snapshot'], snapshot.snapshot_id)
        self.assertEqual(snapshot.to_data()['context']['read_mode'], 'frozen')
        with mock.patch.object(Snapshot, 'capture', side_effect=AssertionError('live read')):
            self.assertEqual(CapturedAssessment.from_json(context.to_json()).assessment, context.assessment)


@unittest.skipIf(os.name == 'nt', 'Transactional folds require POSIX locking')
class CoreFold(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / 'GROUNDING.yaml'
        self.hypotheses = self.root / '.kpopper/hypotheses'
        self.hypotheses.mkdir(parents=True)
        self.doc = {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
                                         'requires': ['arithmetic/v1']}},
                    'known': {'p.x': {'v': 1}}}
        self.write()

    def write(self):
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))

    def hypothesis(self, body, name='candidate'):
        path = self.hypotheses / (name + '.yaml')
        path.write_text('# retained authored comment\n' + yaml.safe_dump(
            {'hypothesis': {'claim': name}, **body}, sort_keys=False))
        return path

    def fold(self, *args):
        with contextlib.redirect_stdout(io.StringIO()):
            return C.fold([str(self.record)], *args)

    def test_fold_promotes_required_module_and_publishes_once(self):
        proposal = self.hypothesis({
            'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
                                  'requires': ['arithmetic/v1', 'composition/v1']}},
            'known': {'p.y': {'rule': {'if': {'bool': True}, 'then': {'ref': 'p.x'},
                                     'else': {'num': '0'}}}}})
        with mock.patch.object(C.P, 'flags', side_effect=AssertionError('legacy evaluator')):
            self.assertEqual(self.fold(), 0)
        after = yaml.safe_load(self.record.read_text())
        self.assertIn('composition/v1', after['meta']['reasoning']['requires'])
        self.assertFalse(proposal.exists())
        report = CapturedAssessment.capture([str(self.record)]).assessment
        self.assertEqual(report['nodes']['p.y']['computation']['value']['numerator'], '1')

    def test_late_hypothesis_edit_refuses_without_record_write(self):
        proposal = self.hypothesis({'known': {'p.y': {'v': 2}}})
        before = self.record.read_bytes()
        original = C.P._bump_updated
        edited = []
        def change(*args, **kwargs):
            result = original(*args, **kwargs)
            if not edited:
                proposal.write_text(proposal.read_text() + '# independent edit\n')
                edited.append(True)
            return result
        with mock.patch.object(C.P, '_bump_updated', side_effect=change):
            with self.assertRaisesRegex((ValueError, SystemExit), 'snapshot_changed|concurrent_edit'):
                self.fold()
        self.assertEqual(self.record.read_bytes(), before)
        self.assertIn('# independent edit', proposal.read_text())

    def test_proposed_reading_cannot_authorize_its_own_replacement(self):
        self.doc['judgments'] = {'d.bound': {'verdict': 'old', 'rests_on': ['p.x'],
                                           'seen': {'p.x': 1}, 'wrong_if': {'expr': 'p.x > 5'}}}
        self.write()
        before = self.record.read_bytes()
        proposal = self.hypothesis({'known': {'p.x': {'v': 10}}, 'judgments': {
            'd.bound': {'verdict': 'new', 'rests_on': ['p.x'], 'seen': {'p.x': 10},
                        'wrong_if': {'expr': 'p.x > 20'}}}, 'meta': copy.deepcopy(self.doc['meta'])})
        with self.assertRaisesRegex(SystemExit, 'takes|names|not clean|standing'):
            self.fold()
        self.assertEqual(self.record.read_bytes(), before)
        self.assertTrue(proposal.exists())

    def test_declared_unknown_remains_visible_without_becoming_false(self):
        self.doc['judgments'] = {'d.wait': {'verdict': 'pending source', 'rests_on': ['p.missing'],
                                          'blocked_on': 'The source has not arrived',
                                          'wrong_if': {'expr': 'p.missing > 5'}, 'seen': {}}}
        self.write()
        self.hypothesis({'known': {'p.y': {'v': 2}}})
        self.assertEqual(self.fold(), 0)
        context = CapturedAssessment.capture([str(self.record)])
        self.assertEqual(context.assessment['nodes']['d.wait']['state']['falsifier']['status'], 'unknown')
        self.assertTrue(O.findings(context)['uncertain']['d.wait'])

    def test_non_boolean_falsifier_never_becomes_a_clean_fold(self):
        before = self.record.read_bytes()
        proposal = self.hypothesis({'meta': copy.deepcopy(self.doc['meta']),
            'judgments': {'d.invalid': {'verdict': 'invalid condition', 'rests_on': [],
                                       'seen': {}, 'wrong_if': {'num': '1'}}}})
        with self.assertRaisesRegex(SystemExit, 'not clean'):
            self.fold()
        self.assertEqual(self.record.read_bytes(), before)
        self.assertTrue(proposal.exists())
