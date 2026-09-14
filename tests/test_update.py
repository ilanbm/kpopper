"""The synchronous report command shares ingestion's atomicity and durable receipts."""
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml
from scripts import ingestion as I


class Update(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / 'GROUNDING.yaml'
        self.state = self.root / 'state'
        self.record.write_text('sources:\n  s.original: {file: original.md}\nknown:\n  order.price: {v: 10, from: s.original}\n  order.quantity: {v: 10, from: s.original}\n')
        self.report = {'event_id': 'report-1', 'source_quote': 'Price 20, quantity 5.', 'date': '2026-09-13',
                       'updates': [{'kind': 'set', 'id': 'order.price', 'value': 20},
                                   {'kind': 'set', 'id': 'order.quantity', 'value': 5}]}

    def run_cli(self, report, *extra):
        return subprocess.run([sys.executable, str(Path(I.__file__).with_name('cli.py')),
                               '--workspace', str(self.root), 'update', '--file', '-',
                               '--state-dir', str(self.state), *extra],
                              input=json.dumps(report), text=True, capture_output=True, timeout=30)

    def test_cli_returns_an_applied_receipt_and_retry_does_not_write_again(self):
        result = self.run_cli(self.report, '--json')
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt['state'], 'applied')
        doc = yaml.safe_load(self.record.read_text())
        self.assertEqual([doc['known'][key]['v'] for key in ('order.price', 'order.quantity')], [20, 5])
        self.assertEqual(doc['known']['order.price']['from'], doc['known']['order.quantity']['from'])
        before = self.record.read_bytes()
        retry = self.run_cli(self.report)
        self.assertEqual(retry.returncode, 0, retry.stderr + retry.stdout)
        self.assertEqual(json.loads(retry.stdout), receipt)
        self.assertEqual(self.record.read_bytes(), before)

    def test_bad_member_leaves_the_whole_record_and_returns_a_nonzero_receipt(self):
        self.report['updates'][1]['id'] = 'order.absent'
        before = self.record.read_bytes()
        result = self.run_cli(self.report)
        self.assertEqual(result.returncode, 1, result.stderr + result.stdout)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt['state'], 'needs_primary')
        self.assertTrue(Path(receipt['source_file']).exists())
        self.assertEqual(self.record.read_bytes(), before)

    def test_only_the_requested_report_is_processed(self):
        pending = dict(self.report, event_id='unrelated')
        event = I.capture(pending, self.record, self.state, start=False)
        receipt = I.update(self.report, self.record, self.state)
        self.assertEqual(receipt['state'], 'applied')
        self.assertEqual(I.status(event['event_id'], self.record, self.state)['state'], event['state'])

    def test_malformed_json_has_a_structured_error_and_no_write(self):
        before = self.record.read_bytes()
        result = self.run_cli([])
        self.assertEqual(result.returncode, 2, result.stderr + result.stdout)
        self.assertIn('error', json.loads(result.stdout))
        self.assertEqual(self.record.read_bytes(), before)

    def test_default_record_is_resolved_once_for_the_whole_operation(self):
        with patch.object(I.P, 'default_paths', side_effect=[[str(self.record)], ['different.yaml']]) as locate:
            receipt = I.update(self.report, state_dir=self.state)
        self.assertEqual(receipt['state'], 'applied', receipt)
        self.assertEqual(locate.call_count, 1)

    def test_unsupported_locking_returns_a_structured_error(self):
        report = self.root / 'report.json'
        report.write_text(json.dumps(self.report))
        before = self.record.read_bytes()
        with patch.object(I, '_file_lock', side_effect=I.LockingUnavailable('durable ingestion writes require fcntl file locking')), contextlib.redirect_stdout(io.StringIO()) as out:
            code = I.update_main(['--file', str(report), '--record', str(self.record), '--state-dir', str(self.state)])
        self.assertEqual(code, 2)
        self.assertIn('fcntl', json.loads(out.getvalue())['error'])
        self.assertEqual(self.record.read_bytes(), before)

    def test_record_replacement_preserves_mode_without_fchmod(self):
        mode = self.record.stat().st_mode & 0o7777
        with patch.object(I.os, 'fchmod', None, create=True):
            I._replace_record(self.record, b'known: {}\n')
        self.assertEqual(self.record.read_bytes(), b'known: {}\n')
        self.assertEqual(self.record.stat().st_mode & 0o7777, mode)
