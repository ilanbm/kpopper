"""Automatic core routing keeps legacy stop-gate safety properties."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock

from scripts import provenance as P


DOCUMENT = '''meta:
  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}
known:
  p.input: {v: 1}
judgments:
  d.ready:
    verdict: ready
    rests_on: [p.input]
    seen: {p.input: 1}
    wrong_if: {expr: p.input > 5}
'''


class CoreDefaultGate(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / 'GROUNDING.yaml'
        self.record.write_text(DOCUMENT, encoding='utf-8')
        self.mark = self.root / 'mark.json'

    def output(self, function, *args, **kwargs):
        stream = io.StringIO()
        with contextlib.redirect_stdout(stream):
            code = function(*args, **kwargs)
        return code, stream.getvalue()

    def test_pointer_closure_and_oversized_marker_select_core(self):
        child = self.root / 'child.yaml'
        child.write_text(DOCUMENT, encoding='utf-8')
        index = self.root / 'index.yaml'
        index.write_text('record: child.yaml\n', encoding='utf-8')
        self.assertTrue(P.core_reader_selected([str(index)]))

        plain = self.root / 'plain.yaml'
        plain.write_text('known:\n  p.value: {v: 1}\n', encoding='utf-8')
        marker = Path(P.layout(plain)['history_authority'])
        marker.parent.mkdir(exist_ok=True)
        marker.write_bytes(b'x' * (P._peer('history_contract').MAX_OBJECT_BYTES + 1))
        self.assertTrue(P.core_reader_selected([str(plain)]))

    def test_moved_input_firing_unchanged_judgment_is_exempt(self):
        self.assertEqual(P.mark(str(self.mark), [str(self.record)]), 0)
        self.record.write_text(DOCUMENT.replace('p.input: {v: 1}', 'p.input: {v: 6}'), encoding='utf-8')
        code, output = self.output(P.gate, str(self.mark), [str(self.record)])
        self.assertEqual(code, 0, output)
        self.assertIn('Updated readings falsified unchanged judgments: d.ready', output)

    def test_changed_judgment_is_not_exempt(self):
        self.assertEqual(P.mark(str(self.mark), [str(self.record)]), 0)
        changed = DOCUMENT.replace('p.input: {v: 1}', 'p.input: {v: 6}') \
                          .replace('p.input > 5', 'p.input > 4')
        self.record.write_text(changed, encoding='utf-8')
        code, output = self.output(P.gate, str(self.mark), [str(self.record)])
        self.assertEqual(code, 2)
        self.assertIn('FAIL d.ready: falsifier holds', output)

    def test_core_nudge_is_persisted_and_only_emitted_once(self):
        self.assertEqual(P.mark(str(self.mark), [str(self.record)]), 0)
        first, first_output = self.output(P.gate, str(self.mark), [str(self.record)], turns=P.NUDGE_TURNS)
        second, second_output = self.output(P.gate, str(self.mark), [str(self.record)], turns=P.NUDGE_TURNS + 1)
        self.assertEqual(first, 2)
        self.assertIn('record untouched', first_output)
        self.assertEqual((second, second_output), (0, ''))
        self.assertTrue(json.loads(self.mark.read_text())['nudged'])

    def test_arbitrary_recorded_for_does_not_satisfy_intent(self):
        self.assertEqual(P.mark(str(self.mark), [str(self.record)]), 0)
        self.record.write_text(DOCUMENT + 'sources:\n  s.fake: {recorded_for: unverified}\n', encoding='utf-8')
        code, output = self.output(P.gate, str(self.mark), [str(self.record)])
        self.assertEqual(code, 2)
        self.assertIn('verify its recorded intent', output)

    def test_temporal_findings_are_fail_closed_without_duplicates(self):
        base = {'body': {'v': 1}, 'fields': {'deps': 'rests_on', 'predicate': 'wrong_if'},
                'coverage': {'complete': True, 'findings': []}, 'computation': None,
                'support': {'status': 'clear', 'reservations': []},
                'state': {'integrity': {'issues': []}, 'falsifier': {'status': 'does_not_hold'}}}
        page = SimpleNamespace(core_build=lambda paths, context=None:
            (None, None, None, None, {'coverage': {}}))
        for status, expected in [('counterexample', 'historical counterexample'),
                                 ('unknown', 'historical evidence unknown'),
                                 ('recovered', None)]:
            node = dict(base, temporal={'status': status})
            context = SimpleNamespace(assessment={'nodes': {'p.value': node}})
            with mock.patch.object(P, '_peer', return_value=page):
                failures, _ = P._core_check_findings(['unused'], context)
            if expected is None:
                self.assertFalse(failures)
            else:
                self.assertEqual(failures, ['p.value: ' + expected])
        holding = dict(base, temporal={'status': 'counterexample'},
                       state={'integrity': {'issues': []}, 'falsifier': {'status': 'holds'}})
        with mock.patch.object(P, '_peer', return_value=page):
            failures, _ = P._core_check_findings(
                ['unused'], SimpleNamespace(assessment={'nodes': {'p.value': holding}}))
        self.assertEqual(failures, ['p.value: falsifier holds'])


if __name__ == '__main__':
    unittest.main()
