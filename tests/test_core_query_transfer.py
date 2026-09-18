"""Stored query/v1 survives writers and complete contribution transfer."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import tempfile
import unittest

import yaml

from scripts import pending_grounding as G, project_modes as M, provenance as P
from scripts.reasoning.evaluate import Evaluator
from scripts.reasoning.runtime import Runtime
from scripts.reasoning.snapshot import Snapshot
from scripts.reasoning.contract import CapabilityError, digest


SCOPE = {'kind': 'project', 'environment': 'query transfer fixture'}


def document():
    return {
        'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
                               'requires': ['arithmetic/v1', 'query/v1']}},
        'parameters': {'p.base': {'v': 2, 'of': '2026-09-01'}},
        'items': {'item.a': {'amount': 3, 'enabled': True},
                  'item.b': {'amount': 5, 'enabled': False}},
        'scopes': {'scope.items': {'collection_scope': {
            'collection': 'items', 'fields': ['amount', 'enabled']}}},
        'calculations': {'m.total': {'rule': {'query': {
            'version': 1, 'scope': 'scope.items', 'op': 'sum',
            'value': {'column': 'amount'}}}}},
        'decisions': {'d.access': {'rests_on': ['p.base'],
            'verdict': 'Use the accessible path',
            'because': 'A qualitative preference',
            'reopened_by': 'Participants request another route',
            'seen': {'p.base': 2}}},
    }


def query_value(doc, native):
    result = Evaluator(Snapshot.from_data(doc), runtime=native).evaluate(
        {'ref': 'm.total'}, declared=['m.total'])
    if result['status'] != 'ok':
        raise AssertionError(result)
    return int(result['value']['fields']['result']['numerator'])


class CoreQueryTransfer(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = Runtime()

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.entry = self.root / 'GROUNDING.yaml'

    def write(self, action):
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.entry)], action)
        return yaml.safe_load(self.entry.read_text())

    @unittest.skipUnless(os.name == 'posix', 'record writers require POSIX locks')
    def test_public_writer_preserves_stored_query_during_unrelated_set(self):
        self.entry.write_text(yaml.safe_dump(document(), sort_keys=False))
        changed = self.write({'kind': 'set', 'id': 'p.base', 'value': 3,
                              'as_of': '2026-09-02'})
        self.assertEqual(changed['parameters']['p.base']['v'], 3)
        self.assertEqual(query_value(changed, self.native), 8)

    @unittest.skipUnless(os.name == 'posix', 'record writers require POSIX locks')
    def test_public_writer_adds_and_executes_stored_query(self):
        source = document()
        source['calculations'] = {'m.seed': {'rule': {'ref': 'p.base'}}}
        self.entry.write_text(yaml.safe_dump(source, sort_keys=False))
        changed = self.write({'kind': 'add', 'id': 'm.total', 'into': 'calculations',
                              'body': {'rule': {'query': {
                                  'version': 1, 'scope': 'scope.items', 'op': 'sum',
                                  'value': {'column': 'amount'}}}}})
        self.assertEqual(query_value(changed, self.native), 8)

    def test_contribution_captures_query_scope_and_every_member(self):
        source = document()
        source['calculations']['m.total']['scope'] = copy.deepcopy(SCOPE)
        bundle = G.prepare(source, ['m.total'], scope=SCOPE, shareability='project')
        captured = bundle['manifest']['document']
        self.assertEqual(set(G.entries(captured)),
                         {'m.total', 'scope.items', 'item.a', 'item.b'})
        self.assertEqual(query_value(captured, self.native), 8)

    @unittest.skipUnless(os.name == 'posix', 'durable contribution writes require POSIX locks')
    def test_store_round_trip_retains_query_and_binds_membership(self):
        source = document()
        source['calculations']['m.total']['scope'] = copy.deepcopy(SCOPE)
        bundle = G.prepare(source, ['m.total'], scope=SCOPE, shareability='project')
        repo = self.root / 'repo'
        repo.mkdir()
        M.git(repo, 'init', '-b', 'main')
        M.git(repo, 'config', 'user.name', 'Test')
        M.git(repo, 'config', 'user.email', 'test@example.test')
        (repo / 'base').write_text('base\n')
        M.git(repo, 'add', 'base')
        M.git(repo, '-c', 'commit.gpgsign=false', 'commit', '-m', 'base')
        store = G.Store(repo)
        receipt = store.capture(bundle, event_id='query', contribution_id='query',
                                shareability='project')
        replay = store.read_bundle(receipt['revision'])
        self.assertEqual(query_value(replay['manifest']['document'], self.native), 8)
        changed = copy.deepcopy(source)
        changed['items']['item.c'] = {'amount': 7, 'enabled': True}
        self.assertFalse(G.equivalent(replay, changed, {}))

    @unittest.skipUnless(os.name == 'posix', 'record writers require POSIX locks')
    def test_malformed_or_missing_query_scope_refuses_without_writing(self):
        for rule in (
                {'query': {'version': 1, 'scope': 'scope.missing', 'op': 'sum',
                           'value': {'column': 'amount'}}},
                {'query': {'version': 1, 'scope': 'scope.items', 'op': 'sum',
                           'value': {'column': 'missing'}}}):
            with self.subTest(rule=rule):
                source = document()
                del source['calculations']
                self.entry.write_text(yaml.safe_dump(source, sort_keys=False))
                before = self.entry.read_bytes()
                with self.assertRaises(P.Refused):
                    self.write({'kind': 'add', 'id': 'm.bad', 'into': 'calculations',
                                'body': {'rule': rule}})
                self.assertEqual(self.entry.read_bytes(), before)

    def test_bundle_with_omitted_captured_member_fails_validation(self):
        source = document()
        source['calculations']['m.total']['scope'] = copy.deepcopy(SCOPE)
        bundle = G.prepare(source, ['m.total'], scope=SCOPE, shareability='project')
        forged = copy.deepcopy(bundle)
        del forged['manifest']['document']['items']['item.b']
        with self.assertRaisesRegex(ValueError, 'identity|closure'):
            G.validate_bundle(forged)

    def test_historical_query_is_validated_from_retained_scope_basis(self):
        source = document()
        result = Evaluator(Snapshot.from_data(source), runtime=self.native).evaluate(
            {'ref': 'm.total'}, declared=['m.total'])
        self.assertEqual(result['status'], 'ok', result)
        historical = {'version': 2, 'value': result['value'], 'basis': result['basis'],
                      'rule': copy.deepcopy(source['calculations']['m.total']['rule'])}
        source['calculations']['m.total'] = {'v': 8}
        source['decisions']['d.query'] = {
            'rests_on': ['m.total'], 'verdict': 'Retain the measured total',
            'because': 'The frozen query result was accepted',
            'reopened_by': 'The retained basis is invalid',
            'seen': {'m.total': {'computed': historical}}}
        source['scopes']['scope.items']['collection_scope']['fields'] = ['enabled']
        G.document_capabilities(source)

        forged = copy.deepcopy(source)
        basis = forged['decisions']['d.query']['seen']['m.total']['computed']['basis']
        basis['scope']['fields'] = ['enabled']
        basis['scope']['digest'] = digest({key: value for key, value in basis['scope'].items()
                                          if key != 'digest'})
        basis['digest'] = digest({key: value for key, value in basis.items() if key != 'digest'})
        with self.assertRaises(CapabilityError) as refused:
            G.document_capabilities(forged)
        self.assertEqual(refused.exception.code, 'invalid_history')


if __name__ == '__main__':
    unittest.main()
