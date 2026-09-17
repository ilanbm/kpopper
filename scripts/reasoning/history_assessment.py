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
ACCEPTANCE = frozenset(('accepted', 'proposed', 'contested', 'refuted', 'corrected',
                        'retired', 'unreviewed', 'unavailable'))
SUPPORT_STATES = ACCEPTANCE | frozenset(('moved', 'fired', 'unknown'))
POLICIES = frozenset(('focused-review/v1', 'falsifiers-only/v1'))


def _require(condition, message):
    if not condition:
        raise ValueError(message)


def _same(left, right):
    return digest(left) == digest(right)


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
    required = {'schema_version', 'profile', 'modules', 'snapshot_id', 'computation_id',
                'implementation', 'resource_profile', 'status', 'value', 'diagnostics',
                'executed_reads', 'potential_dependencies', 'potential_ids', 'basis',
                'assurance', 'cost', 'operational_limits'}
    _require(isinstance(result, dict) and required <= set(result)
             and not set(result) - required - {'interpretation'},
             'v2 schema: invalid computation result')
    _require(result['schema_version'] == 1 and result['profile'] == PROFILE
             and result['snapshot_id'] == snapshot_id
             and isinstance(result['modules'], list)
             and all(isinstance(item, str) for item in result['modules'])
             and (result['computation_id'] is None
                  or isinstance(result['computation_id'], str))
             and (result['implementation'] is None
                  or isinstance(result['implementation'], dict)),
             'v2 schema: invalid computation identity')
    resources = result['resource_profile']
    common = {'version', 'steps', 'depth', 'digits'}
    extended = common | {'value_nodes', 'value_depth', 'value_bytes'}
    valid_resources = isinstance(resources, dict) and (
        resources.get('version') == 'resources/v2' and set(resources) == common
        or resources.get('version') == 'resources/v3' and set(resources) == extended)
    _require(valid_resources and all(type(resources[key]) is int and resources[key] >= 0
                                     for key in ('steps', 'depth', 'digits')),
             'v2 schema: invalid resource profile')
    if resources['version'] == 'resources/v3':
        _require(0 < resources['value_nodes'] <= 10_000
                 and 0 < resources['value_depth'] <= 128
                 and 0 < resources['value_bytes'] <= 16_777_216,
                 'v2 schema: invalid resource profile')
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
    for item in diagnostics:
        _require(isinstance(item, dict) and set(item) == {'code', 'related_ids'}
                 and isinstance(item['code'], str) and isinstance(item['related_ids'], list)
                 and all(isinstance(value, str) for value in item['related_ids']),
                 'v2 schema: invalid diagnostic')
    executed = result['executed_reads']
    _require(isinstance(executed, list), 'v2 schema: invalid executed reads')
    for item in executed:
        _require(isinstance(item, dict) and set(item) == {'kind', 'id', 'fingerprint'}
                 and item['kind'] == 'node' and isinstance(item['id'], str)
                 and isinstance(item['fingerprint'], str)
                 and len(item['fingerprint']) == 64
                 and all(char in '0123456789abcdef' for char in item['fingerprint']),
                 'v2 schema: invalid executed read')
    potential = result['potential_dependencies']
    _require(isinstance(potential, list)
             and all(isinstance(item, dict) and item.get('kind') in ('node', 'scope')
                     for item in potential), 'v2 schema: invalid potential dependencies')
    _require(isinstance(result['potential_ids'], list)
             and all(isinstance(item, str) for item in result['potential_ids']),
             'v2 schema: invalid potential ids')
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
    _require(isinstance(cost, dict)
             and set(cost) == {'steps', 'preflight_steps', 'node_evaluations'}
             and type(cost['steps']) is int and cost['steps'] >= 0
             and type(cost['preflight_steps']) is int and cost['preflight_steps'] >= 0
             and isinstance(cost['node_evaluations'], dict)
             and all(isinstance(key, str) and type(value) is int and value >= 0
                     for key, value in cost['node_evaluations'].items()),
             'v2 schema: invalid computation cost')
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


def _reduce_support_graph(roots, graph, outcomes, *, max_visits=MAX_SUPPORT_VISITS):
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

    def reserve(item):
        marker = digest(item)
        if marker not in reservation_ids:
            reservation_ids.add(marker)
            reservations.append(item)

    for root in roots:
        stack = [('enter', root, (root,))]
        active = set()
        while stack:
            action, current, path = stack.pop()
            if action == 'leave':
                active.discard(current)
                continue
            if current in active:
                subject, _, version = current.partition('@')
                reserve(_reservation('support_cycle', 'unavailable', subject, version, path))
                continue
            if current in visited:
                continue
            if len(visited) >= max_visits:
                raise OperationalLimit('support_limit')
            visited.add(current)
            item = graph.get(current)
            if not isinstance(item, dict):
                subject, _, version = current.partition('@')
                reserve(_reservation('missing_support', 'unavailable', subject, version, path))
                continue
            subject, version = item.get('subject'), item.get('version')
            state, dependencies = item.get('state'), item.get('dependencies')
            _require(isinstance(subject, str) and isinstance(version, str)
                     and current == _key(subject, version)
                     and state in SUPPORT_STATES | {'accepted'}
                     and isinstance(dependencies, list), 'invalid support graph node')
            active.add(current)
            stack.append(('leave', current, path))
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
                stack.append(('enter', child, path + (child,)))
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


def _pin_evidence(projection, subject, version, cache):
    key = subject, version
    if key not in cache:
        cache[key] = history_adapter.pin_review_evidence(
            projection, version, subject=subject)
    return copy.deepcopy(cache[key])


def _subject_projection(projection, subject, graph, outcomes, evidence_cache):
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
        'support': _reduce_support_graph(roots, graph, outcomes),
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
    return result


def _history_summary(projection):
    if projection is None:
        return {'authority_status': 'not_active'}
    authority = projection['authority']
    return {
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
    _require(isinstance(report['snapshot_id'], str) and len(report['snapshot_id']) == 64
             and isinstance(report['base_assessment_revision'], str)
             and len(report['base_assessment_revision']) == 64,
             'v3 identity mismatch')
    nodes, subjects = report['nodes'], report['history_subjects']
    _require(isinstance(nodes, dict) and isinstance(subjects, dict), 'v3 findings must be mappings')
    _require(set(report['assessment_selection']) == set(nodes),
             'v3 assessment selection and node keys disagree')
    _require(set(report['history_selection']) == set(subjects),
             'v3 history selection and subject keys disagree')
    known = set(nodes) | set(subjects)
    _display_selection(report['display_selection'], known, [])
    for node in nodes.values():
        _require(isinstance(node, dict) and set(node) == {
            'body', 'fields', 'state', 'attention', 'computation', 'acceptance', 'history',
            'coverage', 'assurance', 'support'}, 'v3 schema: invalid node fields')
        _validate_v2_node({key: node[key] for key in (
            'body', 'fields', 'state', 'attention', 'computation')})
        acceptance = node['acceptance']
        _require(isinstance(acceptance, dict)
                 and set(acceptance) == {'status', 'head_ids', 'open_act_ids'}
                 and acceptance['status'] in ACCEPTANCE | {'not_applicable'},
                 'v3 schema: invalid node acceptance')
        _require(isinstance(node['coverage'], dict) and isinstance(node['assurance'], dict)
                 and isinstance(node['support'], dict) and isinstance(node['history'], dict),
                 'v3 schema: invalid node evidence')
    for subject in subjects.values():
        _require(isinstance(subject, dict) and subject.get('acceptance') in ACCEPTANCE,
                 'v3 schema: invalid history subject')
    history = report['history']
    _require(isinstance(history, dict) and history.get('authority_status') in ('active', 'not_active'),
             'v3 schema: invalid history summary')
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


def from_v2(snapshot, report, *, display_selection=None):
    """Enrich one complete canonical v2 report without source I/O or evaluation."""
    base = validate_v2(snapshot, report)
    data = snapshot.to_data()
    projection = data['context'].get('history')
    if projection is not None:
        projection = history_contract.validate_projection(projection)
    graph = _support_graph(projection) if projection is not None else {}
    outcomes = _outcomes(base['nodes'])
    evidence_cache = {}
    subjects = {subject: _subject_projection(
                    projection, subject, graph, outcomes, evidence_cache)
                for subject in sorted(projection['subjects'])} if projection is not None else {}
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
    return from_v2(snapshot, report, display_selection=display_selection)
