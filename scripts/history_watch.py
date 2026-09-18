"""Pure merge of captured active-history closures for watch."""
import copy
from dataclasses import replace


def _identity(value):
    from .pending_grounding import identity
    return identity(value)


def _capture(evidence):
    from . import provenance as P
    knowledge_views = P._peer('knowledge_views')
    return knowledge_views.validate_history_evidence(copy.deepcopy(evidence))


def _same_bytes(left, right, label):
    if left != right:
        raise ValueError('history collision: ' + label)


def _modules():
    from . import provenance as P
    return (P._peer('history_contract'), P._peer('history_adapter'),
            P._peer('history_store'), P._peer('history_hypotheses'))


def _physical(record):
    """Return only independently captured physical layers.

    Named history layers are regenerated from the union projection.  Feeding
    their old rendered bodies back to Snapshot would let an older worktree
    overwrite a newer immutable proposal with the same name.
    """
    _, _, _, HH = _modules()
    result = {}
    for item in record.get('hypotheses', []):
        if item.get('kind') == HH.KIND:
            continue
        result[item['name']] = {'doc': copy.deepcopy(item['doc']),
                                'head': copy.deepcopy(item['head']),
                                **({'kind': item['kind']} if item.get('kind') else {})}
    return result


def _merge_physical(records):
    ancestor, main, working = (_physical(records[name]) for name in
                               ('ancestor', 'main', 'working'))
    merged = copy.deepcopy(main)
    changed = set()
    for name in sorted(set(ancestor) | set(working)):
        if ancestor.get(name) == working.get(name):
            continue
        for item in (ancestor.get(name), working.get(name)):
            if item:
                changed.update(_entry_ids(item['doc']))
        if main.get(name) != ancestor.get(name) and main.get(name) != working.get(name):
            raise ValueError('history collision: physical hypothesis ' + name)
        if name in working:
            merged[name] = copy.deepcopy(working[name])
        else:
            merged.pop(name, None)
    return merged, changed


def _entry_ids(document):
    from . import provenance as P
    return set(P._peer('pending_grounding').entries(document))


def _as_of(record):
    observed = record.get('snapshot')
    return observed.get('as_of') if isinstance(observed, dict) else None


def _context(records, captures, kind):
    inputs = {}
    for name in ('ancestor', 'main', 'working'):
        inputs[name] = {
            'identity': _identity(records[name]['history']['sha256']),
            'baseline': copy.deepcopy(captures[name].baseline),
            **({'snapshot_id': records[name]['snapshot']['snapshot_id']}
               if isinstance(records[name].get('snapshot'), dict) else {})}
    return {'read_mode': 'supplied',
            'operation': {'version': 1, 'phase': 'prospective',
                          'kind': kind, 'inputs': inputs}}


def _snapshot(captured, record, hypotheses, *, context):
    _, A, _, _ = _modules()
    adapted = A.from_store_capture(captured)
    return adapted.snapshot(context=context, hypotheses=hypotheses,
                            as_of=_as_of(record))


def merged_snapshot(records):
    """Build a supplied Snapshot from the union of existing immutable history."""
    C, _, H, _ = _modules()

    captures = {name: _capture(records[name]['history']) for name in
                ('ancestor', 'main', 'working')}
    ancestor, main, working = (captures[name] for name in ('ancestor', 'main', 'working'))
    for label, candidate in (('main', main), ('working', working)):
        _same_bytes(candidate.marker, ancestor.marker, label + ' authority')
        _same_bytes(candidate.state['rules'], ancestor.state['rules'], label + ' rules')
        for operation, raw in ancestor.commits.items():
            if candidate.commits.get(operation) != raw:
                raise ValueError('history incomplete ancestor closure: ' + operation)
        for key, raw in ancestor.object_bytes.items():
            if candidate.object_bytes.get(key) != raw:
                raise ValueError('history incomplete ancestor object closure')
    commits = dict(main.commits)
    for operation, raw in working.commits.items():
        if operation in commits and commits[operation] != raw:
            raise ValueError('history collision: commit ' + operation)
        commits[operation] = raw
    raw_objects = dict(main.object_bytes)
    for key, raw in working.object_bytes.items():
        if key in raw_objects and raw_objects[key] != raw:
            raise ValueError('history collision: object ' + key[1])
        raw_objects[key] = raw
    if sum(map(len, commits.values())) + sum(map(len, raw_objects.values())) > H.MAX_CAPTURE_BYTES:
        raise ValueError('history_limit: union closure exceeds captured byte limit')
    selected = C.committed_objects(main.marker, commits, raw_objects)
    state = H.reduce(selected, main.state['rules'])
    baseline = H.baseline(main.marker, commits, state)
    virtual = replace(main, entry_bytes=b'', document={}, commits=commits,
                      objects=selected, object_bytes=raw_objects, state=state,
                      baseline=baseline)
    store = H.Store.__new__(H.Store)
    rendered = store.render(virtual, objects=selected, commits=commits)
    virtual = replace(virtual, entry_bytes=rendered, document=C.decode_document(rendered))
    hypotheses, _ = _merge_physical(records)
    return _snapshot(virtual, records['main'], hypotheses,
                     context=_context(records, captures, 'history_union'))


def _changed(records):
    changed = set()
    before = records['ancestor']['doc']
    after = records['working']['doc']
    before_entries = {name: value for name, value in _entries(before).items()}
    after_entries = {name: value for name, value in _entries(after).items()}
    changed.update(name for name in set(before_entries) | set(after_entries)
                   if before_entries.get(name) != after_entries.get(name))
    _, hypothesis_ids = _merge_physical(records)
    changed.update(hypothesis_ids)
    return sorted(changed)


def _entries(document):
    from . import provenance as P
    return P._peer('pending_grounding').entries(document)


def _project(snapshot):
    from . import provenance as P
    CapturedAssessment = P._peer('reasoning.context').CapturedAssessment
    operations = P._peer('reasoning.operations')
    return operations.findings(CapturedAssessment.from_snapshot(snapshot))


def _disposition_findings(snapshot, baseline):
    """Expose newly contested immutable subjects omitted from the scalar view."""
    current = snapshot.to_data()['context']['history']['subjects']
    prior = baseline.to_data()['context']['history']['subjects']
    result = []
    for subject, state in sorted(current.items()):
        if state['acceptance'] == 'contested' and prior.get(subject, {}).get('acceptance') != 'contested':
            result.append({'kind': 'collision', 'id': subject,
                           'reason': subject + ': contested immutable history heads'})
    return result


def _main_snapshot(records):
    capture = _capture(records['main']['history'])
    return _snapshot(capture, records['main'], _physical(records['main']),
                     context={'read_mode': 'supplied',
                              'operation': {'version': 1, 'phase': 'observed',
                                            'kind': 'history_watch_main',
                                            'identity': _identity(records['main']['history']['sha256']),
                                            'baseline': copy.deepcopy(capture.baseline)}})


def compare(snapshot):
    """Compare captured active-history records without source reads or writes."""
    try:
        candidate = merged_snapshot(snapshot)
        main_snapshot = _main_snapshot(snapshot)
        projected = _project(candidate)
        baseline = _project(main_snapshot)
        findings = _disposition_findings(candidate, main_snapshot)
        for kind, lines, prior in (('falsified', projected['falsified'], baseline['falsified']),
                                   ('uncheckable', projected['holes'], baseline['holes']),
                                   ('uncheckable', projected['moved'], baseline['moved'])):
            for line in sorted(set(lines) - set(prior)):
                finding = {'kind': kind, 'id': line.split(':', 1)[0], 'reason': line}
                finding['fingerprint'] = _identity(finding)
                findings.append(finding)
        return {'state': 'attention' if findings else 'clear', 'findings': findings,
                'changed': _changed(snapshot), 'versions': snapshot.get('versions'),
                'identity': snapshot.get('identity')}
    except (ValueError, SystemExit) as error:
        finding = {'kind': 'uncheckable', 'id': 'record', 'reason': str(error)}
        finding['fingerprint'] = _identity(finding)
        return {'state': 'attention', 'findings': [finding], 'changed': [],
                'versions': snapshot.get('versions'), 'identity': snapshot.get('identity')}
