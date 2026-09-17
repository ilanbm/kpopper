"""Explicit conversion and comparison of detached, captured reasoning worlds.

This module never captures files or changes a record. A caller owns physical field
patching, complete candidate assembly, stale-input checks and publication. Reports
retain typed authored values; report_to_json is their lossless portable encoding.
"""
import copy
import re

from ..pending_grounding import entries, identity, _encode, json_bytes, document_capabilities
from .contract import PROFILE, capabilities, CapabilityError, operational_bounds, OutputBudget, admitted
from .language import lower, references
from .snapshot import Snapshot, _fields
from .assessment import assess, dependency_result
from .evaluate import Evaluator
from .authoring import DECLARATION, declare_document


TRANSFORMATION = 'legacy-to-core/v1'


def report_to_json(report):
    return json_bytes(_encode(report)).decode('utf-8')


def _report(source, document):
    return {'version': 1, 'transformation': TRANSFORMATION,
            'source_snapshot_id': source.snapshot_id, 'candidate_snapshot_id': None,
            'document': copy.deepcopy(document), 'changes': [], 'blockers': [],
            'problems': [], 'fired': [], 'preserved': [], 'comparisons': [],
            'legacy_runtime': {'profile': 'ordinary-reader/v1', 'status': 'not_checked'},
            'complete': False}


def _block(report, code, detail, nid=None, field=None):
    item = {'code': code, 'detail': detail}
    if nid is not None:
        item['id'] = nid
    if field is not None:
        item['field'] = field
    if item not in report['blockers']:
        report['blockers'].append(item)
        report['problems'].append((nid + ': ' if nid else '') + detail)


def _finish(report, operational_limits):
    report['fired'] = sorted(set(report['fired']))
    report['complete'] = not report['blockers']
    OutputBudget(operational_bounds(operational_limits)['output_bytes']).add(report)
    return report


def _candidate(source, document):
    data = source.to_data()
    return Snapshot.from_data(document, context=data['context'], hypotheses=data['hypotheses'],
                              as_of=data['as_of'])


def _scalar_type(tree, bodies, visiting=()):
    if 'ref' in tree:
        nid = tree['ref']
        if nid in visiting or nid not in bodies:
            return None
        body = bodies[nid]
        if isinstance(body, dict):
            if isinstance(body.get('rule'), dict):
                return _scalar_type(lower(body['rule']), bodies, (*visiting, nid))
            body = body.get('v', body.get('quoted'))
        return 'boolean' if type(body) is bool else 'number' if type(body) in (int, float) \
            else 'text' if isinstance(body, str) else 'null' if body is None else 'unsupported'
    for key, kind in (('text', 'text'), ('num', 'number'), ('bool', 'boolean'), ('null', 'null')):
        if key in tree:
            return kind
    kinds = [_scalar_type(child, bodies, visiting) for child in tree.get('args', [])]
    if tree.get('op') in ('add', 'sub', 'mul', 'div'):
        if any(kind in ('text', 'boolean', 'null', 'unsupported') for kind in kinds):
            raise ValueError('text, boolean or nonscalar arithmetic requires explicit typed reconciliation')
        return 'number'
    return 'boolean'


def _prose(reader, text):
    # Date-like rule text and ordinary sentences retain their unavailable reading.
    return bool(re.fullmatch(r'\s*\d{4}-\d{2}-\d{2}\s*', text)) or \
        (not reader.EXPR.search(text) and not reader.CMP.match(text))


def convert_snapshot(snapshot, *, reader, runtime=None, operational_limits=None):
    """Propose supported active-field changes and assess the complete candidate.

    A blocked report is a preview, never permission to publish its document.
    Call compare_snapshots again after composing transformed hypothesis layers and
    recomputing captured conflict witnesses. Neither call consults live sources.
    """
    source = snapshot.to_data()['document']
    report = _report(snapshot, source)
    candidate = report['document']
    try:
        declared = capabilities(source)
        fields = _fields(source)
    except (ValueError, TypeError, RecursionError) as error:
        _block(report, getattr(error, 'code', 'invalid_document'), str(error))
        return _finish(report, operational_limits)
    meta = candidate.get('meta')
    if meta is not None and not isinstance(meta, dict):
        _block(report, 'invalid_metadata', 'metadata must be a mapping before explicit core conversion')
        return _finish(report, operational_limits)
    candidate.setdefault('meta', {})['reasoning'] = copy.deepcopy(DECLARATION)
    if declared['profile'] == PROFILE:
        candidate['meta']['reasoning']['requires'] = list(declared['requires'])
    source_entries = entries(source)
    ids = set(source_entries)
    implicit = []
    for nid, (collection, body) in source_entries.items():
        if not isinstance(body, dict):
            continue
        deps = body.get(fields['deps'], [])
        if isinstance(deps, list) and any(isinstance(dep, str) and reader.is_builtin(dep) for dep in deps):
            _block(report, 'unsupported_core_builtin', 'computed builtin dependencies are outside core/v1', nid, fields['deps'])
        candidates = [('rule', False), (fields['predicate'], True)]
        if 'rule' not in body and isinstance(body.get('v'), str) and reader.rule_refs(body, ids):
            candidates.append(('v', False))
        for field, predicate in candidates:
            value = body.get(field)
            if value is None or value == '':
                continue
            try:
                if isinstance(value, dict):
                    core_tree = lower(value)
                    if declared['profile'] != PROFILE and reader.E.lower(value, predicate) != core_tree:
                        raise ValueError('structured expression is outside the characterized shared subset')
                    if any(reader.is_builtin(ref) for ref in references(core_tree)):
                        raise CapabilityError('unsupported_core_builtin', 'computed builtin input is outside core/v1')
                    continue
                if not isinstance(value, str):
                    raise ValueError('executable fields need text or a supported expression mapping')
                comparison = reader.CMP.match(value) if predicate else None
                try:
                    tree = lower(reader.E.convert_authored(value, predicate=predicate,
                        legacy_rhs=comparison.group(3) if comparison else None))
                except (ValueError, TypeError, SyntaxError, RecursionError):
                    if _prose(reader, value):
                        report['preserved'].append({'collection': collection, 'id': nid,
                            'field': field, 'reason': 'qualitative_prose', 'value': copy.deepcopy(value)})
                        continue
                    raise
                refs = references(tree)
                if any(reader.is_builtin(ref) for ref in refs):
                    raise CapabilityError('unsupported_core_builtin', 'computed builtin input is outside core/v1')
                unknown = set(refs) - ids
                if unknown:
                    raise ValueError('unknown or ambiguous reference: ' + ', '.join(sorted(unknown)))
                if predicate and set(refs) - set(deps if isinstance(deps, list) else []):
                    raise ValueError('predicate references are not declared dependencies')
                if field == 'rule' and any(key in body for key in ('v', 'quoted')):
                    raise ValueError('a stored reading and a calculation require explicit reconciliation')
                target = 'rule' if field == 'v' else field
                replacement = {'expr': value}
                edited = candidate[collection][nid]
                if target != field:
                    del edited[field]
                edited[target] = replacement
                report['changes'].append({'collection': collection, 'id': nid, 'field': field,
                    'target': target, 'before': copy.deepcopy(value), 'after': copy.deepcopy(replacement)})
                implicit.append((nid, field, tree, predicate))
            except (ValueError, TypeError, SyntaxError, RecursionError) as error:
                _block(report, getattr(error, 'code', 'unsupported_conversion'), str(error), nid, field)
    bodies = {nid: body for nid, (_, body) in entries(candidate).items()}
    for nid, field, tree, predicate in implicit:
        try:
            _scalar_type(tree, bodies)
            if predicate:
                kinds = [_scalar_type(child, bodies) for child in tree.get('args', [])]
                if 'text' in kinds and ('number' in kinds or all('ref' in child for child in tree.get('args', []))):
                    raise ValueError('text comparison requires explicit typed reconciliation')
        except (ValueError, TypeError, RecursionError) as error:
            _block(report, 'coercive_expression', str(error), nid, field)
    # Invalid syntax remains an explicit blocker. Comparison still reports every
    # independent supported result available in the final proposed document.
    try:
        candidate = declare_document(candidate)
        report['document'] = candidate
    except (ValueError, TypeError, SyntaxError, RecursionError) as error:
        _block(report, 'invalid_expression', str(error))
    compared = compare_snapshots(snapshot, _candidate(snapshot, candidate), reader=reader,
                                 runtime=runtime, operational_limits=operational_limits)
    for key in ('candidate_snapshot_id', 'comparisons', 'legacy_runtime', 'fired'):
        report[key] = compared[key]
    for blocker in compared['blockers']:
        _block(report, blocker['code'], blocker['detail'], blocker.get('id'), blocker.get('field'))
    return _finish(report, operational_limits)


def _typed(value, reader, *, legacy_numeric=False):
    if value is None:
        return {'type': 'null'}
    if legacy_numeric:
        number = reader.E.number(value)
        if number is not None:
            return {'type': 'number', 'numerator': str(number.numerator),
                    'denominator': str(number.denominator)}
    if isinstance(value, list):
        items = [_typed(item, reader) for item in value]
        return {'type': 'list', 'items': items} if all(item is not None for item in items) else None
    if isinstance(value, dict) and all(isinstance(key, str) for key in value):
        fields = {key: _typed(item, reader) for key, item in value.items()}
        return {'type': 'record', 'fields': fields} if all(item is not None for item in fields.values()) else None
    if type(value) is bool:
        return {'type': 'boolean', 'value': value}
    if isinstance(value, str):
        return {'type': 'text', 'value': value}
    number = reader.E.number(value)
    if number is not None:
        return {'type': 'number', 'numerator': str(number.numerator), 'denominator': str(number.denominator)}
    return None


def _legacy_observation(reader, raw, ids, nid, body, field, predicate, runtime_report):
    value = reader.evaluate(body.get(field), raw, ids) if predicate else reader.value_of(raw, ids, nid)
    if value is not None:
        # The legacy scalar evaluator represents exact fractions as a private
        # ``{rational: [n, d]}`` sentinel. Interpret that shape only when it is
        # the result of a legacy calculation/predicate. A stored T3 record with
        # the same keys is an ordinary record and must never be guessed numeric.
        typed = _typed(value, reader, legacy_numeric=predicate or field == 'rule')
        return {'status': 'known' if typed is not None else 'unrepresentable', 'value': typed}
    explicit = field != 'collection_scope' and isinstance(body, dict) and isinstance(body.get(field), dict)
    dependent = bool(set(reader.predicate_refs(body.get(field))) & {
        name for name, item in raw.items() if isinstance(item, dict) and isinstance(item.get('rule'), dict)}) if predicate else explicit
    return {'status': 'comparison_unavailable' if (explicit or dependent) and runtime_report['status'] == 'unavailable'
            else 'unavailable', 'value': None}


def _result(report, nid, field, before, after, *, blocked, required):
    status = after.get('status', 'unknown')
    after_value = after.get('value') if status == 'ok' else None
    if before['status'] == 'known':
        comparison = 'same' if status == 'ok' and before['value'] == after_value else 'changed'
        if comparison == 'changed':
            _block(report, 'result_changed', 'conversion changes a known type, value or availability', nid, field)
    elif before['status'] == 'unrepresentable':
        comparison = 'unrepresentable'
        _block(report, 'unsupported_value', 'known legacy value is outside supported core scalar types', nid, field)
    elif before['status'] == 'comparison_unavailable':
        comparison = 'comparison_unavailable'
    else:
        comparison = 'newly_executable' if status == 'ok' else 'unavailable'
    if status == 'operational_error':
        _block(report, 'operational_error', 'packaged core runtime could not validate the candidate', nid, field)
    elif required and not admitted(after, blocked=blocked):
        _block(report, 'unavailable_core_result', 'required core result is unavailable without a supported declared hole', nid, field)
    report['comparisons'].append({'id': nid, 'field': field, 'status': comparison,
        'before': before, 'after': {'status': status, 'value': copy.deepcopy(after_value),
                                   'diagnostics': copy.deepcopy(after.get('diagnostics', []))}})


def compare_snapshots(source, candidate, *, reader, runtime=None, operational_limits=None):
    """Compare known results using only the two provided immutable worlds."""
    old_data, new_data = source.to_data(), candidate.to_data()
    old, new = old_data['document'], new_data['document']
    report = _report(source, new)
    report['candidate_snapshot_id'] = candidate.snapshot_id
    try:
        old_cap, new_cap = capabilities(old), capabilities(new)
        if new_cap['profile'] != PROFILE:
            raise CapabilityError('invalid_capability', 'candidate must explicitly declare core/v1')
        document_capabilities(new)
        old_fields, new_fields = _fields(old), _fields(new)
        if old_fields != new_fields:
            _block(report, 'field_roles_changed', 'conversion must preserve captured field-role mappings')
        old_entries, new_entries = entries(old), entries(new)
    except (ValueError, TypeError, RecursionError) as error:
        _block(report, getattr(error, 'code', 'invalid_document'), str(error))
        return _finish(report, operational_limits)
    raw = {nid: body for nid, (_, body) in old_entries.items()}
    ids = set(raw)
    old_assessment = None
    if old_cap['profile'] == PROFILE:
        report['legacy_runtime'] = {'profile': PROFILE, 'status': 'not_applicable'}
        old_assessment = assess(source, runtime=runtime, operational_limits=operational_limits)
    else:
        probe = reader.E.compute(raw, ids)
        report['legacy_runtime'] = {'profile': old_cap['profile'],
            'status': 'unavailable' if probe.get('error') else 'available'}
        if probe.get('error'):
            report['legacy_runtime']['reason'] = str(probe['error'])
        else:
            report['legacy_runtime']['identity'] = {key: probe[key] for key in
                ('core_source_sha256', 'expression_source_sha256') if key in probe}
    try:
        assessed = assess(candidate, runtime=runtime, operational_limits=operational_limits)
    except (ValueError, TypeError, RecursionError) as error:
        _block(report, getattr(error, 'code', 'invalid_candidate'), str(error))
        return _finish(report, operational_limits)
    source_engine = Evaluator(source, runtime=runtime, operational_limits=operational_limits)
    candidate_engine = Evaluator(candidate, runtime=runtime, operational_limits=operational_limits)
    for nid, (_, body) in old_entries.items():
        target = new_entries.get(nid, (None, None))[1]
        if nid not in new_entries:
            _block(report, 'missing_entry', 'candidate omits a captured entry', nid)
            continue
        if isinstance(body, dict):
            old_seen = {old_fields['snapshot']: body[old_fields['snapshot']]} if old_fields['snapshot'] in body else {}
            new_seen = {old_fields['snapshot']: target[old_fields['snapshot']]} if isinstance(target, dict) and old_fields['snapshot'] in target else {}
            if identity(old_seen) != identity(new_seen):
                _block(report, 'history_changed', 'profile conversion must preserve original historical evidence', nid, old_fields['snapshot'])
        node = assessed['nodes'][nid]
        blocked = isinstance(target, dict) and bool(reader._blocked_text(target))
        value_node = not isinstance(body, dict) or any(key in body for key in ('v', 'quoted', 'rule', 'collection_scope'))
        if value_node:
            field = ('collection_scope' if 'collection_scope' in body else 'rule' if 'rule' in body else 'v') \
                if isinstance(body, dict) else 'v'
            result = node['computation']
            if result is None and isinstance(target, dict) and 'collection_scope' in target:
                result = dependency_result(candidate, nid, candidate_engine)
            result = result or {'status': 'unknown', 'value': None, 'diagnostics': []}
            if old_assessment is not None:
                old_result = old_assessment['nodes'][nid]['computation']
                if old_result is None and isinstance(body, dict) and 'collection_scope' in body:
                    old_result = dependency_result(source, nid, source_engine)
                old_result = old_result or {}
                before = {'status': 'known' if old_result.get('status') == 'ok' else 'unavailable',
                          'value': old_result.get('value')}
            else:
                before = _legacy_observation(reader, raw, ids, nid, body, field, False, report['legacy_runtime'])
            required = isinstance(target, dict) and (isinstance(target.get('rule'), dict) or 'collection_scope' in target)
            _result(report, nid, field, before, result, blocked=blocked, required=required)
        if isinstance(body, dict) and old_fields['predicate'] in body:
            field = old_fields['predicate']
            falsifier = node['state']['falsifier']
            result = falsifier.get('computation') or {'status': 'unknown', 'value': None, 'diagnostics': []}
            if old_assessment is not None:
                old_falsifier = old_assessment['nodes'][nid]['state']['falsifier']
                old_result = old_falsifier.get('computation') or {}
                before = {'status': 'known' if old_result.get('status') == 'ok' else 'unavailable',
                          'value': old_result.get('value')}
            else:
                before = _legacy_observation(reader, raw, ids, nid, body, field, True, report['legacy_runtime'])
            required = isinstance(target, dict) and isinstance(target.get(new_fields['predicate']), dict)
            _result(report, nid, field, before, result, blocked=blocked, required=required)
            if result.get('status') == 'ok' and result.get('value') == {'type': 'boolean', 'value': True} \
                    and before.get('value') != {'type': 'boolean', 'value': True}:
                report['fired'].append(nid)
        # Source and judgment citations are not scalar inputs; an actually absent
        # declared dependency still needs the same explicit hole as a missing rule.
        for dependency, finding in node['state']['basis']['dependencies'].items():
            if dependency not in new_entries:
                _result(report, nid, new_fields['deps'], {'status': 'unavailable', 'value': None},
                        finding['computation'], blocked=blocked, required=True)
        for issue in node['state']['integrity']['issues']:
            # Historical missing snapshots remain missing. No migration refreshes
            # them; expression/required-value failures are handled above.
            if issue['code'] in ('invalid_dependencies', 'invalid_snapshot', 'invalid_history',
                                 'unsupported_history', 'invalid_capability'):
                _block(report, issue['code'], 'candidate contains unsupported captured evidence', nid, issue.get('field'))
    return _finish(report, operational_limits)
