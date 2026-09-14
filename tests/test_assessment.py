"""Consumers share findings; scope and policy cannot change the evidence."""
import copy
import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import yaml

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import assessment as A
import export_graph as X
import provenance as P


def record():
    return {'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
            'sources': {'s.note': {'file': 'note.md', 'read': '2026-01-01'}},
            'known': {'p.load': {'v': 61, 'from': 's.note'}, 'p.backup': {'v': True}},
            'judgments': {'d.work': {'verdict': 'Keep the current format',
                'rests_on': ['p.load', 'p.backup'], 'seen': {'p.load': 44, 'p.backup': True},
                'wrong_if': 'p.load > 80'}}}


class Assessment(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / 'GROUNDING.yaml'
        self.doc = record()
        self.save()

    def save(self):
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False), encoding='utf-8')

    def report(self, policy=A.POLICY):
        self.save()
        return A.load([str(self.path)], policy)

    def test_independent_fired_condition_and_missing_input_survive(self):
        del self.doc['known']['p.backup']
        self.doc['judgments']['d.work']['wrong_if'] = 'p.load > 60'
        node = self.report()['nodes']['d.work']
        self.assertEqual(node['state']['falsifier']['status'], 'holds')
        self.assertEqual(node['state']['basis']['dependencies']['p.backup']['comparison'], 'unknown')
        self.assertEqual({item['action'] for item in node['attention']}, {'review', 'resolve_gap'})

    def test_muting_is_policy_not_a_lost_comparison(self):
        report = self.report()
        state = report['nodes']['d.work']['state']
        self.assertEqual(state['basis']['dependencies']['p.load']['comparison'], 'changed')
        self.assertEqual(state['falsifier']['status'], 'does_not_hold')
        self.assertEqual(report['nodes']['d.work']['attention'], [])
        self.doc['judgments']['d.work']['wrong_if'] = 'p.backup == false'
        node = self.report()['nodes']['d.work']
        self.assertEqual(node['attention'][0]['reasons'][0]['related_ids'], ['p.load'])

    def test_true_condition_does_not_require_movement(self):
        j = self.doc['judgments']['d.work']; j['seen']['p.load'] = 61; j['wrong_if'] = 'p.load > 60'
        node = self.report()['nodes']['d.work']
        self.assertEqual(node['state']['basis']['dependencies']['p.load']['comparison'], 'same')
        self.assertEqual(node['attention'], [{'action': 'review', 'reasons': [{'code': 'falsifier_holds'}]}])

    def test_no_condition_and_unknown_condition_are_distinct(self):
        j = self.doc['judgments']['d.work']; j.pop('wrong_if'); j['reopened_by'] = 'Organizer withdraws approval'
        node = self.report()['nodes']['d.work']
        self.assertEqual(node['state']['falsifier']['status'], 'not_declared')
        self.assertEqual(node['body']['reopened_by'], j['reopened_by'])
        j['wrong_if'] = 'p.load > 80 and eventually'
        state = self.report()['nodes']['d.work']['state']
        self.assertEqual(state['falsifier']['status'], 'unknown')
        self.assertIsNone(state['falsifier']['reads'])

    def test_null_false_zero_and_missing_are_not_collapsed(self):
        for value in [None, False, 0]:
            with self.subTest(value=value):
                self.doc['known']['p.backup']['v'] = value
                side = self.report()['nodes']['d.work']['state']['basis']['dependencies']['p.backup']['current']
                self.assertEqual(side['status'], 'recorded')
                self.assertIs(side['value'], value)
        del self.doc['known']['p.backup']
        del self.doc['judgments']['d.work']['seen']['p.backup']
        finding = self.report()['nodes']['d.work']['state']['basis']['dependencies']['p.backup']
        self.assertEqual(finding['current'], {'status': 'missing'})
        self.assertEqual(finding['at_review'], {'status': 'missing'})
        self.assertEqual(set(finding['reasons']), {'baseline_missing', 'current_missing'})

    def test_authored_state_never_overrides_computed_findings(self):
        authored = {'falsifier': {'status': 'holds'}}
        self.doc['judgments']['d.work']['state'] = authored
        node = self.report()['nodes']['d.work']
        self.assertEqual(node['body']['state'], authored)
        self.assertEqual(node['state']['falsifier']['status'], 'does_not_hold')

    def test_malformed_dependency_keeps_independent_condition_result(self):
        self.doc['judgments']['d.work']['rests_on'] = 'p.load'
        self.doc['judgments']['d.work']['wrong_if'] = 'p.load > 60'
        node = self.report()['nodes']['d.work']
        self.assertEqual(node['state']['basis']['status'], 'error')
        self.assertEqual(node['state']['basis']['dependencies'], {})
        self.assertEqual(node['state']['falsifier']['status'], 'holds')
        self.assertIn('repair_record', [item['action'] for item in node['attention']])

    def test_quoted_identifiers_are_not_falsifier_reads(self):
        self.doc['known']['p.label'] = {'v': 'p.load'}
        self.doc['judgments']['d.work']['wrong_if'] = "p.label != 'p.load'"
        state = self.report()['nodes']['d.work']['state']
        self.assertEqual(state['falsifier']['reads'], ['p.label'])
        undeclared = [i for i in state['integrity']['issues'] if i['code'] == 'undeclared_predicate_dependencies']
        self.assertEqual(undeclared[0]['related_ids'], ['p.label'])

    def test_scoped_hypotheses_keep_witnesses_alternatives_and_skips(self):
        folder = self.path.parent / '.kpopper/hypotheses'; folder.mkdir(parents=True)
        (folder / 'a.yaml').write_text('known:\n  p.load: {v: 70}\n')
        report = self.report()
        contention = report['nodes']['p.load']['state']['contention']
        self.assertEqual(contention['status'], 'none_detected')
        self.assertEqual(contention['alternatives'], [{'hypothesis': 'a', 'id': 'p.load'}])
        (folder / 'b.yaml').write_text('known:\n  p.load: {v: 90}\n')
        (folder / 'bad.yaml').write_text('known: [')
        report = self.report()
        contention = report['nodes']['p.load']['state']['contention']
        self.assertEqual(contention['status'], 'detected')
        self.assertEqual({w['claim'] for w in contention['witnesses']}, {70, 90})
        self.assertIn('bad', report['scope']['hypotheses_skipped'])
        self.assertEqual(report['scope']['hypotheses_checked'], ['a', 'b'])

    def test_policy_and_task_selection_need_no_evaluator_or_new_read(self):
        self.doc['judgments']['d.unrelated'] = {'verdict': 'Other work', 'rests_on': ['missing.input'],
                                               'seen': {'missing.input': 10}, 'reopened_by': 'New evidence'}
        report = self.report(); before = copy.deepcopy(report)
        with patch.object(P, 'evaluate', side_effect=AssertionError('must not evaluate')), \
             patch.object(P, 'value_of', side_effect=AssertionError('must not read values')), \
             patch.object(P, 'load', side_effect=AssertionError('must not read files')):
            self.assertEqual(A.selected_attention(report, ['d.work']), {})
            self.assertIn('d.unrelated', A.selected_attention(report))
            self.assertEqual(A.attention(report['nodes']['d.unrelated']['state'], 'falsifiers-only/v1'), [])
        self.assertEqual(report, before)

    def test_export_and_cli_consume_the_same_assessment_without_leaking_values(self):
        report = self.report()
        out = subprocess.run([sys.executable, str(ROOT / 'scripts/cli.py'), 'assess', 'd.work',
                              '--record', str(self.path)], capture_output=True, text=True)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(json.loads(out.stdout)['nodes']['d.work'], report['nodes']['d.work'])
        packet = X.project([str(self.path)], ['d.work'], depth=0)
        self.assertEqual(packet['assessment']['record_revision'], report['record_revision'])
        self.assertEqual(packet['nodes']['d.work']['readings'][0]['comparison'], 'muted')
        self.assertNotIn('current', packet['nodes']['d.work']['readings'][0])
        row = next(line for line in X.render_markdown(packet).splitlines() if line.startswith('| p.load |'))
        self.assertNotIn('61', row)

    def test_existing_flags_consume_shared_falsifier_findings(self):
        doc = P.load([str(self.path)]); ids, judgments, fields = P.infer(doc); raw = P.bodies(doc)
        state = A.judgment_state(judgments['d.work'], raw, ids, fields, judgments)
        state['falsifier']['status'] = 'holds'
        with patch.object(A, 'judgment_state', return_value=state):
            self.assertIn('falsified', P.flags(ids, judgments, fields, raw)['d.work'])

    def test_non_judgment_and_unknown_ids_and_unparseable_record(self):
        state = self.report()['nodes']['s.note']['state']
        self.assertEqual(state['basis']['status'], 'not_applicable')
        self.assertEqual(state['integrity']['status'], 'unassessed')
        with self.assertRaises(ValueError):
            A.selected_attention(self.report(), ['not.here'])
        self.path.write_text('judgments: [')
        with self.assertRaises(yaml.YAMLError):
            A.load([str(self.path)])

    def test_custom_roles_and_record_preservation(self):
        self.doc['schema'] = {'deps': 'based_on', 'snapshot': 'reviewed_values', 'predicate': 'reject_if'}
        j = self.doc['judgments']['d.work']
        for old, new in [('rests_on', 'based_on'), ('seen', 'reviewed_values'), ('wrong_if', 'reject_if')]:
            j[new] = j.pop(old)
        self.save(); before = self.path.read_bytes()
        report = A.load([str(self.path)])
        self.assertEqual(report['nodes']['d.work']['state']['basis']['dependencies']['p.load']['comparison'], 'changed')
        self.assertEqual(report['nodes']['d.work']['body'], j)
        self.assertEqual(self.path.read_bytes(), before)

    def test_policy_changes_attention_but_never_findings_or_identity(self):
        self.doc['judgments']['d.work']['wrong_if'] = 'p.backup == false'
        focused = self.report()
        strict = self.report('falsifiers-only/v1')
        self.assertEqual(focused['record_revision'], strict['record_revision'])
        self.assertEqual(focused['assessment_revision'], strict['assessment_revision'])
        self.assertEqual(focused['nodes']['d.work']['state'], strict['nodes']['d.work']['state'])
        self.assertTrue(focused['nodes']['d.work']['attention'])
        self.assertEqual(strict['nodes']['d.work']['attention'], [])

    def test_formula_change_is_not_reported_as_a_value_change(self):
        try:
            P.E._core_type()()
        except (ValueError, ImportError):
            if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1':
                raise
            self.skipTest('structured assessment needs the expression core')
        self.doc['known']['p.total'] = {'rule': {'expr': 'p.load + 0'}}
        self.doc['judgments']['d.work'].update(rests_on=['p.total'],
            seen={'p.total': {'computed': {'value': 61, 'rule': {'expr': 'p.load * 1'}}}},
            wrong_if={'expr': 'p.total > 80'})
        node = self.report()['nodes']['d.work']
        finding = node['state']['basis']['dependencies']['p.total']
        self.assertEqual(finding['comparison'], 'same')
        self.assertTrue(finding['rule_changed'])
        self.assertEqual(node['attention'][0]['reasons'], [{'code': 'formula_changed', 'related_ids': ['p.total']}])

    def test_evaluator_availability_changes_assessment_identity_not_record_identity(self):
        self.doc['known']['p.total'] = {'rule': {'expr': 'p.load + 0'}}
        self.doc['judgments']['d.work'].update(rests_on=['p.total'],
            seen={'p.total': {'computed': {'value': 61, 'rule': {'expr': 'p.load + 0'}}}},
            wrong_if={'expr': 'p.total > 80'})
        available = self.report()
        with patch.object(P.E, '_CORE', None), patch.object(P.E, '_core_type', side_effect=ValueError('not ready')):
            unavailable = self.report()
        self.assertEqual(available['record_revision'], unavailable['record_revision'])
        if available['nodes']['d.work']['state']['falsifier']['status'] == 'does_not_hold':
            self.assertNotEqual(available['assessment_revision'], unavailable['assessment_revision'])
        finding = unavailable['nodes']['d.work']['state']['basis']['dependencies']['p.total']
        self.assertEqual(finding['current']['status'], 'unavailable')
        self.assertEqual(finding['comparison'], 'unknown')

    def test_invalid_snapshot_and_predicate_types_are_errors_not_absence(self):
        self.doc['judgments']['d.work'].update(seen=['p.load'], wrong_if=False)
        state = self.report()['nodes']['d.work']['state']
        self.assertEqual(state['falsifier']['status'], 'error')
        self.assertEqual(state['falsifier']['reason'], 'invalid_predicate_type')
        self.assertEqual(state['basis']['dependencies']['p.load']['at_review'],
                         {'status': 'unavailable', 'reason': 'invalid_snapshot'})

    def test_empty_structured_condition_is_not_an_absent_condition(self):
        self.doc['judgments']['d.work']['wrong_if'] = {}
        state = self.report()['nodes']['d.work']['state']
        self.assertEqual(state['falsifier']['status'], 'error')
        self.assertEqual(state['falsifier']['reason'], 'invalid_predicate_shape')
        self.assertIsNone(state['falsifier']['reads'])

    def test_unreadable_record_shape_is_a_cli_error_without_traceback(self):
        self.doc['judgments']['d.work']['rests_on'] = 17
        self.save()
        out = subprocess.run([sys.executable, str(ROOT / 'scripts/cli.py'), 'assess', 'd.work',
                              '--record', str(self.path)], capture_output=True, text=True)
        self.assertNotEqual(out.returncode, 0)
        self.assertEqual(out.stdout, '')
        self.assertIn('cannot interpret record structure', out.stderr)
        self.assertNotIn('Traceback', out.stderr)

    def test_non_json_mapping_keys_are_clean_cli_errors(self):
        self.doc['known']['p.load']['history'] = {datetime.date(2026, 1, 1): 1}
        self.save()
        for command in ['assess', 'export']:
            out = subprocess.run([sys.executable, str(ROOT / 'scripts/cli.py'), command, 'd.work',
                                  '--record', str(self.path)], capture_output=True, text=True)
            self.assertEqual(out.returncode, 2, out.stderr)
            self.assertNotIn('Traceback', out.stderr)
            self.assertIn('mapping keys', out.stderr)

    def test_unsupported_condition_keeps_lexical_clues_separate_from_reads(self):
        self.doc['known']['p.other'] = {'v': 3}
        self.doc['judgments']['d.work']['wrong_if'] = 'p.load > 80 or p.other > 1'
        node = self.report()['nodes']['d.work']
        self.assertIsNone(node['state']['falsifier']['reads'])
        self.assertIn('repair_record', [item['action'] for item in node['attention']])
        text = X.render_markdown(X.project([str(self.path)], ['d.work']))
        self.assertIn('Unparsed condition mentions undeclared entries: p.other', text)
        self.assertNotIn('Condition reads undeclared dependencies: p.other', text)

    def test_metadata_is_not_an_entry_or_a_premise_value(self):
        self.doc['meta'] = {'prefixes': {'p': 'premise'}}
        self.doc['judgments']['d.work']['rests_on'].append('prefixes')
        self.doc['judgments']['d.work']['seen']['prefixes'] = 'present'
        report = self.report()
        self.assertNotIn('prefixes', report['nodes'])
        finding = report['nodes']['d.work']['state']['basis']['dependencies']['prefixes']
        self.assertEqual(finding['current'], {'status': 'missing'})
        packet = X.project([str(self.path)], ['d.work'])
        self.assertEqual(packet['nodes']['d.work']['readings'][-1]['current_status'], 'missing')

    def test_historical_calculation_has_a_fixed_shape_without_erasing_source_fields(self):
        self.doc['known']['p.total'] = {'rule': {'expr': 'p.load + 0'}}
        captured = {'value': 61, 'rule': {'expr': 'p.load + 0'}, 'unit': 'kW'}
        self.doc['judgments']['d.work'].update(rests_on=['p.total'], seen={'p.total': {'computed': captured}})
        finding = self.report()['nodes']['d.work']['state']['basis']['dependencies']['p.total']
        self.assertEqual(set(finding['historical_calculation']), {'value', 'rule'})
        self.assertEqual(finding['at_review']['value'], {'computed': captured})

    def test_attention_only_report_can_be_selected_again(self):
        self.doc['judgments']['d.work']['wrong_if'] = 'p.load > 60'
        full = self.report()
        report = {key: value for key, value in full.items() if key != 'nodes'}
        report.update(selection=['d.work'], attention=A.selected_attention(full, ['d.work']))
        self.assertEqual(A.selected_attention(report, ['d.work'], ['review']), report['attention'])
        with self.assertRaises(ValueError):
            A.selected_attention(report, ['not.selected'])

    def test_reports_match_the_shipped_json_schema(self):
        try:
            import jsonschema
        except ImportError:
            if os.environ.get('KPOPPER_REQUIRE_CORE_TESTS') == '1':
                raise
            self.skipTest('schema validation uses the session dependency jsonschema')
        schema = json.loads((ROOT / 'scripts/assessment.schema.json').read_text())
        jsonschema.Draft202012Validator.check_schema(schema)
        del self.doc['known']['p.backup']
        self.doc['judgments']['d.work']['wrong_if'] = 'p.load > 60'
        report = json.loads(json.dumps(self.report(), default=str))
        jsonschema.validate(report, schema)
        selected = {key: value for key, value in report.items() if key != 'nodes'}
        selected.update(selection=['d.work'], attention=A.selected_attention(report, ['d.work']))
        jsonschema.validate(selected, schema)
        malformed = copy.deepcopy(report)
        malformed['nodes']['d.work']['state']['basis']['dependencies']['p.load']['current'] = {
            'status': 'missing', 'value': 61}
        with self.assertRaises(jsonschema.ValidationError):
            jsonschema.validate(malformed, schema)


if __name__ == '__main__':
    unittest.main()
