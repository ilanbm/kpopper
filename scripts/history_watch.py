"""Pure merge of captured active-history closures for watch."""
import copy
from dataclasses import replace


def _identity(value):
    from .pending_grounding import identity
    return identity(value)


def _capture(evidence):
    from . import knowledge_views
    return knowledge_views.validate_history_evidence(copy.deepcopy(evidence))


def _same_bytes(left, right, label):
    if left != right:
        raise ValueError('history collision: ' + label)


def merged_snapshot(records):
    """Build a supplied Snapshot from the union of existing immutable history."""
    from . import history_contract as C
    from . import history_adapter as A
    from . import history_store as H

    ancestor, main, working = (_capture(records[name]['history']) for name in
                               ('ancestor', 'main', 'working'))
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
    selected = C.committed_objects(main.marker, commits, raw_objects)
    state = H.reduce(selected, main.state['rules'])
    baseline = H.baseline(main.marker, commits, state)
    virtual = replace(main, entry_bytes=b'', document={}, commits=commits,
                      objects=selected, object_bytes=raw_objects, state=state,
                      baseline=baseline)
    store = H.Store.__new__(H.Store)
    rendered = store.render(virtual, objects=selected, commits=commits)
    virtual = replace(virtual, entry_bytes=rendered, document=C.decode_document(rendered))
    adapted = A.from_store_capture(virtual)
    hypotheses = {}
    # Working is the authored side of the delta; retain main hypotheses when
    # the working capture has no physical copy of a named layer.
    for item in records['main'].get('hypotheses', []) + records['working'].get('hypotheses', []):
        hypotheses[item['name']] = {'document': copy.deepcopy(item['doc']),
                                    'head': copy.deepcopy(item['head'])}
    as_of = records['main'].get('core', {}).get('assessment', {}).get('as_of')
    snapshot = adapted.snapshot(context={
        'read_mode': 'supplied',
        'operation': {'version': 1, 'phase': 'prospective',
                      'kind': 'history_union',
                      'ancestor': _identity(records['ancestor']['history']['sha256']),
                      'main': _identity(records['main']['history']['sha256']),
                      'working': _identity(records['working']['history']['sha256'])}},
        hypotheses=hypotheses, as_of=as_of)
    return snapshot


def compare(snapshot):
    """Compare captured active-history records without source reads or writes."""
    from .reasoning.context import CapturedAssessment
    from .reasoning import operations
    try:
        candidate = merged_snapshot(snapshot)
        context = CapturedAssessment.from_snapshot(candidate)
        projected = operations.findings(context)
        findings = []
        for kind, lines in (('falsified', projected['falsified']),
                            ('uncheckable', projected['holes']),
                            ('uncheckable', projected['moved'])):
            for line in lines:
                finding = {'kind': kind, 'id': line.split(':', 1)[0], 'reason': line}
                finding['fingerprint'] = _identity(finding)
                findings.append(finding)
        return {'state': 'attention' if findings else 'clear', 'findings': findings,
                'changed': [], 'versions': snapshot.get('versions'),
                'identity': snapshot.get('identity')}
    except (ValueError, SystemExit) as error:
        finding = {'kind': 'uncheckable', 'id': 'record', 'reason': str(error)}
        finding['fingerprint'] = _identity(finding)
        return {'state': 'attention', 'findings': [finding], 'changed': [],
                'versions': snapshot.get('versions'), 'identity': snapshot.get('identity')}
