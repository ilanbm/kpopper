"""Report transport retains multi-file staging and durable operation evidence."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import yaml
from scripts import ingestion as I


class HistoryIngestion(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name).resolve()
        self.record = self.root / 'GROUNDING.yaml'
        self.state = self.root / 'state'
        self.record.write_text(yaml.safe_dump({'sources': {'s.old': {'file': 'old.md', 'read': '2026-09-01'}},
            'known': {'p.price': {'v': 10, 'from': 's.old', 'of': '2026-09-01'}}}, sort_keys=False))
        self.archive = Path(I.P.layout(self.record)['replaced'])
        self.view = Path(I.P.layout(self.record)['view'])
        self.archive.parent.mkdir()
        self.archive.write_bytes(b'# original archive\nreplaced: []\n')
        self.view.write_bytes(b'title: Original\n')
        self.T = I.P._peer('history_transaction')
        self.report = {'event_id': 'retained-report', 'source_quote': 'Price is now 12.',
                       'date': '2026-09-16', 'target': 'p.price', 'value': 12}
        self.event = I.capture(self.report, self.record, self.state, start=False)
        self.journal = self.state / 'journals' / (self.event['event_id'] + '.json')
        self.shared = self.record.parent / self.T.journal_for(self.record)

    def stage_members(self, *args):
        result = self.prepare(*args)
        shadow = result[0]
        # A staged writer's additional role edits must survive report transport.
        self.assertEqual(Path(I.P.layout(shadow)['replaced']).read_bytes(), self.archive_before)
        Path(I.P.layout(shadow)['replaced']).write_bytes(b'# original archive\nreplaced: [retained]\n')
        Path(I.P.layout(shadow)['view']).write_bytes(b'title: Updated\n')
        return result

    def stage(self):
        self.prepare = I._prepare
        self.archive_before = self.archive.read_bytes()
        return patch.object(I, '_prepare', side_effect=self.stage_members)

    def process(self):
        return I.process(self.record, self.state, self.event['event_id'])[0]

    def fail_on(self, name):
        original = self.T._replace
        def replace(path, data):
            if path == self.root / name:
                raise OSError('injected publication interruption')
            return original(path, data)
        return patch.object(self.T, '_replace', side_effect=replace)

    def test_complete_shadow_members_and_receipt_are_retained(self):
        with self.stage():
            result = self.process()
        self.assertEqual(result['state'], 'applied', result)
        journal = json.loads(self.journal.read_text())
        mutation = I._mutation_from_journal(journal)
        self.assertEqual({item['role'] for item in mutation.files}, {'record', 'view', 'replaced'})
        self.assertEqual(self.archive.read_bytes(), b'# original archive\nreplaced: [retained]\n')
        self.assertEqual(self.view.read_bytes(), b'title: Updated\n')
        self.assertEqual(journal['committed_mutation'], mutation.to_data()['digest'])
        self.assertFalse(self.shared.exists())

    def assert_partial_recovery(self, relative):
        with self.stage(), self.fail_on(relative):
            result = self.process()
        self.assertEqual(result['state'], 'recovery_required', result)
        retained = self.shared.read_bytes()
        with self.assertRaisesRegex(ValueError, 'recovery_required'):
            with self.T.reader_guard(self.record.parent, self.T.journal_for(self.record)):
                self.fail('reader exposed partial publication')
        with patch.object(I, '_prepare', side_effect=AssertionError('retry re-prepared event')):
            result = self.process()
        self.assertEqual(result['state'], 'applied', result)
        self.assertTrue(result['recovered'])
        self.assertEqual(I._mutation_from_journal(json.loads(self.journal.read_text())).to_bytes(), retained)
        self.assertEqual(self.archive.read_bytes(), b'# original archive\nreplaced: [retained]\n')

    def test_interruption_before_archive_recovers_original_operation(self):
        self.assert_partial_recovery('.kpopper/replaced.yaml')

    def test_interruption_after_archive_recovers_original_operation(self):
        self.assert_partial_recovery('.kpopper/view.yaml')

    def test_interruption_after_view_recovers_original_operation(self):
        self.assert_partial_recovery('GROUNDING.yaml')

    def test_unrelated_archive_edit_blocks_recovery_without_overwrite(self):
        with self.stage(), self.fail_on('GROUNDING.yaml'):
            self.process()
        self.archive.write_bytes(b'unrelated edit\n')
        before = self.record.read_bytes()
        result = self.process()
        self.assertEqual(result['state'], 'recovery_required', result)
        self.assertIn('concurrent_edit', result['reason'])
        self.assertEqual(self.archive.read_bytes(), b'unrelated edit\n')
        self.assertEqual(self.record.read_bytes(), before)

    def test_completion_receipt_failure_keeps_recovery_evidence(self):
        replace = self.T._replace
        def fail_receipt(path, value):
            if path == self.journal:
                raise OSError('receipt unavailable')
            return replace(path, value)
        with self.stage(), patch.object(self.T, '_replace', side_effect=fail_receipt):
            result = self.process()
        self.assertEqual(result['state'], 'recovery_required', result)
        self.assertTrue(self.shared.exists())
        result = self.process()
        self.assertEqual(result['state'], 'applied', result)
        self.assertTrue(result['recovered'])

    def test_after_images_without_operation_receipt_do_not_acknowledge(self):
        with self.stage(), self.fail_on('GROUNDING.yaml'):
            self.process()
        mutation = I._mutation_from_journal(json.loads(self.journal.read_text()))
        for item in mutation.files:
            (self.root / item['path']).write_bytes(item['after'])
        self.shared.unlink()  # Simulate lost operation evidence, not a completed write.
        result = self.process()
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('after_images_match', result['reason'])

    def test_authority_change_blocks_recovery(self):
        with self.stage(), self.fail_on('GROUNDING.yaml'):
            self.process()
        H = I.P._peer('history_contract')
        marker = H.authority(record_id='other-record', authority='legacy', generation=1)
        Path(I.P.layout(self.record)['history_authority']).write_bytes(H.encode_document(marker))
        before = self.record.read_bytes()
        result = self.process()
        self.assertEqual(result['state'], 'recovery_required', result)
        self.assertIn('authority', result['reason'])
        self.assertEqual(self.record.read_bytes(), before)

    def test_history_shadow_is_explicitly_refused(self):
        prepare = I._prepare
        def add_history(*args):
            result = prepare(*args)
            Path(I.P.layout(result[0])['history']).mkdir(parents=True)
            return result
        before = self.record.read_bytes()
        with patch.object(I, '_prepare', side_effect=add_history):
            result = self.process()
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('unsupported_capability', result['reason'])
        self.assertEqual(self.record.read_bytes(), before)

    def test_changed_pending_ref_blocks_final_publication(self):
        context = I._write_context
        def changed_context(*args):
            value = context(*args)
            if self.journal.exists():
                value['pending_ref'] = 'new-pending-ref'
            return value
        before = self.record.read_bytes()
        with patch.object(I, '_write_context', side_effect=changed_context):
            result = self.process()
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('pending ref changed', result['reason'])
        self.assertEqual(self.record.read_bytes(), before)
        self.assertFalse(self.shared.exists())

    def test_missing_journal_after_partial_write_never_reprepares(self):
        with self.stage(), self.fail_on('GROUNDING.yaml'):
            self.process()
        self.shared.unlink()
        before = self.record.read_bytes()
        with patch.object(I, '_prepare', side_effect=AssertionError('lost transaction re-prepared')):
            result = self.process()
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('lost its recovery journal', result['reason'])
        self.assertEqual(self.record.read_bytes(), before)

    def test_recovery_does_not_duplicate_an_event_receipt(self):
        with self.stage():
            result = self.process()
        original = self.record.read_bytes()
        duplicate = I.capture(self.report, self.record, self.state, start=False)
        self.assertEqual(duplicate, result)
        self.assertEqual(I.process(self.record, self.state), [])
        self.assertEqual(self.record.read_bytes(), original)

    def test_completion_directory_sync_failure_retains_shared_journal(self):
        sync = self.T._sync
        def fail_receipt_sync(directory):
            if directory == self.journal.parent:
                raise OSError('receipt directory sync unavailable')
            return sync(directory)
        with self.stage(), patch.object(self.T, '_sync', side_effect=fail_receipt_sync):
            result = self.process()
        self.assertEqual(result['state'], 'recovery_required', result)
        self.assertTrue(self.shared.exists())
        result = self.process()
        self.assertEqual(result['state'], 'applied', result)
        self.assertTrue(result['recovered'])

    def test_completed_event_can_acknowledge_after_unrelated_view_edit(self):
        with self.stage(), self.assertRaises(I._CrashAfterCommit):
            I.process(self.record, self.state, self.event['event_id'], _crash_after_commit=True)
        self.view.write_bytes(b'title: Updated by a later writer\n')
        before = self.record.read_bytes()
        result = self.process()
        self.assertEqual(result['state'], 'applied', result)
        self.assertTrue(result['recovered'])
        self.assertEqual(self.view.read_bytes(), b'title: Updated by a later writer\n')
        self.assertEqual(self.record.read_bytes(), before)

    def test_other_event_journal_cannot_acknowledge_this_report(self):
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.record, self.state, self.event['event_id'], _crash_after_commit=True)
        other = I.capture(dict(self.report, event_id='another-report'), self.record, self.state, start=False)
        other_journal = self.state / 'journals' / (other['event_id'] + '.json')
        other_journal.write_bytes(self.journal.read_bytes())
        before = self.record.read_bytes()
        result = I.process(self.record, self.state, other['event_id'])[0]
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertIn('retained event', result['reason'])
        self.assertEqual(self.record.read_bytes(), before)

    def test_report_publication_does_not_use_compatibility_byte_replacement(self):
        with patch.object(I, '_replace_record', side_effect=AssertionError('report bypassed prepared transport')):
            result = self.process()
        self.assertEqual(result['state'], 'applied', result)
        self.assertIn('mutation', json.loads(self.journal.read_text()))
