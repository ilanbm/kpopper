"""Combined assessment v3 keeps history and computation independent."""
import copy
import contextlib
import io
import json
from pathlib import Path
import tempfile
import tracemalloc
import unittest
from unittest import mock

try:
    import jsonschema
except ImportError:  # Installed-package smoke environments need no schema library.
    jsonschema = None

from scripts import history_adapter as HA, history_contract as HC, versions
from scripts import assessment as PublicAssessment
from scripts.pending_grounding import identity
from scripts.reasoning import assessment as V2
from scripts.reasoning import history_assessment as V3
from scripts.reasoning.contract import OperationalLimit, digest
from scripts.reasoning.snapshot import Snapshot, SnapshotError


FIELDS = {'value': 'v', 'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
HEADERS = {'meta': {'reasoning': {
    'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}}}


class UnavailableRuntime:
    def request_many(self, requests):
        raise OSError('deliberately unavailable')


def claim(subject, body, *, kind='reading', pins=None, operation=None, collection=None):
    return HC.make_object(subject=subject, kind=kind, by='writer', on='2026-09-17',
                          operation=operation or 'claim-' + subject, body=body, pins=pins,
                          authored={'collection': collection or (
                                        'decisions' if kind == 'judgment' else 'readings'),
                                    'fields': FIELDS, 'profile': 'core/v1'})


def captured(*claims, acceptance=None, dispositions=None, complete=True, acts=()):
    objects = {item['id']: item for item in (*claims, *acts)}
    states = {}
    for item in claims:
        state = states.setdefault(item['subject'], {
            'acceptance': (acceptance or {}).get(item['subject'], 'accepted'),
            'heads': [], 'open_acts': []})
        state['heads'].append(item['id'])
    for item in acts:
        states[item['subject']]['open_acts'].append(item['id'])
    states = {subject: {**state, 'heads': sorted(state['heads'])}
              for subject, state in sorted(states.items())}
    baseline = {'version': 1, 'record_id': 'record', 'authority_generation': 1,
                'committed_set_digest': identity('commits'),
                'heads': {subject: state['heads'] for subject, state in states.items()},
                'open_acts': {subject: state['open_acts'] for subject, state in states.items()
                              if state['open_acts']}}
    projection = {
        'projection_version': 1,
        'authority': HC.authority(record_id='record', authority='history', generation=1),
        'baseline': baseline,
        'identity_schemes': [HC.ID_SCHEME],
        'rules': {}, 'rules_digest': identity({}), 'closure_digest': identity(objects),
        'coverage': {'scope': 'all', 'subjects': sorted(states), 'complete': complete},
        'subjects': states,
        'pins': {item['id']: {'subject': item['subject'], 'version': item['id'],
                             'status': 'recorded', 'object': item}
                 for item in claims},
        'dispositions': dispositions or {
            subject: {'marks': {}, 'proposals': [], 'contested_claims': [],
                      'reviews': [], 'implied': []}
            for subject in states},
        'integrity': {'complete': complete, 'findings': []},
    }
    return HA.capture_history(objects, projection, document=HEADERS)


def v2(snapshot, selection=None, *, policy='focused-review/v1', runtime=None):
    return V2.assess(snapshot, selection, policy=policy, runtime=runtime)


def query_result_value(result, counts):
    fields = {key: {'type': 'number', 'numerator': str(value), 'denominator': '1'}
              for key, value in counts.items()}
    fields['result'] = copy.deepcopy(result)
    return {'type': 'record', 'fields': fields}


def promote_query_result(result, snapshot_id, counts, diagnostics=None):
    raw_result = (copy.deepcopy(result.get('value'))
                  if isinstance(result.get('value'), dict)
                  else {'type': 'number', 'numerator': '1', 'denominator': '1'})
    node = copy.deepcopy(result['potential_dependencies'][0])
    scope = {'kind': 'scope', 'scope_id': 'scope.items',
             'definition_digest': '1' * 64, 'membership_digest': '2' * 64,
             'projected_inputs_digest': '3' * 64}
    resources = {'version': 'resources/v4', 'steps': 1_000_000,
                 'depth': 128, 'digits': 256, 'value_nodes': 10_000,
                 'value_depth': 128, 'value_bytes': 16_777_216,
                 'candidates': 10_000, 'field_reads': 100_000}
    scope_basis = {'version': 1, 'recipe': 'scope-inputs/v2', 'profile': 'core/v1',
                   'modules': ['arithmetic/v1'], 'witness': copy.deepcopy(scope),
                   'members': ['p.input'], 'fields': ['amount'], 'dependencies': [],
                   'historical_detail': 'fingerprints_only', 'as_of': None}
    scope_basis['digest'] = digest(scope_basis)
    preflight = {'candidates': 1, 'field_reads': 1,
                 'preflight_steps': 1, 'step_upper_bound': 1}
    operation = {'query': {'version': 1, 'scope': 'scope.items', 'op': 'count',
                           'where': {'column': 'amount'}}}
    basis = {'version': 1, 'recipe': 'query-inputs/v1', 'profile': 'core/v1',
             'modules': ['arithmetic/v1', 'query/v1'], 'as_of': None,
             'operation': operation, 'scope': scope_basis,
             'dependencies': [node, scope], 'resources': resources,
             'preflight_cost': preflight, 'query_counts': counts}
    basis['digest'] = digest(basis)
    result.update(
        schema_version=2, modules=['arithmetic/v1', 'query/v1'],
        resource_profile=resources, status='ok',
        potential_dependencies=[node, scope], executed_reads=copy.deepcopy([node, scope]),
        potential_ids=['p.input', 'scope.items'], diagnostics=diagnostics or [],
        value=query_result_value(raw_result, counts), query_counts=copy.deepcopy(counts),
        basis=basis, computation_id=digest({'snapshot_id': snapshot_id, 'basis': basis,
                                            'resources': resources}),
        cost={'steps': 1, 'preflight_steps': 1, 'node_evaluations': {},
              'candidates': 1, 'field_reads': 1, 'evaluated_field_reads': 1})
    return node, scope


class HistoryAssessmentTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        root = Path(__file__).resolve().parents[1]
        path = root / 'scripts/reasoning/history_assessment.schema.json'
        cls.schema = json.loads(path.read_text())
        cls.assessment_schema = json.loads(
            (root / 'scripts/reasoning/assessment.schema.json').read_text())
        if jsonschema is not None:
            jsonschema.Draft202012Validator.check_schema(cls.schema)
            jsonschema.Draft202012Validator.check_schema(cls.assessment_schema)

    def test_rejects_forged_and_attention_only_v2(self):
        snapshot = Snapshot.from_data({**HEADERS, 'readings': {'p.input': {'v': 1}}})
        report = v2(snapshot)
        forged = copy.deepcopy(report)
        forged['nodes']['p.input']['body']['v'] = 2
        with self.assertRaisesRegex(ValueError, 'assessment_revision'):
            V3.from_v2(snapshot, forged)
        attention_only = copy.deepcopy(report)
        attention_only['attention'] = {'p.input': report['nodes']['p.input']['attention']}
        del attention_only['nodes']
        with self.assertRaisesRegex(ValueError, 'canonical v2 nodes report'):
            V3.from_v2(snapshot, attention_only)
        wrong_selection = copy.deepcopy(report)
        wrong_selection['selection'] = []
        wrong_selection['assessment_revision'] = digest({
            key: value for key, value in wrong_selection.items() if key != 'assessment_revision'})
        with self.assertRaisesRegex(ValueError, 'selection and node keys'):
            V3.from_v2(snapshot, wrong_selection)
        invalid_schema = copy.deepcopy(report)
        invalid_schema['nodes']['p.input']['computation']['forged'] = True
        invalid_schema['assessment_revision'] = digest({
            key: value for key, value in invalid_schema.items() if key != 'assessment_revision'})
        with self.assertRaisesRegex(ValueError, 'invalid computation result'):
            V3.from_v2(snapshot, invalid_schema)

    def test_result_v2_accepts_exact_scope_witnesses_and_rejects_forgery(self):
        snapshot = Snapshot.from_data({**HEADERS, 'readings': {'p.input': {'v': 1}}})
        report = v2(snapshot)
        result = report['nodes']['p.input']['computation']
        counts = {'input_count': 1, 'definite_match_count': 1,
                  'unknown_membership_count': 0, 'unknown_value_count': 0, 'error_count': 0}
        promote_query_result(result, snapshot.snapshot_id, counts)
        report['assessment_revision'] = digest({
            key: value for key, value in report.items() if key != 'assessment_revision'})
        V3.from_v2(snapshot, report)
        if jsonschema is not None:
            jsonschema.validate(report, self.assessment_schema)

        mutations = {
            'order': lambda r: r['potential_dependencies'].reverse(),
            'duplicate': lambda r: r['potential_dependencies'].append(
                copy.deepcopy(r['potential_dependencies'][-1])),
            'actual-outside-potential': lambda r: r['executed_reads'][0].update(
                projected_inputs_digest='4' * 64),
            'scope-extra-key': lambda r: r['potential_dependencies'][-1].update(forged=True),
            'scope-missing-key': lambda r: r['potential_dependencies'][-1].pop(
                'membership_digest'),
            'noncanonical-ids': lambda r: r.update(potential_ids=['scope.items', 'p.input']),
            'witness-id-mismatch': lambda r: r.update(
                potential_ids=['p.input', 'scope.other']),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name):
                forged = copy.deepcopy(report)
                mutate(forged['nodes']['p.input']['computation'])
                forged['assessment_revision'] = digest({
                    key: value for key, value in forged.items() if key != 'assessment_revision'})
                if jsonschema is not None and name in ('scope-extra-key', 'scope-missing-key'):
                    with self.assertRaises(jsonschema.ValidationError):
                        jsonschema.validate(forged, self.assessment_schema)
                with self.assertRaisesRegex(ValueError, 'witness|dependencies|reads|potential ids'):
                    V3.from_v2(snapshot, forged)

    def test_result_v2_diagnostics_and_counts_are_closed_and_canonical(self):
        snapshot = Snapshot.from_data({**HEADERS, 'readings': {'p.input': {'v': 1}}})
        report = v2(snapshot)
        result = report['nodes']['p.input']['computation']
        counts = {'input_count': 1, 'definite_match_count': 1,
                  'unknown_membership_count': 0, 'unknown_value_count': 0,
                  'error_count': 0}
        promote_query_result(result, snapshot.snapshot_id, counts, diagnostics=[{
            'code': 'missing_column', 'related_ids': ['p.input', 'scope.items'],
            'locations': [{'candidate': 'p.input', 'column': 'amount', 'phase': 'value'}],
        }])
        report['assessment_revision'] = digest({
            key: value for key, value in report.items() if key != 'assessment_revision'})
        V3.from_v2(snapshot, report)
        if jsonschema is not None:
            jsonschema.validate(report, self.assessment_schema)
        for mutation in ('location-extra', 'location-order', 'negative-count', 'missing-count',
                         'null-counts', 'count-mismatch', 'candidate-mismatch',
                         'basis-count-mismatch', 'basis-dependency-mismatch',
                         'computation-id-mismatch'):
            forged = copy.deepcopy(report)
            computation = forged['nodes']['p.input']['computation']
            if mutation == 'location-extra':
                computation['diagnostics'][0]['locations'][0]['extra'] = True
            elif mutation == 'location-order':
                computation['diagnostics'][0]['locations'] = [
                    {'candidate': 'z', 'column': 'v', 'phase': 'value'},
                    {'candidate': 'a', 'column': 'v', 'phase': 'value'}]
            elif mutation == 'negative-count':
                computation['query_counts']['error_count'] = -1
            elif mutation == 'missing-count':
                computation['query_counts'].pop('error_count')
            elif mutation == 'null-counts':
                computation['query_counts'] = None
            elif mutation == 'count-mismatch':
                computation['value']['fields']['input_count']['numerator'] = '2'
            elif mutation == 'candidate-mismatch':
                computation['cost']['candidates'] = 2
            elif mutation == 'basis-count-mismatch':
                computation['basis']['query_counts']['input_count'] = 2
            elif mutation == 'basis-dependency-mismatch':
                computation['basis']['dependencies'] = computation['basis']['dependencies'][:-1]
            else:
                computation['computation_id'] = '0' * 64
            forged['assessment_revision'] = digest({
                key: value for key, value in forged.items() if key != 'assessment_revision'})
            if jsonschema is not None and mutation in (
                    'location-extra', 'negative-count', 'missing-count', 'null-counts'):
                with self.assertRaises(jsonschema.ValidationError):
                    jsonschema.validate(forged, self.assessment_schema)
            with self.subTest(mutation=mutation), self.assertRaisesRegex(
                    ValueError, 'diagnostic|location|query counts|query result|successful query|finalized|preflight|basis|identity'):
                V3.from_v2(snapshot, forged)

    def test_accepted_unknown_and_error_remain_independent(self):
        cases = [
            ({'rests_on': [], 'wrong_if': 'Supplier changes terms'}, None, 'unknown'),
            ({'rests_on': [], 'wrong_if': {'op': 'eq', 'args': [
                {'num': '1'}, {'num': '1'}]}}, UnavailableRuntime(), 'error'),
        ]
        for body, runtime, expected in cases:
            with self.subTest(expected=expected):
                head = claim('d.ready', body, kind='judgment', operation='decision-' + expected)
                snapshot = captured(head).snapshot(as_of='2026-09-17')
                result = V3.from_v2(snapshot, v2(snapshot, runtime=runtime))
                node = result['nodes']['d.ready']
                self.assertEqual(node['acceptance']['status'], 'accepted')
                self.assertEqual(node['state']['falsifier']['status'], expected)
                if jsonschema is not None:
                    jsonschema.validate(result, self.schema)

    def test_fired_falsifier_attention_is_canonical_v2_and_v3(self):
        decision = claim('d.ready', {'rests_on': [], 'wrong_if': {'bool': True}},
                         kind='judgment', operation='fired')
        snapshot = captured(decision).snapshot()
        base = v2(snapshot)
        reason = base['nodes']['d.ready']['attention'][0]['reasons'][0]
        self.assertEqual(reason, {'code': 'falsifier_holds', 'related_ids': []})
        result = V3.from_v2(snapshot, base)
        self.assertEqual(result['nodes']['d.ready']['state']['falsifier']['status'], 'holds')
        if jsonschema is not None:
            jsonschema.validate(result, self.schema)

    def test_nonaccepted_subjects_never_enter_computational_nodes(self):
        accepted = claim('p.current', {'v': 1}, operation='current')
        proposed = claim('p.proposal', {'v': 2}, operation='proposal')
        snapshot = captured(accepted, proposed, acceptance={'p.proposal': 'proposed'}).snapshot()
        result = V3.from_v2(snapshot, v2(snapshot))
        if jsonschema is not None:
            jsonschema.validate(result, self.schema)
        self.assertEqual(result['history']['authority_status'], 'active')
        self.assertEqual(set(result['nodes']), {'p.current'})
        self.assertEqual(set(result['history_subjects']), {'p.current', 'p.proposal'})
        self.assertEqual(result['history_subjects']['p.proposal']['acceptance'], 'proposed')

    def test_query_scope_requires_complete_all_history_membership(self):
        item = claim('item.a', {'v': 1}, collection='items')
        scope = claim('scope.items', {'collection_scope': {
            'collection': 'items', 'fields': ['v']}}, collection='scopes')
        snapshot = captured(item, scope).snapshot()
        rows = snapshot.capture_query_scope('scope.items').query_rows()
        self.assertEqual(rows, [{'id': 'item.a', 'fields': {'v': {
            'status': 'known', 'value': {
                'type': 'number', 'numerator': '1', 'denominator': '1'}}}}])

        # These mutations model independently validated selected, missing and
        # corrupt history captures at the narrow query-preparation boundary.
        # Snapshot replay validates their full outer shape separately.
        mutations = {
            'selected': lambda h: h['coverage'].update(scope='selected'),
            'incomplete': lambda h: h['coverage'].update(complete=False),
            'missing-member': lambda h: h['coverage']['subjects'].remove('item.a'),
            'corrupt': lambda h: h['integrity'].update(
                complete=False, findings=[{'code': 'pin_corrupt', 'subject': 'item.a',
                                           'object_id': '', 'detail': 'fixture'}]),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name):
                candidate = captured(item, scope).snapshot()
                mutate(candidate._Snapshot__data['context']['history'])
                with self.assertRaisesRegex(SnapshotError, 'incomplete_history_scope'):
                    candidate.capture_query_scope('scope.items')

    def test_seen_pin_and_current_are_separate_evidence(self):
        source = claim('p.input', {'v': 3}, operation='input')
        decision = claim('d.ready', {'rests_on': {'p.input': source['id']},
                                     'seen': {'p.input': 2}}, kind='judgment',
                         pins={'p.input': source['id']}, operation='decision')
        snapshot = captured(source, decision).snapshot()
        result = V3.from_v2(snapshot, v2(snapshot, ['d.ready']))
        if jsonschema is not None:
            jsonschema.validate(result, self.schema)
        node = result['nodes']['d.ready']
        basis = node['state']['basis']['dependencies']['p.input']
        support = node['history']['recorded_support']['p.input']
        self.assertEqual(basis['at_review']['value'], 2)
        self.assertEqual(basis['current']['value']['numerator'], '3')
        self.assertEqual(support['version'], source['id'])
        self.assertEqual(support['status'], 'recorded')
        self.assertEqual(support['value']['numerator'], '3')
        self.assertEqual(node['history']['pin_review_evidence'], [])

    def test_review_pin_evidence_is_distinct_from_head_support(self):
        source = claim('p.input', {'v': 3}, operation='input')
        decision = claim('d.ready', {'rests_on': {'p.input': source['id']}},
                         kind='judgment', pins={'p.input': source['id']}, operation='decision')
        review = HC.make_object(subject='d.ready', kind='act', by='reviewer', on='2026-09-17',
                                operation='review', body={'act': 'review', 'of': decision['id'],
                                'over': [], 'because': 'checked',
                                'read': {'p.input': source['id']}})
        dispositions = {
            'p.input': {'marks': {}, 'proposals': [], 'contested_claims': [],
                        'reviews': [], 'implied': []},
            'd.ready': {'marks': {}, 'proposals': [], 'contested_claims': [],
                        'reviews': [review], 'implied': []},
        }
        snapshot = captured(source, decision, acts=(review,), dispositions=dispositions).snapshot()
        result = V3.from_v2(snapshot, v2(snapshot, ['d.ready']))
        node = result['nodes']['d.ready']
        review_evidence = node['history']['pin_review_evidence'][0]
        self.assertEqual(review_evidence['review_id'], review['id'])
        self.assertEqual(review_evidence['read']['p.input']['version'], source['id'])
        self.assertEqual(node['assurance']['recorded_evidence_kinds'],
                         ['review_pin', 'version_pin'])
        if jsonschema is not None:
            jsonschema.validate(result, self.schema)

    def test_no_history_is_valid_but_makes_no_acceptance_claim(self):
        snapshot = Snapshot.from_data({**HEADERS, 'readings': {'p.input': {'v': 1}}})
        result = V3.from_v2(snapshot, v2(snapshot))
        self.assertEqual(result['history']['authority_status'], 'not_active')
        self.assertEqual(result['nodes']['p.input']['acceptance']['status'], 'not_applicable')
        self.assertEqual(result['history_subjects'], {})
        if jsonschema is not None:
            jsonschema.validate(result, self.schema)

    def test_support_reducer_propagates_chain_reservations_and_bounds_cycles(self):
        graph = {
            'd.root@r': {'subject': 'd.root', 'version': 'r', 'state': 'accepted',
                         'dependencies': [{'subject': 'd.child', 'version': 'c'}]},
            'd.child@c': {'subject': 'd.child', 'version': 'c', 'state': 'corrected',
                          'dependencies': [{'subject': 'd.root', 'version': 'r'}]},
        }
        reduced = V3._reduce_support_graph(['d.root@r'], graph, {
            'd.root': 'fired', 'd.child': 'unknown'}, max_visits=10)
        states = {item['state'] for item in reduced['reservations']}
        self.assertIn('corrected', states)
        self.assertIn('unknown', states)
        self.assertIn('unavailable', states)
        cycle = next(item for item in reduced['reservations'] if item['code'] == 'support_cycle')
        self.assertEqual(cycle['path'], ['d.root@r', 'd.child@c', 'd.root@r'])
        self.assertLessEqual(reduced['visited_count'], 2)
        with self.assertRaisesRegex(OperationalLimit, 'support_limit'):
            V3._reduce_support_graph(['d.root@r'], graph, {}, max_visits=1)

    def test_support_reducer_preserves_every_reserved_state(self):
        reserved = ['proposed', 'contested', 'corrected', 'refuted', 'retired',
                    'unreviewed', 'moved', 'unavailable']
        graph = {'p.' + state + '@v': {'subject': 'p.' + state, 'version': 'v',
                 'state': state, 'dependencies': []} for state in reserved}
        roots = sorted(graph)
        outcomes = {'p.accepted': 'fired', 'p.unknown': 'unknown'}
        graph.update({
            'p.accepted@v': {'subject': 'p.accepted', 'version': 'v',
                             'state': 'accepted', 'dependencies': []},
            'p.unknown@v': {'subject': 'p.unknown', 'version': 'v',
                            'state': 'accepted', 'dependencies': []},
        })
        roots.extend(['p.accepted@v', 'p.unknown@v'])
        result = V3._reduce_support_graph(roots, graph, outcomes)
        self.assertEqual({item['state'] for item in result['reservations']},
                         set(reserved) | {'fired', 'unknown'})

    def test_long_support_chain_uses_bounded_linear_traversal_memory(self):
        length = 3000
        graph = {}
        for index in range(length):
            key = 's' + str(index) + '@v'
            graph[key] = {'subject': 's' + str(index), 'version': 'v',
                          'state': 'accepted', 'dependencies': (
                              [{'subject': 's' + str(index + 1), 'version': 'v'}]
                              if index + 1 < length else [])}
        tracemalloc.start()
        result = V3._reduce_support_graph(['s0@v'], graph, {})
        _, peak = tracemalloc.get_traced_memory()
        tracemalloc.stop()
        self.assertEqual(result['visited_count'], length)
        self.assertEqual(result['status'], 'clear')
        self.assertLess(peak, 12 * 1024 * 1024)

    def test_support_budget_is_shared_across_subject_reductions(self):
        graph = {
            'a@v': {'subject': 'a', 'version': 'v', 'state': 'accepted',
                    'dependencies': [{'subject': 'b', 'version': 'v'}]},
            'b@v': {'subject': 'b', 'version': 'v', 'state': 'accepted',
                    'dependencies': []},
        }
        budget = {'remaining': 2, 'path_items': 20}
        self.assertEqual(V3._reduce_support_graph(
            ['a@v'], graph, {}, shared_budget=budget)['visited_count'], 2)
        with self.assertRaisesRegex(OperationalLimit, 'support_limit'):
            V3._reduce_support_graph(['b@v'], graph, {}, shared_budget=budget)

    def test_current_moved_and_fired_support_outcomes_remain_independent(self):
        nodes = {
            'd.parent': {'state': {
                'falsifier': {'status': 'does_not_hold'},
                'basis': {'dependencies': {'p.input': {
                    'comparison': 'changed', 'basis_comparison': 'changed',
                    'current': {'status': 'ok'}}}},
            }, 'computation': None},
            'p.input': {'state': {
                'falsifier': {'status': 'holds'},
                'basis': {'dependencies': {}},
            }, 'computation': None},
        }
        outcomes = V3._outcomes(nodes)
        self.assertEqual(outcomes['p.input'], ['fired', 'moved'])
        graph = {'p.input@v': {'subject': 'p.input', 'version': 'v',
                               'state': 'accepted', 'dependencies': []}}
        reduced = V3._reduce_support_graph(['p.input@v'], graph, outcomes)
        self.assertEqual({item['state'] for item in reduced['reservations']},
                         {'fired', 'moved'})

    def test_public_validator_rejects_forged_v3_and_cycle_paths_validate(self):
        snapshot = Snapshot.from_data({**HEADERS, 'readings': {'p.input': {'v': 1}}})
        result = V3.from_v2(snapshot, v2(snapshot))
        self.assertEqual(V3.validate(result), result)
        forged = copy.deepcopy(result)
        forged['nodes']['p.input']['acceptance']['status'] = 'accepted'
        with self.assertRaisesRegex(ValueError, 'inactive history'):
            V3.validate(forged)
        if jsonschema is not None:
            cycle = copy.deepcopy(result)
            cycle['nodes']['p.input']['support'] = {
                'reducer': V3.SUPPORT_REDUCER, 'status': 'reserved',
                'visited': ['p.input@v'], 'visited_count': 1,
                'reservations': [{'code': 'support_cycle', 'state': 'unavailable',
                                  'subject': 'p.input', 'version': 'v',
                                  'path': ['p.input@v', 'p.other@v', 'p.input@v']}]}
            cycle['findings_revision'] = digest(V3._findings_preimage(cycle))
            cycle['envelope_revision'] = digest({
                key: value for key, value in cycle.items() if key != 'envelope_revision'})
            jsonschema.validate(cycle, self.schema)

    def test_public_validator_rejects_invalid_active_history_identity_and_source_state(self):
        source = claim('p.input', {'v': 1}, operation='input')
        snapshot = captured(source).snapshot()
        result = V3.from_v2(snapshot, v2(snapshot))
        for mutation in ('identity', 'source_state', 'generation', 'integrity'):
            forged = copy.deepcopy(result)
            if mutation == 'identity':
                forged['history']['authority']['identity'] = 'not-a-digest'
            elif mutation == 'source_state':
                forged['history_subjects']['p.input']['source_state'] = 'invented'
            elif mutation == 'generation':
                forged['history']['authority']['generation'] = -1
            else:
                forged['history']['integrity']['findings'] = [1]
            forged['findings_revision'] = digest(V3._findings_preimage(forged))
            forged['envelope_revision'] = digest({
                key: value for key, value in forged.items() if key != 'envelope_revision'})
            with self.subTest(mutation=mutation), self.assertRaisesRegex(
                    ValueError, 'history|source state'):
                V3.validate(forged)

    def test_findings_ignore_attention_and_display_but_envelope_does_not(self):
        snapshot = Snapshot.from_data({**HEADERS, 'readings': {'p.input': {'v': 1}}})
        focused = V3.from_v2(snapshot, v2(snapshot, policy='focused-review/v1'),
                             display_selection=['p.input'])
        falsifiers = V3.from_v2(snapshot, v2(snapshot, policy='falsifiers-only/v1'),
                                display_selection=[])
        if jsonschema is not None:
            jsonschema.validate(focused, self.schema)
            jsonschema.validate(falsifiers, self.schema)
        self.assertEqual(focused['findings_revision'], falsifiers['findings_revision'])
        self.assertNotEqual(focused['envelope_revision'], falsifiers['envelope_revision'])

    def test_adapter_is_source_free_and_does_not_evaluate(self):
        source = claim('p.input', {'v': 1}, operation='input')
        snapshot = captured(source).snapshot()
        report = v2(snapshot)
        with mock.patch('builtins.open', side_effect=AssertionError('filesystem')), \
                mock.patch.object(Path, 'read_bytes', side_effect=AssertionError('filesystem')), \
                mock.patch.object(V2, 'assess', side_effect=AssertionError('evaluator')), \
                mock.patch.object(versions, '_evaluate', side_effect=AssertionError('fallback')):
            result = V3.from_v2(snapshot, report)
        if jsonschema is not None:
            jsonschema.validate(result, self.schema)
        self.assertEqual(result['snapshot_id'], snapshot.snapshot_id)

    def test_convenience_entry_computes_v2_exactly_once_and_adapter_is_immutable(self):
        snapshot = Snapshot.from_data({**HEADERS, 'readings': {'p.input': {'v': 1}}})
        report = v2(snapshot, runtime=UnavailableRuntime())
        before = copy.deepcopy(report)
        adapted = V3.from_v2(snapshot, report)
        if jsonschema is not None:
            jsonschema.validate(adapted, self.schema)
        self.assertEqual(report, before)
        with mock.patch.object(V2, 'assess', wraps=V2.assess) as assessed:
            convenient = V3.assess(snapshot, runtime=UnavailableRuntime())
        if jsonschema is not None:
            jsonschema.validate(convenient, self.schema)
        self.assertEqual(assessed.call_count, 1)

    def test_whole_envelope_refuses_overflow_without_partial_output(self):
        snapshot = Snapshot.from_data({**HEADERS, 'readings': {'p.input': {'v': 1}}})
        report = v2(snapshot)
        report['operational_limits']['output_bytes'] = 3000
        report['assessment_revision'] = digest({key: value for key, value in report.items()
                                                if key != 'assessment_revision'})
        with self.assertRaisesRegex(OperationalLimit, 'output_limit'):
            V3.from_v2(snapshot, report)

    def test_public_cli_exposes_explicit_v3_without_changing_v2_default(self):
        document = '''meta:\n  reasoning:\n    version: 2\n    profile: core/v1\n    requires: [arithmetic/v1]\ndecisions:\n  d.ready:\n    rests_on: []\n    wrong_if: Supplier changes terms\n'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'GROUNDING.yaml'
            path.write_text(document, encoding='utf-8')
            v2_report = PublicAssessment.load([str(path)], profile='core/v1',
                                              selection=['d.ready'])
            v3_report = PublicAssessment.load([str(path)], profile='core/v1',
                                              selection=['d.ready'], history=True,
                                              display_selection=['d.ready'])
            self.assertEqual(v2_report['schema_version'], 2)
            self.assertEqual(v3_report['schema_version'], 3)
            self.assertEqual(v3_report['history']['authority_status'], 'not_active')
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                PublicAssessment.main(['d.ready', '--profile', 'core/v1', '--history',
                                       '--record', str(path)])
            emitted = json.loads(output.getvalue())
            self.assertEqual(emitted['envelope_revision'], v3_report['envelope_revision'])
            with self.assertRaises(SystemExit):
                PublicAssessment.main(['d.ready', '--profile', 'core/v1', '--history',
                                       '--attention-only', '--record', str(path)])


if __name__ == '__main__':
    unittest.main()
