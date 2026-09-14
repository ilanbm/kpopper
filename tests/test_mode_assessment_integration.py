"""Recorded assessment selects the same knowledge world as the ordinary readers."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

from scripts import assessment as A, export_graph as X, provenance as P, project_modes as M
from tests.test_pending_grounding import Repository, fixture_bundle


class ModeAssessment(Repository):
    def setUp(self):
        super().setUp()
        self.record = self.root / 'GROUNDING.yaml'
        self.doc = {'known': {'local.one': {'v': 1}},
                    'judgments': {'claim.ready': {'rests_on': ['local.one'], 'seen': {'local.one': 1},
                                                  'wrong_if': 'local.one > 2', 'verdict': 'ready'}}}
        self.record.write_text(P.yaml.safe_dump(self.doc, sort_keys=False))
        self.environment = patch.dict(os.environ, {'KPOPPER_READ_MODE': 'live',
                                                    'XDG_STATE_HOME': str(self.base / 'state'),
                                                    'KPOPPER_PRIVATE_HOME': str(self.base / 'private')})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def cli(self, *args):
        return subprocess.run([sys.executable, str(Path(A.__file__).with_name('cli.py')),
                               '--workspace', str(self.root), *args],
                              text=True, capture_output=True, timeout=30)

    def test_simple_assessment_and_export_share_identity_and_historical_readings(self):
        shared = self.base / 'shared.yaml'
        shared.write_bytes(self.record.read_bytes())
        M.Project(self.root).configure('simple', record=str(shared))
        changed = copy.deepcopy(self.doc)
        changed['known']['local.one']['v'] = 3
        shared.write_text(P.yaml.safe_dump(changed, sort_keys=False))
        before = {path: path.read_bytes() for path in (self.record, shared)}
        alias = A.load([str(self.record)])
        direct = A.load([str(shared)])
        self.assertEqual(alias, direct)
        state = alias['nodes']['claim.ready']['state']
        self.assertEqual(state['falsifier']['status'], 'holds')
        self.assertEqual(state['basis']['dependencies']['local.one']['at_review']['value'], 1)
        packet = X.project([str(self.record)], ['claim.ready'], depth=0)
        self.assertEqual(packet['assessment']['assessment_revision'], alias['assessment_revision'])
        self.assertNotIn('current', packet['nodes']['claim.ready']['readings'][0])
        for frozen, expected in ((False, 'holds'), (True, 'does_not_hold')):
            result = self.cli(*(['--frozen'] if frozen else []), 'assess', 'claim.ready',
                              '--record', str(self.record))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)['nodes']['claim.ready']['state']['falsifier']['status'], expected)
        self.assertEqual({path: path.read_bytes() for path in before}, before)

    def test_pending_conflict_is_attention_without_adoption_or_frozen_drift(self):
        before_bytes = self.record.read_bytes()
        with patch.dict(os.environ, {'KPOPPER_READ_MODE': 'frozen'}):
            frozen_before = A.load([str(self.record)])
        first = self.capture(fixture_bundle('local.one', 3))
        live = A.load([str(self.record)])
        self.assertEqual(live['nodes']['local.one']['body']['v'], 1)
        self.assertEqual(live['nodes']['local.one']['state']['contention']['status'], 'detected')
        self.assertEqual(live['nodes']['local.one']['attention'][0]['action'], 'inspect_alternatives')
        self.assertEqual(live['nodes']['claim.ready']['state']['falsifier']['status'], 'does_not_hold')
        self.assertIn('pending-' + first['revision'], live['scope']['knowledge']['contributions_checked'])
        packet = X.project([str(self.record)], ['local.one'])
        self.assertEqual(packet['assessment']['assessment_revision'], live['assessment_revision'])
        self.assertEqual(packet['contributions'], 1)
        self.assertEqual(packet['hypotheses'], 0)
        result = self.cli('assess', 'local.one', '--attention-only', '--record', str(self.record))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('local.one', json.loads(result.stdout)['attention'])
        self.capture(fixture_bundle('local.one', 4), event_id='second', contribution_id='second')
        self.assertNotEqual(A.load([str(self.record)])['record_revision'], live['record_revision'])
        with patch.dict(os.environ, {'KPOPPER_READ_MODE': 'frozen'}):
            self.assertEqual(A.load([str(self.record)]), frozen_before)
        self.assertEqual(self.record.read_bytes(), before_bytes)

    def test_complete_body_conflict_survives_equal_scalar_values(self):
        bundle = fixture_bundle('local.one', 1)
        self.record.write_text(P.yaml.safe_dump(bundle['manifest']['document'], sort_keys=False))
        self.capture(bundle)
        matching = A.load([str(self.record)])
        self.assertEqual(matching['nodes']['local.one']['state']['contention']['status'], 'none_detected')
        document = P.yaml.safe_load(self.record.read_text())
        document['sources']['s.vendor']['read'] = '2026-09-13'
        document['known']['local.one']['scope']['environment'] = 'API v1'
        self.record.write_text(P.yaml.safe_dump(document, sort_keys=False))
        report = A.load([str(self.record)])
        self.assertEqual(report['nodes']['local.one']['state']['contention']['status'], 'detected')
        self.assertEqual(report['nodes']['s.vendor']['state']['contention']['status'], 'detected')
        self.assertEqual(report['nodes']['local.one']['attention'][0]['action'], 'inspect_alternatives')

    def target_fixture(self):
        bundle = fixture_bundle('local.one', 1)
        self.record.write_text(P.yaml.safe_dump(bundle['manifest']['document'], sort_keys=False))
        for name, content in bundle['files'].items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
        M.git(self.root, 'add', '.')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Recorded fixture')
        remote = self.base / 'remote.git'
        M.git(self.root, 'init', '--bare', str(remote))
        M.git(self.root, 'remote', 'add', 'team', str(remote))
        M.Project(self.root).publication_config('team', 'trunk', grant=False)
        target = self.worktree('target-context')
        self.capture(bundle)
        return target, bundle

    def target_version(self, target, bundle, environment):
        document = copy.deepcopy(bundle['manifest']['document'])
        document['known']['local.one']['scope']['environment'] = environment
        (target / 'GROUNDING.yaml').write_text(P.yaml.safe_dump(document, sort_keys=False))
        M.git(target, 'add', 'GROUNDING.yaml')
        M.git(target, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Target metadata')
        revision = M.git(target, 'rev-parse', 'HEAD').stdout.decode().strip()
        M.git(self.root, 'update-ref', 'refs/remotes/team/trunk', revision)
        return revision

    def test_target_metadata_changes_assessment_and_export_identity_without_exposing_omitted_bodies(self):
        target, bundle = self.target_fixture()
        self.target_version(target, bundle, 'TARGET-DETAILS-ONE')
        before = A.load([str(self.record)])
        packet_before = X.project([str(self.record)], ['local.one'], depth=0)
        target_revision = self.target_version(target, bundle, 'TARGET-DETAILS-TWO')
        after = A.load([str(self.record)])
        packet_after = X.project([str(self.record)], ['local.one'], depth=0)
        self.assertNotEqual(after['record_revision'], before['record_revision'])
        self.assertNotEqual(after['assessment_revision'], before['assessment_revision'])
        self.assertNotEqual(packet_after['snapshot'], packet_before['snapshot'])
        self.assertEqual(after['scope']['knowledge']['target']['revision'], target_revision)
        self.assertEqual(after['nodes']['local.one']['state']['contention']['method'],
                         'readable_hypothesis_pairs_and_knowledge_identity')
        self.assertEqual(after['scope']['hypotheses_checked'], [])
        self.assertTrue(after['scope']['knowledge']['contributions_checked'])
        self.assertNotIn('TARGET-DETAILS-TWO', json.dumps(packet_after))

    def test_live_target_context_matches_the_shipped_schema(self):
        try:
            import jsonschema
        except ImportError:
            if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1':
                raise
            self.skipTest('schema validation requires the session dependency jsonschema')
        target, bundle = self.target_fixture()
        self.target_version(target, bundle, 'API v1')
        report = A.load([str(self.record)])
        jsonschema.validate(report, json.loads(Path(A.__file__).with_name('assessment.schema.json').read_text()))

    def test_unavailable_configured_target_is_disclosed_as_partial_assessment(self):
        self.target_fixture()
        report = A.load([str(self.record)])
        self.assertEqual(report['scope']['coverage'], 'partial')
        target = report['scope']['knowledge']['target']
        self.assertEqual(target['status'], 'unavailable')
        self.assertEqual(target['ref'], 'refs/remotes/team/trunk')
        self.assertTrue(target['reason'])


if __name__ == '__main__':
    unittest.main()
