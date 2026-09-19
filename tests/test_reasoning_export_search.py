"""Export and standalone search share one history-aware core assessment."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import yaml

from scripts import export_graph as E
from scripts import search as S
from scripts.reasoning import context as C


class CoreExportSearchTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / 'GROUNDING.yaml'
        self.source = self.root / 'evidence.md'
        self.source.write_text('secondary-source-needle\n', encoding='utf-8')
        self.document = {
            'meta': {'reasoning': {
                'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
            'sources': {'s.evidence': {'name': 'Evidence', 'file': 'evidence.md'}},
            'readings': {
                'p.value': {'v': 1, 'name': 'Primary value', 'from': 's.evidence'},
                'p.false': {'v': False, 'name': 'Typed false needle'},
            },
            'decisions': {
                'd.unknown': {
                    'rests_on': ['p.value'], 'seen': {'p.value': 1},
                    'wrong_if': 'a prose condition', 'verdict': 'Unknown needle decision',
                },
                'd.false': {
                    'rests_on': ['p.value'], 'seen': {'p.value': 1},
                    'wrong_if': {'expr': 'p.value > 3'}, 'verdict': 'False condition',
                },
            },
        }
        self.record.write_text(yaml.safe_dump(
            self.document, allow_unicode=True, sort_keys=False), encoding='utf-8')

    def captured(self):
        return C.CapturedAssessment.capture([str(self.record)])

    def test_declared_profile_selects_core_for_direct_export_and_search(self):
        exported = E.project([str(self.record)], ['p.value'])
        searched = S.search('Primary value', record=str(self.record))
        self.assertEqual(exported['profile'], 'core/v1')
        self.assertIn('snapshot_id', searched)
        self.assertEqual(exported['snapshot_id'],
                         E.project([str(self.record)], ['p.value'], profile='core/v1')['snapshot_id'])
        self.assertEqual(searched['snapshot_id'],
                         S.search('Primary value', record=str(self.record), profile='core/v1')['snapshot_id'])

    def test_export_projects_one_context_and_never_turns_unknown_into_false(self):
        captured = self.captured()
        with mock.patch.object(C.CapturedAssessment, 'capture', return_value=captured) as capture, \
                mock.patch.object(E.P, 'load', side_effect=AssertionError('second load')), \
                mock.patch.object(E.P, 'evaluate', side_effect=AssertionError('second evaluator')):
            packet = E.project([str(self.record)], ['d.unknown'], profile='core/v1')
            rendered = E.render_markdown(packet)
        capture.assert_called_once_with([str(self.record)])
        self.assertEqual(packet['snapshot_id'], captured.snapshot_id)
        self.assertEqual(packet['findings_revision'], captured.findings_revision)
        condition = packet['nodes']['d.unknown']['condition']
        self.assertIsNone(condition['result'])
        self.assertEqual(packet['nodes']['d.unknown']['status']['falsifier']['status'], 'unknown')
        self.assertIn('not evaluated', rendered)
        self.assertNotIn('false on current', rendered)
        self.assertIn(captured.snapshot_id, rendered)
        self.assertIn(captured.findings_revision, rendered)

    def test_export_uses_context_impacts_and_generic_typed_rendering(self):
        packet = E.project([str(self.record)], ['d.false'], depth=2, profile='core/v1')
        self.assertIn(('d.false', 'impact', 'p.value'), packet['edges'])
        false_packet = E.project([str(self.record)], ['p.false'], profile='core/v1')
        self.assertEqual(false_packet['nodes']['p.false']['status']['computation']['value_text'],
                         'false')
        self.assertIn('calculated current: false', E.render_markdown(false_packet))

    def test_search_indexes_bodies_after_one_context_without_semantic_recalculation(self):
        captured = self.captured()
        with mock.patch.object(C.CapturedAssessment, 'capture', return_value=captured) as capture, \
                mock.patch.object(S.P, 'load', side_effect=AssertionError('second load')), \
                mock.patch.object(S.P, 'evaluate', side_effect=AssertionError('second evaluator')):
            result = S.search('Unknown needle', record=str(self.record), profile='core/v1')
        capture.assert_called_once()
        self.assertEqual(capture.call_args.args[0], [str(self.record.resolve())])
        hit = next(row for row in result['results'] if row['id'] == 'd.unknown')
        self.assertEqual(result['snapshot_id'], captured.snapshot_id)
        self.assertEqual(result['findings_revision'], captured.findings_revision)
        self.assertIsNone(hit['findings']['status']['falsifier']['holds'])
        self.assertEqual(hit['findings']['status']['falsifier']['status'], 'unknown')

    def test_search_clipping_keeps_canonical_findings_and_secondary_corpus_identity(self):
        full = S.search('Unknown needle', record=str(self.record), limit=1,
                        chars=10000, profile='core/v1')
        clipped = S.search('Unknown needle', record=str(self.record), limit=1,
                           chars=3000, profile='core/v1')
        self.assertEqual(clipped['snapshot_id'], full['snapshot_id'])
        self.assertEqual(clipped['findings_revision'], full['findings_revision'])
        self.assertEqual(clipped['results'][0]['findings'], full['results'][0]['findings'])
        self.assertEqual(clipped['corpus_revision'], clipped['revision'])
        self.assertNotEqual(clipped['corpus_revision'], clipped['findings_revision'])

    def test_search_core_exact_source_read_and_named_unsupported_option(self):
        found = S.search('secondary-source-needle', record=str(self.record), profile='core/v1')
        source = next(row for row in found['results'] if row['kind'] == 'source')
        read = S.read(source['ref'], found['revision'], record=str(self.record),
                      profile='core/v1')
        self.assertEqual(read['content'], self.source.read_bytes().decode('utf-8'))
        self.assertEqual(read['snapshot_id'], found['snapshot_id'])
        self.assertEqual(read['findings_revision'], found['findings_revision'])
        with self.assertRaisesRegex(ValueError,
                                    'core_profile_option_unsupported: --state-dir'):
            S.search('needle', record=str(self.record), state_dir=str(self.root / 'state'),
                     profile='core/v1')

    def test_profile_is_explicit_and_legacy_result_shape_is_unchanged(self):
        legacy_record = self.root / 'LEGACY.yaml'
        legacy = dict(self.document)
        legacy.pop('meta')
        legacy_record.write_text(yaml.safe_dump(legacy, sort_keys=False), encoding='utf-8')
        ordinary = S.search('Unknown needle', record=str(legacy_record))
        self.assertNotIn('snapshot_id', ordinary)
        self.assertNotIn('findings_revision', ordinary)
        self.assertIsInstance(ordinary['results'][0]['status'], str)
        self.assertNotIn('profile', E.project([str(legacy_record)], ['d.unknown']))


if __name__ == '__main__':
    unittest.main()
