"""Pure history-aware adaptation of one canonical core assessment.

The adapter never evaluates an expression and never opens a source.  Its only
inputs are an immutable ``Snapshot`` and the complete schema-v2 nodes report
computed for that snapshot.  ``assess`` is the convenience boundary that
computes v2 once and immediately adapts it.
"""
import copy

from .. import history_adapter as history_adapter
from .. import history_contract as history_contract
from . import assessment as base_assessment
from .contract import (OPERATIONAL_LIMITS, OperationalLimit, OutputBudget, PROFILE,
                       digest, operational_bounds, validate_value)
from .snapshot import Snapshot


SCHEMA_VERSION = 3
SUPPORT_REDUCER = 'necessary-support/v1'
MAX_SUPPORT_VISITS = 100_000
MAX_SUPPORT_PATH_ITEMS = 200_000
MAX_TEMPORAL_REPLAYS = 64
ACCEPTANCE = frozenset(('accepted', 'proposed', 'contested', 'refuted', 'corrected',
                        'retired', 'unreviewed', 'unavailable'))
SUPPORT_STATES = ACCEPTANCE | frozenset(('moved', 'fired', 'unknown'))
POLICIES = frozenset(('focused-review/v1', 'falsifiers-only/v1'))
HEX = frozenset('0123456789abcdef')


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def _same(left, right):
    return digest(left) == digest(right)


def _is_digest(value):
    return isinstance(value, str) and len(value) == 64 and set(value) <= HEX


def _validate_ids(value, message):
    _require(isinstance(value, list) and all(isinstance(item, str) for item in value)
             and len(value) == len(set(value)), message)


def _validate_canonical_ids(value, message):
    _require(isinstance(value, list) and all(isinstance(item, str) for item in value)
             and value == sorted(set(value)), message)


def _witness_key(value):
    """Canonical closed witness identity: nodes precede scopes, then exact fields."""
    _require(isinstance(value, dict), 'v2 schema: invalid dependency witness')
    if value.get('kind') == 'node':
        _require(set(value) == {'kind', 'id', 'fingerprint'}
                 and isinstance(value['id'], str) and value['id']
                 and _is_digest(value['fingerprint']),
                 'v2 schema: invalid node witness')
        return (0, value['id'], value['fingerprint'])
    if value.get('kind') == 'scope':
        _require(set(value) == {'kind', 'scope_id', 'definition_digest',
                                'membership_digest', 'projected_inputs_digest'}
                 and isinstance(value['scope_id'], str) and value['scope_id']
                 and all(_is_digest(value[key]) for key in (
                     'definition_digest', 'membership_digest', 'projected_inputs_digest')),
                 'v2 schema: invalid scope witness')
        return (1, value['scope_id'], value['definition_digest'],
                value['membership_digest'], value['projected_inputs_digest'])
    raise ValueError('v2 schema: invalid dependency witness kind')


def _validate_witnesses(value, message):
    _require(isinstance(value, list), message)
    keys = [_witness_key(item) for item in value]
    _require(keys == sorted(set(keys)), message)
    return keys


def _validate_locations(value):
    _require(isinstance(value, list), 'v2 schema: invalid diagnostic locations')
    keys = []
    for item in value:
        _require(isinstance(item, dict) and set(item) == {'candidate', 'column', 'phase'}
                 and all(isinstance(item[key], str) for key in ('candidate', 'column', 'phase'))
                 and item['candidate'] and item['column']
                 and item['phase'] in ('where', 'value', 'preflight', 'aggregate'),
                 'v2 schema: invalid diagnostic location')
        keys.append((item['candidate'], item['column'], item['phase']))
    _require(keys == sorted(set(keys)), 'v2 schema: noncanonical diagnostic locations')
    return tuple(keys)


def _validate_attention(value):
    _require(isinstance(value, list), 'v2 schema: attention must be an array')
    for action in value:
        _require(isinstance(action, dict) and set(action) == {'action', 'reasons'}
                 and isinstance(action['action'], str) and isinstance(action['reasons'], list),
                 'v2 schema: invalid attention action')
        for reason in action['reasons']:
            _require(isinstance(reason, dict) and set(reason) == {'code', 'related_ids'}
                     and isinstance(reason['code'], str)
                     and isinstance(reason['related_ids'], list)
                     and all(isinstance(item, str) for item in reason['related_ids']),
                     'v2 schema: invalid attention reason')


def _validate_bounds(value, message='v2 operational limits mismatch'):
    _require(isinstance(value, dict) and set(value) == set(OPERATIONAL_LIMITS), message)
    _require(operational_bounds(value) == value, message)


def _validate_result(result, snapshot_id):
    common = {'schema_version', 'profile', 'modules', 'snapshot_id', 'computation_id',
                'implementation', 'resource_profile', 'status', 'value', 'diagnostics',
                'executed_reads', 'potential_dependencies', 'potential_ids', 'basis',
                'assurance', 'cost', 'operational_limits'}
    _require(isinstance(result, dict) and result.get('schema_version') in (1, 2),
             'v2 schema: invalid computation result')
    required = common | ({'query_counts'} if result['schema_version'] == 2 else set())
    _require(required <= set(result)
             and not set(result) - required - {'interpretation'},
             'v2 schema: invalid computation result')
    _require(result['profile'] == PROFILE
             and result['snapshot_id'] == snapshot_id
             and isinstance(result['modules'], list)
             and result['modules'] == sorted(set(result['modules']))
             and all(isinstance(item, str) for item in result['modules'])
             and (result['computation_id'] is None
                  or isinstance(result['computation_id'], str))
             and (result['implementation'] is None
                  or isinstance(result['implementation'], dict)),
             'v2 schema: invalid computation identity')
    resources = result['resource_profile']
    resource_common = {'version', 'steps', 'depth', 'digits'}
    extended = resource_common | {'value_nodes', 'value_depth', 'value_bytes'}
    query_resources = extended | {'candidates', 'field_reads'}
    valid_resources = isinstance(resources, dict) and (
        resources.get('version') == 'resources/v2' and set(resources) == resource_common
        or resources.get('version') == 'resources/v3' and set(resources) == extended
        or resources.get('version') == 'resources/v4' and set(resources) == query_resources)
    _require(valid_resources and all(type(resources[key]) is int and resources[key] >= 0
                                     for key in ('steps', 'depth', 'digits')),
             'v2 schema: invalid resource profile')
    if resources['version'] in ('resources/v3', 'resources/v4'):
        _require(0 < resources['value_nodes'] <= 10_000
                 and 0 < resources['value_depth'] <= 128
                 and 0 < resources['value_bytes'] <= 16_777_216,
                 'v2 schema: invalid resource profile')
    if resources['version'] == 'resources/v4':
        _require(0 < resources['candidates'] <= 10_000
                 and 0 < resources['field_reads'] <= 100_000,
                 'v2 schema: invalid resource profile')
    _require((result['schema_version'] == 2) == (resources['version'] == 'resources/v4'),
             'v2 schema: result/resource version mismatch')
    status = result['status']
    _require(status in ('ok', 'unknown', 'error', 'limit',
                        'unsupported_capability', 'operational_error'),
             'v2 schema: invalid computation status')
    if status == 'ok':
        validate_value(result['value'])
    else:
        _require(result['value'] is None, 'v2 schema: non-ok computation has a value')
    diagnostics = result['diagnostics']
    _require(isinstance(diagnostics, list), 'v2 schema: invalid diagnostics')
    diagnostic_keys = []
    for item in diagnostics:
        expected = {'code', 'related_ids'} | ({'locations'} if result['schema_version'] == 2 else set())
        _require(isinstance(item, dict) and set(item) == expected
                 and isinstance(item['code'], str) and isinstance(item['related_ids'], list)
                 and item['related_ids'] == sorted(set(item['related_ids']))
                 and all(isinstance(value, str) for value in item['related_ids']),
                 'v2 schema: invalid diagnostic')
        locations = _validate_locations(item['locations']) if result['schema_version'] == 2 else ()
        diagnostic_keys.append((item['code'], tuple(item['related_ids']), locations))
    _require(diagnostic_keys == sorted(set(diagnostic_keys)),
             'v2 schema: noncanonical diagnostics')
    executed = _validate_witnesses(result['executed_reads'], 'v2 schema: invalid executed reads')
    potential = _validate_witnesses(result['potential_dependencies'],
                                    'v2 schema: invalid potential dependencies')
    _require(set(executed) <= set(potential),
             'v2 schema: executed reads exceed potential dependencies')
    _validate_canonical_ids(result['potential_ids'], 'v2 schema: invalid potential ids')
    if result['schema_version'] == 2:
        witness_ids = sorted(item['id'] if item['kind'] == 'node' else item['scope_id']
                             for item in result['potential_dependencies'])
        _require(result['potential_ids'] == witness_ids,
                 'v2 schema: potential ids disagree with dependency witnesses')
    _require(result['basis'] is None or isinstance(result['basis'], dict),
             'v2 schema: invalid computation basis')
    assurance = result['assurance']
    _require(isinstance(assurance, dict)
             and {'kind', 'formal_scope'} <= set(assurance)
             and not set(assurance) - {'kind', 'formal_scope', 'implementation'}
             and assurance['kind'] == 'computed'
             and isinstance(assurance['formal_scope'], list)
             and all(isinstance(item, str) for item in assurance['formal_scope'])
             and ('implementation' not in assurance
                  or isinstance(assurance['implementation'], str)),
             'v2 schema: invalid assurance')
    cost = result['cost']
    cost_fields = {'steps', 'preflight_steps', 'node_evaluations'} | (
        {'candidates', 'field_reads', 'evaluated_field_reads'}
        if result['schema_version'] == 2 else set())
    _require(isinstance(cost, dict)
             and set(cost) == cost_fields
             and type(cost['steps']) is int and cost['steps'] >= 0
             and type(cost['preflight_steps']) is int and cost['preflight_steps'] >= 0
             and isinstance(cost['node_evaluations'], dict)
             and all(isinstance(key, str) and type(value) is int and value >= 0
                     for key, value in cost['node_evaluations'].items()),
             'v2 schema: invalid computation cost')
    if result['schema_version'] == 2:
        _require(all(type(cost[key]) is int and cost[key] >= 0 for key in (
            'candidates', 'field_reads', 'evaluated_field_reads')),
            'v2 schema: invalid computation cost')
        counts = result['query_counts']
        if counts is not None:
            count_fields = {'input_count', 'definite_match_count',
                            'unknown_membership_count', 'unknown_value_count', 'error_count'}
            _require(isinstance(counts, dict) and set(counts) == count_fields
                     and all(type(value) is int and value >= 0 for value in counts.values()),
                     'v2 schema: invalid query counts')
            _require(counts['definite_match_count'] <= counts['input_count']
                     and counts['unknown_membership_count'] <= counts['input_count']
                     and counts['unknown_value_count'] <= counts['definite_match_count']
                     and counts['error_count'] <= counts['input_count']
                     and cost['candidates'] == counts['input_count'],
                     'v2 schema: inconsistent query counts')
        _require(counts is None or status in ('ok', 'unknown', 'error'),
                 'v2 schema: preflight status cannot claim query counts')
        _require((counts is None) == (result['basis'] is None),
                 'v2 schema: query counts and finalized basis disagree')
        _require(counts is not None or result['computation_id'] is None,
                 'v2 schema: preflight query cannot claim computation identity')
        if counts is not None:
            _require(executed == potential,
                     'v2 schema: completed query reads disagree with prepared witnesses')
            basis = result['basis']
            basis_keys = {'version', 'recipe', 'profile', 'modules', 'as_of', 'operation',
                          'scope', 'dependencies', 'resources', 'preflight_cost',
                          'query_counts', 'digest'}
            _require(isinstance(basis, dict) and set(basis) == basis_keys
                     and basis.get('version') == 1
                     and basis.get('recipe') == 'query-inputs/v1'
                     and basis.get('profile') == PROFILE
                     and basis.get('modules') == result['modules']
                     and basis.get('dependencies') == result['potential_dependencies']
                     and basis.get('resources') == result['resource_profile']
                     and basis.get('query_counts') == counts
                     and isinstance(basis.get('digest'), str)
                     and digest({key: value for key, value in basis.items() if key != 'digest'})
                     == basis['digest'],
                     'v2 schema: invalid finalized query basis')
            operation = basis.get('operation')
            query = operation.get('query') if isinstance(operation, dict) else None
            scope = basis.get('scope')
            scope_witnesses = [item for item in result['potential_dependencies']
                               if item.get('kind') == 'scope']
            _require(isinstance(query, dict) and set(operation) == {'query'}
                     and isinstance(query.get('scope'), str)
                     and isinstance(scope, dict) and len(scope_witnesses) == 1
                     and scope.get('witness') == scope_witnesses[0]
                     and query['scope'] == scope_witnesses[0]['scope_id']
                     and isinstance(scope.get('digest'), str)
                     and digest({key: value for key, value in scope.items() if key != 'digest'})
                     == scope['digest'],
                     'v2 schema: invalid finalized query scope basis')
            preflight = basis.get('preflight_cost')
            _require(isinstance(preflight, dict)
                     and set(preflight) == {'candidates', 'field_reads',
                                            'preflight_steps', 'step_upper_bound'}
                     and all(type(value) is int and value >= 0 for value in preflight.values())
                     and preflight['candidates'] == cost['candidates']
                     and preflight['field_reads'] == cost['field_reads']
                     and preflight['preflight_steps'] == cost['preflight_steps'],
                     'v2 schema: query preflight cost disagrees with result')
            expected_computation = digest({'snapshot_id': result['snapshot_id'],
                                           'basis': basis,
                                           'resources': result['resource_profile']})
            _require(result['computation_id'] == expected_computation,
                     'v2 schema: query computation identity disagrees with basis')
        _require(status != 'ok' or counts is not None,
                 'v2 schema: successful query lacks counts')
        if status == 'ok':
            fields = result['value'].get('fields') if isinstance(result['value'], dict) else None
            expected_fields = {'input_count', 'definite_match_count',
                               'unknown_membership_count', 'unknown_value_count',
                               'error_count', 'result'}
            _require(result['value'].get('type') == 'record' and isinstance(fields, dict)
                     and set(fields) == expected_fields,
                     'v2 schema: invalid successful query result')
            for key, expected in counts.items():
                _require(fields[key] == {'type': 'number', 'numerator': str(expected),
                                         'denominator': '1'},
                         'v2 schema: query result/count mismatch')
    _validate_bounds(result['operational_limits'], 'v2 schema: invalid result limits')
    if 'interpretation' in result:
        interpretation = result['interpretation']
        _require(isinstance(interpretation, dict)
                 and set(interpretation) == {'declared_profile', 'explicit_override'}
                 and isinstance(interpretation['declared_profile'], str)
                 and interpretation['explicit_override'] == PROFILE,
                 'v2 schema: invalid interpretation')


def _validate_v2_node(node):
    _require(isinstance(node, dict)
             and set(node) == {'body', 'fields', 'state', 'attention', 'computation'},
             'v2 schema: invalid node fields')
    fields = node['fields']
    _require(isinstance(fields, dict) and set(fields) == {'deps', 'snapshot', 'predicate'}
             and all(isinstance(item, str) for item in fields.values()),
             'v2 schema: invalid field roles')
    state = node['state']
    _require(isinstance(state, dict)
             and set(state) == {'basis', 'falsifier', 'contention', 'integrity'},
             'v2 schema: invalid state')
    _require(isinstance(state['basis'], dict)
             and {'status', 'dependencies'} <= set(state['basis'])
             and isinstance(state['basis']['dependencies'], dict),
             'v2 schema: invalid basis state')
    _require(isinstance(state['falsifier'], dict)
             and {'status', 'expression', 'reads'} <= set(state['falsifier']),
             'v2 schema: invalid falsifier state')
    contention = state['contention']
    _require(isinstance(contention, dict)
             and set(contention) == {'status', 'witnesses', 'alternatives'}
             and contention['status'] in ('detected', 'none_detected')
             and isinstance(contention['witnesses'], list)
             and isinstance(contention['alternatives'], list),
             'v2 schema: invalid contention state')
    integrity = state['integrity']
    _require(isinstance(integrity, dict)
             and set(integrity) == {'status', 'checks', 'issues'}
             and integrity['status'] == 'assessed'
             and isinstance(integrity['checks'], list)
             and all(isinstance(item, str) for item in integrity['checks'])
             and isinstance(integrity['issues'], list),
             'v2 schema: invalid integrity state')
    _validate_attention(node['attention'])
    _require(node['computation'] is None or isinstance(node['computation'], dict),
             'v2 schema: invalid computation')
    # The canonical typed identity walk rejects cycles, non-string mapping keys,
    # non-finite floats and unsupported Python values without source access.
    digest(node)


def validate_v2(snapshot, report):
    """Validate and detach the complete canonical schema-v2 nodes envelope."""
    _require(isinstance(snapshot, Snapshot), 'combined assessment requires a Snapshot')
    expected = {'schema_version', 'assessment_profile', 'attention_policy', 'snapshot_id',
                'as_of', 'scope', 'selection', 'assessment_revision', 'nodes',
                'operational_limits'}
    _require(isinstance(report, dict) and set(report) == expected,
             'canonical v2 nodes report required')
    _require(report['schema_version'] == 2 and report['assessment_profile'] == PROFILE,
             'v2 schema/profile mismatch')
    _require(report['attention_policy'] in POLICIES, 'v2 attention policy mismatch')
    data = snapshot.to_data()
    _require(report['snapshot_id'] == snapshot.snapshot_id, 'v2 snapshot mismatch')
    _require(report['as_of'] == data['as_of'], 'v2 as_of mismatch')
    scope = report['scope']
    _require(isinstance(scope, dict)
             and set(scope) == {'context', 'hypotheses', 'external_sources_fetched', 'evidence'}
             and isinstance(scope['context'], dict) and isinstance(scope['hypotheses'], dict)
             and scope['external_sources_fetched'] is False and isinstance(scope['evidence'], str),
             'v2 schema: invalid scope')
    selection, nodes = report['selection'], report['nodes']
    _require(isinstance(selection, list) and all(isinstance(item, str) for item in selection)
             and len(selection) == len(set(selection)), 'v2 schema: invalid selection')
    _require(isinstance(nodes, dict) and all(isinstance(item, str) for item in nodes)
             and set(selection) == set(nodes),
             'v2 assessment selection and node keys disagree')
    _require(not set(nodes) - set(data['nodes']), 'v2 contains nodes outside snapshot')
    revision = report['assessment_revision']
    _require(isinstance(revision, str) and revision == digest({
        key: value for key, value in report.items() if key != 'assessment_revision'}),
        'v2 assessment_revision mismatch')
    for node_id, node in nodes.items():
        _validate_v2_node(node)
        source = data['nodes'][node_id]
        _require(_same(node['body'], source['body']) and _same(node['fields'], source['fields']),
                 'v2 node does not match snapshot: ' + node_id)
        if node['computation'] is not None:
            _validate_result(node['computation'], snapshot.snapshot_id)
        falsifier = node['state']['falsifier'].get('computation')
        if falsifier is not None:
            _validate_result(falsifier, snapshot.snapshot_id)
        for finding in node['state']['basis']['dependencies'].values():
            _require(isinstance(finding, dict), 'v2 schema: invalid basis dependency')
            computation = finding.get('computation')
            if computation is not None:
                _validate_result(computation, snapshot.snapshot_id)
    bounds = report['operational_limits']
    _validate_bounds(bounds)
    return copy.deepcopy(report)


def _key(subject, version):
    return subject + '@' + version


def _reservation(code, state, subject, version, path):
    return {'code': code, 'state': state, 'subject': subject, 'version': version,
            'path': list(path)}


def _reduce_support_graph(roots, graph, outcomes, *, max_visits=MAX_SUPPORT_VISITS,
                          shared_budget=None):
    """Bounded iterative support reduction over already captured facts.

    ``graph`` is a normalized view of captured version pins.  No predicate body
    is present, so this reducer cannot become a second evaluator.
    """
    _require(isinstance(roots, list) and all(isinstance(item, str) for item in roots),
             'invalid support roots')
    _require(isinstance(graph, dict) and isinstance(outcomes, dict), 'invalid support graph')
    _require(type(max_visits) is int and 0 < max_visits <= MAX_SUPPORT_VISITS,
             'invalid support visit limit')
    visited, reservations = set(), []
    reservation_ids = set()
    budget = shared_budget if shared_budget is not None else {
        'remaining': max_visits, 'path_items': MAX_SUPPORT_PATH_ITEMS}
    _require(isinstance(budget, dict) and set(budget) == {'remaining', 'path_items'}
             and type(budget['remaining']) is int and type(budget['path_items']) is int,
             'invalid support budget')

    def reserve(item):
        cost = len(item['path'])
        if cost > budget['path_items']:
            raise OperationalLimit('support_limit')
        marker = digest(item)
        if marker not in reservation_ids:
            budget['path_items'] -= cost
            reservation_ids.add(marker)
            reservations.append(item)

    for root in roots:
        stack = [('enter', root)]
        active, path = set(), []
        while stack:
            action, current = stack.pop()
            if action == 'leave':
                active.discard(current)
                _require(path and path[-1] == current, 'invalid support traversal')
                path.pop()
                continue
            if current in active:
                subject, _, version = current.partition('@')
                reserve(_reservation('support_cycle', 'unavailable', subject, version,
                                     path + [current]))
                continue
            if current in visited:
                continue
            if len(visited) >= max_visits or budget['remaining'] <= 0:
                raise OperationalLimit('support_limit')
            visited.add(current)
            budget['remaining'] -= 1
            item = graph.get(current)
            if not isinstance(item, dict):
                subject, _, version = current.partition('@')
                reserve(_reservation('missing_support', 'unavailable', subject, version,
                                     path + [current]))
                continue
            subject, version = item.get('subject'), item.get('version')
            state, dependencies = item.get('state'), item.get('dependencies')
            _require(isinstance(subject, str) and isinstance(version, str)
                     and current == _key(subject, version)
                     and state in SUPPORT_STATES | {'accepted'}
                     and isinstance(dependencies, list), 'invalid support graph node')
            active.add(current)
            path.append(current)
            stack.append(('leave', current))
            if state != 'accepted':
                reserve(_reservation('support_state', state, subject, version, path))
            current_outcomes = outcomes.get(subject, [])
            if isinstance(current_outcomes, str):
                current_outcomes = [current_outcomes]
            _require(isinstance(current_outcomes, list), 'invalid support outcomes')
            for outcome in current_outcomes:
                _require(outcome in ('fired', 'moved', 'unknown', 'unavailable'),
                         'invalid support outcome')
                reserve(_reservation('current_' + outcome, outcome, subject, version, path))
            children = []
            for dependency in dependencies:
                _require(isinstance(dependency, dict)
                         and set(dependency) == {'subject', 'version'}
                         and isinstance(dependency['subject'], str)
                         and isinstance(dependency['version'], str),
                         'invalid support dependency')
                children.append(_key(dependency['subject'], dependency['version']))
            for child in reversed(sorted(children)):
                stack.append(('enter', child))
    reservations.sort(key=lambda item: (item['path'], item['code'], item['state']))
    return {'reducer': SUPPORT_REDUCER,
            'status': 'reserved' if reservations else 'clear',
            'visited': sorted(visited), 'visited_count': len(visited),
            'reservations': reservations}


def _subject_state(projection, subject, version):
    disposition = projection.get('dispositions', {}).get(subject, {})
    mark = disposition.get('marks', {}).get(version)
    if mark in ('replaced', 'superseded'):
        return 'moved'
    if mark in ACCEPTANCE:
        return mark
    current = projection['subjects'].get(subject)
    if current is None:
        return 'unavailable'
    if version in current['heads']:
        return current['acceptance']
    if version in disposition.get('proposals', []):
        return 'proposed'
    return 'moved'


def _support_graph(projection):
    graph = {}
    for version, witness in projection['pins'].items():
        subject = witness['subject']
        dependencies = []
        if witness['status'] == 'recorded':
            obj = witness['object']
            dependencies = [{'subject': dependency, 'version': pinned}
                            for dependency, pinned in sorted(obj.get('pins', {}).items())]
            dependencies.extend({'subject': dependency, 'version': 'unavailable:' + reason}
                                for dependency, reason in sorted(obj.get('pin_gaps', {}).items()))
        graph[_key(subject, version)] = {
            'subject': subject, 'version': version,
            'state': (_subject_state(projection, subject, version)
                      if witness['status'] == 'recorded' else 'unavailable'),
            'dependencies': dependencies,
        }
    return graph


def _outcomes(nodes):
    result = {}

    def add(subject, state):
        result.setdefault(subject, set()).add(state)

    for subject, node in nodes.items():
        falsifier = node['state']['falsifier']['status']
        if falsifier == 'holds':
            add(subject, 'fired')
        elif falsifier == 'unknown':
            add(subject, 'unknown')
        elif falsifier == 'error':
            add(subject, 'unavailable')
        elif isinstance(node.get('computation'), dict):
            status = node['computation'].get('status')
            if status == 'unknown':
                add(subject, 'unknown')
            elif status not in (None, 'ok'):
                add(subject, 'unavailable')
        for dependency, finding in node['state']['basis']['dependencies'].items():
            if finding.get('comparison') == 'changed' or finding.get('rule_changed') is True \
                    or finding.get('basis_comparison') == 'changed':
                add(dependency, 'moved')
            current = finding.get('current')
            if isinstance(current, dict) and current.get('status') == 'unknown':
                add(dependency, 'unknown')
            elif isinstance(current, dict) and current.get('status') not in (None, 'ok'):
                add(dependency, 'unavailable')
    return {subject: sorted(states) for subject, states in result.items()}


def _semantic_node(node):
    """Comparison preimage for replayed truth, excluding adapter build identity."""
    node = copy.deepcopy(node)
    computations = [node.get('computation'), node.get('state', {}).get('falsifier', {}).get('computation')]
    computations.extend(item.get('computation') for item in
                        node.get('state', {}).get('basis', {}).get('dependencies', {}).values())
    for computation in computations:
        if not isinstance(computation, dict):
            continue
        computation.pop('implementation', None)
        if isinstance(computation.get('assurance'), dict):
            computation['assurance'].pop('implementation', None)
    return node


def _semantic_falsifier(value):
    holder = {'computation': copy.deepcopy(value.get('computation')),
              'state': {'falsifier': copy.deepcopy(value), 'basis': {'dependencies': {}}}}
    return _semantic_node(holder)['state']['falsifier']


def _temporal_replays(projection):
    """Replay each retained observation through the canonical v2 evaluator."""
    temporal = projection.get('temporal')
    if temporal is None:
        return {}
    results = {}
    count = 0
    for observation in temporal['observations']:
        for claim in observation['claims']:
            subject, version = claim['subject'], claim['claim_id']
            episode = {
                'evidence_version': 1, 'operation': observation['operation'],
                'phase': observation['phase'], 'subject': subject, 'claim_id': version,
                'evidence_kind': observation['evidence_kind'],
                'applicability': claim['applicability'],
                'predicate_digest': claim['predicate_digest'],
                'anchors': copy.deepcopy(claim['anchors']), 'snapshot_id': None,
                'as_of': None, 'verification': 'unknown', 'outcome': 'unknown',
                'finding': None, 'result': None,
            }
            try:
                count += 1
                if count > MAX_TEMPORAL_REPLAYS:
                    raise OperationalLimit('temporal_replay_limit')
                historical = Snapshot.from_json(observation['snapshot'])
                episode['snapshot_id'] = historical.snapshot_id
                episode['as_of'] = historical.to_data()['as_of']
                retained = (validate_v2(historical, observation['assessment'])
                            if observation['assessment'] is not None else None)
                _require(retained is None or subject in retained['nodes'],
                         'temporal subject absent from retained assessment')
                replayed = (base_assessment.assess(
                    historical, [subject], policy=retained['attention_policy'],
                    operational_limits=retained['operational_limits']) if retained is not None
                    else base_assessment.assess(historical, [subject]))
                if retained is not None:
                    _require(_same(_semantic_node(replayed['nodes'][subject]),
                                   _semantic_node(retained['nodes'][subject])),
                             'temporal replay result mismatch')
                node = replayed['nodes'][subject]
                episode['verification'] = 'verified'
                status = node['state']['falsifier']['status']
                episode['result'] = copy.deepcopy(node['state']['falsifier'])
                episode['outcome'] = ('counterexample' if status == 'holds' else
                                      'not_counterexample' if status in ('does_not_hold', 'not_declared')
                                      else 'unknown')
                if episode['outcome'] == 'unknown':
                    episode['finding'] = 'temporal_falsifier_' + status
            except Exception as error:
                episode['finding'] = getattr(error, 'code', None) or \
                    str(error).split(':', 1)[0] or 'temporal_replay_failed'
            results.setdefault(subject, []).append(episode)
    for subject in results:
        results[subject].sort(key=lambda item: (
            item['claim_id'], item['snapshot_id'] or '', item['operation'], item['phase']))
    return results


def _unknown_temporal_replays(projection):
    results = {}
    for observation in projection.get('temporal', {}).get('observations', []):
        try:
            historical = Snapshot.from_json(observation['snapshot'])
            snapshot_id, as_of = historical.snapshot_id, historical.to_data()['as_of']
        except (ValueError, TypeError, KeyError):
            snapshot_id, as_of = None, None
        for claim in observation['claims']:
            results.setdefault(claim['subject'], []).append({
                'evidence_version': 1, 'operation': observation['operation'],
                'phase': observation['phase'], 'subject': claim['subject'],
                'claim_id': claim['claim_id'], 'applicability': claim['applicability'],
                'evidence_kind': observation['evidence_kind'],
                'predicate_digest': claim['predicate_digest'],
                'anchors': copy.deepcopy(claim['anchors']), 'snapshot_id': snapshot_id,
                'as_of': as_of, 'verification': 'unknown', 'outcome': 'unknown',
                'finding': 'temporal_replay_not_performed', 'result': None,
            })
    return results


def _validate_retained_temporal(projection, results):
    _require(isinstance(results, dict), 'invalid retained temporal evidence')
    expected = {}
    for observation in projection.get('temporal', {}).get('observations', []):
        snapshot_id = as_of = None
        retained = None
        try:
            snapshot = Snapshot.from_json(observation['snapshot'])
            retained = (validate_v2(snapshot, observation['assessment'])
                        if observation['assessment'] is not None else None)
            snapshot_id, as_of = snapshot.snapshot_id, snapshot.to_data()['as_of']
        except (ValueError, TypeError, KeyError):
            pass
        for claim in observation['claims']:
            key = (observation['operation'], observation['phase'], claim['subject'],
                   claim['claim_id'], claim['predicate_digest'])
            expected[key] = (claim, snapshot_id, as_of, retained,
                             observation['evidence_kind'])
    seen = set()
    for subject, episodes in results.items():
        _require(isinstance(subject, str) and isinstance(episodes, list),
                 'invalid retained temporal evidence')
        for episode in episodes:
            _validate_temporal({'version': 1, 'applicability': episode.get('applicability'),
                'status': 'unknown', 'current_falsifier': 'unavailable', 'complete': False,
                'counterexample_claim_ids': [], 'episodes': [episode], 'findings': []})
            key = (episode['operation'], episode['phase'], episode['subject'],
                   episode['claim_id'], episode['predicate_digest'])
            _require(subject == episode['subject'] and key in expected and key not in seen,
                     'retained temporal evidence does not match source receipts')
            claim, snapshot_id, as_of, retained, evidence_kind = expected[key]
            _require(episode['applicability'] == claim['applicability']
                     and episode['evidence_kind'] == evidence_kind
                     and episode['anchors'] == claim['anchors']
                     and episode['snapshot_id'] == snapshot_id and episode['as_of'] == as_of,
                     'retained temporal evidence does not match source receipts')
            if episode['verification'] == 'verified':
                result = episode['result']
                _require(isinstance(result, dict)
                         and {'status', 'expression', 'reads'} <= set(result),
                         'verified temporal evidence has no result')
                computation = result.get('computation')
                if computation is not None:
                    _require(snapshot_id is not None, 'verified temporal evidence has no snapshot')
                    _validate_result(computation, snapshot_id)
                if retained is not None:
                    _require(_same(_semantic_falsifier(result), _semantic_falsifier(
                        retained['nodes'][subject]['state']['falsifier'])),
                        'retained temporal result does not match source receipt')
                status = result['status']
                expected_outcome = ('counterexample' if status == 'holds' else
                                    'not_counterexample' if status in ('does_not_hold', 'not_declared')
                                    else 'unknown')
                _require(episode['outcome'] == expected_outcome,
                         'retained temporal outcome does not match source receipt')
            seen.add(key)
    _require(seen == set(expected), 'incomplete retained temporal evidence')
    return copy.deepcopy(results)


def _temporal_state(projection, subject, current_node, episodes):
    heads = set(projection['subjects'][subject]['heads'])
    head_objects = [projection['pins'][version]['object'] for version in sorted(heads)
                    if projection['pins'].get(version, {}).get('status') == 'recorded']
    metadata = [obj['body'].get('temporal') for obj in head_objects
                if isinstance(obj.get('body'), dict) and obj['body'].get('temporal') is not None]
    if not metadata:
        return None
    _require(len(metadata) == len(head_objects)
             and len({digest(item) for item in metadata}) == 1,
             'incompatible temporal heads')
    applicability = metadata[0]['applicability']
    exact = [item for item in episodes if item['claim_id'] in heads]
    relevant_findings = [copy.deepcopy(item) for item in projection['temporal']['findings']
                         if item.get('object_id') in heads or not item.get('object_id')]
    verified_counterexamples = [item for item in exact
                                if item['verification'] == 'verified'
                                and item['outcome'] == 'counterexample']
    unknown = (bool(relevant_findings)
               or any(item['verification'] != 'verified' or item['outcome'] == 'unknown'
                      for item in exact)
               or not exact)
    current = (current_node['state']['falsifier']['status']
               if current_node is not None else 'unavailable')
    if current == 'holds':
        status = 'counterexample'
    elif applicability == 'current' and current == 'does_not_hold':
        status = 'recovered' if verified_counterexamples else ('unknown' if unknown else 'clear')
    elif applicability in ('anchored', 'general') and verified_counterexamples:
        status = 'counterexample'
    elif current in ('unknown', 'error') or unknown:
        status = 'unknown'
    else:
        status = 'clear'
    findings = relevant_findings
    findings.extend({'code': item['finding'], 'subject': subject,
                     'object_id': item['claim_id'], 'detail': 'historical replay was not verified'}
                    for item in exact if item['finding'])
    return {'version': 1, 'applicability': applicability, 'status': status,
            'current_falsifier': current, 'complete': not unknown,
            'counterexample_claim_ids': sorted({item['claim_id']
                                                for item in verified_counterexamples}),
            'episodes': copy.deepcopy(episodes), 'findings': findings}


def _pin_evidence(projection, subject, version, cache):
    key = subject, version
    if key not in cache:
        cache[key] = history_adapter.pin_review_evidence(
            projection, version, subject=subject)
    return copy.deepcopy(cache[key])


def _subject_projection(projection, subject, graph, outcomes, evidence_cache, support_budget):
    state = projection['subjects'][subject]
    disposition = projection.get('dispositions', {}).get(subject, {
        'marks': {}, 'proposals': [], 'contested_claims': [], 'reviews': [], 'implied': []})
    heads = [_pin_evidence(projection, subject, version, evidence_cache)
             for version in state['heads']]
    selected = projection['pins'].get(min(state['heads'])) if state['heads'] else None
    pins = selected['object'].get('pins', {}) if selected and selected['status'] == 'recorded' else {}
    recorded = {dependency: _pin_evidence(projection, dependency, version, evidence_cache)
                for dependency, version in sorted(pins.items())}
    reviews = copy.deepcopy(disposition.get('reviews', []))
    review_evidence = _review_evidence(projection, reviews, evidence_cache)
    covered = subject in projection['coverage']['subjects']
    findings = [copy.deepcopy(item) for item in projection['integrity']['findings']
                if item.get('subject') == subject]
    roots = [_key(subject, version) for version in state['heads']]
    return {
        'acceptance': state['acceptance'],
        'head_ids': list(state['heads']), 'open_act_ids': list(state['open_acts']),
        'head_witnesses': heads,
        'proposals': list(disposition.get('proposals', [])),
        'reviews': reviews,
        'disposition_marks': copy.deepcopy(disposition.get('marks', {})),
        'contested_claims': list(disposition.get('contested_claims', [])),
        'implied_reservations': copy.deepcopy(disposition.get('implied', [])),
        'recorded_support': recorded,
        'pin_review_evidence': review_evidence,
        'coverage': {'included': covered,
                     'complete': bool(covered and projection['coverage']['complete'] and not findings),
                     'findings': findings},
        'support': _reduce_support_graph(roots, graph, outcomes,
                                         shared_budget=support_budget),
        **({'source_state': disposition['source_state']} if 'source_state' in disposition else {}),
    }


def _review_evidence(projection, reviews, evidence_cache):
    result = []
    for review in reviews:
        readings = {dependency: _pin_evidence(
                        projection, dependency, version, evidence_cache)
                    for dependency, version in sorted(review['body'].get('read', {}).items())}
        result.append({'review_id': review['id'], 'of': review['body']['of'], 'read': readings})
    return result


def _assurance(node, recorded_support, reviews):
    dependencies = {}
    for dependency, finding in node['state']['basis']['dependencies'].items():
        computation = finding.get('computation')
        dependencies[dependency] = copy.deepcopy(computation.get('assurance')) \
            if isinstance(computation, dict) else None
    predicate = node['state']['falsifier'].get('computation')
    current = node.get('computation')
    kinds = set()
    if recorded_support:
        kinds.add('version_pin')
    if reviews:
        kinds.add('review_pin')
    return {
        'actual': {
            'node': copy.deepcopy(current.get('assurance')) if isinstance(current, dict) else None,
            'falsifier': copy.deepcopy(predicate.get('assurance')) if isinstance(predicate, dict) else None,
            'dependencies': dependencies,
        },
        'recorded_evidence_kinds': sorted(kinds),
    }


def _node_with_history(node_id, node, projection, subjects, graph, outcomes):
    result = copy.deepcopy(node)
    if projection is None:
        result.update(
            acceptance={'status': 'not_applicable', 'head_ids': [], 'open_act_ids': []},
            history={'subject_id': None, 'current_head_witnesses': [], 'proposals': [],
                     'reviews': [], 'disposition_marks': {}, 'contested_claims': [],
                     'implied_reservations': [], 'recorded_support': {},
                     'pin_review_evidence': []},
            coverage={'included': False, 'complete': True, 'findings': []},
            assurance=_assurance(node, {}, []),
            support={'reducer': SUPPORT_REDUCER, 'status': 'clear', 'visited': [],
                     'visited_count': 0, 'reservations': []})
        return result
    state = projection['subjects'].get(node_id)
    if state is None:
        finding = {'code': 'history_subject_unavailable', 'subject': node_id,
                   'object_id': '', 'detail': 'computational node is outside captured history coverage'}
        result.update(
            acceptance={'status': 'unavailable', 'head_ids': [], 'open_act_ids': []},
            history={'subject_id': node_id, 'current_head_witnesses': [], 'proposals': [],
                     'reviews': [], 'disposition_marks': {}, 'contested_claims': [],
                     'implied_reservations': [], 'recorded_support': {},
                     'pin_review_evidence': []},
            coverage={'included': False, 'complete': False, 'findings': [finding]},
            assurance=_assurance(node, {}, []),
            support={'reducer': SUPPORT_REDUCER, 'status': 'reserved', 'visited': [],
                     'visited_count': 0,
                     'reservations': [_reservation('missing_support', 'unavailable', node_id, '', ())]})
        return result
    projected = subjects[node_id]
    result.update(
        acceptance={'status': state['acceptance'], 'head_ids': list(state['heads']),
                    'open_act_ids': list(state['open_acts'])},
        history={'subject_id': node_id,
                 'current_head_witnesses': copy.deepcopy(projected['head_witnesses']),
                 'proposals': copy.deepcopy(projected['proposals']),
                 'reviews': copy.deepcopy(projected['reviews']),
                 'disposition_marks': copy.deepcopy(projected['disposition_marks']),
                 'contested_claims': copy.deepcopy(projected['contested_claims']),
                 'implied_reservations': copy.deepcopy(projected['implied_reservations']),
                 'recorded_support': copy.deepcopy(projected['recorded_support']),
                 'pin_review_evidence': copy.deepcopy(projected['pin_review_evidence'])},
        coverage=copy.deepcopy(projected['coverage']),
        assurance=_assurance(node, projected['recorded_support'],
                             projected['pin_review_evidence']),
        support=copy.deepcopy(projected['support']))
    if 'temporal' in projected:
        result['temporal'] = copy.deepcopy(projected['temporal'])
        temporal = result['temporal']
        if temporal['status'] in ('counterexample', 'unknown'):
            code = ('historical_counterexample' if temporal['status'] == 'counterexample'
                    else 'historical_evidence_unknown')
            reason = {'code': code, 'related_ids': temporal['counterexample_claim_ids']}
            if not any(action['action'] == 'review' and reason in action['reasons']
                       for action in result['attention']):
                result['attention'].append({'action': 'review', 'reasons': [reason]})
    return result


def _history_summary(projection):
    if projection is None:
        return {'authority_status': 'not_active'}
    authority = projection['authority']
    result = {
        'authority_status': 'active',
        'projection_version': projection['projection_version'],
        'authority': {'record_id': authority['record_id'], 'generation': authority['generation'],
                      'identity': digest(authority)},
        'committed_set_digest': projection['baseline']['committed_set_digest'],
        'closure_digest': projection['closure_digest'],
        'coverage': copy.deepcopy(projection['coverage']),
        'identity_schemes': list(projection['identity_schemes']),
        'integrity': copy.deepcopy(projection['integrity']),
    }
    if history_contract.TEMPORAL_APPLICABILITY in projection.get('requires', []):
        result['capabilities'] = [history_contract.TEMPORAL_APPLICABILITY]
    return result


def _display_selection(value, known, default):
    result = list(default if value is None else value)
    _require(all(isinstance(item, str) for item in result)
             and len(result) == len(set(result)) and not set(result) - set(known),
             'invalid display selection')
    return result


def _findings_preimage(report):
    nodes = {node_id: {key: copy.deepcopy(value) for key, value in node.items()
                       if key != 'attention'}
             for node_id, node in report['nodes'].items()}
    return {key: copy.deepcopy(report[key]) for key in (
        'schema_version', 'assessment_profile', 'snapshot_id', 'as_of',
        'assessment_selection', 'history_selection', 'scope', 'history') } | {
            'nodes': nodes, 'history_subjects': copy.deepcopy(report['history_subjects'])}


def _validate_coverage(value):
    _require(isinstance(value, dict) and set(value) == {'included', 'complete', 'findings'}
             and type(value['included']) is bool and type(value['complete']) is bool
             and isinstance(value['findings'], list)
             and all(isinstance(item, dict) for item in value['findings']),
             'v3 schema: invalid coverage')


def _validate_pin_evidence(value):
    expected = {'status', 'evidence_kind', 'subject', 'version', 'body', 'profile',
                'value_status', 'value', 'basis_status', 'basis', 'findings'}
    _require(isinstance(value, dict) and set(value) == expected
             and value['status'] in ('recorded', 'unavailable')
             and value['evidence_kind'] == 'version_pin'
             and (value['subject'] is None or isinstance(value['subject'], str))
             and isinstance(value['version'], str)
             and (value['profile'] is None or isinstance(value['profile'], str))
             and value['value_status'] in ('recorded', 'unavailable')
             and value['basis_status'] in ('recorded', 'not_recorded', 'unavailable')
             and isinstance(value['findings'], list)
             and all(isinstance(item, dict) for item in value['findings']),
             'v3 schema: invalid pin evidence')


def _validate_review_evidence(value):
    _require(isinstance(value, dict) and set(value) == {'review_id', 'of', 'read'}
             and isinstance(value['review_id'], str) and isinstance(value['of'], str)
             and isinstance(value['read'], dict), 'v3 schema: invalid review evidence')
    for evidence in value['read'].values():
        _validate_pin_evidence(evidence)


def _validate_support(value):
    expected = {'reducer', 'status', 'visited', 'visited_count', 'reservations'}
    _require(isinstance(value, dict) and set(value) == expected
             and value['reducer'] == SUPPORT_REDUCER
             and value['status'] in ('clear', 'reserved')
             and isinstance(value['visited'], list)
             and all(isinstance(item, str) for item in value['visited'])
             and value['visited'] == sorted(set(value['visited']))
             and type(value['visited_count']) is int
             and value['visited_count'] == len(value['visited'])
             and isinstance(value['reservations'], list), 'v3 schema: invalid support')
    for item in value['reservations']:
        _require(isinstance(item, dict)
                 and set(item) == {'code', 'state', 'subject', 'version', 'path'}
                 and all(isinstance(item[key], str) for key in ('code', 'state', 'subject', 'version'))
                 and item['state'] in SUPPORT_STATES
                 and isinstance(item['path'], list)
                 and all(isinstance(path, str) for path in item['path']),
                 'v3 schema: invalid support reservation')
    _require((value['status'] == 'reserved') == bool(value['reservations']),
             'v3 schema: support status mismatch')


def _validate_assurance(value):
    _require(isinstance(value, dict) and set(value) == {'actual', 'recorded_evidence_kinds'}
             and isinstance(value['actual'], dict)
             and set(value['actual']) == {'node', 'falsifier', 'dependencies'}
             and (value['actual']['node'] is None or isinstance(value['actual']['node'], dict))
             and (value['actual']['falsifier'] is None
                  or isinstance(value['actual']['falsifier'], dict))
             and isinstance(value['actual']['dependencies'], dict)
             and all(item is None or isinstance(item, dict)
                     for item in value['actual']['dependencies'].values())
             and isinstance(value['recorded_evidence_kinds'], list)
             and value['recorded_evidence_kinds'] == sorted(set(value['recorded_evidence_kinds']))
             and set(value['recorded_evidence_kinds']) <= {'version_pin', 'review_pin'},
             'v3 schema: invalid assurance')


def _validate_node_history(value):
    expected = {'subject_id', 'current_head_witnesses', 'proposals', 'reviews',
                'disposition_marks', 'contested_claims', 'implied_reservations',
                'recorded_support', 'pin_review_evidence'}
    _require(isinstance(value, dict) and set(value) == expected
             and (value['subject_id'] is None or isinstance(value['subject_id'], str))
             and all(isinstance(value[key], list) for key in (
                 'current_head_witnesses', 'proposals', 'reviews', 'contested_claims',
                 'implied_reservations', 'pin_review_evidence'))
             and isinstance(value['disposition_marks'], dict)
             and isinstance(value['recorded_support'], dict),
             'v3 schema: invalid node history')
    _validate_ids(value['proposals'], 'v3 schema: invalid node history proposals')
    _validate_ids(value['contested_claims'], 'v3 schema: invalid node history contention')
    _require(all(isinstance(item, dict) for item in value['reviews'])
             and all(isinstance(item, dict) for item in value['implied_reservations'])
             and all(isinstance(key, str) and mark in (
                 'corrected', 'refuted', 'retired', 'replaced', 'superseded')
                 for key, mark in value['disposition_marks'].items()),
             'v3 schema: invalid node history dispositions')
    for evidence in value['current_head_witnesses']:
        _validate_pin_evidence(evidence)
    for evidence in value['recorded_support'].values():
        _validate_pin_evidence(evidence)
    for evidence in value['pin_review_evidence']:
        _validate_review_evidence(evidence)


def _validate_temporal(value):
    expected = {'version', 'applicability', 'status', 'current_falsifier', 'complete',
                'counterexample_claim_ids', 'episodes', 'findings'}
    _require(isinstance(value, dict) and set(value) == expected and value['version'] == 1
             and value['applicability'] in ('current', 'anchored', 'general')
             and value['status'] in ('clear', 'recovered', 'counterexample', 'unknown')
             and isinstance(value['current_falsifier'], str)
             and type(value['complete']) is bool
             and isinstance(value['episodes'], list) and len(value['episodes']) <= MAX_TEMPORAL_REPLAYS
             and isinstance(value['findings'], list), 'v3 schema: invalid temporal assessment')
    _validate_ids(value['counterexample_claim_ids'],
                  'v3 schema: invalid temporal counterexamples')
    for episode in value['episodes']:
        keys = {'evidence_version', 'operation', 'phase', 'subject', 'claim_id',
                'evidence_kind', 'applicability', 'predicate_digest', 'anchors',
                'snapshot_id', 'as_of', 'verification', 'outcome', 'finding', 'result'}
        _require(isinstance(episode, dict) and set(episode) == keys
                 and episode['evidence_version'] == 1
                 and episode['phase'] in ('before', 'after')
                 and episode['evidence_kind'] in (
                     'recorded_receipt', 'reconstructed_committed_world')
                 and episode['applicability'] in ('current', 'anchored', 'general')
                 and episode['verification'] in ('verified', 'unknown')
                 and episode['outcome'] in ('counterexample', 'not_counterexample', 'unknown')
                 and (episode['snapshot_id'] is None or _is_digest(episode['snapshot_id']))
                 and (episode['as_of'] is None or isinstance(episode['as_of'], str))
                 and (episode['finding'] is None or isinstance(episode['finding'], str))
                 and (episode['result'] is None or isinstance(episode['result'], dict))
                 and isinstance(episode['anchors'], dict)
                 and set(episode['anchors']) == {'on', 'at', 'applies'},
                 'v3 schema: invalid temporal episode')


def _validate_history_summary(value):
    _require(isinstance(value, dict) and value.get('authority_status') in ('active', 'not_active'),
             'v3 schema: invalid history summary')
    if value['authority_status'] == 'not_active':
        _require(set(value) == {'authority_status'}, 'v3 schema: invalid inactive history')
        return
    expected = {'authority_status', 'projection_version', 'authority', 'committed_set_digest',
                'closure_digest', 'coverage', 'identity_schemes', 'integrity'}
    _require(set(value) in (expected, expected | {'capabilities'}) and value['projection_version'] == 1
             and isinstance(value['authority'], dict)
             and set(value['authority']) == {'record_id', 'generation', 'identity'}
             and isinstance(value['authority']['record_id'], str)
             and type(value['authority']['generation']) is int
             and value['authority']['generation'] >= 0
             and _is_digest(value['authority']['identity'])
             and _is_digest(value['committed_set_digest'])
             and _is_digest(value['closure_digest'])
             and isinstance(value['coverage'], dict)
             and set(value['coverage']) == {'scope', 'subjects', 'complete'}
             and value['coverage']['scope'] in ('all', 'selected')
             and isinstance(value['coverage']['subjects'], list)
             and value['coverage']['subjects'] == sorted(set(value['coverage']['subjects']))
             and type(value['coverage']['complete']) is bool
             and isinstance(value['identity_schemes'], list)
             and all(isinstance(item, str) for item in value['identity_schemes'])
             and value['identity_schemes'] == sorted(set(value['identity_schemes']))
             and isinstance(value['integrity'], dict)
             and set(value['integrity']) == {'complete', 'findings'}
             and type(value['integrity']['complete']) is bool
             and isinstance(value['integrity']['findings'], list)
             and all(isinstance(item, dict) for item in value['integrity']['findings']),
             'v3 schema: invalid active history')
    if 'capabilities' in value:
        history_contract.validate_history_requires(value['capabilities'])


def validate(report):
    """Validate and detach one canonical v3 envelope for downstream consumers."""
    expected = {'schema_version', 'assessment_profile', 'attention_policy', 'snapshot_id', 'as_of',
                'assessment_selection', 'history_selection', 'display_selection',
                'operational_limits', 'scope', 'history', 'base_assessment_revision', 'nodes',
                'history_subjects', 'findings_revision', 'envelope_revision'}
    _require(isinstance(report, dict) and set(report) == expected,
             'canonical v3 assessment required')
    _require(report['schema_version'] == SCHEMA_VERSION
             and report['assessment_profile'] == PROFILE
             and report['attention_policy'] in POLICIES,
             'v3 schema/profile mismatch')
    _validate_bounds(report['operational_limits'], 'v3 operational limits mismatch')
    _require(_is_digest(report['snapshot_id'])
             and _is_digest(report['base_assessment_revision']),
             'v3 identity mismatch')
    nodes, subjects = report['nodes'], report['history_subjects']
    _require(isinstance(nodes, dict) and isinstance(subjects, dict), 'v3 findings must be mappings')
    _validate_ids(report['assessment_selection'], 'v3 schema: invalid assessment selection')
    _validate_ids(report['history_selection'], 'v3 schema: invalid history selection')
    _validate_ids(report['display_selection'], 'v3 schema: invalid display selection')
    _require(set(report['assessment_selection']) == set(nodes),
             'v3 assessment selection and node keys disagree')
    _require(set(report['history_selection']) == set(subjects),
             'v3 history selection and subject keys disagree')
    known = set(nodes) | set(subjects)
    _display_selection(report['display_selection'], known, [])
    for node in nodes.values():
        _require(isinstance(node, dict) and set(node) in ({
            'body', 'fields', 'state', 'attention', 'computation', 'acceptance', 'history',
            'coverage', 'assurance', 'support'}, {
            'body', 'fields', 'state', 'attention', 'computation', 'acceptance', 'history',
            'coverage', 'assurance', 'support', 'temporal'}), 'v3 schema: invalid node fields')
        _validate_v2_node({key: node[key] for key in (
            'body', 'fields', 'state', 'attention', 'computation')})
        acceptance = node['acceptance']
        _require(isinstance(acceptance, dict)
                 and set(acceptance) == {'status', 'head_ids', 'open_act_ids'}
                 and acceptance['status'] in ACCEPTANCE | {'not_applicable'},
                 'v3 schema: invalid node acceptance')
        _validate_ids(acceptance['head_ids'], 'v3 schema: invalid node head ids')
        _validate_ids(acceptance['open_act_ids'], 'v3 schema: invalid node open acts')
        _validate_coverage(node['coverage'])
        _validate_assurance(node['assurance'])
        _validate_support(node['support'])
        _validate_node_history(node['history'])
        if 'temporal' in node:
            _validate_temporal(node['temporal'])
    for subject in subjects.values():
        required = {'acceptance', 'head_ids', 'open_act_ids', 'head_witnesses', 'proposals',
                    'reviews', 'disposition_marks', 'contested_claims', 'implied_reservations',
                    'recorded_support', 'pin_review_evidence', 'coverage', 'support'}
        _require(isinstance(subject, dict) and required <= set(subject)
                 and set(subject) <= required | {'source_state', 'temporal'}
                 and subject.get('acceptance') in ACCEPTANCE
                 and all(isinstance(subject[key], list) for key in (
                     'head_ids', 'open_act_ids', 'head_witnesses', 'proposals', 'reviews',
                     'contested_claims', 'implied_reservations', 'pin_review_evidence'))
                 and isinstance(subject['disposition_marks'], dict)
                 and isinstance(subject['recorded_support'], dict),
                 'v3 schema: invalid history subject')
        _validate_ids(subject['head_ids'], 'v3 schema: invalid history head ids')
        _validate_ids(subject['open_act_ids'], 'v3 schema: invalid history open acts')
        _validate_ids(subject['proposals'], 'v3 schema: invalid history proposals')
        _validate_ids(subject['contested_claims'], 'v3 schema: invalid history contention')
        _require(all(isinstance(item, dict) for item in subject['reviews'])
                 and all(isinstance(item, dict) for item in subject['implied_reservations'])
                 and all(isinstance(key, str) and mark in (
                     'corrected', 'refuted', 'retired', 'replaced', 'superseded')
                     for key, mark in subject['disposition_marks'].items()),
                 'v3 schema: invalid history dispositions')
        for evidence in subject['head_witnesses']:
            _validate_pin_evidence(evidence)
        for evidence in subject['recorded_support'].values():
            _validate_pin_evidence(evidence)
        for evidence in subject['pin_review_evidence']:
            _validate_review_evidence(evidence)
        if 'source_state' in subject:
            _require(subject['source_state'] in ('committed', 'prepared_candidate'),
                     'v3 schema: invalid history source state')
        _validate_coverage(subject['coverage'])
        _validate_support(subject['support'])
        if 'temporal' in subject:
            _validate_temporal(subject['temporal'])
    history = report['history']
    _validate_history_summary(history)
    if history['authority_status'] == 'not_active':
        _require(not subjects and all(node['acceptance']['status'] == 'not_applicable'
                                      for node in nodes.values()),
                 'v3 inactive history made an acceptance claim')
    _require(report['findings_revision'] == digest(_findings_preimage(report)),
             'v3 findings_revision mismatch')
    _require(report['envelope_revision'] == digest({
        key: value for key, value in report.items() if key != 'envelope_revision'}),
        'v3 envelope_revision mismatch')
    OutputBudget(report['operational_limits']['output_bytes']).add(report)
    return copy.deepcopy(report)


def from_v2(snapshot, report, *, display_selection=None, temporal_evidence=None):
    """Enrich one canonical v2 report without source I/O or evaluation."""
    base = validate_v2(snapshot, report)
    data = snapshot.to_data()
    projection = data['context'].get('history')
    if projection is not None:
        projection = history_contract.validate_projection(projection)
    graph = _support_graph(projection) if projection is not None else {}
    outcomes = _outcomes(base['nodes'])
    temporal_replays = (_validate_retained_temporal(projection, temporal_evidence)
                        if temporal_evidence is not None and projection is not None
                        else _unknown_temporal_replays(projection) if projection is not None else {})
    evidence_cache = {}
    support_budget = {'remaining': MAX_SUPPORT_VISITS,
                      'path_items': MAX_SUPPORT_PATH_ITEMS}
    subjects = {subject: _subject_projection(
                    projection, subject, graph, outcomes, evidence_cache, support_budget)
                for subject in sorted(projection['subjects'])} if projection is not None else {}
    if projection is not None and 'temporal' in projection:
        for subject in subjects:
            temporal = _temporal_state(projection, subject, base['nodes'].get(subject),
                                       temporal_replays.get(subject, []))
            if temporal is not None:
                subjects[subject]['temporal'] = temporal
    nodes = {node_id: _node_with_history(node_id, node, projection, subjects, graph, outcomes)
             for node_id, node in base['nodes'].items()}
    history_selection = sorted(subjects)
    known = set(nodes) | set(history_selection)
    result = {
        'schema_version': SCHEMA_VERSION, 'assessment_profile': PROFILE,
        'attention_policy': base['attention_policy'], 'snapshot_id': base['snapshot_id'],
        'as_of': base['as_of'], 'assessment_selection': list(base['selection']),
        'history_selection': history_selection,
        'display_selection': _display_selection(display_selection, known, base['selection']),
        'operational_limits': copy.deepcopy(base['operational_limits']),
        'scope': copy.deepcopy(base['scope']), 'history': _history_summary(projection),
        'base_assessment_revision': base['assessment_revision'],
        'nodes': nodes, 'history_subjects': subjects,
    }
    result['findings_revision'] = digest(_findings_preimage(result))
    result['envelope_revision'] = digest(result)
    # Charge the exact complete public envelope once, after enrichment.  No
    # clipping or partial semantic result escapes this boundary.
    OutputBudget(result['operational_limits']['output_bytes']).add(result)
    return validate(result)


adapt = from_v2


def assess(snapshot, selection=None, *, policy='focused-review/v1', runtime=None,
           operational_limits=None, display_selection=None):
    """Compute schema v2 exactly once, then return its history-aware v3 envelope."""
    report = base_assessment.assess(snapshot, selection, policy=policy, runtime=runtime,
                                    operational_limits=operational_limits)
    projection = snapshot.to_data()['context'].get('history')
    temporal_evidence = (_temporal_replays(history_contract.validate_projection(projection))
                         if projection is not None and 'temporal' in projection else {})
    return from_v2(snapshot, report, display_selection=display_selection,
                   temporal_evidence=temporal_evidence)
