"""The explicit core page is a source-free projection with secondary page identity."""
import copy
import html
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import render_page as R
from scripts.reasoning import page_assessment
from scripts.reasoning.context import CapturedAssessment
from scripts.reasoning.snapshot import Snapshot


DOCUMENT = {
    'meta': {'name': 'Core values', 'reasoning': {
        'version': 2, 'profile': 'core/v1',
        'requires': ['arithmetic/v1', 'composition/v1']}},
    'readings': {
        'v.zero': {'v': 0},
        'v.false': {'v': False},
        'v.null': {'v': None},
        'v.list': {'v': [0, False, None, 'text']},
        'v.record': {'v': {'z': None, 'a': [False, 0]}},
    },
    'judgments': {
        'd.safe': {
            'rests_on': ['v.false'], 'seen': {'v.false': False},
            'verdict': 'false remains false',
            'wrong_if': {'op': 'eq', 'args': [{'ref': 'v.false'}, {'bool': True}]},
        },
    },
}


class CorePageConsumerTests(unittest.TestCase):
    def setUp(self):
        self.context = CapturedAssessment.from_snapshot(Snapshot.from_data(DOCUMENT))
        self.brief = (b'# exact captured bytes\n'
                      b'title: Core page\n'
                      b'sections:\n'
                      b'  - title: Everything\n'
                      b'    why: Generic typed rendering\n'
                      b'    pick: all\n')

    def build(self, brief=None):
        return R.core_build_from_context(
            self.context, self.brief if brief is None else brief,
            '/tmp/output/page.html', record_root='/tmp/record')

    def test_pure_seam_uses_retained_v3_without_legacy_truth_or_source_io(self):
        before = self.context.assessment
        with mock.patch.object(R.P, 'load', side_effect=AssertionError('semantic reload')), \
                mock.patch.object(R.P, 'flags', side_effect=AssertionError('legacy flags')), \
                mock.patch.object(R.P, 'evaluate', side_effect=AssertionError('legacy evaluator')), \
                mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('source read')), \
                mock.patch('subprocess.run', side_effect=AssertionError('runtime call')):
            page, entries, judgments, ids, info = self.build()

        readable = html.unescape(page)
        self.assertIn('data-profile="core/v1"', page)
        self.assertIn('data-value-kind="typed">0', page)
        self.assertIn('data-value-kind="typed">false', page)
        self.assertIn('[0, false, null, "text"]', readable)
        self.assertIn('{"a": [false, 0], "z": null}', readable)
        self.assertIn('data-state-dimension="acceptance"', page)
        self.assertIn('data-state-dimension="falsifier"', page)
        self.assertEqual(set(entries) | set(judgments), ids)
        self.assertEqual(info['snapshot_id'], self.context.snapshot_id)
        self.assertEqual(info['findings_revision'], self.context.findings_revision)
        self.assertEqual(page_assessment.validate(
            info['page_assessment'], self.context.assessment), info['page_assessment'])
        self.assertEqual(self.context.assessment, before)

    def test_brief_and_coverage_change_only_the_page_secondary_revision(self):
        canonical = copy.deepcopy(self.context.assessment)
        first = self.build(b'title: One\nsections:\n- title: One\n  pick: v.zero\n')[4]
        second = self.build(b'title: Two\nsections:\n- title: Two\n  pick: all\n')[4]

        self.assertEqual(first['snapshot_id'], second['snapshot_id'])
        self.assertEqual(first['findings_revision'], second['findings_revision'])
        self.assertNotEqual(first['coverage'], second['coverage'])
        self.assertNotEqual(first['page_assessment_revision'],
                            second['page_assessment_revision'])
        self.assertEqual(self.context.assessment, canonical)

    def test_exact_brief_bytes_are_bound_even_when_yaml_meaning_is_equal(self):
        first = self.build(b'title: Same\n')[4]['page_assessment']
        second = self.build(b'# comment\ntitle: Same\n')[4]['page_assessment']
        self.assertNotEqual(first['brief_identity'], second['brief_identity'])
        self.assertNotEqual(first['page_assessment_revision'],
                            second['page_assessment_revision'])

    def test_absent_brief_draws_only_the_record_and_binds_absence(self):
        page, _, _, _, info = R.core_build_from_context(self.context, None)
        self.assertNotIn('data-page-tab=', page)
        self.assertEqual(info['tabs'], [])
        self.assertEqual(info['page_assessment']['brief_identity'], {'status': 'absent'})

    def test_explicit_profile_route_captures_one_context_once(self):
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'GROUNDING.yaml'
            brief = Path(directory) / 'view.yaml'
            record.write_text('not read by the mocked capture\n', encoding='utf-8')
            brief.write_bytes(self.brief)
            with mock.patch.object(CapturedAssessment, 'capture',
                                   return_value=self.context) as capture, \
                    mock.patch.object(R.P, 'load', side_effect=AssertionError('legacy reload')), \
                    mock.patch.object(R.P, 'flags', side_effect=AssertionError('legacy flags')), \
                    mock.patch.object(R.P, 'evaluate', side_effect=AssertionError('legacy evaluator')):
                result = R.build([str(record)], str(brief), profile='core/v1')
        capture.assert_called_once_with([str(record)], policy='focused-review/v1')
        self.assertEqual(result[4]['snapshot_id'], self.context.snapshot_id)
        self.assertEqual(result[4]['profile'], 'core/v1')

    def test_core_verify_reuses_a_supplied_context_and_checks_bound_output(self):
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'GROUNDING.yaml'
            brief = Path(directory) / 'view.yaml'
            record.write_text('not read by a supplied context\n', encoding='utf-8')
            brief.write_bytes(self.brief)
            with mock.patch.object(CapturedAssessment, 'capture',
                                   side_effect=AssertionError('recapture')):
                self.assertEqual(R.verify([str(record)], str(brief), profile='core/v1',
                                          context=self.context), 0)

    def test_missing_selector_fails_page_verify(self):
        brief = b'title: Missing\nsections:\n- title: Missing\n  pick: missing.prefix\n'
        _, _, _, _, info = self.build(brief)
        self.assertEqual(info['coverage']['unresolved_selectors'], ['missing.prefix'])
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'GROUNDING.yaml'
            view = Path(directory) / 'view.yaml'
            record.write_text('not read by supplied context\n', encoding='utf-8')
            view.write_bytes(brief)
            self.assertEqual(R.verify([str(record)], str(view), profile='core/v1',
                                      context=self.context), 1)

    def test_stale_shape_and_renderer_misfit_fail_page_verify(self):
        brief = (b'title: Stale\n'
                 b'shape: {entries: 999, judgments: 999, flagged: 999, blocked: 999}\n'
                 b'sections:\n- title: Bad fit\n  as: comparison\n  pick: all\n')
        _, _, _, _, info = self.build(brief)
        self.assertTrue(info['coverage']['stale_shapes'])
        self.assertTrue(info['coverage']['renderer_misfits'])
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'GROUNDING.yaml'
            view = Path(directory) / 'view.yaml'
            record.write_text('not read by supplied context\n', encoding='utf-8')
            view.write_bytes(brief)
            self.assertEqual(R.verify([str(record)], str(view), profile='core/v1',
                                      context=self.context), 1)

    def test_default_build_does_not_enter_the_core_route(self):
        with tempfile.TemporaryDirectory() as directory:
            record = Path(directory) / 'GROUNDING.yaml'
            record.write_text(
                'known:\n  x: {v: 1}\njudgments:\n'
                '  d: {rests_on: [x], seen: {x: 1}, verdict: ok, wrong_if: x > 2}\n',
                encoding='utf-8')
            with mock.patch.object(R, 'core_build', side_effect=AssertionError('core route')):
                page, _, _, _, info = R.build([str(record)])
        self.assertIn('<html', page)
        self.assertNotIn('profile', info)

    def test_captured_core_context_selects_core_without_a_redundant_profile(self):
        with mock.patch.object(R, 'core_build', return_value='captured core') as build:
            self.assertEqual(R.build(['record.yaml'], context=self.context), 'captured core')
        build.assert_called_once_with(['record.yaml'], None, None, read_mode=None, context=self.context)


if __name__ == '__main__':
    unittest.main()
