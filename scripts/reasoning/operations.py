"""Captured core findings and existing admission adapters for operational consumers.

An operational candidate is explicitly prospective. History-backed candidates
require their own validated prospective history and never lose their authority
metadata just to make an ordinary document evaluable.
"""
import copy

from . import authoring
from .context import CapturedAssessment
from .contract import PROFILE, capabilities, digest, admitted
from .snapshot import Snapshot, capture_source
from .. import history_authoring, provenance as P


READER_REFUSAL = 'unsupported_capability: use core/v1 consumer'


def selected(document):
    return capabilities(document)['profile'] == PROFILE


def _ordinary(snapshot):
    data = snapshot.to_data()
    if 'history' in data['context'] or 'history' in data['document'].get('meta', {}) \
            or any(h.get('kind') == 'named-history-hypothesis/v1' for h in data['hypotheses'].values()):
        raise ValueError('prospective_history_required: use the prepared history operation')


class World(authoring.World):
    def __init__(self, document, snapshot):
        super().__init__(history_authoring.READER, document, original=snapshot)
        if self.snapshot.snapshot_id != snapshot.snapshot_id:
            raise ValueError('operation_snapshot_mismatch')
        self.context = CapturedAssessment.from_snapshot(snapshot)
        self._assessment = self.context.base_assessment
        self._readings.update({key: copy.deepcopy(node['computation'])
                               for key, node in self._assessment['nodes'].items()
                               if node['computation'] is not None})

    def predicate_for(self, identifier):
        node = self._assessment['nodes'].get(identifier)
        return {'holds': True, 'does_not_hold': False}.get(
            node['state']['falsifier']['status']) if node is not None else None


def bind(document, snapshot):
    document._operation_snapshot = snapshot
    document._operation_world = None
    return document


def load(paths, *, as_of=None, allow_history=False):
    source = capture_source(paths, as_of=as_of)
    document = source.document
    if not selected(document):
        raise ValueError('record_profile_changed: retry the operational read')
    if not allow_history:
        _ordinary(source.snapshot)
    document._operation_source = source
    return bind(document, source.snapshot)


def prepared(base, mutation):
    """Bind one prepared history candidate without rereading or editing its source.

    Retained pending/target observations remain source observations. Only the
    committed history and its named layers advance to the candidate generation.
    This projection grants no publication authority; the caller verifies the
    original source and uses the existing prepared-operation publication gate.
    """
    from .. import history_adapter, history_hypotheses, history_prospective
    prior = snapshot_for(base)
    source = getattr(base, '_operation_source', None)
    captured = source.history_capture if source is not None else None
    if captured is None:
        raise ValueError('prepared_history_capture_required')
    data = prior.to_data()
    if digest(history_adapter.from_store_capture(captured).document) != digest(data['document']):
        raise ValueError('operation_capture_mismatch')
    context = copy.deepcopy(data['context'])
    original_view = context.get('history_view')
    for key in ('history', 'history_hypotheses', 'history_view', 'operation'):
        context.pop(key, None)
    context['operation_source'] = {'snapshot_id': prior.snapshot_id,
        'context_digest': digest(data['context']), 'history_view': original_view}
    remaining = {name: value for name, value in data['hypotheses'].items()
                 if value.get('kind') != history_hypotheses.KIND}
    snapshot = history_prospective.snapshot_after(captured, mutation, context=context,
                                                   hypotheses=remaining, as_of=data['as_of'])
    candidate = snapshot.to_data()
    document = P.Record(candidate['document'])
    document.hypotheses = {name: copy.deepcopy(value) for name, value in base.hypotheses.items()
                          if value.get('kind') != history_hypotheses.KIND}
    named, _ = history_hypotheses.layers(candidate['context']['history'], document)
    document.hypotheses.update(named)
    document._operation_source = source
    return bind(document, snapshot)


def derive(document, base, selection=(), *, proposals=()):
    """Bind a candidate to the original observation without any new source reads."""
    prior = snapshot_for(base)
    _ordinary(prior)
    data = prior.to_data()
    context = copy.deepcopy(data['context'])
    context['read_mode'] = 'supplied'
    context['operation'] = {'version': 1, 'phase': 'prospective',
                            'kind': 'hypothesis_union', 'base_snapshot': prior.snapshot_id,
                            'selection': sorted(set(selection)),
                            'inputs': [{'name': h['name'], 'document': copy.deepcopy(h['doc']),
                                        'head': copy.deepcopy(h['head'])} for h in proposals]}
    remaining = {name: value for name, value in data['hypotheses'].items() if name not in selection}
    if any(data['hypotheses'].get(name, {}).get('kind') == 'contribution' for name in selection):
        raise ValueError('pending_publication_required: a contribution is not an ordinary fold')
    document = authoring.declare_document(document)
    required = set(document['meta']['reasoning']['requires'])
    for hypothesis in proposals:
        if selected(hypothesis['doc']):
            required.update(capabilities(hypothesis['doc'])['requires'])
    document['meta']['reasoning']['requires'] = sorted(required)
    document._operation_source = getattr(base, '_operation_source', None)
    return bind(document, Snapshot.from_data(document, context=context, hypotheses=remaining,
                                             as_of=data['as_of']))


def snapshot_for(document):
    snapshot = getattr(document, '_operation_snapshot', None)
    if snapshot is None:
        snapshot = Snapshot.from_data(document, hypotheses=getattr(document, 'hypotheses', {}))
        bind(document, snapshot)
    if digest(snapshot.to_data()['document']) != digest(dict(document)):
        raise ValueError('operation_document_changed: derive a new prospective snapshot')
    return snapshot


def world(document):
    snapshot = snapshot_for(document)
    result = getattr(document, '_operation_world', None)
    if result is None:
        result = document._operation_world = World(document, snapshot)
    return result


def view(document):
    current = world(document)
    ids, judgments, fields = history_authoring.READER.infer(document)
    return ids, judgments, fields, current.raw


def findings(context):
    """Project operation policy without recomputing predicates or acceptance."""
    report = context.assessment
    result = {'falsified': [], 'holes': [], 'moved': [], 'notes': [], 'uncertain': {}}
    history = report['history']
    if history['authority_status'] == 'active' and (
            not history['coverage']['complete'] or not history['integrity']['complete']):
        result['holes'].append('history: incomplete captured coverage or integrity')
    for identifier, node in sorted(report['nodes'].items()):
        state, body = node['state'], node['body']
        blocked = bool(P._blocked_text(body)) if isinstance(body, dict) else False
        uncertain = []
        missing = {dep for dep, value in state['basis']['dependencies'].items()
                   if value['current']['status'] != 'ok'}
        for issue in state['integrity']['issues']:
            code = issue['code']
            allowed = blocked and (code in ('missing_dependency', 'missing_reference', 'missing_field')
                or code == 'missing_snapshot' and set(issue.get('related_ids', [])) <= missing)
            (result['notes'] if allowed else result['holes']).append(identifier + ': ' + code)
            uncertain.append(code)
        if not node['coverage']['complete']:
            result['holes'].append(identifier + ': incomplete history coverage')
        if state['contention']['status'] == 'detected':
            result['holes'].append(identifier + ': contested captured alternatives')
        condition = state['falsifier']
        if condition['status'] == 'holds':
            result['falsified'].append(identifier + ': wrong_if holds under core/v1')
        elif condition['status'] in ('unknown', 'error'):
            computed = condition.get('computation')
            allowed = (computed is None and not isinstance(condition.get('expression'), dict)
                       or computed is not None and blocked and admitted(computed, blocked=True))
            line = identifier + ': falsifier ' + condition['status']
            (result['notes'] if allowed else result['holes']).append(line)
            uncertain.append('falsifier ' + condition['status'])
        computed = node['computation']
        if computed is not None and computed['status'] not in ('ok', 'unknown'):
            result['holes'].append(identifier + ': computation ' + computed['status'])
        elif computed is not None and computed['status'] == 'unknown' and isinstance(body, dict) \
                and isinstance(body.get('rule'), dict):
            (result['notes'] if blocked and admitted(computed, blocked=True) else result['holes']).append(
                identifier + ': computation unknown')
            uncertain.append('computation unknown')
        for dependency, reading in sorted(state['basis']['dependencies'].items()):
            if reading['comparison'] == 'changed' or reading['rule_changed'] \
                    or reading['basis_comparison'] == 'changed':
                evidence = {key: reading[key] for key in
                            ('current', 'at_review', 'comparison', 'rule_changed', 'basis_comparison')}
                evidence['basis'] = reading['computation']['basis']
                result['moved'].append(identifier + ': ' + dependency + ' value/rule/basis changed ['
                                       + digest(evidence)[:16] + ']')
        if node['support']['status'] == 'reserved':
            result['notes'].append(identifier + ': declared support is reserved')
            uncertain.append('support reserved')
        if uncertain:
            result['uncertain'][identifier] = sorted(set(uncertain))
    for key in ('falsified', 'holes', 'moved', 'notes'):
        result[key] = sorted(set(result[key]))
    return result


def check(document):
    result = findings(world(document).context)
    return result['falsified'] + result['holes'], result['moved']


def condition(document, expression):
    """An operation-head condition, with its full exact result retained."""
    if not isinstance(expression, dict):
        return None, None
    from .language import lower, references
    tree = lower(expression)
    current = world(document)
    result = current.engine.evaluate(tree, declared=references(tree))
    value = result.get('value')
    truth = value['value'] if result['status'] == 'ok' and value \
        and value['type'] == 'boolean' else None
    return truth, result
