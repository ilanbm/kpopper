"""The experimental consumer preserves independent findings and versioning."""
import copy
import json
import os
from pathlib import Path
import unittest

from scripts.reasoning.assessment import assess
from scripts.reasoning.snapshot import Snapshot
from scripts.reasoning.evaluate import compare_basis
from scripts.reasoning.contract import digest
from test_reasoning_contract import record


class UnavailableRuntime:
    def request_many(self, requests):
        raise OSError('deliberately unavailable')


class ReasoningAssessmentTests(unittest.TestCase):
    def test_runtime_failure_is_not_false_or_falsification(self):
        report = assess(Snapshot.from_data(record()), ['d.order'], runtime=UnavailableRuntime())
        node = report['nodes']['d.order']
        self.assertEqual(report['schema_version'], 2)
        self.assertEqual(node['state']['falsifier']['status'], 'error')
        self.assertEqual(node['state']['falsifier']['computation']['status'], 'operational_error')
        self.assertNotIn('falsifier_holds', [r['code'] for a in node['attention'] for r in a['reasons']])

    def test_old_history_without_basis_is_not_automatic_attention(self):
        result = assess(Snapshot.from_data(record()), ['d.order'], runtime=UnavailableRuntime())
        finding = result['nodes']['d.order']['state']['basis']['dependencies']['m.total']
        self.assertEqual(finding['basis_comparison'], 'not_recorded')
        self.assertEqual(finding['at_review']['value']['computed']['value'], 30)
        self.assertNotIn('basis', finding['at_review']['value']['computed'])

    def test_prose_remains_a_declared_unknown(self):
        doc = record()
        doc['decisions']['d.order']['wrong_if'] = 'The supplier changes its commitments.'
        report = assess(Snapshot.from_data(doc), ['d.order'], runtime=UnavailableRuntime())
        finding = report['nodes']['d.order']['state']['falsifier']
        self.assertEqual(finding['status'], 'unknown')
        self.assertEqual(finding['reason'], 'declared_prose')

    def test_history_digest_substitution_is_unavailable(self):
        basis = {'version': 1, 'profile': 'core/v1', 'modules': ['arithmetic/v1'],
                 'as_of': None, 'dependencies': []}
        basis['digest'] = digest(basis)
        self.assertEqual(compare_basis(basis, basis), 'same')
        wrong = {**basis, 'as_of': '2026-09-14'}
        self.assertEqual(compare_basis(basis, wrong), 'unavailable')

    def test_separate_schema_does_not_redefine_legacy_v1(self):
        try:
            import jsonschema
        except ImportError:
            if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1':
                raise
            self.skipTest('schema validation uses the installed test dependency jsonschema')
        path = Path(__file__).resolve().parents[1] / 'scripts'
        new_schema = json.loads((path / 'reasoning/assessment.schema.json').read_text())
        old_schema = json.loads((path / 'assessment.schema.json').read_text())
        report = assess(Snapshot.from_data(record()), ['d.order'], runtime=UnavailableRuntime())
        jsonschema.validate(report, new_schema)
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate(report, old_schema)
        broken = copy.deepcopy(report)
        broken['schema_version'] = 1
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate(broken, new_schema)


if __name__ == '__main__':
    unittest.main()
