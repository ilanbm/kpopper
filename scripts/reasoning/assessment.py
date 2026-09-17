"""Experimental core findings in a separate versioned assessment envelope."""
import copy

from .. import assessment as legacy_assessment
from .. import expressions as E
from .contract import (PROFILE, CapabilityError, capabilities, digest, operational_bounds,
                       OperationalLimit, OutputBudget, validate_value)
from .evaluate import Evaluator, compare_basis
from .language import lower
from .snapshot import SnapshotError


def _historical_value(value):
    if type(value) is bool:
        return {'type': 'boolean', 'value': value}
    number = E.number(value)
    if number is not None:
        return {'type': 'number', 'numerator': str(number.numerator), 'denominator': str(number.denominator)}
    if value is None:
        return {'type': 'null'}
    if isinstance(value, str):
        return {'type': 'text', 'value': value}
    return None


def _history(old, document):
    """Decode only explicitly versioned typed values; preserve legacy mappings."""
    historical = old.get('computed') if isinstance(old, dict) else None
    if not isinstance(historical, dict):
        if isinstance(old, dict) and 'computed' in old:
            return None, None, 'invalid_history'
        return _historical_value(old), None, None
    if 'version' not in historical:
        if 'value' not in historical:
            return None, historical, 'invalid_history'
        return _historical_value(historical['value']), historical, None
    if type(historical['version']) is not int or historical['version'] != 2:
        return None, historical, 'unsupported_history'
    declaration = document.get('meta', {})
    declaration = declaration.get('reasoning', {}) if isinstance(declaration, dict) else {}
    if not isinstance(declaration, dict) or declaration.get('version') != 2 \
            or declaration.get('profile') != PROFILE:
        return None, historical, 'invalid_capability'
    try:
        if set(historical) - {'version', 'value', 'basis', 'rule'} \
                or not {'version', 'value', 'basis'} <= set(historical):
            raise ValueError('invalid history envelope')
        value = validate_value(historical['value'])
        if compare_basis(historical['basis'], historical['basis']) != 'same':
            raise ValueError('invalid historical basis')
    except (ValueError, TypeError, RecursionError):
        return None, historical, 'invalid_history'
    return value, historical, None


def _is_scope(data, dependency):
    body = data['nodes'].get(dependency, {}).get('body')
    return isinstance(body, dict) and 'collection_scope' in body


def dependency_result(snapshot, dependency, evaluator=None):
    """Return the shared scalar-or-scope review result over this exact snapshot.

    Scopes are captured input summaries, never arithmetic references. The scope
    witness lives in potential_dependencies and in the complete basis; no native
    executed node read or proof claim is manufactured for this Python capture.
    """
    engine = evaluator if evaluator is not None else Evaluator(snapshot)
    if engine.snapshot.snapshot_id != snapshot.snapshot_id:
        raise ValueError('dependency evaluator belongs to a different snapshot')
    if not _is_scope(engine.data, dependency):
        return engine.evaluate({'ref': dependency}, declared=[dependency])
    result = {'schema_version': 1, 'profile': PROFILE, 'modules': [],
              'snapshot_id': snapshot.snapshot_id, 'computation_id': None,
              'implementation': None, 'resource_profile': {'version': 'resources/v2', **engine.limits},
              'operational_limits': dict(engine.operational_limits),
              'status': 'unknown', 'value': None, 'diagnostics': [], 'executed_reads': [],
              'potential_dependencies': [], 'potential_ids': [dependency], 'basis': None,
              'assurance': {'kind': 'computed', 'formal_scope': []},
              'cost': {'steps': 0, 'preflight_steps': 0, 'node_evaluations': {}}}
    try:
        cap = capabilities(engine.data['document'], profile=PROFILE)
        result['modules'] = cap['requires']
        if cap.get('experimental_override'):
            result['interpretation'] = {'declared_profile': cap['declared_profile'], 'explicit_override': PROFILE}
        capture = snapshot.capture_scope(dependency)
        result.update(status='ok', value=capture.value, basis=capture.basis,
                      potential_dependencies=[capture.witness],
                      potential_ids=sorted({dependency, *capture.candidates}))
        result['computation_id'] = digest({'snapshot_id': snapshot.snapshot_id,
                                          'scope_basis': result['basis'],
                                          'resources': result['resource_profile']})
    except (SnapshotError, CapabilityError) as error:
        result.update(status='limit' if error.code == 'limit' else
                      'unsupported_capability' if error.code == 'unsupported_capability' else 'error',
                      diagnostics=[{'code': error.code, 'related_ids': [dependency]}])
    OutputBudget(engine.operational_limits['output_bytes']).add(result)
    return result


def _rule_changed(old, current):
    if old is None or current is None:
        return old != current
    try:
        return lower(old) != lower(current)
    except (ValueError, TypeError, RecursionError, SyntaxError):
        # An unsupported historical rule is not evidence of semantic equality.
        return None


def _scope_projection(data, visible, budget):
    """Bind the full captured world by snapshot ID; display only relevant bodies."""
    original = data['context']
    context = {key: original[key] for key in (
        'read_mode', 'original_read_mode', 'project', 'target', 'source_collection') if key in original}
    if isinstance(context.get('project'), dict):
        context['project'] = {key: context['project'][key] for key in (
            'version', 'mode', 'generation', 'routing_identity', 'publication_identity') if key in context['project']}
    context['conflicts'] = {nid: variants
                            for nid, variants in original.get('conflicts', {}).items() if nid in visible}
    pending = original.get('pending', {})
    context['pending'] = {'ref': pending.get('ref'),
                          'bundle_revisions': sorted(pending.get('bundles', {})),
                          'observations': 'bound_in_snapshot', 'remote_acceptance': 'unassessed'}
    hypotheses = {}
    for name, hyp in data['hypotheses'].items():
        document = {}
        for collection, entries in hyp['document'].items():
            if collection in ('meta', 'schema', 'record', 'also') or not isinstance(entries, dict):
                continue
            selected = {nid: body for nid, body in entries.items() if nid in visible}
            if selected:
                document[collection] = selected
        if document and 'schema' in hyp['document']:
            document['schema'] = hyp['document']['schema']
        hypotheses[name] = {'kind': hyp.get('kind', 'hypothesis'), 'document': document,
                            'status': 'unreadable' if hyp['error'] else 'inspected'}
    scope = {'context': context, 'hypotheses': hypotheses, 'external_sources_fetched': False,
            'evidence': 'full input remains bound by snapshot_id; bodies are projected to the selected dependency closure'}
    budget.add(scope)
    return copy.deepcopy(scope)


def assess(snapshot, selection=None, *, policy='focused-review/v1', runtime=None, operational_limits=None):
    bounds = operational_bounds(operational_limits)
    budget = OutputBudget(bounds['output_bytes'])
    data = snapshot.to_data()
    selected = []
    selected_set = set()
    for index, nid in enumerate(sorted(data['nodes']) if selection is None else selection):
        if index >= bounds['batch_requests']:
            raise OperationalLimit('batch_request_limit')
        if nid in selected_set:
            continue
        if len(selected) >= bounds['batch_requests']:
            raise OperationalLimit('batch_request_limit')
        selected.append(nid)
        selected_set.add(nid)
    if set(selected) - set(data['nodes']):
        raise ValueError('unknown assessment ID')
    engine = Evaluator(snapshot, runtime=runtime, operational_limits=bounds)
    tasks = {}
    def task(key, value):
        if key not in tasks and len(tasks) >= bounds['batch_requests']:
            raise OperationalLimit('batch_request_limit')
        tasks[key] = value
    for nid in selected:
        node = data['nodes'][nid]
        body, fields = node['body'], node['fields']
        OutputBudget(budget.remaining).add(body)
        deps = body.get(fields['deps']) if isinstance(body, dict) else None
        if isinstance(deps, list) and all(isinstance(dep, str) for dep in deps):
            for dep in deps:
                task(('value', dep), ({'ref': dep}, [dep]))
        if isinstance(body, dict) and isinstance(body.get(fields['predicate']), dict):
            task(('predicate', nid), (body[fields['predicate']], deps if isinstance(deps, list) else []))
        if not isinstance(body, dict) or fields['deps'] not in body or 'rule' in body:
            task(('value', nid), ({'ref': nid}, [nid]))
    scalar_tasks = {key: value for key, value in tasks.items()
                    if key[0] != 'value' or not _is_scope(data, key[1])}
    computed = dict(zip(scalar_tasks, engine.evaluate_many(list(scalar_tasks.values()))))
    for key in tasks:
        if key not in computed:
            computed[key] = dependency_result(snapshot, key[1], engine)
    visible = set(selected)
    for result in computed.values():
        visible.update(result['potential_ids'])
    nodes = {}
    conflicts = data['context'].get('conflicts', {})
    for nid in selected:
        node = data['nodes'][nid]
        body, fields = node['body'], node['fields']
        body_map = body if isinstance(body, dict) else {}
        judgment = fields['deps'] in body_map
        deps = body_map.get(fields['deps'])
        valid_deps = isinstance(deps, list) and all(isinstance(dep, str) for dep in deps)
        issues, readings = [], {}
        if judgment and not valid_deps:
            issues.append({'code': 'invalid_dependencies', 'field': fields['deps'], 'related_ids': []})
        seen = body_map.get(fields['snapshot'], {})
        if not isinstance(seen, dict):
            issues.append({'code': 'invalid_snapshot', 'field': fields['snapshot'], 'related_ids': []})
            seen = {}
        for dep in dict.fromkeys(deps if valid_deps else []):
            now = computed[('value', dep)]
            old = seen.get(dep)
            old_typed, historical, history_error = _history(old, data['document']) \
                if dep in seen else (None, None, None)
            same = now['status'] == 'ok' and old_typed is not None
            comparison = ('same' if old_typed == now['value'] else 'changed') if same else 'unknown'
            rule = data['nodes'].get(dep, {}).get('body', {})
            rule = rule.get('rule') if isinstance(rule, dict) else None
            rule_changed = None
            if historical is not None and 'rule' in historical and not history_error:
                rule_changed = _rule_changed(historical['rule'], rule)
            basis_comparison = 'unavailable' if history_error else \
                compare_basis(now['basis'], historical.get('basis') if historical else None)
            if history_error:
                issues.append({'code': history_error, 'field': fields['snapshot'], 'related_ids': [dep]})
            readings[dep] = {
                'current': {'status': now['status'], 'value': now['value']},
                'at_review': {'status': 'unavailable' if history_error else
                              'recorded' if dep in seen else 'missing', 'value': copy.deepcopy(old)},
                'comparison': comparison, 'rule_changed': rule_changed,
                'basis_comparison': basis_comparison, 'computation': now,
            }
            if dep not in data['nodes']:
                issues.append({'code': 'missing_dependency', 'field': fields['deps'], 'related_ids': [dep]})
            if dep not in seen:
                issues.append({'code': 'missing_snapshot', 'field': fields['snapshot'], 'related_ids': [dep]})
            for diagnostic in now['diagnostics']:
                issues.append({**diagnostic, 'field': fields['deps']})
        predicate = body_map.get(fields['predicate'])
        falsifier = {'status': 'not_declared' if judgment else 'not_applicable',
                     'expression': copy.deepcopy(predicate), 'reads': []}
        if predicate not in (None, ''):
            result = computed.get(('predicate', nid))
            if result is None:
                falsifier.update(status='unknown', reason='declared_prose', reads=None)
            else:
                boolean = result['value'] and result['value']['type'] == 'boolean'
                status = ('holds' if result['value']['value'] else 'does_not_hold') \
                    if result['status'] == 'ok' and boolean else \
                    'unknown' if result['status'] == 'unknown' else 'error'
                falsifier.update(status=status, computation=result,
                                 reads=[w['id'] for w in result['executed_reads']])
                if status in ('error', 'unknown'):
                    falsifier['reason'] = result['status'] if result['status'] != 'ok' else 'type_error'
                issues.extend({**item, 'field': fields['predicate']} for item in result['diagnostics'])
        alternatives = [{'hypothesis': name, 'id': nid}
                        for name, hyp in sorted(data['hypotheses'].items()) if not hyp['error']
                        and any(isinstance(members, dict) and nid in members for members in hyp['document'].values())]
        state = {'basis': {'status': 'assessed' if judgment and valid_deps else 'error' if judgment else 'not_applicable',
                           'dependencies': readings},
                 'falsifier': falsifier,
                 'contention': {'status': 'detected' if nid in conflicts else 'none_detected',
                                'witnesses': copy.deepcopy(conflicts.get(nid, [])), 'alternatives': alternatives},
                 'integrity': {'status': 'assessed', 'checks': ['core/v1'], 'issues': issues}}
        attention = legacy_assessment.attention(state, policy)
        changed_basis = [dep for dep, finding in readings.items() if finding['basis_comparison'] == 'changed']
        if changed_basis and policy == 'focused-review/v1':
            attention.append({'action': 'review', 'reasons': [
                {'code': 'computational_basis_changed', 'related_ids': changed_basis}]})
        result_node = {'body': body, 'fields': fields, 'state': state,
                       'attention': attention, 'computation': computed.get(('value', nid))}
        budget.add({nid: result_node})
        result_node['body'] = copy.deepcopy(body)
        nodes[nid] = result_node
    report = {'schema_version': 2, 'assessment_profile': PROFILE, 'attention_policy': policy,
              'snapshot_id': snapshot.snapshot_id, 'as_of': data['as_of'],
              'scope': _scope_projection(data, visible, budget), 'selection': selected, 'nodes': nodes,
              'operational_limits': bounds}
    OutputBudget(bounds['output_bytes']).add(report)
    report['assessment_revision'] = digest(report)
    OutputBudget(bounds['output_bytes']).add(report)
    return report
