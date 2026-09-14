"""Main's authoring/report writers must preserve project-mode boundaries."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts import ingestion as I, pending_grounding as G, project_modes as M, expression_cli as X


class ModeWriters(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name).resolve()
        self.root = self.base / 'project'; self.root.mkdir()
        M.git(self.root, 'init', '-b', 'main')
        M.git(self.root, 'config', 'user.name', 'Fixture')
        M.git(self.root, 'config', 'user.email', 'fixture@example.invalid')
        self.record = self.root / 'GROUNDING.yaml'
        self.doc = {'sources': {'s.one': {'name': 'Public source', 'url': 'https://example.test/one'}},
                    'known': {'fact.x': {'v': 1, 'from': 's.one'}, 'fact.y': {'v': 2, 'from': 's.one'}},
                    'judgments': {'claim.good': {'rests_on': ['fact.x'], 'seen': {'fact.x': 1},
                                                'verdict': 'holds', 'wrong_if': 'fact.x < 0'}}}
        self.write()
        M.git(self.root, 'add', 'GROUNDING.yaml')
        M.git(self.root, '-c', 'commit.gpgsign=false', 'commit', '-m', 'Fixture')
        self.project = M.Project(self.root)
        self.addCleanup(patch.stopall)
        patch.dict(os.environ, {'XDG_STATE_HOME': str(self.base / 'state'),
                                'KPOPPER_PRIVATE_HOME': str(self.base / 'private'),
                                'KPOPPER_SESSION_DISABLE': '1'}).start()

    def write(self):
        self.record.write_text(I.P.yaml.safe_dump(self.doc, sort_keys=False))

    def report(self, **extra):
        return dict(source_quote='The authorized report changes x to 3 and y to 4.', date='2026-09-14',
                    updates=[{'kind': 'set', 'id': 'fact.x', 'value': 3},
                             {'kind': 'set', 'id': 'fact.y', 'value': 4}], **extra)

    def capture(self, body, name='pending.fact'):
        scope = {'kind': 'external', 'environment': 'account A'}
        doc = copy.deepcopy(self.doc)
        doc.setdefault('known', {})[name] = dict(body, scope=scope)
        return G.Store(self.project).capture(G.prepare(doc, [name], scope=scope, shareability='project'),
                                             event_id=name, contribution_id=name, shareability='project')

    def test_private_original_source_refuses_entire_atomic_batch(self):
        self.doc['sources']['s.private'] = {'name': 'SECRET-SOURCE', 'private': True}
        self.doc['known']['fact.y']['from'] = 's.private'; self.write()
        before = self.record.read_bytes()
        result = I.update(self.report(), self.record)
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertEqual(self.record.read_bytes(), before)
        self.assertTrue(Path(result['source_file']).is_file())
        self.assertIsNone(G.Store(self.project).head())

    def test_pending_overlay_does_not_block_local_atomic_update_or_leak_to_yaml(self):
        self.capture({'v': 9, 'from': 's.one'})
        result = I.update(self.report(), self.record)
        self.assertEqual(result['state'], 'applied', result)
        doc = I.P.yaml.safe_load(self.record.read_text())
        self.assertEqual(doc['known']['fact.x']['v'], 3)
        self.assertNotIn('pending.fact', doc['known'])

    def test_explicit_project_report_captures_one_portable_batch_without_yaml_write(self):
        before = self.record.read_bytes()
        scope = {'kind': 'external', 'environment': 'account A'}
        result = I.update(self.report(shareability='project', scope=scope), self.record)
        self.assertEqual(result['state'], 'project_captured', result)
        self.assertEqual(self.record.read_bytes(), before)
        snap = G.Store(self.project).snapshot()
        self.assertEqual(len(snap['events']), 1)
        bundle = next(iter(snap['bundles'].values()))
        self.assertEqual(set(bundle['manifest']['roots']), {'fact.x', 'fact.y'})
        for key, value in (('fact.x', 3), ('fact.y', 4)):
            body = G.entries(bundle['manifest']['document'])[key][1]
            self.assertEqual((body['v'], body['scope']), (value, scope))
        self.assertEqual(list(bundle['files'].values()), [self.report()['source_quote'].encode()])
        self.assertNotIn(str(self.base), str(bundle['manifest']))

    def test_project_report_retry_returns_same_complete_durable_receipt(self):
        report = self.report(event_id='same-report', shareability='project',
                             scope={'kind': 'external', 'environment': 'account A'})
        first = I.update(report, self.record)
        second = I.update(report, self.record)
        self.assertEqual(second, first)
        self.assertEqual(len(G.Store(self.project).events()), 1)

    def test_git_capture_recovers_after_receipt_crash_and_later_checkout_change(self):
        report = self.report(event_id='crash-report', shareability='project',
                             scope={'kind': 'external', 'environment': 'account A'})
        finish = I._finish
        def crash(root, event, envelope, state, *args, **kwargs):
            if state == 'project_captured':
                raise KeyboardInterrupt('after Git capture, before ingestion receipt')
            return finish(root, event, envelope, state, *args, **kwargs)
        with patch.object(I, '_finish', side_effect=crash), self.assertRaises(KeyboardInterrupt):
            I.update(report, self.record)
        head = G.Store(self.project).head()
        self.assertIsNotNone(head)
        self.doc['known']['fact.x']['v'] = 8; self.write()
        result = I.update(report, self.record)
        self.assertEqual(result['state'], 'project_captured', result)
        self.assertEqual(G.Store(self.project).head(), head)
        self.assertEqual(I.P.yaml.safe_load(self.record.read_text())['known']['fact.x']['v'], 8)

    def test_policy_change_after_intake_retains_report_without_local_write(self):
        event = I.capture(self.report(), self.record, start=False)
        shared = self.base / 'shared.yaml'; shared.write_bytes(self.record.read_bytes())
        self.project.configure('simple', record=str(shared))
        before = self.record.read_bytes()
        result = I.process(self.record, event['state_dir'], event_id=event['event_id'])[0]
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('policy changed', result['reason'])
        self.assertEqual(self.record.read_bytes(), before)
        self.assertEqual(I.status(event['event_id'], self.record, event['state_dir'])['state'], 'needs_primary')

    def test_simple_capture_process_status_and_delivery_share_the_resolved_store(self):
        from scripts import ingestion_delivery as delivery
        shared = self.base / 'shared.yaml'; shared.write_bytes(self.record.read_bytes())
        self.project.configure('simple', record=str(shared))
        event = I.capture(self.report(), self.record, start=False)
        self.assertEqual(I.status(event['event_id'], self.record)['state'], 'captured')
        self.assertEqual(I.process(self.record, event_id=event['event_id'])[0]['state'], 'applied')
        self.assertEqual(I.status(event['event_id'], self.record)['state'], 'applied')
        self.assertEqual(I.state_path(self.record), I.state_path(shared))
        queued = delivery.capture(dict(self.report(), event_id='notify'), 'fixture-task', self.record, start=False)
        self.assertTrue(queued)
        _, state, exists = I._existing_layout(shared)
        self.assertTrue(exists)
        events = [I._load(p) for p in (state / 'events').glob('*.json')]
        self.assertTrue(all(e['project_root'] == str(self.root) for e in events))

    def test_locked_formula_write_rechecks_newly_private_dependencies(self):
        R = I.P._peer('recording')
        route = R.route
        def change_source(*args, **kwargs):
            receipt = route(*args, **kwargs)
            self.doc['sources']['s.one']['private'] = True; self.write()
            return receipt
        action = {'kind': 'add', 'id': 'fact.total', 'body': {'rule': 'fact.x + fact.y'}}
        with patch.object(R, 'route', side_effect=change_source), contextlib.redirect_stdout(io.StringIO()) as output:
            I.P.apply([str(self.record)], action)
        self.assertEqual(json.loads(output.getvalue())['state'], 'private draft')
        self.assertNotIn('fact.total', self.record.read_text())

    def test_source_permission_change_before_git_capture_is_not_published(self):
        G_runtime = I.P._peer('pending_grounding')
        capture = G_runtime.Store.capture
        def revoke_then_capture(store, *args, **kwargs):
            self.doc['meta'] = {'privacy': 'private'}; self.write()
            return capture(store, *args, **kwargs)
        with patch.object(G_runtime.Store, 'capture', revoke_then_capture):
            result = I.update(self.report(shareability='project', scope={'kind': 'external', 'environment': 'account A'}), self.record)
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIsNone(G.Store(self.project).head())

    def test_simple_explicit_checkout_path_updates_only_shared_record(self):
        shared = self.base / 'shared.yaml'; shared.write_bytes(self.record.read_bytes())
        self.project.configure('simple', record=str(shared))
        before = self.record.read_bytes()
        result = I.update(self.report(), self.record)
        self.assertEqual(result['state'], 'applied', result)
        self.assertEqual(self.record.read_bytes(), before)
        self.assertEqual(I.P.yaml.safe_load(shared.read_text())['known']['fact.x']['v'], 3)

    def test_simple_report_retains_explicit_scope(self):
        shared = self.base / 'shared.yaml'; shared.write_bytes(self.record.read_bytes())
        self.project.configure('simple', record=str(shared))
        scope = {'kind': 'external', 'environment': 'account A'}
        result = I.update(self.report(shareability='project', scope=scope), self.record)
        self.assertEqual(result['state'], 'applied', result)
        self.assertEqual(I.P.yaml.safe_load(shared.read_text())['known']['fact.x']['scope'], scope)

    def test_simple_expression_migration_resolves_shared_record(self):
        self.doc['known']['fact.total'] = {'rule': 'fact.x + fact.y'}; self.write()
        shared = self.base / 'shared.yaml'; shared.write_bytes(self.record.read_bytes())
        self.project.configure('simple', record=str(shared))
        before = self.record.read_bytes()
        result = X.migrate(str(self.record), apply=True, readable=True)
        self.assertTrue(result['applied'], result)
        self.assertEqual(self.record.read_bytes(), before)
        self.assertIsInstance(I.P.yaml.safe_load(shared.read_text())['known']['fact.total']['rule'], dict)

    def test_project_capture_normalizes_expression_and_keeps_dependencies(self):
        action = {'kind': 'add', 'id': 'fact.total', 'body': {'rule': 'fact.x + fact.y'},
                  'scope': 'external', 'environment': 'account A', 'shareability': 'project'}
        with contextlib.redirect_stdout(io.StringIO()):
            I.P.apply([str(self.record)], action)
        bundle = next(iter(G.Store(self.project).snapshot()['bundles'].values()))
        entries = G.entries(bundle['manifest']['document'])
        self.assertIsInstance(entries['fact.total'][1]['rule'], dict)
        self.assertTrue({'fact.x', 'fact.y', 's.one'} <= set(entries))

    def test_project_judgment_capture_fills_new_review_snapshot(self):
        action = {'kind': 'add', 'id': 'claim.new', 'into': 'judgments',
                  'body': {'rests_on': ['fact.x'], 'verdict': 'ready', 'wrong_if': 'fact.x < 0'},
                  'scope': 'external', 'environment': 'account A', 'shareability': 'project'}
        with contextlib.redirect_stdout(io.StringIO()):
            I.P.apply([str(self.record)], action)
        bundle = next(iter(G.Store(self.project).snapshot()['bundles'].values()))
        self.assertEqual(G.entries(bundle['manifest']['document'])['claim.new'][1]['seen'], {'fact.x': 1})

    def test_invalid_structured_expression_cannot_be_acknowledged_as_captured(self):
        action = {'kind': 'add', 'id': 'fact.total', 'body': {'rule': {'op': 'add', 'args': []}},
                  'scope': 'external', 'environment': 'account A', 'shareability': 'project'}
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises((ValueError, SystemExit)):
            I.P.apply([str(self.record)], action)
        self.assertIsNone(G.Store(self.project).head())
