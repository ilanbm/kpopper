"""Acceptance for pure prospective operations over captured history context."""
import copy
import shutil
import unittest
from pathlib import Path
from unittest import mock

from scripts import history_hypotheses as HH
from scripts import history_store as H
from scripts.reasoning import operations
from scripts.reasoning import snapshot as S
from scripts.reasoning.contract import digest
from tests import test_history_advanced as _history_advanced


class HistoryOperationContext(unittest.TestCase):
    """The operation candidate keeps one original capture while changing one named group."""

    def setUp(self):
        fixture = _history_advanced.HistoryAdvanced()
        fixture.setUp()
        self.fixture = fixture
        self.addCleanup(fixture.doCleanups)

        # Make the source authority the active checkout, then add two independent named groups.
        shutil.copytree(fixture.source.root, fixture.root, dirs_exist_ok=True)
        self.record = fixture.record
        self.store = H.Store(self.record)
        self._commit(HH.prepare(self.record, "selected",
                                {"kind": "add", "id": "p.selected", "body": {"v": 2},
                                 "into": "readings"},
                                by="writer", operation="selected-proposal"))
        self._commit(HH.prepare(self.record, "retained",
                                {"kind": "add", "id": "p.retained", "body": {"v": 3},
                                 "into": "readings"},
                                 by="writer", operation="retained-proposal"))
        # Keep an independent pending bundle in the same target context.
        fixture.configure_target()
        self.bundle = fixture.pending()

    def _commit(self, mutation):
        return HH.commit(self.record, mutation, verify=lambda data: None)

    def _base(self):
        # Snapshot capture is a fixture preflight: failures here identify a bad history fixture,
        # before the operation API under test is called.
        S.Snapshot.capture(self.record, read_mode="live", as_of="2026-09-17")
        return operations.load([str(self.record)], as_of="2026-09-17", allow_history=True)

    def test_prepared_fold_removes_only_selected_group_and_retains_context(self):
        base = self._base()
        original_source = base._operation_source
        original_capture = copy.deepcopy(original_source.history_capture)
        before_capture = copy.deepcopy(self.store.capture())
        mutation = HH.prepare_fold(self.record, ["selected"], because="verified selected group",
                                   by="reviewer", operation="fold-selected")

        candidate = operations.prepared(base, mutation)
        context = operations.world(candidate).context
        data = operations.snapshot_for(candidate).to_data()

        self.assertIs(candidate._operation_source, original_source)
        self.assertEqual(context.operation_source["snapshot_id"], base._operation_snapshot.snapshot_id)
        self.assertEqual(context.context_digest, digest(base._operation_snapshot.to_data()["context"]))
        self.assertEqual(context.history_view, base._operation_snapshot.to_data()["context"]["history_view"])
        self.assertNotIn("selected", data["hypotheses"])
        self.assertIn("retained", data["hypotheses"])
        self.assertIn("history", data["context"])
        self.assertIn("history_contributions", data["context"])
        self.assertIn(self.bundle["revision"], data["context"]["history_contributions"])
        self.assertEqual(data["context"]["target"], base._operation_snapshot.to_data()["context"]["target"])
        self.assertEqual(context.snapshot_id, operations.snapshot_for(candidate).snapshot_id)
        self.assertEqual(original_source.history_capture, original_capture)
        self.assertEqual(self.store.capture().inventory, before_capture.inventory)

    def test_prepared_projection_does_no_store_or_filesystem_io(self):
        base = self._base()
        mutation = HH.prepare_fold(self.record, ["selected"], because="verified selected group",
                                   by="reviewer", operation="fold-selected-io")
        with mock.patch.object(H.Store, "capture", side_effect=AssertionError("store I/O")), \
             mock.patch.object(Path, "read_bytes", side_effect=AssertionError("filesystem I/O")):
            candidate = operations.prepared(base, mutation)
        self.assertIs(candidate._operation_source, base._operation_source)
        self.assertNotIn("selected", operations.snapshot_for(candidate).to_data()["hypotheses"])
        self.assertIn("retained", operations.snapshot_for(candidate).to_data()["hypotheses"])

    def test_original_capture_verifies_mutation_after_load(self):
        base = self._base()
        original = self.record.read_bytes()
        self.record.write_bytes(original + b"\n")
        with self.assertRaisesRegex((S.SnapshotError, ValueError), "snapshot_changed|changed"):
            base._operation_source.verify()

    def test_ordinary_derive_still_refuses_history_backed_base(self):
        base = self._base()
        with self.assertRaisesRegex(ValueError, "prospective_history_required"):
            operations.derive(base, base, [])


if __name__ == "__main__":
    unittest.main()
