"""Required T2 regressions: retained also layouts and final-world batch validation.

These are success oracles for supported behavior, not tests that bless current
refusals. All records and migration destinations are disposable fixtures.
"""
import contextlib
import copy
import io
from pathlib import Path
import unittest

from scripts import history_authoring as A, history_contract as C, history_store as H
from scripts import history_migration as M, provenance as P
from scripts.reasoning.snapshot import Snapshot, SnapshotError
from tests import test_history_migration as migration_fixtures
from tests import test_history_authoring as authoring_fixtures
from tests import test_history_store as storage_fixtures


class RetainedAlsoLayout(unittest.TestCase):
    def setUp(self):
        fixture = migration_fixtures.Migration()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture, self.root = fixture, fixture.root

    def migrated(self, pointer):
        source = self.fixture.fixture(shape='pointer', core=True, archive=False)
        member = source.parent / 'data/values.yaml'
        original_member = member.read_bytes()
        source.write_bytes(C.encode_document({pointer: ['data/values.yaml']}))
        plan = M.prepare(source)
        self.assertFalse(plan.problems)
        destination = self.root / 'active-copy'
        plan.publish(destination)
        entry = destination / source.name
        captured = H.Store(entry).capture()
        self.assertEqual(captured.document[pointer], ['data/values.yaml'])
        mapped = captured.document['meta']['history_import']['members']
        self.assertIn({'path': 'data/values.yaml', 'sha256': C.sha256(original_member),
                       'role': 'retained_original'}, mapped)
        self.assertEqual(Snapshot.capture(entry, read_mode='frozen').to_data()['nodes']['p.input']['body']['v'], 2)
        return entry, destination / 'data/values.yaml', original_member

    def write_and_check(self, entry, member, original_member):
        before = H.Store(entry).capture()
        judgment = before.objects[before.state['subjects']['d.stable']['head']]
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                result = P.apply([str(entry)], {'kind': 'set', 'id': 'p.input', 'value': 3,
                                                'as_of': '2026-09-17'})
        finally:
            self.assertEqual(member.read_bytes(), original_member)
            self.assertEqual(H.Store(entry).capture().objects[judgment['id']], judgment)
        self.assertEqual(result, 0)
        after = H.Store(entry).capture()
        self.assertEqual(after.state['subjects']['p.input']['body']['v'], 3)
        self.assertEqual(after.objects[judgment['id']], judgment)
        self.assertEqual(member.read_bytes(), original_member)
        self.assertEqual(Snapshot.capture(entry, read_mode='frozen').to_data()['nodes']['p.input']['body']['v'], 3)

    def test_mapped_record_pointer_direct_write_control(self):
        self.write_and_check(*self.migrated('record'))

    def test_mapped_also_pointer_allows_same_direct_write_as_record_pointer(self):
        self.write_and_check(*self.migrated('also'))

    def test_mapped_member_changed_during_publication_verifier_refuses(self):
        entry, member, original_member = self.migrated('also')
        before = H.Store(entry).capture()
        mutation = A.prepare(entry, {'kind': 'set', 'id': 'p.input', 'value': 3, 'as_of': '2026-09-17'})
        with self.assertRaisesRegex(C.HistoryError, 'retained_history_mismatch'):
            A.commit(entry, mutation, verify=lambda data: member.write_bytes(original_member + b'# external edit\n'))
        after = H.Store(entry).capture()
        self.assertEqual(after.commits, before.commits)
        self.assertEqual(after.objects, before.objects)
        self.assertEqual(after.entry_bytes, before.entry_bytes)

    def test_unmapped_live_also_pointer_remains_unsupported(self):
        fixture = storage_fixtures.Storage()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        document = C.decode_document(fixture.entry.read_bytes())
        document['also'] = ['unbound.yaml']
        fixture.entry.write_bytes(C.encode_document(document))
        (fixture.entry.parent / 'unbound.yaml').write_bytes(C.encode_document({'readings': {'p.other': {'v': 9}}}))
        fixture.publish([authoring_fixtures.claim()], op='unmapped-bootstrap')
        with self.assertRaisesRegex(SnapshotError, 'history_composite_capture_unsupported'):
            Snapshot.capture(fixture.entry, read_mode='frozen')


class FinalWorldBatches(unittest.TestCase):
    def setUp(self):
        fixture = authoring_fixtures.Authoring()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.fixture, self.entry, self.store = fixture, fixture.entry, fixture.store
        self.old = authoring_fixtures.claim('d.old', kind='judgment', op='old-judgment', body={
            'verdict': 'older observation', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input < 0'},
            'seen': {'p.input': {'original_historical_value': 0}}}, pins={'p.input': fixture.original['id']})
        fixture.fixture.publish([self.old], op='old-judgment-bootstrap')
        self.baseline = self.store.capture()

    def prepared(self, actions, operation):
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                mutation = A.prepare_batch(self.entry, copy.deepcopy(actions), operation=operation,
                                           recorded_at='2026-09-17T12:00:00Z')
        finally:
            self.assertEqual(self.store.capture().objects[self.old['id']], self.old)
            self.assertEqual(self.store.capture().inventory, self.baseline.inventory)
        files = mutation.files
        final = C.decode_document(next(item['after'] for item in files if item['role'] == 'record'))
        self.assertEqual(final['judgments']['d.old']['seen'], self.old['body']['seen'])
        self.assertEqual(self.store.capture().objects[self.old['id']], self.old)
        self.assertEqual(self.store.capture().inventory, self.baseline.inventory)
        # New receipts capture the typed basis from the final world, even when
        # its dependency is authored after the judgment in the same batch.
        seen = final['judgments']['d.new']['seen']
        self.assertEqual(set(seen), set(final['judgments']['d.new']['rests_on']))
        for dependency, observation in seen.items():
            self.assertEqual(observation['computed']['value'], {
                'type': 'number', 'numerator': str(final['readings'][dependency]['v']),
                'denominator': '1'})
            self.assertEqual(observation['computed']['basis']['expression'], {'ref': dependency})
            self.assertIsNone(observation['computed']['basis']['as_of'])
        self.assertEqual(mutation.to_data()['receipt']['after']['assessment']['nodes']['d.new']['state']['falsifier']['status'],
                         'does_not_hold')
        return mutation, final

    def test_judgment_validation_uses_later_input_update_in_final_world(self):
        judgment = {'kind': 'add', 'id': 'd.new', 'into': 'judgments', 'body': {
            'verdict': 'input is at least two', 'rests_on': ['p.input'], 'wrong_if': {'expr': 'p.input < 2'}}}
        update = {'kind': 'set', 'id': 'p.input', 'value': 2, 'as_of': '2026-09-17'}
        _, control = self.prepared([update, judgment], 'control-update-first')
        mutation, final = self.prepared([judgment, update], 'required-judgment-first')
        self.assertEqual(final['readings'], control['readings'])
        self.assertEqual(final['judgments'], control['judgments'])
        new = [C.decode_document(item['after']) for item in mutation.files if item['role'] == 'history_object']
        input_claim = next(obj for obj in new if obj['subject'] == 'p.input' and obj['kind'] != 'act')
        judgment_claim = next(obj for obj in new if obj['subject'] == 'd.new' and obj['kind'] != 'act')
        self.assertEqual(judgment_claim['pins']['p.input'], input_claim['id'])

    def test_judgment_dependency_may_be_introduced_later_in_same_final_world(self):
        judgment = {'kind': 'add', 'id': 'd.new', 'into': 'judgments', 'body': {
            'verdict': 'new input is below five', 'rests_on': ['p.future'], 'wrong_if': {'expr': 'p.future > 5'}}}
        dependency = {'kind': 'add', 'id': 'p.future', 'into': 'readings', 'body': {'v': 3}}
        _, control = self.prepared([dependency, judgment], 'control-dependency-first')
        mutation, final = self.prepared([judgment, dependency], 'required-dependency-last')
        self.assertEqual(final['readings'], control['readings'])
        self.assertEqual(final['judgments'], control['judgments'])
        new = [C.decode_document(item['after']) for item in mutation.files if item['role'] == 'history_object']
        dependency_claim = next(obj for obj in new if obj['subject'] == 'p.future' and obj['kind'] != 'act')
        judgment_claim = next(obj for obj in new if obj['subject'] == 'd.new' and obj['kind'] != 'act')
        self.assertEqual(judgment_claim['pins']['p.future'], dependency_claim['id'])


if __name__ == '__main__':
    unittest.main()
