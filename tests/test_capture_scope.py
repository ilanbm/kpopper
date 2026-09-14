"""Only this record's applied reports exempt the gate's purpose reminder."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import tempfile
import unittest

import yaml
from scripts import ingestion as I

P = I.P


@unittest.skipUnless(os.name == 'posix', 'durable ingestion requires POSIX locking')
class CaptureScope(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.record, self.other = self.root / 'record.yaml', self.root / 'other.yaml'
        self.state = self.root / 'custom-state'
        self.mark = self.root / 'mark.json'
        self.doc = {'sources': {'s.original': {'file': 'original.md', 'read': '2026-09-13'}},
                    'known': {'facts.count': {'v': 1, 'from': 's.original', 'of': '2026-09-13'}}}
        (self.root / 'original.md').write_text('Count 1.', encoding='utf-8')
        for record in (self.record, self.other): self.save(self.doc, record)
        with contextlib.redirect_stdout(io.StringIO()): P.mark(str(self.mark), [str(self.record)])

    def save(self, doc, record=None):
        (record or self.record).write_text(yaml.safe_dump(doc, sort_keys=False), encoding='utf-8')

    def report(self, **extra):
        return {'event_id': 'count-report', 'target': 'facts.count', 'value': 2,
                'date': '2026-09-14', 'source_quote': 'Count 2.', **extra}

    def gate(self, record=None):
        with contextlib.redirect_stdout(io.StringIO()) as out:
            code = P.gate(str(self.mark), [str(record or self.record)])
        return code, out.getvalue()

    def inject_source(self, event, source=None):
        source_id = 's.ingest_' + event['event_id']
        doc = copy.deepcopy(self.doc)
        doc['sources'][source_id] = source or {
            'file': event['source_file'], 'read': '2026-09-14', 'recorded_for': 'Record the new count.'}
        doc['known']['facts.new'] = {'v': 2, 'from': source_id}
        self.save(doc)

    def test_applied_capture_from_another_record_cannot_exempt_this_gate(self):
        receipt = I.update(self.report(), self.other, self.root / 'other-state')
        self.assertEqual(receipt['state'], 'applied', receipt)
        source = yaml.safe_load(self.other.read_text())['sources'][receipt['source']]
        self.inject_source(receipt, source)
        code, text = self.gate()
        self.assertEqual(code, 2, text)
        self.assertIn('recorded no intent', text)

    def test_captured_or_refused_report_cannot_exempt_handwritten_entries(self):
        for refused in (False, True):
            with self.subTest(refused=refused):
                self.save(self.doc)
                state = self.root / ('refused' if refused else 'captured')
                event = I.capture(self.report(target='facts.absent' if refused else 'facts.count'),
                                  self.record, state, start=False)
                if refused:
                    outcome = I.process(self.record, state, event['event_id'])[0]
                    self.assertEqual(outcome['state'], 'needs_primary', outcome)
                event = I.status(event['event_id'], self.record, state)
                self.inject_source(event)
                code, text = self.gate()
                self.assertEqual(code, 2, text)
                self.assertIn('recorded no intent', text)

    def test_custom_state_directory_and_resolved_record_alias_remain_valid(self):
        receipt = I.update(self.report(), self.record, self.state)
        self.assertEqual(receipt['state'], 'applied', receipt)
        self.assertEqual(self.gate()[0], 0)
        alias = self.root / 'record-alias.yaml'
        alias.symlink_to(self.record)
        self.assertEqual(self.gate(alias)[0], 0)

    def test_pointer_and_nested_also_keep_the_actual_record_owner(self):
        entry = self.root / 'entry.yaml'
        middle = self.root / 'middle.yaml'
        entry.write_text('record: middle.yaml\n', encoding='utf-8')
        middle.write_text('also: {domain: record.yaml}\n', encoding='utf-8')
        with contextlib.redirect_stdout(io.StringIO()): P.mark(str(self.mark), [str(entry)])
        receipt = I.update(self.report(), self.record, self.state)
        self.assertEqual(receipt['state'], 'applied', receipt)
        code, text = self.gate(entry)
        self.assertEqual(code, 0, text)

    def test_a_copied_source_cannot_borrow_another_checked_files_receipt(self):
        receipt = I.update(self.report(), self.other, self.root / 'other-state')
        source = yaml.safe_load(self.other.read_text())['sources'][receipt['source']]
        paths = [str(self.other), str(self.record)]
        with contextlib.redirect_stdout(io.StringIO()): P.mark(str(self.mark), paths)
        self.inject_source(receipt, source)
        with contextlib.redirect_stdout(io.StringIO()) as out:
            code = P.gate(str(self.mark), paths)
        self.assertEqual(code, 2, out.getvalue())
        self.assertIn('recorded no intent', out.getvalue())

    def test_source_origin_follows_collection_order_as_well_as_file_order(self):
        receipt = I.update(self.report(), self.other, self.root / 'other-state')
        source = yaml.safe_load(self.other.read_text())['sources'][receipt['source']]
        paths = [str(self.record), str(self.other)]
        with contextlib.redirect_stdout(io.StringIO()): P.mark(str(self.mark), paths)
        doc = copy.deepcopy(self.doc)
        doc['known'][receipt['source']] = source
        doc['known']['facts.new'] = {'v': 2, 'from': receipt['source']}
        self.save(doc)
        with contextlib.redirect_stdout(io.StringIO()) as out:
            code = P.gate(str(self.mark), paths)
        self.assertEqual(code, 2, out.getvalue())
        self.assertIn('recorded no intent', out.getvalue())

    def test_an_effective_metadata_override_keeps_its_own_file_origin(self):
        receipt = I.update(self.report(), self.other, self.root / 'other-state')
        source = yaml.safe_load(self.other.read_text())['sources'][receipt['source']]
        paths = [str(self.other), str(self.record)]
        with contextlib.redirect_stdout(io.StringIO()): P.mark(str(self.mark), paths)
        doc = copy.deepcopy(self.doc)
        doc['schema'] = {receipt['source']: source}
        doc['known']['facts.new'] = {'v': 2, 'from': receipt['source']}
        self.save(doc)
        self.assertEqual(P.check_lines(paths)[0], [])
        with contextlib.redirect_stdout(io.StringIO()) as out:
            code = P.gate(str(self.mark), paths)
        self.assertEqual(code, 2, out.getvalue())
        self.assertIn('recorded no intent', out.getvalue())

    def test_missing_corrupt_or_foreign_ownership_marker_does_not_qualify(self):
        receipt = I.update(self.report(), self.record, self.state)
        self.assertEqual(receipt['state'], 'applied', receipt)
        marker = self.state / 'record.json'
        for contents in (None, '{broken', json.dumps({'record': str(self.other)})):
            with self.subTest(contents=contents):
                if contents is None: marker.unlink()
                else: marker.write_text(contents, encoding='utf-8')
                code, text = self.gate()
                self.assertEqual(code, 2, text)

    def test_receipt_must_be_applied_and_bound_to_this_exact_capture(self):
        receipt = I.update(self.report(), self.record, self.state)
        self.assertEqual(receipt['state'], 'applied', receipt)
        path = self.state / 'receipts' / (receipt['event_id'] + '.json')
        for change in ({'state': 'needs_primary'}, {'source': 's.different'},
                       {'event_id': 'different'}, {'source_sha256': '0' * 64},
                       {'envelope_sha256': '0' * 64}, {'source_file': 'different.txt'}):
            with self.subTest(change=change):
                path.write_text(json.dumps({**receipt, **change}), encoding='utf-8')
                code, text = self.gate()
                self.assertEqual(code, 2, text)
        path.unlink()
        self.assertEqual(self.gate()[0], 2)

    def test_recovery_publishes_a_receipt_that_qualifies_without_rewriting_the_record(self):
        event = I.capture(self.report(), self.record, self.state, start=False)
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.record, self.state, event['event_id'], _crash_after_commit=True)
        before = self.record.read_bytes()
        receipt = I.process(self.record, self.state, event['event_id'])[0]
        self.assertEqual(receipt['state'], 'applied', receipt)
        self.assertTrue(receipt['recovered'])
        self.assertEqual(self.record.read_bytes(), before)
        self.assertEqual(self.gate()[0], 0)


if __name__ == '__main__': unittest.main()
