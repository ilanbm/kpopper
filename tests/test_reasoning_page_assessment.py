"""The page-secondary contract binds presentation inputs without owning truth."""
import copy
import unittest

from scripts.reasoning import page_assessment as P
from scripts.reasoning import assessment as V2
from scripts.reasoning import history_assessment as V3
from scripts.reasoning.contract import OperationalLimit, digest
from scripts.reasoning.snapshot import Snapshot


class UnavailableRuntime:
    def request_many(self, requests):
        raise OSError('deliberately unavailable')


def canonical(value=1, *, as_of=None):
    snapshot = Snapshot.from_data({
        'meta': {'reasoning': {
            'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
        'readings': {'d.one': {'v': value}},
    }, as_of=as_of)
    return V3.from_v2(snapshot, V2.assess(snapshot, runtime=UnavailableRuntime()))


class PageAssessmentTests(unittest.TestCase):
    def inputs(self, report, values=None, **kwargs):
        return P.capture_page_inputs(report, values or {
            'coverage': {'covered': 3, 'spill': 1},
            'arrangements': {'v.now': {'status': 'holds'}},
        }, **kwargs)

    def test_identity_is_deterministic_and_binds_every_secondary_input(self):
        report = canonical()
        brief = P.capture_brief(b'# arrangement\ntitle: Now\n')
        inputs = self.inputs(report)
        first = P.build(report, brief, inputs)
        second = P.build(copy.deepcopy(report), copy.deepcopy(brief), copy.deepcopy(inputs))
        self.assertEqual(first, second)
        self.assertEqual(P.validate(first, report), first)
        self.assertEqual(first['snapshot_id'], report['snapshot_id'])
        self.assertEqual(first['findings_revision'], report['findings_revision'])
        self.assertEqual(first['brief_identity'], brief)
        self.assertEqual(first['page_inputs']['revision'], inputs['page_inputs_revision'])
        self.assertEqual(first['page_projection_version'], P.PAGE_PROJECTION_VERSION)
        self.assertEqual(first['page_assessment_revision'], digest({
            key: value for key, value in first.items() if key != 'page_assessment_revision'}))

    def test_brief_and_page_input_changes_are_presentation_only(self):
        report = canonical()
        original = P.build(report, P.capture_brief(b'title: One\n'),
                           self.inputs(report, {'selection': ['d.one']}))
        brief_changed = P.build(report, P.capture_brief(b'title: Two\n'),
                                self.inputs(report, {'selection': ['d.one']}))
        input_changed = P.build(report, P.capture_brief(b'title: One\n'),
                                self.inputs(report, {'selection': []}))
        for changed in (brief_changed, input_changed):
            self.assertEqual(changed['snapshot_id'], original['snapshot_id'])
            self.assertEqual(changed['findings_revision'], original['findings_revision'])
            self.assertNotEqual(changed['page_assessment_revision'],
                                original['page_assessment_revision'])

    def test_canonical_finding_or_snapshot_change_changes_page_identity(self):
        report = canonical()
        original = P.build(report, P.capture_brief(None), self.inputs(report))
        changed_findings = canonical(2)
        changed_snapshot = canonical(1, as_of='2026-09-17')
        finding_page = P.build(changed_findings, P.capture_brief(None),
                               self.inputs(changed_findings))
        snapshot_page = P.build(changed_snapshot, P.capture_brief(None),
                                self.inputs(changed_snapshot))
        self.assertNotEqual(finding_page['page_assessment_revision'],
                            original['page_assessment_revision'])
        self.assertNotEqual(snapshot_page['page_assessment_revision'],
                            original['page_assessment_revision'])

    def test_page_inputs_cannot_overwrite_or_copy_canonical_v3_findings(self):
        report = canonical()
        for field in ('snapshot_id', 'findings_revision', 'envelope_revision',
                      'assessment_revision', 'page_assessment_revision'):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'cannot overwrite'):
                P.capture_page_inputs(report, {field: 'forged'})
        page = P.build(report, P.capture_brief(None), self.inputs(report))
        self.assertNotIn('nodes', page)
        self.assertNotIn('history', page)
        self.assertNotIn('scope', page)
        self.assertIsNot(page['page_inputs']['values'], report['nodes'])
        report['nodes']['d.one']['state']['basis']['status'] = 'forged-after-capture'
        self.assertEqual(page['findings_revision'], report['findings_revision'])

    def test_malformed_or_mismatched_inputs_are_refused(self):
        report = canonical()
        brief = P.capture_brief(b'title: Now\n')
        inputs = self.inputs(report)
        bad_report = copy.deepcopy(report)
        bad_report['nodes'] = {}
        with self.assertRaisesRegex(ValueError, 'selection|envelope_revision'):
            P.build(bad_report, brief, inputs)
        for bad_brief in ({}, {'status': 'absent', 'sha256': '0' * 64},
                          {'status': 'captured', 'byte_length': -1, 'sha256': '0' * 64}):
            with self.subTest(brief=bad_brief), self.assertRaises(ValueError):
                P.build(report, bad_brief, inputs)
        mismatched = copy.deepcopy(inputs)
        mismatched['findings_revision'] = '9' * 64
        with self.assertRaisesRegex(ValueError, 'different canonical assessment'):
            P.build(report, brief, mismatched)
        tampered = copy.deepcopy(inputs)
        tampered['values']['coverage']['covered'] = 4
        with self.assertRaisesRegex(ValueError, 'page_inputs_revision'):
            P.build(report, brief, tampered)
        page = P.build(report, brief, inputs)
        forged = copy.deepcopy(page)
        forged['page_assessment_revision'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'page_assessment_revision'):
            P.validate(forged, report)
        with self.assertRaisesRegex(ValueError, 'projection version'):
            P.build(report, brief, inputs, page_projection_version=2)

    def test_input_and_output_overflow_refuse_the_whole_envelope(self):
        report = canonical()
        with self.assertRaisesRegex(OperationalLimit, 'page_brief_input_limit'):
            P.capture_brief(b'x' * 129, operational_limits={'input_bytes': 128})
        forged_brief = {'status': 'captured', 'byte_length': 129, 'sha256': '0' * 64}
        with self.assertRaisesRegex(OperationalLimit, 'page_brief_input_limit'):
            P.build(report, forged_brief, self.inputs(report),
                    operational_limits={'input_bytes': 128})
        with self.assertRaisesRegex(OperationalLimit, 'page_input_limit'):
            self.inputs(report, {'large': 'x' * 256},
                        operational_limits={'input_bytes': 128})
        brief = P.capture_brief(None)
        inputs = self.inputs(report)
        with self.assertRaisesRegex(OperationalLimit, 'page_assessment_output_limit'):
            P.build(report, brief, inputs, operational_limits={'output_bytes': 128})

    def test_contract_is_pure_and_does_not_retain_mutable_arguments(self):
        report = canonical()
        values = {'nodes': [{'id': 'd.one'}], 'coverage': {'covered': 1}}
        inputs = P.capture_page_inputs(report, values)
        page = P.build(report, P.capture_brief('title: Now\n'), inputs)
        values['nodes'][0]['id'] = 'changed'
        inputs['values']['coverage']['covered'] = 99
        self.assertEqual(page['page_inputs']['values']['nodes'][0]['id'], 'd.one')
        self.assertEqual(page['page_inputs']['values']['coverage']['covered'], 1)
        self.assertEqual(P.validate(page, report), page)


if __name__ == '__main__':
    unittest.main()
