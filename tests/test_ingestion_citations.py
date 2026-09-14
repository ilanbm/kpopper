"""A retained report may cite an existing document without replacing its provenance."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml
from scripts import ingestion as I


@unittest.skipUnless(os.name == 'posix', 'durable ingestion requires POSIX locking')
class ExistingCitations(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record, self.state = self.root / 'GROUNDING.yaml', self.root / 'state'
        self.doc = {'sources': {'s.contract': {'file': 'contract.md', 'read': '2026-09-13'}},
                    'known': {'shipping.cost': {'v': 80, 'from': 's.contract', 'at': 'clause 3',
                                                 'of': '2026-09-13'}},
                    'judgments': {'c.budget': {'rests_on': ['shipping.cost'], 'seen': {'shipping.cost': 80},
                                              'verdict': 'Within budget', 'wrong_if': 'shipping.cost > 100'}}}
        self.save(self.doc)
        (self.root / 'contract.md').write_text('סעיף 3: מחיר המשלוח 90. סעיף 4: הקיבולת 5.\n', encoding='utf-8')

    def save(self, doc):
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False, allow_unicode=True), encoding='utf-8')

    def read(self):
        return yaml.safe_load(self.record.read_text(encoding='utf-8'))

    def report(self, **changes):
        return {'event_id': 'contract-report', 'source': 's.contract', 'at': 'clause 3',
                'record_sha256': I._sha(self.record.read_bytes()), 'date': '2026-09-14',
                'source_quote': 'סעיף 3: מחיר המשלוח 90. סעיף 4: הקיבולת 5.',
                'updates': [{'kind': 'set', 'id': 'shipping.cost', 'value': 90}], **changes}

    def test_cli_keeps_document_citation_and_links_the_retained_report(self):
        report = self.report()
        command = [sys.executable, str(Path(I.__file__).with_name('cli.py')), '--workspace', str(self.root),
                   'update', '--file', '-', '--state-dir', str(self.state)]
        result = subprocess.run(command, input=json.dumps(report), text=True, encoding='utf-8',
                                capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        receipt = json.loads(result.stdout)
        self.assertEqual(receipt['state'], 'applied')
        self.assertEqual(receipt['cited_source'], 's.contract')
        doc = self.read()
        self.assertEqual(doc['known']['shipping.cost'],
                         {'v': 90, 'from': 's.contract', 'at': 'clause 3', 'of': '2026-09-14'})
        self.assertEqual(doc['sources']['s.contract'], self.doc['sources']['s.contract'])
        self.assertEqual(doc['judgments'], self.doc['judgments'])
        self.assertEqual(doc['sources'][receipt['source']]['from'], 's.contract')
        self.assertEqual(Path(receipt['source_file']).read_text(encoding='utf-8'), report['source_quote'])
        after = self.record.read_bytes()
        self.assertEqual(I.update(report, self.record, self.state), receipt)
        self.assertEqual(self.record.read_bytes(), after)

    def test_batch_add_and_set_use_per_reading_locations_and_one_source(self):
        report = self.report(updates=[{'kind': 'set', 'id': 'shipping.cost', 'value': 90},
                                     {'kind': 'add', 'id': 'shipping.capacity', 'at': 'clause 4',
                                      'body': {'v': 5}}])
        receipt = I.update(report, self.record, self.state)
        self.assertEqual(receipt['state'], 'applied', receipt)
        known = self.read()['known']
        self.assertEqual(known['shipping.cost']['at'], 'clause 3')
        self.assertEqual(known['shipping.capacity']['at'], 'clause 4')
        self.assertEqual(known['shipping.capacity']['from'], 's.contract')
        self.assertEqual(I.P.check_lines([str(self.record)])[0], [])

    def test_legacy_single_update_and_recovery_keep_the_same_citation(self):
        report = self.report(target='shipping.cost', value=90)
        del report['updates']
        event = I.capture(report, self.record, self.state, start=False)
        with self.assertRaises(I._CrashAfterCommit):
            I.process(self.record, self.state, event['event_id'], _crash_after_commit=True)
        after = self.record.read_bytes()
        receipt = I.process(self.record, self.state, event['event_id'])[0]
        self.assertEqual(receipt['state'], 'applied', receipt)
        self.assertTrue(receipt['recovered'])
        self.assertEqual(receipt['cited_source'], 's.contract')
        self.assertEqual(self.read()['known']['shipping.cost']['from'], 's.contract')
        self.assertEqual(self.record.read_bytes(), after)

    def test_missing_or_non_source_ids_retain_report_without_writing(self):
        for source in ('s.missing', 'shipping.cost', 'c.budget', 'graph.nodes'):
            with self.subTest(source=source):
                before = self.record.read_bytes()
                receipt = I.update(self.report(source=source, event_id=source), self.record, self.state)
                self.assertEqual(receipt['state'], 'needs_primary', receipt)
                self.assertIn('recorded source', receipt['reason'])
                self.assertEqual(self.record.read_bytes(), before)
                self.assertTrue(Path(receipt['source_file']).is_file())

    def test_source_citation_requires_prior_read_and_an_explicit_location(self):
        for field in ('record_sha256', 'at'):
            with self.subTest(field=field):
                before = self.record.read_bytes()
                report = self.report(event_id=field)
                del report[field]
                receipt = I.update(report, self.record, self.state)
                self.assertEqual(receipt['state'], 'needs_primary', receipt)
                self.assertIn(field, receipt['reason'])
                self.assertEqual(self.record.read_bytes(), before)

    def test_a_source_change_after_capture_or_during_prepare_is_not_accepted(self):
        for when in ('after_capture', 'during_prepare'):
            with self.subTest(when=when):
                self.save(self.doc)
                report = self.report(event_id=when)
                event = I.capture(report, self.record, self.state, start=False)
                changed = copy.deepcopy(self.doc)
                changed['sources']['s.contract']['file'] = 'different-contract.md'
                if when == 'after_capture':
                    self.save(changed)
                    receipt = I.process(self.record, self.state, event['event_id'])[0]
                else:
                    prepare = I._prepare
                    def change_source(*args):
                        prepared = prepare(*args)
                        self.save(changed)
                        return prepared
                    with patch.object(I, '_prepare', side_effect=change_source):
                        receipt = I.process(self.record, self.state, event['event_id'])[0]
                self.assertEqual(receipt['state'], 'needs_primary', receipt)
                self.assertEqual(self.read(), changed)

    def test_one_bad_member_preserves_all_readings_and_original_sources(self):
        before = self.record.read_bytes()
        report = self.report(updates=[{'kind': 'set', 'id': 'shipping.cost', 'value': 90},
                                     {'kind': 'add', 'id': 'c.invalid', 'body': {
                                         'rests_on': ['absent.value'], 'verdict': 'Unknown',
                                         'wrong_if': 'absent.value > 0'}}])
        receipt = I.update(report, self.record, self.state)
        self.assertEqual(receipt['state'], 'needs_primary', receipt)
        self.assertEqual(self.record.read_bytes(), before)

    def test_explicit_source_selects_its_collection_among_multiple_source_groups(self):
        self.doc['evidence'] = {'s.other': {'file': 'other.md'}}
        self.save(self.doc)
        receipt = I.update(self.report(source='s.other'), self.record, self.state)
        self.assertEqual(receipt['state'], 'applied', receipt)
        self.assertEqual(self.read()['known']['shipping.cost']['from'], 's.other')
        self.assertIn(receipt['source'], self.read()['evidence'])

    def test_per_reading_locations_work_without_a_shared_location(self):
        report = self.report(updates=[{'kind': 'set', 'id': 'shipping.cost', 'value': 90, 'at': 'סעיף 3'}])
        del report['at']
        receipt = I.update(report, self.record, self.state)
        self.assertEqual(receipt['state'], 'applied', receipt)
        self.assertEqual(self.read()['known']['shipping.cost']['at'], 'סעיף 3')

    def test_existing_source_cannot_be_replaced_by_its_own_capture(self):
        report = self.report()
        source = 's.ingest_' + I._event_id(self.record.resolve(), report)
        self.doc['sources'][source] = {'file': 'previous-report.md'}
        self.save(self.doc)
        report = self.report(source=source)
        before = self.record.read_bytes()
        receipt = I.update(report, self.record, self.state)
        self.assertEqual(receipt['state'], 'needs_primary', receipt)
        self.assertIn('own capture', receipt['reason'])
        self.assertEqual(self.record.read_bytes(), before)

    def test_malformed_source_and_location_are_rejected_before_capture(self):
        for changes in ({'source': None}, {'source': ''}, {'source': []}, {'at': None}, {'at': ' '}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                I.capture(self.report(**changes), self.record, self.state, start=False)
        self.assertFalse(self.state.exists())


if __name__ == '__main__': unittest.main()
