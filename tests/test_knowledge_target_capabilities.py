"""Comparison interpretation respects capability identity at the committed-target seam."""
import copy
import json
from unittest.mock import patch

from scripts import provenance as P, project_modes as M, pending_grounding as G
from scripts.reasoning.snapshot import Snapshot
from tests.test_pending_grounding import Repository, fixture_bundle

W = G.P._peer('watch')

CORE = {'version': 1, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}


class TargetCapabilities(Repository):
    def setUp(self):
        super().setUp()
        self.record = self.root / 'GROUNDING.yaml'
        self.document = fixture_bundle()['manifest']['document']
        self.record.write_text(P.yaml.safe_dump(self.document))
        M.git(self.root, 'add', 'GROUNDING.yaml')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Record')
        self.capture()
        project = M.Project(self.root)
        config = {**project.config(), 'mode': 'advanced', 'publication': {
            'remote': 'origin', 'repository': 'https://example.test/repo.git',
            'target': 'trunk', 'branch': 'pending', 'standing_permission': False}}
        project.state.mkdir(parents=True, exist_ok=True)
        project.config_path.write_text(json.dumps(config))
        M.git(self.root, 'update-ref', 'refs/remotes/origin/trunk', 'HEAD')

    def target(self, declaration=None, *, hypotheses=()):
        document = copy.deepcopy(self.document)
        if declaration is not None:
            document['meta'] = {'reasoning': declaration}
        return {'doc': document, 'hypotheses': list(hypotheses), 'hash': 'fixture'}

    def core_load(self, target):
        token = P._CORE_READS.set(True)
        try:
            with patch.object(W, '_records', return_value=target):
                return P.load([str(self.record)])
        finally:
            P._CORE_READS.reset(token)

    def test_equal_bodies_with_distinct_target_profiles_are_contested(self):
        legacy = self.core_load(self.target())
        common = self.core_load(self.target(CORE))
        self.assertFalse(legacy.knowledge_conflicts)
        self.assertIn('api.limit', common.knowledge_conflicts)
        self.assertFalse(getattr(common, 'target_unavailable', None))

    def test_target_hypothesis_profile_is_bound_even_with_identical_bodies(self):
        target = self.target(hypotheses=[{'name': 'core', 'doc': self.target(CORE)['doc'], 'head': {}}])
        live = self.core_load(target)
        self.assertIn('api.limit', live.knowledge_conflicts)
        self.assertFalse(getattr(live, 'target_unavailable', None))

    def test_target_seam_rejects_unsupported_before_interpreting_any_target_entries(self):
        for declaration, code in [({**CORE, 'requires': ['arithmetic/v1', 'future/v1']}, 'unsupported_capability'),
                                   ({**CORE, 'profile': 'core/v999'}, 'unsupported_capability'),
                                   ({**CORE, 'version': True}, 'invalid_capability')]:
            for in_hypothesis in (False, True):
                with self.subTest(declaration=declaration, in_hypothesis=in_hypothesis):
                    target = self.target(declaration)
                    target['doc']['known']['api.limit']['v'] = 999
                    if in_hypothesis:
                        target = self.target(hypotheses=[{'name': 'future', 'doc': target['doc'], 'head': {}}])
                        target['doc']['known']['api.limit']['v'] = 888
                    live = self.core_load(target)
                    self.assertIn(code, live.target_unavailable)
                    self.assertFalse(live.knowledge_conflicts)

    def test_target_seam_preserves_ordinary_reader_guard(self):
        with patch.object(W, '_records', return_value=self.target(CORE)):
            live = P.load([str(self.record)])
        self.assertIn('unsupported_capability', live.target_unavailable)
        self.assertFalse(live.knowledge_conflicts)

    def commit_target(self, declaration, *, hypothesis=False):
        target = self.target(declaration)['doc']
        if hypothesis:
            path = self.root / '.kpopper/hypotheses/target.yaml'
            path.parent.mkdir(parents=True)
        else:
            path = self.record
        path.write_text(P.yaml.safe_dump(target))
        M.git(self.root, 'add', str(path.relative_to(self.root)))
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Target interpretation')
        M.git(self.root, 'update-ref', 'refs/remotes/origin/trunk', 'HEAD')
        M.git(self.root, 'reset', '--hard', 'HEAD~1')

    def test_committed_target_core_capture_and_ordinary_guard_restore_context(self):
        self.commit_target(CORE)
        previous = W.P._CORE_READS.get()
        snapshot = Snapshot.capture([str(self.record)])
        data = snapshot.to_data()
        self.assertEqual(data['context']['target']['status'], 'observed')
        self.assertIn('api.limit', data['context']['conflicts'])
        self.assertEqual(W.P._CORE_READS.get(), previous)
        ordinary = P.load([str(self.record)])
        self.assertIn('unsupported_capability', ordinary.target_unavailable)
        self.assertFalse(ordinary.knowledge_conflicts)
        self.assertEqual(W.P._CORE_READS.get(), previous)
        self.assertEqual(Snapshot.from_json(snapshot.to_json()).snapshot_id, snapshot.snapshot_id)

    def test_committed_target_hypothesis_profile_survives_snapshot_replay(self):
        self.commit_target(CORE, hypothesis=True)
        snapshot = Snapshot.capture([str(self.record)])
        data = snapshot.to_data()
        self.assertEqual(data['context']['target']['status'], 'observed')
        self.assertIn('api.limit', data['context']['conflicts'])
        self.assertEqual(data['context']['target']['snapshot']['hypotheses'][0]['doc']['meta']['reasoning'], CORE)
        replay = Snapshot.from_json(snapshot.to_json())
        self.assertEqual(replay.snapshot_id, snapshot.snapshot_id)
        self.assertIn('api.limit', replay.to_data()['context']['conflicts'])

    def test_committed_target_hypothesis_requirements_refuse_before_comparison(self):
        self.commit_target({**CORE, 'requires': ['arithmetic/v1', 'future/v1']}, hypothesis=True)
        previous = W.P._CORE_READS.get()
        data = Snapshot.capture([str(self.record)]).to_data()
        self.assertEqual(data['context']['target']['status'], 'unavailable')
        self.assertIn('unsupported_capability', data['context']['target']['reason'])
        self.assertFalse(data['context']['conflicts'])
        self.assertEqual(W.P._CORE_READS.get(), previous)

    def test_live_pending_compares_one_proposal_to_checkout_but_supplied_does_not(self):
        # Live contributions are compared against the checkout (and target).
        # Supplied hypotheses only derive contention between disagreeing alternatives.
        changed = copy.deepcopy(self.document)
        changed['known']['api.limit']['v'] = 9
        self.record.write_text(P.yaml.safe_dump(changed))
        live = self.core_load(self.target())
        self.assertIn('api.limit', live.knowledge_conflicts)
        supplied = Snapshot.from_data(changed, hypotheses={'proposal': {'doc': self.document}})
        self.assertNotIn('api.limit', supplied.to_data()['context'].get('conflicts', {}))
        self.assertEqual(supplied.to_data()['nodes']['api.limit']['body']['v'], 9)
