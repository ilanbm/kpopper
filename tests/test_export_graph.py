"""Portable exports preserve meaning even when Mermaid is unavailable."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))


def example():
    return {
        'schema': {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'},
        'sources': {'s.note': {'name': 'Source "primary" <script> & #quot; [check] `text`',
                               'file': 'notes.md'}},
        'known': {'m.cost': {'v': 12, 'from': 's.note', 'name': 'Cost'},
                  'm.other': {'v': 3, 'from': 'a prose locator'},
                  'm.total': {'rule': 'm.cost + m.other'}},
        'judgments': {
            'd.choice': {'verdict': 'Continue', 'rests_on': ['m.cost'],
                         'seen': {'m.cost': 10}, 'wrong_if': 'm.cost > 20'},
            'd.moved': {'verdict': 'Review again', 'rests_on': ['m.cost'],
                        'seen': {'m.cost': 10}, 'reopened_by': 'A new quote arrives'},
            'd.false': {'verdict': 'Decision', 'rests_on': ['m.cost'],
                        'seen': {'m.cost': 10}, 'wrong_if': 'm.cost > 11'}},
        'findings': {'hyp.old': {'v': 'refuted', 'refutes': ['d.moved'], 'at': 'Trial failed'}},
    }


class ExportGraph(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / 'GROUNDING.yaml'
        self.doc = example()
        self.save()

    def save(self):
        self.record.write_text(yaml.safe_dump(self.doc, allow_unicode=True), encoding='utf-8')

    def cli(self, *args):
        return subprocess.run([sys.executable, str(ROOT / 'scripts/cli.py'),
                               '--workspace', str(self.root), 'export', *args],
                              capture_output=True, text=True)

    def packet(self, *ids, **kwargs):
        import export_graph as E
        return E.project([str(self.record)], list(ids), **kwargs)

    def test_default_is_readable_text_without_diagram_html_or_images_and_no_writes(self):
        before = self.record.read_bytes()
        out = self.cli('d.choice', '--depth', '2')
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn('d.choice', out.stdout)
        self.assertIn('s.note', out.stdout)
        self.assertIn('rests_on', out.stdout)
        for marker in ('```', '<script>', '![', 'flowchart'):
            self.assertNotIn(marker, out.stdout)
        self.assertEqual(before, self.record.read_bytes())
        self.assertFalse((self.root / '.kpopper').exists())

    def test_status_uses_reader_semantics_and_refutation_is_a_distinct_relation(self):
        packet = self.packet('d.choice', 'd.moved', 'd.false', 'hyp.old')
        self.assertNotIn('moved', packet['nodes']['d.choice']['states'])
        self.assertIn('moved', packet['nodes']['d.moved']['states'])
        self.assertNotIn('falsified', packet['nodes']['d.moved']['states'])
        self.assertIn('falsified', packet['nodes']['d.false']['states'])
        self.assertIn(('hyp.old', 'refutes', 'd.moved'), packet['edges'])

    def test_depth_zero_labels_history_without_leaking_omitted_current_values(self):
        self.doc['known']['m.cost']['v'] = 98765
        self.save()
        out = self.cli('d.false', '--depth', '0').stdout
        self.assertIn('at_review', out)
        self.assertIn('historical', out)
        self.assertIn('not included in this excerpt', out)
        self.assertNotIn('98765', out)
        self.assertIn('true on current recorded values', out)

    def test_muted_change_is_explicit_without_asking_for_a_new_review(self):
        out = self.cli('d.choice').stdout
        self.assertIn('within condition; no review flag', out)
        self.assertIn('false on current recorded values', out)
        self.assertNotIn('MOVED', out)
        self.assertNotIn('absence of a flag', out.lower())

    def test_omitting_current_values_preserves_comparison_results(self):
        self.doc['judgments']['d.same'] = {
            'verdict': 'Continue', 'rests_on': ['m.cost'],
            'seen': {'m.cost': 12}, 'wrong_if': 'm.cost > 20'}
        self.doc['judgments']['d.unreviewed'] = {
            'verdict': 'Pending', 'rests_on': ['m.cost'], 'wrong_if': 'm.cost > 20'}
        self.save()
        for nid in ['d.choice', 'd.moved', 'd.false', 'd.same', 'd.unreviewed']:
            with self.subTest(nid=nid):
                full = self.packet(nid)['nodes'][nid]
                bounded = self.packet(nid, depth=0)['nodes'][nid]
                self.assertEqual(bounded['readings'][0]['comparison'], full['readings'][0]['comparison'])
                self.assertEqual(bounded['condition'], full['condition'])
                self.assertEqual(bounded['readings'][0]['current_status'], 'omitted')
                self.assertNotIn('current', bounded['readings'][0])
        text = self.cli('d.choice', '--depth', '0').stdout
        self.assertIn('changed; within condition; no review flag', text)
        self.assertIn('not included in this excerpt', text)

    def test_selected_malformed_dependencies_are_not_exported_as_character_ids(self):
        self.doc['judgments']['d.choice']['rests_on'] = 'm.cost'
        self.save()
        before = self.record.read_bytes()
        out = self.cli('d.choice')
        self.assertNotEqual(out.returncode, 0)
        self.assertEqual(out.stdout, '')
        self.assertIn('d.choice', out.stderr)
        self.assertIn('rests_on must be a list', out.stderr)
        self.assertEqual(self.record.read_bytes(), before)
        # An unrelated valid entry can still be read from the same record.
        self.assertEqual(self.cli('s.note').returncode, 0)

    def test_undeclared_condition_input_is_visible_without_erasing_its_result(self):
        self.doc['judgments']['d.choice']['rests_on'] = ['m.other']
        self.doc['judgments']['d.choice']['seen'] = {'m.other': 3}
        self.save()
        for predicate, outcome in [('m.cost > 20', 'false'), ('m.cost > 11', 'true')]:
            with self.subTest(predicate=predicate):
                self.doc['judgments']['d.choice']['wrong_if'] = predicate
                self.save()
                text = self.cli('d.choice', '--depth', '0').stdout
                self.assertIn(outcome + ' on current recorded values', text)
                self.assertIn('Condition reads undeclared dependencies: m.cost', text)

    def test_fired_condition_survives_another_missing_dependency(self):
        body = self.doc['judgments']['d.false']
        body['rests_on'].append('missing.backup')
        body['seen']['missing.backup'] = True
        body['blocked_on'] = {'missing': ['missing.backup'], 'why': 'Awaiting confirmation'}
        self.save()
        out = self.cli('d.false').stdout
        self.assertIn('FALSIFIED', out)
        self.assertIn('BLOCKED', out)
        self.assertIn('true on current recorded values', out)
        row = next(line for line in out.splitlines() if line.startswith('| missing.backup |'))
        self.assertIn('true', row)
        self.assertIn('missing from record', row)
        self.assertNotIn('changed', row)
        diagram = self.cli('d.false', '--format', 'mermaid').stdout
        self.assertIn('MISSING', diagram)
        self.assertIn('FALSIFIED:', diagram)
        self.assertIn('BLOCKED:', diagram)
        self.assertNotIn('null', diagram)

    def test_null_false_zero_and_absent_snapshot_are_distinct(self):
        self.doc['known'].update({'m.null': {'v': None}, 'm.false': {'v': False}, 'm.zero': {'v': 0}})
        self.doc['judgments']['d.values'] = {'verdict': 'Values',
            'rests_on': ['m.null', 'm.false', 'm.zero'], 'seen': {'m.null': 1, 'm.false': False},
            'reopened_by': 'A changed input'}
        self.save()
        out = self.cli('d.values').stdout
        rows = {line.split(' | ')[0]: line for line in out.splitlines() if line.startswith('| m.')}
        self.assertIn('null (recorded)', rows['| m.null'])
        self.assertIn('not compared', rows['| m.null'])
        self.assertIn('false | false', rows['| m.false'])
        self.assertIn('not recorded | 0', rows['| m.zero'])

    def test_custom_snapshot_role_and_optional_raw_details(self):
        self.doc['schema']['snapshot'] = 'baseline'
        for body in self.doc['judgments'].values():
            body['baseline'] = body.pop('seen')
        self.save()
        before = self.record.read_bytes()
        out = self.cli('d.choice', '--details')
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn('at_review', out.stdout)
        self.assertIn('historical snapshot \\(baseline\\)', out.stdout)
        self.assertEqual(before, self.record.read_bytes())

    def test_source_and_rule_readings_do_not_gain_new_comparison_semantics(self):
        self.doc['sources']['s.note']['read'] = '2026-09-14'
        self.doc['judgments']['d.source'] = {'verdict': 'Reading',
            'rests_on': ['s.note', 'm.total'],
            'seen': {'s.note': 'read 2026-09-13', 'm.total': 'm.cost - m.other'},
            'reopened_by': 'New evidence'}
        self.save()
        out = self.cli('d.source').stdout
        self.assertIn('2026-09-14', out)
        self.assertIn('not compared by reader', out)
        self.assertNotIn('MOVED', out)

    def test_large_dependency_tables_disclose_their_limit(self):
        deps = [f'm.extra{i}' for i in range(25)]
        self.doc['known'].update({d: {'v': i} for i, d in enumerate(deps)})
        self.doc['judgments']['d.wide'] = {'rests_on': deps, 'seen': {d: 0 for d in deps},
                                         'reopened_by': 'A change'}
        self.save()
        out = self.cli('d.wide', '--depth', '0').stdout
        rows = [line for line in out.splitlines() if line.startswith('| m.extra')]
        self.assertEqual(len(rows), 12)
        self.assertIn('13 dependency rows omitted', out)

    def test_non_string_identifiers_are_rejected_without_a_traceback(self):
        self.doc['known'][2024] = {'v': 1}
        self.save()
        out = self.cli('d.choice')
        self.assertNotEqual(out.returncode, 0)
        self.assertIn('string', out.stderr)
        self.assertNotIn('Traceback', out.stderr)

    def test_mixed_metadata_keys_do_not_break_record_revision(self):
        self.doc['known']['m.cost']['metadata'] = {2024: 'old', 'current': 'new'}
        self.save()
        out = self.cli('d.choice')
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(out.stdout, self.cli('d.choice').stdout)

    def test_collection_names_are_escaped_in_text_headings(self):
        self.doc['notes\n```<script>'] = self.doc.pop('known')
        self.save()
        out = self.cli('m.cost').stdout
        self.assertNotIn('```', out)
        self.assertNotIn('<script>', out)

    def test_bare_values_keep_units_in_the_diagram(self):
        self.doc['known']['m.other']['unit'] = 'percent'
        self.save()
        out = self.cli('m.other', '--format', 'mermaid').stdout
        self.assertIn('3 percent', out)

    def test_detail_hint_names_the_export_command(self):
        self.assertIn('kpop export ID --details', self.cli('d.choice').stdout)

    def test_support_and_impact_keep_original_orientation_and_do_not_infer_prose_links(self):
        support = self.packet('d.choice', depth=2)
        self.assertEqual(set(support['nodes']), {'d.choice', 'm.cost', 's.note'})
        self.assertIn(('d.choice', 'rests_on', 'm.cost'), support['edges'])
        impact = self.packet('s.note', direction='impact', depth=2)
        self.assertIn('d.choice', impact['nodes'])
        self.assertIn(('m.cost', 'from', 's.note'), impact['edges'])
        rule = self.packet('m.total')
        self.assertIn(('m.total', 'rule_reads', 'm.cost'), rule['edges'])
        self.assertNotIn('a prose locator', self.packet('m.other')['nodes'])

    def test_cycle_caps_depth_and_missing_endpoints_are_explicit(self):
        self.doc['known']['m.cost']['from'] = 'm.total'
        self.doc['judgments']['d.choice']['rests_on'].append('missing.entry')
        self.save()
        packet = self.packet('d.choice', depth=4, max_nodes=3)
        self.assertEqual(len(packet['nodes']), 3)
        self.assertGreater(packet['frontier_edges'], 0)
        self.assertGreater(packet['outside_nodes'], 0)
        self.assertTrue(packet['nodes']['missing.entry']['missing'])
        zero = self.packet('d.choice', depth=0)
        self.assertEqual(list(zero['nodes']), ['d.choice'])
        self.assertGreater(zero['frontier_edges'], 0)

    def test_mermaid_is_explicit_and_never_replaces_the_readable_summary(self):
        plain = self.cli('d.choice', '--depth', '2').stdout
        both = self.cli('d.choice', '--depth', '2', '--format', 'markdown-mermaid').stdout
        self.assertTrue(both.startswith(plain))
        self.assertEqual(both.count('```mermaid'), 1)
        raw = self.cli('d.choice', '--depth', '2', '--format', 'mermaid').stdout
        self.assertTrue(raw.startswith('flowchart TB\n'))
        self.assertIn('#34;', raw)
        self.assertIn('v: 12', raw)
        self.assertNotIn('<script>', raw)
        self.assertNotIn('`text`', raw)

    def test_long_values_clipping_and_hypotheses_are_disclosed(self):
        self.doc['known']['m.cost']['name'] = 'Long label ' * 100
        self.save()
        folder = self.root / '.kpopper/hypotheses'
        folder.mkdir(parents=True)
        (folder / 'rival.yaml').write_text('known:\n  m.cost: {v: 99}\n')
        (folder / 'another.yaml').write_text('known:\n  m.cost: {v: 100}\n')
        packet = self.packet('m.cost')
        self.assertEqual(packet['hypotheses'], 2)
        self.assertIn('contested', packet['nodes']['m.cost']['states'])
        out = self.cli('m.cost').stdout
        self.assertIn('characters omitted', out)
        self.assertIn('hypotheses', out)

    def test_validation_help_json_and_determinism(self):
        for args in [(), ('unknown',), ('d.choice', '--depth', '5'),
                     ('d.choice', '--max-nodes', '0'),
                     ('d.choice', 'd.false', '--max-nodes', '1')]:
            out = self.cli(*args)
            self.assertNotEqual(out.returncode, 0)
            self.assertEqual(out.stdout, '')
        self.assertIn('--format', self.cli('--help').stdout)
        first = self.cli('d.choice').stdout
        self.assertEqual(first, self.cli('d.choice').stdout)
        wrapped = self.cli('d.choice', '--json')
        self.assertEqual(json.loads(wrapped.stdout)['output'], first)

    def test_source_only_record_and_inferred_field_names(self):
        self.doc = {'known': {'a.value': {'v': 1, 'from': 'some notes'}}}
        self.save()
        self.assertIn('a.value', self.packet('a.value')['nodes'])
        self.doc = example()
        self.doc['schema'] = {'deps': 'depends', 'snapshot': 'reviewed', 'predicate': 'fails'}
        for body in self.doc['judgments'].values():
            for old, new in [('rests_on', 'depends'), ('seen', 'reviewed'), ('wrong_if', 'fails')]:
                if old in body:
                    body[new] = body.pop(old)
        self.save()
        packet = self.packet('d.false')
        self.assertIn('falsified', packet['nodes']['d.false']['states'])
        self.assertIn(('d.false', 'rests_on', 'm.cost'), packet['edges'])

    def test_dense_graph_edge_cap_and_metadata_are_not_silent(self):
        self.doc = {'meta': {'name': 'Not an entry'},
                    'schema': {'deps': 'rests_on', 'snapshot': 'seen'},
                    'known': {f'm.n{i}': {'v': i} for i in range(16)},
                    'judgments': {f'd.n{i}': {'rests_on': [f'm.n{j}' for j in range(16)],
                                              'seen': {f'm.n{j}': j for j in range(16)}}
                                  for i in range(8)}}
        self.save()
        packet = self.packet(*[f'd.n{i}' for i in range(8)], max_nodes=32)
        self.assertEqual(len(packet['edges']), 96)
        self.assertEqual(packet['omitted_edges'], 32)
        self.assertEqual(packet['outside_nodes'], 0)
        self.assertNotIn('name', packet['nodes'])
        self.assertNotEqual(self.cli('name').returncode, 0)

    def test_unreadable_hypothesis_and_clipped_dangerous_labels_are_visible(self):
        folder = self.root / '.kpopper/hypotheses'
        folder.mkdir(parents=True)
        (folder / 'broken.yaml').write_text('known: [')
        self.doc['sources']['s.note']['name'] = 'Long label ' * 15 + '\n```\n%%{init: evil}%%'
        self.save()
        packet = self.packet('s.note')
        self.assertIn('broken', packet['hypothesis_errors'])
        text = self.cli('s.note', '--format', 'markdown-mermaid').stdout
        self.assertIn('Unreadable hypothesis broken', text)
        self.assertEqual(text.count('```'), 2)
        self.assertNotIn('%%{init:', text)
        raw = self.cli('s.note', '--format', 'mermaid').stdout
        self.assertIn('1 labels shortened', raw)
        self.assertIn('1 unreadable hypotheses', raw)


if __name__ == '__main__':
    unittest.main()
