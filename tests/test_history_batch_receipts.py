"""Batch validation receipts do not retain every intermediate record document."""
import unittest

from scripts import history_authoring as A, history_contract as C
from tests import test_history_authoring as fixture


class BatchReceipts(unittest.TestCase):
    def setUp(self):
        self.fixture = fixture.Authoring()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.entry = self.fixture.entry

    def batch(self, count, **kwargs):
        return A.prepare_batch(self.entry, [{'kind': 'add', 'id': 'p.item' + str(i),
            'body': {'v': i, 'note': 'recorded evidence ' * 80}, 'as_of': '2026-09-17'}
            for i in range(count)], operation='batch-' + str(count), recorded_at='2026-09-17T12:00:00Z', **kwargs)

    def test_step_receipts_are_digest_witnesses_and_replay_remains_exact(self):
        mutation = self.batch(8)
        receipt = mutation.to_data()['receipt']
        self.assertEqual(receipt['before']['authoring']['version'], 3)
        for step in receipt['after']['authoring']['steps']:
            self.assertEqual(set(step), {'operation', 'receipt_digest'})
            self.assertEqual(len(step['receipt_digest']), 64)
        A.commit(self.entry, mutation, verify=lambda data: None)
        A.commit(self.entry, mutation, verify=lambda data: None)
        self.assertEqual(len(self.fixture.store.capture().commits), 2)

    def test_additional_actions_do_not_add_copies_of_all_intermediate_documents(self):
        small = self.batch(4)
        large = self.batch(16)
        size = lambda m: len(next(f['after'] for f in m.files if f['role'] == 'history_commit'))
        self.assertLess(size(large), 5 * size(small))

    def test_retained_version_two_batch_still_replays_without_rewriting_its_receipt(self):
        old = self.batch(2, _receipt_version=2)
        original = old.to_bytes()
        self.assertEqual(old.to_data()['receipt']['before']['authoring']['version'], 2)
        self.assertIn('receipt', old.to_data()['receipt']['after']['authoring']['steps'][0])
        A.commit(self.entry, old, verify=lambda data: None)
        A.commit(self.entry, old, verify=lambda data: None)
        self.assertEqual(old.to_bytes(), original)
