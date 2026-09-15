"""Experimental core findings in a separate versioned assessment envelope."""
import copy

from .. import assessment as legacy_assessment
from .. import expressions as E
from .contract import PROFILE, digest
from .evaluate import Evaluator, compare_basis
from .language import literal, lower


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


def _scope_projection(data, visible):
    """Bind the full captured world by snapshot ID; display only relevant bodies."""
    original = data['context']
    context = {key: copy.deepcopy(original[key]) for key in (
        'read_mode', 'original_read_mode', 'project', 'target', 'source_collection') if key in original}
    if isinstance(context.get('project'), dict):
        context['project'] = {key: context['project'][key] for key in (
            'mode', 'generation', 'routing_identity') if key in context['project']}
    context['conflicts'] = {nid: copy.deepcopy(variants)
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
            selected = {nid: copy.deepcopy(body) for nid, body in entries.items() if nid in visible}
            if selected:
                document[collection] = selected
        if document and 'schema' in hyp['document']:
            document['schema'] = copy.deepcopy(hyp['document']['schema'])
        hypotheses[name] = {'kind': hyp.get('kind', 'hypothesis'), 'document': document,
                            'status': 'unreadable' if hyp['error'] else 'inspected'}
    return {'context': context, 'hypotheses': hypotheses, 'external_sources_fetched': False,
            'evidence': 'full input remains bound by snapshot_id; bodies are projected to the selected dependency closure'}


def assess(snapshot, selection=None, *, policy='focused-review/v1', runtime=None):
    data = snapshot.to_data()
    selected = sorted(data['nodes']) if selection is None else list(dict.fromkeys(selection))
    if set(selected) - set(data['nodes']):
        raise ValueError('unknown assessment ID')
    engine = Evaluator(snapshot, runtime=runtime)
    tasks = {}
    for nid in selected:
        node = data['nodes'][nid]
        body, fields = node['body'], node['fields']
        deps = body.get(fields['deps']) if isinstance(body, dict) else None
        if isinstance(deps, list) and all(isinstance(dep, str) for dep in deps):
            for dep in deps:
                tasks[('value', dep)] = ({'ref': dep}, [dep])
        if isinstance(body, dict) and isinstance(body.get(fields['predicate']), dict):
            tasks[('predicate', nid)] = (body[fields['predicate']], deps if isinstance(deps, list) else [])
        if not isinstance(body, dict) or fields['deps'] not in body or 'rule' in body:
            tasks[('value', nid)] = ({'ref': nid}, [nid])
    computed = dict(zip(tasks, engine.evaluate_many(list(tasks.values()))))
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
            historical = old.get('computed') if isinstance(old, dict) else None
            historical = historical if isinstance(historical, dict) else None
            old_value = historical.get('value') if historical else old
            old_typed = _historical_value(old_value) if dep in seen else None
            same = now['status'] == 'ok' and old_typed is not None
            comparison = ('same' if old_typed == now['value'] else 'changed') if same else 'unknown'
            rule = data['nodes'].get(dep, {}).get('body', {})
            rule = rule.get('rule') if isinstance(rule, dict) else None
            rule_changed = None
            if historical is not None and 'rule' in historical:
                rule_changed = not E.same(historical['rule'], rule)
            basis_comparison = compare_basis(now['basis'], historical.get('basis') if historical else None)
            readings[dep] = {
                'current': {'status': now['status'], 'value': now['value']},
                'at_review': {'status': 'recorded' if dep in seen else 'missing', 'value': copy.deepcopy(old)},
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
        nodes[nid] = {'body': copy.deepcopy(body), 'fields': fields, 'state': state,
                      'attention': attention, 'computation': computed.get(('value', nid))}
    report = {'schema_version': 2, 'assessment_profile': PROFILE, 'attention_policy': policy,
              'snapshot_id': snapshot.snapshot_id, 'as_of': data['as_of'],
              'scope': _scope_projection(data, visible), 'selection': selected, 'nodes': nodes}
    report['assessment_revision'] = digest(report)
    return report
