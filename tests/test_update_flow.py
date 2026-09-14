"""Source reports remain actionable through the writer, page and host notice paths."""
import contextlib
import copy
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import yaml
from scripts import ingestion as I, ingestion_hooks as H

P = I.P


@unittest.skipUnless(os.name == 'posix', 'durable ingestion requires POSIX locking')
class UpdateFlow(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name).resolve()
        self.work = self.base / 'project'; self.work.mkdir()
        self.record = self.work / 'GROUNDING.yaml'
        self.brief = self.work / '.kpopper/view.yaml'; self.brief.parent.mkdir()
        self.doc = {'sources': {'s.original': {'name': 'Initial report', 'file': 'original.md', 'read': '2026-09-09'}},
                    'known': {'order.price': {'v': 20, 'from': 's.original', 'of': '2026-09-09'},
                              'order.quantity': {'v': 5, 'from': 's.original', 'of': '2026-09-09'},
                              'order.limit': {'v': 30, 'from': 's.original'}},
                    'judgments': {'c.budget': {'rests_on': ['order.price', 'order.limit'], 'verdict': 'The unit price fits',
                                               'wrong_if': 'order.price > order.limit',
                                               'seen': {'order.price': 20, 'order.limit': 30}}}}
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        (self.work / 'original.md').write_text('Price 20; quantity 5; unit price limit 30.')
        self.brief.write_text('title: Order\nsections:\n  - title: Current\n    pick: all\n')
        env = patch.dict(os.environ, {'XDG_STATE_HOME': str(self.base / 'state')})
        env.start(); self.addCleanup(env.stop)

    def report(self, **extra):
        return {'event_id': 'order-report', 'date': '2026-09-10', 'source_quote': 'Price 40; quantity 2.',
                'updates': [{'kind': 'set', 'id': 'order.price', 'value': 40},
                            {'kind': 'set', 'id': 'order.quantity', 'value': 2}], **extra}

    def test_report_with_existing_page_applies_and_surfaces_a_real_contradiction(self):
        before = copy.deepcopy(self.doc['judgments'])
        mark = self.base / 'session-mark.json'
        with contextlib.redirect_stdout(io.StringIO()): P.mark(str(mark), [str(self.record)])
        result = I.update(self.report(), self.record)
        self.assertEqual(result['state'], 'applied', result)
        saved = yaml.safe_load(self.record.read_text())
        self.assertEqual(saved['known']['order.price']['v'], 40)
        self.assertEqual(saved['known']['order.quantity']['v'], 2)
        self.assertEqual(saved['judgments'], before)
        self.assertEqual(result['newly_fired_judgments'], ['c.budget'])
        source = saved['sources'][result['source']]
        self.assertNotIn('asked', source)  # External evidence did not invent a new reading occasion.
        self.assertIn('recorded_for', source)
        with contextlib.redirect_stdout(io.StringIO()) as out:
            code = P.gate(str(mark), [str(self.record)])
        self.assertEqual(code, 0, out.getvalue())
        self.assertFalse(list((self.base / 'state').glob('kpopper/followups/**/followups.yaml')))
        for host in ('claude', 'codex'):
            with H._cwd(self.work):
                stdout, stderr, status = H.handle({'session_id': 'test-' + host, 'cwd': str(self.work)}, host, 'start')
            self.assertEqual(status, 0, stderr)
            self.assertIn('KPOPPER_ATTENTION', stdout)
            self.assertIn('c.budget', stdout)

    def test_one_bad_item_retains_source_and_no_requested_updates(self):
        report = self.report()
        report['updates'].append({'kind': 'set', 'id': 'order.missing', 'value': 7})
        before = self.record.read_bytes()
        result = I.update(report, self.record)
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertEqual(self.record.read_bytes(), before)
        self.assertIn('order.missing', result['reason'])
        self.assertEqual(Path(result['source_file']).read_text(), report['source_quote'])

    def test_a_gate_refusal_keeps_the_actual_diagnostic(self):
        def refuse(*args, **kwargs):
            print('FAIL view.coverage: a named tab lost its required source coverage')
            return 2
        before = self.record.read_bytes()
        with patch.object(P, 'gate', side_effect=refuse):
            result = I.update(self.report(), self.record)
        self.assertEqual(result['state'], 'needs_primary')
        self.assertIn('view.coverage', result['reason'])
        self.assertIn('required source coverage', result['reason'])
        self.assertEqual(self.record.read_bytes(), before)
        self.assertTrue(Path(result['source_file']).is_file())
        self.assertIn('view.coverage', '\n'.join(result['validation_issues']))

    def test_actual_page_constraint_is_not_bypassed_by_a_captured_report(self):
        self.doc['sources']['s.layout'] = {'name': 'Review the order', 'asked': 'Review order warnings', 'read': '2026-09-09'}
        self.doc['known']['order.price']['from'] = 's.layout'
        self.doc['judgments']['v.coverage'] = {'rests_on': ['s.layout', 'page.spill'], 'verdict': 'Warnings are covered',
            'born': '2026-09-09', 'wrong_if': 'page.spill > 0', 'seen': {'s.layout': 'read 2026-09-09', 'page.spill': 0}}
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        self.brief.write_text('title: Order\ntabs:\n  - title: Checks\n    serves: [s.layout]\n    sections:\n      - title: Coverage\n        pick: order.price\n')
        self.assertEqual(P.check_lines([str(self.record)])[0], [])
        before = self.record.read_bytes()
        result = I.update(self.report(), self.record)
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertEqual(self.record.read_bytes(), before)
        self.assertIn('s.layout', result['reason'])
        self.assertTrue(result['validation_issues'])

    def test_an_unsupported_platform_fails_before_writing_state(self):
        with patch.object(I, '_require_locking', side_effect=I.LockingUnavailable('durable ingestion writes require fcntl file locking')):
            with self.assertRaises(I.LockingUnavailable):
                I.update(self.report(), self.record)
        self.assertFalse((self.base / 'state').exists())

    def test_ordinary_unserved_requests_still_require_page_attention(self):
        mark = self.base / 'mark.json'
        with contextlib.redirect_stdout(io.StringIO()):
            P.mark(str(mark), [str(self.record)])
            P.apply([str(self.record)], {'kind': 'add', 'id': 's.request', 'body': {'asked': 'Review the next offer', 'name': 'A real request'}})
            P.apply([str(self.record)], {'kind': 'add', 'id': 'offer.price', 'body': {'v': 10, 'from': 's.request'}})
            code = P.gate(str(mark), [str(self.record)])
        self.assertEqual(code, 2)

    def test_an_arbitrary_source_cannot_claim_the_capture_exemption(self):
        for name in ('s.note', 's.ingest_fake'):
            with self.subTest(name=name):
                self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
                mark = self.base / 'mark.json'
                with contextlib.redirect_stdout(io.StringIO()) as out:
                    P.mark(str(mark), [str(self.record)])
                    P.apply([str(self.record)], {'kind': 'add', 'id': name, 'body': {
                        'file': str(self.work / 'original.md'), 'recorded_for': 'A work request'}})
                    P.apply([str(self.record)], {'kind': 'add', 'id': 'offer.price', 'body': {'v': 10, 'from': name}})
                    code = P.gate(str(mark), [str(self.record)])
                self.assertEqual(code, 2, out.getvalue())
                self.assertIn('recorded no intent', out.getvalue())

    def test_capture_exemption_requires_intact_evidence(self):
        result = I.update(self.report(), self.record)
        source = yaml.safe_load(self.record.read_text())['sources'][result['source']]
        self.assertTrue(I.recording_source(result['source'], source, self.record))
        Path(result['source_file']).write_text('Changed after capture')
        self.assertFalse(I.recording_source(result['source'], source, self.record))
        event = Path(result['source_file']).parent.parent / 'events' / (result['event_id'] + '.json')
        event.write_text('{broken json')
        self.assertFalse(I.recording_source(result['source'], source, self.record))

    def test_completing_a_missing_input_can_falsify_an_unchanged_judgment(self):
        self.doc['judgments']['c.shipping'] = {'rests_on': ['order.shipping'], 'seen': {'order.shipping': 10},
            'verdict': 'Shipping fits', 'wrong_if': 'order.shipping > 20', 'blocked_on': 'Awaiting shipping quote'}
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        before = copy.deepcopy(self.doc['judgments'])
        result = I.update(self.report(record_sha256=I._sha(self.record.read_bytes()),
            updates=[{'kind': 'add', 'id': 'order.shipping', 'body': {'v': 25}}]), self.record)
        self.assertEqual(result['state'], 'applied', result)
        self.assertEqual(result['newly_fired_judgments'], ['c.shipping'])
        self.assertEqual(yaml.safe_load(self.record.read_text())['judgments'], before)

    def test_falsification_does_not_excuse_missing_review_history(self):
        self.doc['judgments']['c.shipping'] = {'rests_on': ['order.shipping'], 'seen': {},
            'verdict': 'Shipping fits', 'wrong_if': 'order.shipping > 20', 'blocked_on': 'Awaiting shipping quote'}
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        before = self.record.read_bytes()
        result = I.update(self.report(record_sha256=I._sha(before),
            updates=[{'kind': 'add', 'id': 'order.shipping', 'body': {'v': 25}}]), self.record)
        self.assertEqual(result['state'], 'needs_primary', result)
        self.assertEqual(self.record.read_bytes(), before)
        self.assertIn('no snapshot for order.shipping', result['reason'])

    def test_page_question_retires_only_after_review_and_fresh_measurement(self):
        from scripts import render_page as R
        self.doc['judgments']['c.page'] = {'rests_on': ['order.price', 'page.spill'],
            'seen': {'order.price': 20, 'page.spill': 0}, 'verdict': 'Covered', 'wrong_if': 'page.spill > 0'}
        self.record.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        result = I.update(self.report(), self.record)
        self.assertEqual(result['state'], 'applied', result)
        self.assertIn('c.page', result['actionable_judgments'])
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.record)], {'kind': 'review', 'id': 'c.page'})
        self.assertTrue(any(s['category'] == 'question' for s in I.pending(self.record)))
        R.measured_build([str(self.record)], str(self.brief))
        with patch.object(R, 'build', side_effect=AssertionError('pending must not render')):
            self.assertFalse(any(s['category'] == 'question' for s in I.pending(self.record)))
            self.assertTrue(any(s['category'] == 'contradiction' for s in I.pending(self.record)))
            self.record.write_bytes(self.record.read_bytes() + b'\n# changed inputs\n')
            self.assertTrue(any(s['category'] == 'question' for s in I.pending(self.record)))
