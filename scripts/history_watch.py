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


def _validated_record(record):
    """Bind one public Snapshot to its separately transported history evidence."""
    from . import provenance as P
    Snapshot = P._peer('reasoning.snapshot').Snapshot
    captured = _capture(record['history'])
    raw = record.get('snapshot')
    if not isinstance(raw, dict):
        raise ValueError('history snapshot evidence is missing')
    observed = Snapshot.from_snapshot(copy.deepcopy(raw))
    data = observed.to_data()
    adapted = _modules()[1].from_store_capture(captured)
    if _identity(data['document']) != _identity(adapted.document) \
            or _identity(record.get('doc')) != _identity(data['document']):
        raise ValueError('history snapshot document mismatch')
    if _identity(data['context'].get('history')) != _identity(adapted.projection):
        raise ValueError('history snapshot closure mismatch')
    hypothesis_items = record.get('hypotheses', [])
    hypotheses = {item['name']: {
        'document': copy.deepcopy(item['doc']), 'head': copy.deepcopy(item['head']), 'error': None,
        **({'kind': item['kind']} if item.get('kind') else {})}
        for item in hypothesis_items}
    if len(hypotheses) != len(hypothesis_items):
        raise ValueError('history snapshot contains duplicate hypothesis names')
    if _identity(hypotheses) != _identity(data['hypotheses']):
        raise ValueError('history snapshot hypotheses mismatch')
    return captured, observed


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


def _context(records, captures, observations, kind):
    inputs = {}
    for name in ('ancestor', 'main', 'working'):
        inputs[name] = {
            'identity': _identity(records[name]['history']['sha256']),
            'baseline': copy.deepcopy(captures[name].baseline),
            'snapshot_id': observations[name].snapshot_id}
    return {'read_mode': 'supplied',
            'operation': {'version': 1, 'phase': 'prospective',
                          'kind': kind, 'inputs': inputs}}


def _snapshot(captured, hypotheses, *, context, as_of):
    _, A, _, _ = _modules()
    adapted = A.from_store_capture(captured)
    return adapted.snapshot(context=context, hypotheses=hypotheses, as_of=as_of)


def merged_snapshot(records):
    """Build a supplied Snapshot from the union of existing immutable history."""
    C, _, H, _ = _modules()

    validated = {name: _validated_record(records[name]) for name in
                 ('ancestor', 'main', 'working')}
    captures = {name: value[0] for name, value in validated.items()}
    observations = {name: value[1] for name, value in validated.items()}
    if len({observation.to_data()['as_of'] for observation in observations.values()}) != 1:
        raise ValueError('history inputs have incompatible temporal bases')
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
    return _snapshot(virtual, hypotheses,
                     context=_context(records, captures, observations, 'history_union'),
                     as_of=observations['main'].to_data()['as_of'])


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
    before_hypotheses = {item['name']: item for item in records['ancestor'].get('hypotheses', [])}
    after_hypotheses = {item['name']: item for item in records['working'].get('hypotheses', [])}
    for name in set(before_hypotheses) | set(after_hypotheses):
        if before_hypotheses.get(name) != after_hypotheses.get(name):
            changed.update(_entries(before_hypotheses.get(name, {}).get('doc', {})))
            changed.update(_entries(after_hypotheses.get(name, {}).get('doc', {})))
    return sorted(changed)


def _entries(document):
    from . import provenance as P
    return P._peer('pending_grounding').entries(document)


def _project(snapshot):
    from . import provenance as P
    CapturedAssessment = P._peer('reasoning.context').CapturedAssessment
    operations = P._peer('reasoning.operations')
    return operations.findings(CapturedAssessment.from_snapshot(snapshot))


def _finding(kind, subject, reason):
    value = {'kind': kind, 'id': subject, 'reason': reason}
    value['fingerprint'] = _identity(value)
    return value


def _disposition_findings(snapshot, baseline):
    """Expose newly contested immutable subjects omitted from the scalar view."""
    current = snapshot.to_data()['context']['history']['subjects']
    prior = baseline.to_data()['context']['history']['subjects']
    result = []
    for subject, state in sorted(current.items()):
        if state['acceptance'] == 'contested' and prior.get(subject, {}).get('acceptance') != 'contested':
            result.append(_finding('collision', subject,
                                   subject + ': contested immutable history heads'))
    return result


def _shared_snapshot(record):
    from . import provenance as P
    if record.get('history'):
        return _validated_record(record)[1]
    Snapshot = P._peer('reasoning.snapshot').Snapshot
    if isinstance(record.get('snapshot'), dict):
        observed = Snapshot.from_snapshot(record['snapshot'])
    elif record.get('core', {}).get('snapshot') is not None:
        observed = Snapshot.from_json(record['core']['snapshot'])
    else:
        raise ValueError('shared facts need a captured core/v1 interpretation')
    data = observed.to_data()
    if _identity(data['document']) != _identity(record.get('doc')):
        raise ValueError('shared snapshot document mismatch')
    if 'watch_capture' in data['context'] and data['context']['watch_capture'] != {'hash': record.get('hash')}:
        raise ValueError('shared snapshot capture mismatch')
    return observed


def _scenario_findings(snapshot, records):
    """Assess authored hypothetical deltas, without simulating acceptance acts."""
    from . import provenance as P
    scenario = P._peer('reasoning.scenario')
    current = snapshot.to_data()['hypotheses']
    shared = records.get('shared')
    if not current and not shared:
        return [], []
    old = {item['name']: item for item in records['ancestor'].get('hypotheses', [])}
    selection = {}
    for name in sorted(current):
        # An unchanged alternative supplies its head, not stale inherited values
        # that could conceal a newer main reading from that condition.
        before = _entries(old.get(name, {}).get('doc', {}))
        after = _entries(current[name]['document'])
        selection[name] = sorted(identifier for identifier in after
                                 if before.get(identifier) != after[identifier])
    observed_shared = _shared_snapshot(shared) if shared else None
    candidate = scenario.build(snapshot, selection, shared=observed_shared)
    assessed = scenario.assess(candidate)
    if any(selection.values()) or shared:
        baseline = scenario.assess(scenario.build(snapshot, {name: [] for name in selection}))
    else:
        baseline = assessed
    findings = []

    def finding(kind, identifier, reason, **details):
        item = _finding(kind, identifier, reason)
        item.update(perspective='scenario', source_snapshot_id=assessed['source_snapshot_id'],
                    scenario_id=assessed['scenario_id'], **details)
        item['fingerprint'] = _identity({key: value for key, value in item.items() if key != 'fingerprint'})
        findings.append(item)

    for identifier in assessed['collisions']:
        finding('collision', identifier, identifier + ': captured hypothetical/shared bodies disagree')
    for kind, key in (('falsified', 'falsified'), ('uncheckable', 'holes'), ('uncheckable', 'moved')):
        for line in sorted(set(assessed['findings'][key]) - set(baseline['findings'][key])):
            finding(kind, line.split(':', 1)[0], line)
    for head in assessed['heads']:
        if head['truth'] is True:
            finding('falsified', head['name'], head['name'] + ': hypothetical wrong_if holds under core/v1',
                    computation=head['computation'])
        elif head['truth'] is None:
            finding('uncheckable', head['name'], head['name'] + ': hypothetical condition is unavailable',
                    computation=head['computation'])
    return findings, [assessed]


def _main_snapshot(records):
    capture, observed = _validated_record(records['main'])
    return _snapshot(capture, _physical(records['main']),
                     context={'read_mode': 'supplied',
                              'operation': {'version': 1, 'phase': 'observed',
                                            'kind': 'history_watch_main',
                                            'identity': _identity(records['main']['history']['sha256']),
                                            'baseline': copy.deepcopy(capture.baseline)}},
                     as_of=observed.to_data()['as_of'])


def compare(snapshot):
    """Compare captured active-history records without source reads or writes."""
    try:
        candidate = merged_snapshot(snapshot)
        main_snapshot = _main_snapshot(snapshot)
        projected = _project(candidate)
        baseline = _project(main_snapshot)
        scenario_findings, scenarios = _scenario_findings(candidate, snapshot)
        findings = _disposition_findings(candidate, main_snapshot) + scenario_findings
        for kind, lines, prior in (('falsified', projected['falsified'], baseline['falsified']),
                                   ('uncheckable', projected['holes'], baseline['holes']),
                                   ('uncheckable', projected['moved'], baseline['moved'])):
            for line in sorted(set(lines) - set(prior)):
                findings.append(_finding(kind, line.split(':', 1)[0], line))
        return {'state': 'attention' if findings else 'clear', 'findings': findings,
                'changed': _changed(snapshot), 'versions': snapshot.get('versions'),
                'identity': snapshot.get('identity'), 'scenarios': scenarios}
    except (ValueError, SystemExit) as error:
        finding = {'kind': 'uncheckable', 'id': 'record', 'reason': str(error)}
        finding['fingerprint'] = _identity(finding)
        return {'state': 'attention', 'findings': [finding], 'changed': [],
                'versions': snapshot.get('versions'), 'identity': snapshot.get('identity')}
