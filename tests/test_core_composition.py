"""Real native composition through public writers and original T2 history bytes.

KPOPPER_COMPOSITION_TEST_ARCHIVE optionally selects a verified candidate archive.
These tests never substitute a Python evaluator or claim success without native KP3.
"""
import base64
import contextlib
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zlib

import yaml
from scripts import history_authoring as A, history_contract as C, history_store as H, history_transaction as T
from scripts import provenance as P
from scripts.reasoning import authoring, runtime
from scripts.reasoning.evaluate import Evaluator
from scripts.reasoning.snapshot import Snapshot


class CoreComposition(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.entry = self.root / 'GROUNDING.yaml'
        archive = os.environ.get('KPOPPER_COMPOSITION_TEST_ARCHIVE')
        if archive:
            original = runtime.Runtime.__init__
            def initialize(instance, supplied=None, **kwargs):
                return original(instance, supplied or archive, **kwargs)
            selection = patch.object(runtime.Runtime, '__init__', initialize)
            selection.start()
            self.addCleanup(selection.stop)
        self.native = runtime.Runtime()
        self.doc = {'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
            'requires': ['arithmetic/v1', 'composition/v1']}},
            'known': {'p.scalar': {'v': 1}, 'p.empty': {'v': {}},
                      'p.ratio': {'rule': {'expr': '1 / 3'}}}}

    def write(self, action):
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.entry)], action)
        return yaml.safe_load(self.entry.read_text())

    def test_read_only_runtime_routes_scalar_and_container_closures(self):
        engine = Evaluator(Snapshot.from_data(self.doc), runtime=self.native)
        container = engine.evaluate({'ref': 'p.empty'}, declared=['p.empty'])
        scalar = engine.evaluate({'ref': 'p.scalar'}, declared=['p.scalar'])
        self.assertEqual((container['status'], container['value']['type'],
                          container['implementation']['protocol']),
                         ('ok', 'record', 'KP3'))
        self.assertEqual((scalar['status'], scalar['value']['type'],
                          scalar['implementation']['protocol']),
                         ('ok', 'number', 'KP2'))

    @unittest.skipUnless(os.name == 'posix', 'record writers require POSIX locks')
    def test_public_authoring_infers_composition_and_retains_after_scalar_edit(self):
        original = copy.deepcopy(self.doc)
        original['meta']['reasoning']['requires'] = ['arithmetic/v1']
        del original['known']['p.empty']
        self.entry.write_text(yaml.safe_dump(original, sort_keys=False))
        doc = self.write({'kind': 'add', 'id': 'p.container', 'body': {'v': [True, None, {}, [1]]}})
        self.assertEqual(doc['meta']['reasoning']['requires'], ['arithmetic/v1', 'composition/v1'])
        doc = self.write({'kind': 'set', 'id': 'p.scalar', 'value': 2, 'as_of': '2026-09-18'})
        self.assertEqual(doc['meta']['reasoning']['requires'], ['arithmetic/v1', 'composition/v1'])
        result = Evaluator(Snapshot.from_data(doc), runtime=self.native).evaluate({'ref': 'p.container'}, declared=['p.container'])
        self.assertEqual(result['status'], 'ok', result)
        self.assertEqual(authoring.authored_value(result['value']), [True, None, {}, [1]])
        self.assertEqual(result['implementation']['protocol'], 'KP3')
        scalar = Evaluator(Snapshot.from_data(doc), runtime=self.native).evaluate({'ref': 'p.scalar'}, declared=['p.scalar'])
        self.assertEqual(scalar['implementation']['protocol'], 'KP2')

    @unittest.skipUnless(os.name == 'posix', 'record writers require POSIX locks')
    def test_real_dominance_diagnostics_cannot_bypass_writer_admission(self):
        self.entry.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        original = self.entry.read_bytes()
        for expression, blocked, succeeds in [
            ('false and missing', False, False), ('false and missing', True, True),
            ('false and (1 / 0 == 0)', True, False),
            ('false and field(p.empty, "missing")', True, True)]:
            action = {'kind': 'add', 'id': 'p.test', 'body': {'rule': {'expr': expression}}}
            if blocked:
                action['body']['blocked_on'] = 'await input'
            self.entry.write_bytes(original)
            if succeeds:
                self.write(action)
            else:
                with self.assertRaises(P.Refused):
                    self.write(action)
                self.assertEqual(self.entry.read_bytes(), original)

    def test_typed_native_comparison_does_not_confuse_records_with_rationals(self):
        doc = copy.deepcopy(self.doc)
        doc['known']['p.record'] = {'v': {'rational': ['1', '3']}}
        doc['known']['p.nested'] = {'rule': {'expr': '[1 / 3]'}}
        doc['known']['p.same'] = {'v': [{'rational': ['1', '3']}, True]}
        world = authoring.World(P, doc)
        self.assertFalse(world.same_value('p.ratio', {'rational': ['1', '3']}))
        self.assertFalse(world.same_value('p.nested', [{'rational': ['1', '3']}]))
        self.assertTrue(world.same_value('p.record', {'rational': ['1', '3']}))
        self.assertTrue(world.same_value('p.same', [{'rational': ['1', '3']}, True]))
        self.assertFalse(world.same_value('p.same', [{'rational': ['1', '3']}, 1]))

    def load_t2(self):
        fixture = json.loads((Path(__file__).parent / 'fixtures/history_composition_t2.json').read_text())
        self.assertEqual(fixture['source_revision'], 'e4deaf4ec3afd94fb96ddbdec6e5ddd1d190942b')
        raw = zlib.decompress(base64.b64decode(fixture['payload']))
        self.assertEqual(hashlib.sha256(raw).hexdigest(), fixture['sha256'])
        data = json.loads(raw)
        for name, contents in data['files'].items():
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(bytes.fromhex(contents))
        return T.PreparedMutation.from_bytes(bytes.fromhex(data['pending']))

    @unittest.skipUnless(os.name == 'posix', 'history mutation fixture is POSIX-bound')
    def test_original_t2_committed_history_accepts_new_composition_without_rewriting(self):
        self.load_t2()
        store = H.Store(self.entry)
        before = store.capture()
        old_basis = Evaluator(Snapshot.capture([str(self.entry)], read_mode='frozen'), runtime=self.native).basis('p.scalar')
        prepared = A.prepare(self.entry, {'kind': 'add', 'id': 'p.composed',
            'body': {'rule': {'expr': '[p.scalar, {"ok": true}]'}}, 'as_of': '2026-09-18'},
            operation='new-composition', recorded_at='2026-09-18T00:00:00Z')
        A.commit(self.entry, prepared, verify=lambda _: None)
        after = store.capture()
        self.assertEqual({key: after.object_bytes[key] for key in before.object_bytes}, before.object_bytes)
        self.assertEqual({key: after.commits[key] for key in before.commits}, before.commits)
        self.assertEqual(after.document['meta']['reasoning']['requires'], ['arithmetic/v1', 'composition/v1'])
        engine = Evaluator(Snapshot.capture([str(self.entry)], read_mode='frozen'), runtime=self.native)
        self.assertEqual(engine.basis('p.scalar'), old_basis)
        result = engine.evaluate({'ref': 'p.composed'}, declared=['p.composed'])
        self.assertEqual(authoring.authored_value(result['value']), [2, {'ok': True}])

    @unittest.skipUnless(os.name == 'posix', 'history mutation fixture is POSIX-bound')
    def test_original_t2_pending_receipt_cannot_rebind_to_new_native_implementation(self):
        pending = self.load_t2()
        before = H.Store(self.entry).capture()
        raw = pending.to_bytes()
        native = self.native.implementation
        original = A._recorded_adapter_audit(pending.to_data()['receipt'])
        self.assertNotIn(A._native_audit_key(native), original,
                         'This cross-version test requires the new composition runtime')
        with self.assertRaisesRegex(C.HistoryError, 'authoring_native_audit_mismatch'):
            A.verify_prepared(self.entry, pending)
        self.assertEqual(pending.to_bytes(), raw)
        self.assertEqual(H.Store(self.entry).capture().inventory, before.inventory)


if __name__ == '__main__':
    unittest.main()
