"""Regressions discovered by independent review of integrated project modes."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts import pending_grounding as G, project_modes as M, knowledge_views as V, recording as R
from scripts.session.store import native_record
from tests import test_pending_publication as F


class ReviewRegressions(unittest.TestCase):
    def setUp(self):
        F.PublicationTests.setUp(self)
        runtime_publisher = G.P._peer('pending_publication')
        self.addCleanup(patch.stopall)
        patch.object(runtime_publisher, 'trigger_after_capture', return_value={'started': False, 'reason': 'fixture'}).start()

    def apply(self, action):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            result = G.P.apply([str(self.root / 'GROUNDING.yaml')], action)
        return result, out.getvalue()

    def seed(self):
        doc = {'known': {'fact.one': {'v': 1, 'from': 'old source', 'at': 'old-section'}},
               'judgments': {'claim.one': {'rests_on': ['fact.one'], 'seen': {'fact.one': 1},
                                          'wrong_if': 'fact.one < 0', 'verdict': 'holds'}}}
        (self.root / 'GROUNDING.yaml').write_text(G.P.yaml.safe_dump(doc))
        return doc

    def test_local_set_and_review_keep_working(self):
        self.seed()
        self.assertEqual(self.apply({'kind': 'set', 'id': 'fact.one', 'value': 2})[0], 0)
        self.assertEqual(self.apply({'kind': 'review', 'id': 'claim.one'})[0], 0)
        doc = G.P.yaml.safe_load((self.root / 'GROUNDING.yaml').read_text())
        self.assertEqual(doc['known']['fact.one']['v'], 2)
        self.assertEqual(doc['judgments']['claim.one']['seen']['fact.one'], 2)

    def test_routed_set_preserves_value_kind_citation_date_and_scope(self):
        doc = self.seed()
        doc['known']['fact.one'] = {'quoted': 'old quotation', 'from': 'old', 'at': 'old-section'}
        doc.pop('judgments')
        (self.root / 'GROUNDING.yaml').write_text(G.P.yaml.safe_dump(doc))
        code, output = self.apply({'kind': 'set', 'id': 'fact.one', 'value': 'new quotation',
                                   'source': 'new', 'at': 'new-section', 'as_of': '2026-09-14',
                                   'scope': 'external', 'environment': 'account A', 'shareability': 'project'})
        self.assertEqual(code, 0)
        receipt = json.loads(output)
        body = G.entries(self.store.read_bundle(receipt['revision'])['manifest']['document'])['fact.one'][1]
        self.assertEqual(body, {'quoted': 'new quotation', 'from': 'new', 'at': 'new-section',
                                'of': '2026-09-14', 'scope': {'kind': 'external', 'environment': 'account A'}})
        self.assertEqual(G.P.yaml.safe_load((self.root / 'GROUNDING.yaml').read_text()), doc)

    def test_scope_is_required_at_capture_and_preserved_by_snapshot(self):
        scope = {'kind': 'external', 'environment': 'account A'}
        doc = {'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
               'sources': {'s.vendor': {'name': 'Vendor', 'labels': ['shareable']}},
               'known': {'fact.one': {'v': 1, 'from': 's.vendor'}}}
        with self.assertRaisesRegex(ValueError, 'exact scope'):
            G.prepare(doc, ['fact.one'], scope=scope, shareability='project')
        doc['known']['fact.one']['scope'] = scope
        bundle = G.prepare(doc, ['fact.one'], scope=scope, shareability='project')
        other = copy.deepcopy(doc)
        other['known']['fact.one']['scope']['environment'] = 'account B'
        self.assertFalse(G.equivalent(bundle, other, {}))
        receipt = self.store.capture(bundle, event_id='scoped', contribution_id='one', shareability='project')
        snapshot = V.materialize(self.project, receipt['revision'], self.base / 'snapshot')
        with patch.dict(os.environ, {'KPOPPER_READ_MODE': 'frozen'}), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(G.P.check([snapshot['record']]), 0)
        exported = G.P.yaml.safe_load(Path(snapshot['record']).read_text())
        self.assertEqual(exported['known']['fact.one']['scope'], scope)
        self.assertEqual(exported['sources'], doc['sources'])
        malformed = copy.deepcopy(exported)
        malformed['judgments'] = {'broken': {'rests_on': ['missing'], 'seen': {}, 'wrong_if': 'missing < 0'}}
        ids, judgments, fields = G.P.infer(malformed)
        self.assertTrue(G.P.flags(ids, judgments, fields, G.P.with_builtins(malformed, ids, judgments, fields)))

    def test_simple_and_feature_local_writes_keep_declared_scope(self):
        self.seed()
        for kind in ('feature', 'unclear'):
            code, _ = self.apply({'kind': 'set', 'id': 'fact.one', 'value': 2,
                                 'scope': kind, 'environment': 'feature checkout', 'shareability': 'project'})
            self.assertEqual(code, 0)
            body = G.P.yaml.safe_load((self.root / 'GROUNDING.yaml').read_text())['known']['fact.one']
            self.assertEqual(body['scope'], {'kind': kind, 'environment': 'feature checkout'})
        shared = self.base / 'shared.yaml'
        shared.write_bytes((self.root / 'GROUNDING.yaml').read_bytes())
        self.project.configure('simple', record=str(shared))
        code, _ = self.apply({'kind': 'add', 'id': 'fact.two', 'body': {'v': 2, 'from': 'public source'},
                             'scope': 'external', 'environment': 'account A', 'shareability': 'project'})
        self.assertEqual(code, 0)
        self.assertEqual(G.P.yaml.safe_load(shared.read_text())['known']['fact.two']['scope']['environment'], 'account A')

    def test_mode_change_after_routing_cannot_write_retired_destination(self):
        self.seed()
        shared = self.base / 'shared.yaml'
        shared.write_bytes((self.root / 'GROUNDING.yaml').read_bytes())
        self.project.configure('simple', record=str(shared))
        before = shared.read_bytes()
        routing = G.P._peer('recording')
        route = routing.route

        def change_mode(*args, **kwargs):
            result = route(*args, **kwargs)
            self.project.configure('advanced', record='GROUNDING.yaml')
            return result

        with patch.object(routing, 'route', side_effect=change_mode):
            with self.assertRaisesRegex(G.P.Refused, 'destination changed'):
                self.apply({'kind': 'add', 'id': 'fact.two', 'body': {'v': 2, 'from': 'source'}})
        self.assertEqual(shared.read_bytes(), before)
        self.assertNotIn('fact.two', (self.root / 'GROUNDING.yaml').read_text())

    def test_checked_simple_sources_match_resolved_record_bytes(self):
        self.seed()
        shared = self.base / 'shared.yaml'
        shared.write_bytes((self.root / 'GROUNDING.yaml').read_bytes())
        self.project.configure('simple', record=str(shared))
        doc = G.P.yaml.safe_load(shared.read_text())
        doc['known']['fact.two'] = {'v': 2, 'from': 'public source'}
        shared.write_text(G.P.yaml.safe_dump(doc))
        result = native_record(self.root / 'GROUNDING.yaml', Path(G.P.__file__))
        self.assertIn('fact.two', result['nodes'])
        self.assertEqual(result['sources']['record']['text'], shared.read_text())
        self.assertEqual(result['sources']['record']['location'], str(shared))

    def test_mode_change_before_route_cannot_capture_from_retired_path(self):
        self.seed()
        shared = self.base / 'shared.yaml'
        shared.write_bytes((self.root / 'GROUNDING.yaml').read_bytes())
        self.project.configure('simple', record=str(shared))
        routing = G.P._peer('recording')
        route = routing.route

        def change_before_route(*args, **kwargs):
            self.project.configure('advanced', record='GROUNDING.yaml')
            return route(*args, **kwargs)

        with patch.object(routing, 'route', side_effect=change_before_route):
            with self.assertRaisesRegex(G.P.Refused, 'destination changed'):
                self.apply({'kind': 'add', 'id': 'fact.two', 'body': {'v': 2, 'from': 'source'},
                            'scope': 'project', 'environment': 'project', 'shareability': 'project'})
        self.assertIsNone(self.store.head())

    def test_custom_schema_cannot_hide_a_missing_dependency_declaration(self):
        for body in ({'depends_on': ['missing.fact'], 'reviewed': {}, 'invalid_when': 'missing.fact < 1', 'v': 'holds'},
                     {'depends_on': ['missing.fact'], 'v': 'holds'}):
            doc = {'schema': {'deps': 'relies_on', 'snapshot': 'reviewed', 'predicate': 'invalid_when'},
                   'known': {'claim.one': body}}
            with self.assertRaisesRegex(SystemExit, 'nothing this reader'):
                G.P.infer(doc)

    def test_record_level_private_permission_survives_selected_closure(self):
        scope = {'kind': 'project', 'environment': 'project'}
        doc = {'meta': {'privacy': 'private'},
               'known': {'fact.one': {'v': 'PRIVATE-RECORD-SECRET', 'from': 'source', 'scope': scope}}}
        (self.root / 'GROUNDING.yaml').write_text(G.P.yaml.safe_dump(doc))
        with self.assertRaisesRegex(ValueError, 'private'):
            G.prepare(doc, ['fact.one'], scope=scope, shareability='project')
        with patch.dict(os.environ, {'KPOPPER_PRIVATE_HOME': str(self.base / 'private-drafts')}):
            code, output = self.apply({'kind': 'set', 'id': 'fact.one', 'value': 'PRIVATE-RECORD-SECRET',
                                      'scope': scope, 'shareability': 'project'})
            self.assertEqual(code, 0)
            receipt = json.loads(output)
            self.assertEqual(receipt['state'], 'private draft')
            payload = G._decode(json.loads(Path(receipt['path']).read_text()))
            self.assertEqual(payload['document']['meta']['privacy'], 'private')
        self.assertIsNone(self.store.head())

    def test_affects_names_reached_hypotheses_and_contributions(self):
        doc = self.seed()
        path = self.root / '.kpopper/hypotheses/proposal.yaml'
        path.parent.mkdir(parents=True)
        path.write_text('judgments:\n  claim.new: {rests_on: [fact.one], seen: {fact.one: 1}, wrong_if: "fact.one < 0"}\n')
        doc['judgments']['claim.pending'] = copy.deepcopy(doc['judgments']['claim.one'])
        doc['judgments']['claim.pending']['scope'] = {'kind': 'project', 'environment': 'this project'}
        bundle = G.prepare(doc, ['claim.pending'], scope={'kind': 'project', 'environment': 'this project'}, shareability='project')
        self.store.capture(bundle, event_id='reach', contribution_id='claim', shareability='project')
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(G.P.affects([str(self.root / 'GROUNDING.yaml')], ['fact.one']), 0)
        self.assertIn('hypothesis proposal', out.getvalue())
        self.assertIn('contribution pending-', out.getvalue())
