"""Workspace open has an explicit one-context core route."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import provenance as P, workspace_cli as W


DOCUMENT = '''meta:
  name: Core workspace
  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}
known:
  p.input: {v: 1}
judgments:
  d.ready:
    rests_on: [p.input]
    seen: {p.input: 1}
    wrong_if: Supplier changes terms
'''


class CoreWorkspaceOpen(unittest.TestCase):
    def test_explicit_core_open_uses_one_context_and_never_legacy_session(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'GROUNDING.yaml'
            path.write_text(DOCUMENT, encoding='utf-8')
            output = io.StringIO()
            with mock.patch.object(W.W, 'locate', return_value={
                    'workspace': directory, 'record': str(path), 'status': 'found',
                    'key': 'core-workspace'}), \
                    mock.patch.object(W.S, 'read_view', side_effect=AssertionError('legacy session')), \
                    contextlib.redirect_stdout(output):
                code = W.open_context(['--json', '--profile', 'core/v1', str(path)])
        self.assertEqual(code, 0)
        result = json.loads(output.getvalue())
        self.assertEqual(result['assessment_profile'], 'core/v1')
        self.assertRegex(result['snapshot_id'], r'^[0-9a-f]{64}$')
        self.assertRegex(result['findings_revision'], r'^[0-9a-f]{64}$')
        self.assertEqual(result['followups_status'], 'not_projected_for_core/v1')

    def test_legacy_default_still_calls_the_existing_opening(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'GROUNDING.yaml'
            path.write_text('known:\n  p.input: {v: 1}\n', encoding='utf-8')
            completed = type('Result', (), {'stdout': 'legacy opening\n', 'stderr': '',
                                             'returncode': 0})()
            with mock.patch.object(W.W, 'locate', return_value={
                    'workspace': directory, 'record': str(path), 'status': 'found',
                    'key': 'legacy-workspace'}), \
                    mock.patch.object(W.S, 'read_view', return_value=(completed, False)) as read, \
                    mock.patch('scripts.followups.summary', return_value=None):
                code = W.open_context([str(path)])
        self.assertEqual(code, 0)
        read.assert_called_once()

    def test_default_core_opening_is_bounded_but_json_keeps_attention(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'GROUNDING.yaml'
            path.write_text(DOCUMENT, encoding='utf-8')
            nodes = {('d.' + str(index)): {
                'attention': [{'reasons': [{'code': 'needs_review_' + ('x' * 80)}]}],
                'support': {'status': 'clear', 'reservations': []},
                'state': {'falsifier': {'status': 'does_not_hold'}}}
                for index in range(100)}
            context = type('Context', (), {
                'assessment': {'nodes': nodes, 'history_subjects': {},
                               'attention_policy': 'focused-review/v1'},
                'snapshot_id': 's' * 64, 'findings_revision': 'f' * 64,
                'snapshot': type('Snapshot', (), {'to_data': lambda self: {
                    'document': {'meta': {'name': 'Bounded'}}}})()})()
            module = type('ContextModule', (), {
                'CaptureError': ValueError,
                'CapturedAssessment': type('Capture', (), {
                    'capture': staticmethod(lambda files: context)})})
            # only the capture is replaced; every other sibling the opener asks for is real
            peer = P._peer
            def peers(name):
                return module if name == 'reasoning.context' else peer(name)
            location = {'workspace': directory, 'record': str(path), 'status': 'found',
                        'key': 'core-workspace'}
            with mock.patch.object(W.W, 'locate', return_value=location), \
                    mock.patch.object(P, 'core_reader_selected', return_value=True), \
                    mock.patch.object(P, '_peer', side_effect=peers), \
                    contextlib.redirect_stdout(output := io.StringIO()):
                self.assertEqual(W.open_context([str(path)]), 0)
            self.assertLessEqual(len(output.getvalue().rstrip()), W.CORE_OPEN_CHARS)
            self.assertIn('more attention items omitted', output.getvalue())
            with mock.patch.object(W.W, 'locate', return_value=location), \
                    mock.patch.object(P, 'core_reader_selected', return_value=True), \
                    mock.patch.object(P, '_peer', side_effect=peers), \
                    contextlib.redirect_stdout(output := io.StringIO()):
                self.assertEqual(W.open_context(['--json', str(path)]), 0)
            payload = json.loads(output.getvalue())
            self.assertEqual(len(payload['attention']), 100)
            self.assertGreater(payload['text_omitted_attention'], 0)


if __name__ == '__main__':
    unittest.main()
