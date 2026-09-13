"""Live proposals, frozen writes and privacy at actual file/Git/session boundaries."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from scripts import provenance as P, pending_grounding as G, project_modes as M
from scripts import knowledge_views as V, recording as R
from tests.test_pending_grounding import Repository, fixture_bundle


class Views(Repository):
    def setUp(self):
        super().setUp()
        self.record = self.root / 'GROUNDING.yaml'
        self.doc = {'known': {'local.extra': {'v': 1, 'from': 'measurement'}}}
        self.record.write_text(P.yaml.safe_dump(self.doc))
        M.git(self.root, 'add', '.')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Record')

    def test_live_and_frozen_read_separate_and_never_mutate_checkout(self):
        before = self.record.read_bytes()
        receipt = self.capture()
        live = P.load([str(self.record)])
        self.assertEqual(len(live.contributions), 1)
        self.assertIn('api.limit', live.hypotheses['pending-' + receipt['revision']]['ids'])
        self.assertNotIn('api.limit', P.bodies(live))
        self.assertEqual(P.hypothesis_line(live), '')
        self.assertEqual(live.hypotheses['pending-' + receipt['revision']]['kind'], 'contribution')
        frozen = P.load([str(self.record)], read_mode='frozen')
        self.assertFalse(frozen.hypotheses)
        self.assertEqual(before, self.record.read_bytes())
        for command, args in [('open', []), ('pull', ['api.limit']), ('check', [])]:
            out = subprocess.run([sys.executable, 'scripts/cli.py', '--workspace', str(self.root), command, *args],
                                 cwd=Path(__file__).resolve().parents[1], text=True, capture_output=True)
            self.assertEqual(out.returncode, 0, out.stderr)
            self.assertIn('PENDING', out.stdout)
            self.assertIn('captured locally', out.stdout)

    def test_raw_mutation_lock_excludes_overlay_from_every_writer(self):
        self.capture()
        with P._locked(str(self.record)):
            self.assertEqual(P.load([str(self.record)]).read_mode, 'frozen')
            self.assertFalse(P.load([str(self.record)]).hypotheses)
        self.assertTrue(P.load([str(self.record)]).hypotheses)

    def test_full_body_conflict_inherited_and_hypotheses(self):
        bundle = fixture_bundle()
        self.record.write_text(P.yaml.safe_dump(bundle['manifest']['document']))
        self.capture(bundle)
        self.assertFalse(P.load([str(self.record)]).knowledge_conflicts)
        doc = copy.deepcopy(bundle['manifest']['document'])
        doc['sources']['s.vendor']['read'] = '2026-09-13'
        self.record.write_text(P.yaml.safe_dump(doc))
        live = P.load([str(self.record)])
        self.assertIn('s.vendor', live.knowledge_conflicts)
        self.assertIn('s.vendor', P.contested(live))
        hyp = Path(P.hypothesis_path([str(self.record)], 'alternate'))
        hyp.parent.mkdir(parents=True)
        hyp.write_text(P.yaml.safe_dump({'known': {'api.limit': {'v': 9, 'from': 's.vendor'}}}))
        self.assertIn('api.limit', P.load([str(self.record)]).knowledge_conflicts)

    def test_identical_entry_body_with_different_schema_remains_visible_conflict(self):
        doc = {'known': {'fact.one': {'v': 1, 'from': 'source'}},
               'judgments': {'claim.one': {'rests_on': ['fact.one'], 'seen': {'fact.one': 1},
                                           'wrong_if': 'fact.one < 0', 'verdict': 'holds'}}}
        bundle = G.prepare(doc, ['claim.one'], scope={'kind': 'project', 'environment': 'shared'},
                           shareability='project')
        self.capture(bundle)
        doc['schema'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        self.record.write_text(P.yaml.safe_dump(doc))
        self.assertIn('claim.one', P.load([str(self.record)]).knowledge_conflicts)

    def test_pending_survives_removed_origin_and_new_worktree(self):
        feature = self.worktree()
        store = G.Store(feature)
        store.capture(fixture_bundle(), event_id='other', contribution_id='limit', shareability='project')
        M.git(self.root, 'worktree', 'remove', str(feature))
        replacement = self.worktree('replacement')
        self.assertEqual(len(P.load([str(replacement / 'GROUNDING.yaml')]).contributions), 1)

    def test_portable_snapshot_has_exact_bytes_and_frozen_reproduction(self):
        receipt = self.capture()
        snapshot = self.base / 'snapshot'
        result = V.materialize(self.root, receipt['revision'], snapshot)
        self.assertEqual(result['read_mode'], 'frozen')
        self.assertEqual((snapshot / 'evidence/vendor.txt').read_bytes(), fixture_bundle()['files']['evidence/vendor.txt'])
        doc = P.load([str(snapshot / 'GROUNDING.yaml')], read_mode='frozen')
        self.assertEqual(dict(doc), fixture_bundle()['manifest']['document'])
        self.capture(fixture_bundle(value=20), event_id='next')
        self.assertEqual(dict(P.load([str(snapshot / 'GROUNDING.yaml')], read_mode='frozen')), dict(doc))
        with self.assertRaises(ValueError):
            V.materialize(self.root, receipt['revision'], snapshot)

    def test_checked_session_revision_changes_with_pending_and_frozen_does_not(self):
        try:
            from scripts.session.store import native_record
            from scripts.session.model import digest
        except ImportError:
            self.skipTest('session extra unavailable')
        reader = Path(P.__file__)
        before = native_record(self.record, reader, read_mode='frozen')
        self.capture()
        live = native_record(self.record, reader)
        self.assertIn('api.limit', live['nodes'])
        self.assertIn('pending', live['nodes']['api.limit']['states'])
        self.assertIn('s.vendor', live['nodes'])
        source = live['nodes']['api.limit']['record_source']
        self.assertTrue(source.startswith('contribution.'))
        self.assertIn('api.limit', live['sources'][source]['text'])
        self.assertTrue(live['sources'][source]['location'].startswith('git:'))
        self.assertEqual(digest(before), digest(native_record(self.record, reader, read_mode='frozen')))
        self.assertNotEqual(digest(before), digest(live))

    def test_capture_before_first_checkout_record_is_readable(self):
        self.record.unlink()
        self.capture()
        live = P.load([str(self.record)])
        self.assertTrue(live.contributions)
        with contextlib.redirect_stdout(io.StringIO()) as out:
            P.opening([str(self.record)])
        self.assertIn('PENDING', out.getvalue())
        self.assertFalse(self.record.exists())


class Routing(Views):
    def action(self, **extra):
        return dict(kind='add', id='project.finding', body={'v': 10, 'from': 'document'}, **extra)

    def test_explicit_project_write_acknowledges_git_without_checkout_mutation(self):
        before = self.record.read_bytes()
        with contextlib.redirect_stdout(io.StringIO()) as out:
            P.apply([str(self.record)], self.action(shareability='project', scope='external',
                                                  environment='API v2', event_id='capture'))
        receipt = json.loads(out.getvalue())
        self.assertEqual(receipt['state'], 'captured')
        self.assertEqual(before, self.record.read_bytes())
        self.assertEqual(G.Store(self.root).head(), receipt['ledger_commit'])

    def test_private_and_unclear_routes_retain_structured_draft_without_git_objects(self):
        home = self.base / 'private'
        before = M.git(self.root, 'count-objects', '-v').stdout
        for index, action in enumerate([self.action(scope='project', environment='project'),
                                        self.action(shareability='private'),
                                        self.action(shareability='project', scope='project', environment='project')]):
            if index == 2:
                action['body']['privacy'] = 'private'
            with patch.dict(os.environ, {'KPOPPER_PRIVATE_HOME': str(home)}):
                receipt = R.route([str(self.record)], action, P)
            self.assertEqual(receipt['state'], 'private draft')
            draft = G._decode(json.loads(Path(receipt['path']).read_bytes()))
            self.assertEqual(draft['action']['body']['v'], 10)
        self.assertEqual(before, M.git(self.root, 'count-objects', '-v').stdout)
        self.assertIsNone(G.Store(self.root).head())

    def test_private_source_overrides_project_declaration_and_legacy_write(self):
        self.doc['sources'] = {'s.private': {'name': 'private evidence', 'privacy': 'private'}}
        self.record.write_text(P.yaml.safe_dump(self.doc))
        before = self.record.read_bytes()
        for extra in ({}, {'shareability': 'project', 'scope': 'project', 'environment': 'all'}):
            action = self.action(**extra)
            action['body']['from'] = 's.private'
            with patch.dict(os.environ, {'KPOPPER_PRIVATE_HOME': str(self.base / 'private')}):
                receipt = R.route([str(self.record)], action, P)
            self.assertEqual(receipt['state'], 'private draft')
        self.assertEqual(before, self.record.read_bytes())

    def test_feature_commit_measurement_remains_fact_without_review_refresh(self):
        commit = M.git(self.root, 'rev-parse', 'HEAD').stdout.decode().strip()
        action = self.action(scope='code', shareability='project', environment='test command', commit=commit)
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.record)], action)
        body = P.bodies(P.load([str(self.record)]))['project.finding']
        self.assertEqual(body['v'], 10)
        self.assertEqual(body['scope']['commit'], commit)
        self.assertNotIn('seen', body)
        self.assertIsNone(G.Store(self.root).head())

    def test_legacy_local_write_has_no_implicit_capture(self):
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.record)], self.action())
        self.assertIn('project.finding', P.bodies(P.load([str(self.record)])))
        self.assertIsNone(G.Store(self.root).head())

    def test_explicit_import_retains_private_source_locator_without_git_write(self):
        legacy = self.base / 'legacy.yaml'
        legacy.write_text(P.yaml.safe_dump({'sources': {'s.old': {'location': 'file:///private/source.txt'}},
                                            'known': {'fact.old': {'v': 10, 'from': 's.old'}}}))
        before = legacy.read_bytes()
        command = [sys.executable, str(Path(P.__file__).with_name('cli.py')),
                   '--workspace', str(self.root), 'knowledge', 'import', str(legacy),
                   '--shareability', 'project', '--scope', 'external', '--environment', 'vendor']
        out = subprocess.run(command, text=True, capture_output=True,
                             env=dict(os.environ, KPOPPER_PRIVATE_HOME=str(self.base / 'private')))
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(json.loads(out.stdout)['state'], 'private draft')
        self.assertEqual(before, legacy.read_bytes())
        self.assertIsNone(G.Store(self.root).head())

    def test_invalid_private_marker_is_retained_privately(self):
        action = self.action(shareability='project', scope='project', environment='project')
        action['body']['private'] = 'unknown'
        with patch.dict(os.environ, {'KPOPPER_PRIVATE_HOME': str(self.base / 'private')}):
            receipt = R.route([str(self.record)], action, P)
        self.assertEqual(receipt['state'], 'private draft')
        self.assertIsNone(G.Store(self.root).head())

    def test_configuration_then_writer_does_not_recursively_acquire_policy_lock(self):
        command = "from scripts import project_modes as M, provenance as P; p=M.Project(%r); p.configure('advanced'); P.apply([str(p.record())], {'kind':'add','id':'other.finding','body':{'v':2,'from':'source'}})" % str(self.root)
        out = subprocess.run([sys.executable, '-c', command], text=True, capture_output=True, timeout=10)
        self.assertEqual(out.returncode, 0, out.stderr)


class Simple(unittest.TestCase):
    def test_shared_record_and_named_hypothesis_are_not_per_session(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            record = root / 'GROUNDING.yaml'
            record.write_text(P.yaml.safe_dump({'known': {'fact.one': {'v': 1, 'from': 'source'}}}))
            for nid in ('fact.two', 'fact.three'):
                with contextlib.redirect_stdout(io.StringIO()):
                    P.apply([str(record)], {'kind': 'add', 'id': nid,
                        'body': {'v': 2, 'from': 'source'}, 'hypothesis': 'joint',
                        'scope': 'feature', 'shareability': 'project'})
            doc = P.load([str(record)])
            self.assertEqual(set(doc.hypotheses), {'joint'})
            self.assertEqual(doc.hypotheses['joint']['ids'], {'fact.two', 'fact.three'})
            self.assertEqual(set(P.bodies(doc)), {'fact.one'})
            self.assertEqual(M.Project(root).config()['mode'], 'simple')

    def test_simple_config_routes_low_level_legacy_root_calls_to_shared_record(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve()
            repo = root / 'repo'
            repo.mkdir()
            M.git(repo, 'init', '-b', 'trunk')
            local = repo / 'GROUNDING.yaml'
            shared = root / 'shared.yaml'
            original = P.yaml.safe_dump({'known': {'fact.one': {'v': 1, 'from': 'source'}}})
            local.write_text(original)
            shared.write_text(original)
            M.Project(repo).configure('simple', record=str(shared))
            with contextlib.redirect_stdout(io.StringIO()):
                P.apply([str(local)], {'kind': 'add', 'id': 'fact.two', 'body': {'v': 2, 'from': 'source'}})
            self.assertEqual(local.read_text(), original)
            self.assertIn('fact.two', P.bodies(P.load([str(shared)])))
            self.assertIn('fact.two', P.bodies(P.load([str(local)])))
            self.assertNotIn('fact.two', P.bodies(P.load([str(local)], read_mode='frozen')))


if __name__ == '__main__':
    unittest.main()
