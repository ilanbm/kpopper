"""Internal core batch preparation and assessment gate."""
import copy
import hashlib
from pathlib import Path

from .authoring import World, prepare, declare
from .contract import PROFILE, digest, OutputBudget


def graph(reader, record, target=None, profile=None):
    doc, world = prepare(reader, [str(record)], {'profile': profile})
    if world is None:
        return None
    ids, jud, fields = reader.infer(doc)
    report = world.assessment()
    seeds = target if isinstance(target, list) else [target] if target else []
    hit, touched, derived = reader.reach_of(ids, jud, world.raw, seeds)
    for seed in seeds:
        if seed in jud:
            hit.setdefault(seed, seed)
    states = {}
    for nid in jud:
        tag, reason = world.state(nid)
        node = report['nodes'][nid]
        states[nid] = {'tag': tag, 'reason': reason,
            'evaluation': {'holds': True, 'does_not_hold': False}.get(node['state']['falsifier']['status']),
            'name': reader.named(jud[nid]['body']), 'verdict': jud[nid]['body'].get('verdict')}
    from ..pending_grounding import _encode
    return {'hash': hashlib.sha256(Path(record).read_bytes()).hexdigest(),
            'assessment_profile': PROFILE, 'snapshot_id': report['snapshot_id'],
            'assessment': _encode(report), 'judgments': states,
            'reach': {'judgments': sorted(hit), 'via': hit,
                      'touched': sorted(touched), 'derived': derived}}


def _failures(reader, report):
    failures = {}
    for nid, node in report['nodes'].items():
        body = node['body'] if isinstance(node['body'], dict) else {}
        blocked = bool(reader._blocked_text(body))
        issues = node['state']['integrity']['issues']
        for issue in issues:
            if blocked and issue['code'] in ('missing_dependency', 'missing_reference'):
                continue
            key = (nid, issue['code'], tuple(issue.get('related_ids', [])))
            failures[key] = f"{nid}: {issue['code']}"
        result = node['computation']
        if isinstance(body.get('rule'), dict) and result and result['status'] != 'ok':
            for diagnostic in result['diagnostics']:
                if blocked and diagnostic['code'] == 'missing_reference':
                    continue
                key = (nid, diagnostic['code'], tuple(diagnostic.get('related_ids', [])))
                failures[key] = f"{nid}: {diagnostic['code']}"
        fields = node['fields']
        if fields['deps'] in body:
            predicate = node['state']['falsifier']
            if predicate['status'] == 'holds':
                failures[(nid, 'fired', ())] = f'{nid}: wrong_if holds'
            elif predicate['status'] in ('unknown', 'error', 'not_declared') and not blocked \
                    and not (predicate['status'] == 'not_declared' and reader._reopened_text(body)):
                failures[(nid, 'unavailable_predicate', ())] = f'{nid}: condition cannot be computed'
    return failures


def gate(reader, before, after):
    """Compare actual core findings; old failures and new fired inputs stay distinct."""
    if before.get('assessment_profile') != PROFILE or after.get('assessment_profile') != PROFILE:
        raise ValueError('mixed assessment profiles in core writer gate')
    from ..pending_grounding import _decode
    old_report, new_report = _decode(before['assessment']), _decode(after['assessment'])
    was, now = _failures(reader, old_report), _failures(reader, new_report)
    out = []
    for key, message in now.items():
        if key in was:
            continue
        nid, code, _ = key
        old = old_report['nodes'].get(nid)
        current = new_report['nodes'][nid]
        if code == 'fired' and old is not None and old['state']['falsifier']['status'] == 'does_not_hold' \
                and digest(old['body']) == digest(current['body']):
            continue
        out.append(message)
    return out


def receipt(before, after):
    value = {'version': 1, 'profile': PROFILE,
             'before': digest(before), 'after': digest(after)}
    return {**value, 'digest': digest(value)}


def verify_receipt(value, before, prepared):
    if value != receipt(before, prepared):
        raise ValueError('invalid core writer gate evidence')


def stage(reader, paths, actions, *, profile=None):
    """Normalize/validate a complete candidate, then use the existing text editors."""
    doc, before = prepare(reader, paths, {'profile': profile})
    if before is None:
        raise ValueError('core batch needs an explicitly selected core profile')
    ids, jud, fields = reader.infer(doc)
    fields = {**fields, **before.fields}
    candidate = copy.deepcopy(doc)
    homes = {}
    from ..pending_grounding import entries
    initial = entries(doc)
    for action in actions:
        nid = action['id']
        if action['kind'] == 'set':
            collection, body = initial[nid]
            candidate[collection][nid] = reader._peer('recording').set_body(body, action)
        else:
            collection = reader._collection_for(candidate, ids, jud, fields, nid, action['body'], action.get('into'))
            if nid in initial:
                raise ValueError(nid + ': batch add cannot replace an existing entry')
            candidate.setdefault(collection, {})[nid] = copy.deepcopy(action['body'])
        homes[nid] = collection
    unnormalized = World(reader, candidate, original=before.snapshot)
    normalized, diagnostics = [], []
    for action in actions:
        authored, notes = unnormalized.normalize(action, previous_raw=before.raw)
        normalized.append(authored)
        diagnostics.extend(notes)
        if authored['kind'] == 'add':
            candidate[homes[authored['id']]][authored['id']] = authored['body']
    final = World(reader, candidate, original=before.snapshot)
    final_ids, final_jud, final_fields = reader.infer(candidate)
    final_fields = {**final_fields, **final.fields}
    for action in normalized:
        nid = action['id']
        validation_doc = copy.deepcopy(candidate)
        if action['kind'] == 'set':
            validation_doc[homes[nid]][nid] = copy.deepcopy(initial[nid][1])
            validation_ids = final_ids
        else:
            validation_doc[homes[nid]].pop(nid)
            validation_ids = final_ids - {nid}
        validation = World(reader, validation_doc, original=before.snapshot)
        failures = validation.validate(action, validation_doc, validation_ids,
                                       {key: item for key, item in final_jud.items() if key != nid}, final_fields)
        if failures:
            raise ValueError('; '.join(failures))
    budget = OutputBudget(final.bounds['output_bytes'])
    for action in normalized:
        body = candidate[homes[action['id']]][action['id']]
        if action['kind'] == 'add' and isinstance(body, dict) and final_fields['deps'] in body:
            seen = reader._snapshot(body[final_fields['deps']], final.raw, final_ids, final_jud, paths, None)
            budget.add(seen)
            body[final_fields['snapshot']] = seen
    # Complete semantic evidence is available before any bytes are published.
    World(reader, candidate, original=before.snapshot).assessment()
    lines = Path(paths[0]).read_text(encoding='utf-8').split('\n')
    for action in normalized:
        nid = action['id']
        if action['kind'] == 'set':
            reader._set_in(lines, nid, action['value'], action.get('as_of'), action.get('why'),
                           action.get('source'), action.get('at'))
            if '_record_scope' in action:
                reader._replace_in(lines, nid, candidate[homes[nid]][nid])
        else:
            reader._ensure_collection(lines, homes[nid])
            reader._add_in(lines, nid, candidate[homes[nid]][nid], homes[nid])
    declare(lines, reader)
    reader._bump_updated(lines, actions[0]['as_of'])
    encoded = '\n'.join(lines)
    parsed = reader.parse(text=encoded)
    # Reject a text encoding that changed a typed claim or its historical evidence.
    if digest({nid: [collection, body] for nid, (collection, body) in entries(parsed).items()}) != digest({nid: [collection, body] for nid, (collection, body) in entries(candidate).items()}):
        raise ValueError('staged batch text does not preserve the complete authored entries')
    return encoded.encode('utf-8'), diagnostics
