"""Validated computational what-ifs over retained observations.

A scenario makes no history admission, acceptance, or support claim.  Its entire
source observation is retained and replay derives the overlay again.  The normal
history Snapshot still binds its generated document to committed authority.
"""
import copy

from .contract import MAX_REQUEST_BYTES, OutputBudget, _check_finite, capabilities, digest


KIND = 'computational-scenario/v1'


def _require(condition, message):
    if not condition:
        raise ValueError('invalid_computational_scenario: ' + message)


def _source(value):
    from .snapshot import Snapshot
    _check_finite(value)
    _require(isinstance(value, dict) and isinstance(value.get('context'), dict), 'missing source')
    _require('scenario' not in value['context'], 'nested scenarios are not supported')
    return Snapshot.from_snapshot(value)


def _entries(document):
    from ..pending_grounding import entries
    return entries(document)


def _legacy_literals(document, label):
    from .snapshot import _fields
    semantic_fields = set(_fields(document).values()) | {
        'rests_on', 'seen', 'wrong_if', 'verdict', 'rule', 'blocked_on', 'reopened_by'}
    for identifier, (_, body) in _entries(document).items():
        if isinstance(body, dict):
            literal_or_source = any(key in body for key in ('v', 'quoted', 'asked', 'url', 'file', 'read'))
            _require(literal_or_source and not semantic_fields.intersection(body),
                     label + ' is not a literal reading or source: ' + identifier)
        else:
            _require(body is None or type(body) in (str, bool, int, float),
                     label + ' is not a literal reading: ' + identifier)


def _derive(source, selection, shared):
    from .. import provenance as P
    from . import authoring
    from .snapshot import _fields
    data = source.to_data()
    _require(capabilities(data['document'])['profile'] == 'core/v1', 'source profile is not core/v1')
    if 'history' in data['context']:
        history = data['context']['history']
        _require(history['coverage']['complete'] and history['integrity']['complete'],
                 'source history is incomplete')
    _require(isinstance(selection, dict), 'selection must name captured hypotheses')
    document = P.Record(copy.deepcopy(data['document']))
    document.hypotheses = {}
    # This is the existing authored computational projection, not a generated
    # history view. The original declaration/closure remains in source below.
    document.get('meta', {}).pop('history', None)
    known = _entries(document)
    overlays, chosen, collisions, heads = [], {}, [], {}
    required = set(capabilities(document)['requires'])
    base_fields = _fields(document)

    def overlay(origin, identifier, collection, body, versions=()):
        value = [collection, body]
        if identifier in chosen and digest(chosen[identifier]) != digest(value):
            collisions.append(identifier)
            return
        chosen[identifier] = copy.deepcopy(value)
        overlays.append({'origin': origin, 'id': identifier, 'digest': digest(value),
                         'versions': sorted(versions)})

    if shared is not None:
        other = shared.to_data()
        shared_profile = capabilities(other['document'])['profile']
        if shared_profile != 'core/v1':
            # Literal legacy readings have the same authored value in this
            # what-if. Executable or ambiguous legacy meaning cannot silently
            # acquire the core interpretation of the receiving computation.
            _require(not authoring._promotion_blockers(P, other['document'], context=document),
                     'shared legacy executable fields need an explicit core/v1 interpretation')
            _legacy_literals(other['document'], 'shared legacy entry')
        else:
            _require(_fields(other['document']) == base_fields, 'incompatible shared field roles')
        _require(other['as_of'] == data['as_of'], 'shared temporal basis differs')
        if 'history' in other['context']:
            history = other['context']['history']
            _require(history['coverage']['complete'] and history['integrity']['complete'],
                     'shared history is incomplete')
        if shared_profile == 'core/v1':
            required.update(capabilities(other['document'])['requires'])
        for identifier, (collection, body) in sorted(_entries(other['document']).items()):
            if identifier in known:
                if digest(list(known[identifier])) != digest([collection, body]):
                    collisions.append(identifier)
                continue
            overlay('shared', identifier, collection, body)

    for name, identifiers in sorted(selection.items()):
        _require(name in data['hypotheses'], 'unknown hypothesis ' + str(name))
        _require(isinstance(identifiers, list) and all(isinstance(item, str) for item in identifiers)
                 and identifiers == sorted(set(identifiers)), 'noncanonical selection')
        hypothesis = data['hypotheses'][name]
        _require(not hypothesis['error'], 'unreadable hypothesis ' + name)
        _require(hypothesis.get('kind') != 'contribution', 'pending contribution is not a selected hypothesis')
        source_entries = _entries(hypothesis['document'])
        _require(set(identifiers) <= set(source_entries), 'selection outside hypothesis ' + name)
        declared = hypothesis['document'].get('meta', {}).get('reasoning')
        if declared is None:
            _require(not authoring._promotion_blockers(P, hypothesis['document'], context=document)
                     and not hypothesis['head'].get('wrong_if'),
                     'legacy hypothesis needs an explicit core/v1 interpretation: ' + name)
            _legacy_literals(hypothesis['document'], 'legacy hypothesis entry')
        else:
            _require(capabilities(hypothesis['document'])['profile'] == 'core/v1',
                     'incompatible hypothesis profile ' + name)
            required.update(capabilities(hypothesis['document'])['requires'])
        schema = hypothesis['document'].get('schema', {})
        _require(isinstance(schema, dict) and all(role not in base_fields or base_fields[role] == field
                     for role, field in schema.items()), 'incompatible hypothesis field roles ' + name)
        heads[name] = copy.deepcopy(hypothesis['head'])
        versions = data['context'].get('history_hypotheses', {}).get('groups', {}).get(name, {})
        for identifier in identifiers:
            collection, body = source_entries[identifier]
            if isinstance(body, dict):
                predicate_field = base_fields['predicate']
                _require(not any(isinstance(body.get(field), str) and body[field]
                                 for field in ('rule', predicate_field)),
                         'uninterpreted hypothetical expression ' + identifier)
            overlay(name, identifier, collection, body, versions.get(identifier, ()))

    # Conflicting selections never win by dictionary iteration order.  They stay
    # on the observed baseline, with an explicit collision in the scenario.
    for identifier, (collection, body) in sorted(chosen.items()):
        if identifier not in collisions:
            document = P.layered(document, {'doc': {collection: {identifier: body}}})
    document = authoring.declare_document(document)
    document['meta']['reasoning']['requires'] = sorted(
        required | set(document['meta']['reasoning']['requires']))
    return dict(document), overlays, sorted(set(collisions)), heads


def build(source, selection, *, shared=None):
    """Select only named captured bodies; callers cannot supply replacement text."""
    from .snapshot import Snapshot
    source = _source(source.to_data())
    shared = _source(shared.to_data()) if shared is not None else None
    document, overlays, collisions, heads = _derive(source, selection, shared)
    evidence = {'version': 1, 'kind': KIND, 'source': source.to_data(),
                'source_snapshot_id': source.snapshot_id,
                'selection': copy.deepcopy(selection),
                'shared': shared.to_data() if shared is not None else None,
                'shared_snapshot_id': shared.snapshot_id if shared is not None else None,
                'overlays': overlays, 'collisions': collisions, 'heads': heads}
    OutputBudget(MAX_REQUEST_BYTES, 'scenario_limit').add(evidence)
    result = Snapshot.from_data(document, context={'read_mode': 'supplied', 'scenario': evidence},
                                as_of=source.to_data()['as_of'])
    result.to_json()  # The complete outer envelope, including repeated node bodies, must fit.
    return result


def validate(document, context, hypotheses, as_of, *, snapshot_data=None):
    """Replay the exact derivation, including full source identity and provenance."""
    evidence = context['scenario']
    _check_finite({'document': document, 'context': context, 'hypotheses': hypotheses})
    OutputBudget(MAX_REQUEST_BYTES, 'scenario_limit').add(evidence)
    if snapshot_data is not None:
        from ..pending_grounding import _encode
        _check_finite(snapshot_data)
        OutputBudget(MAX_REQUEST_BYTES, 'scenario_limit').add(_encode(snapshot_data))
    expected = {'version', 'kind', 'source', 'source_snapshot_id', 'selection', 'shared',
                'shared_snapshot_id', 'overlays', 'collisions', 'heads'}
    _require(isinstance(evidence, dict) and set(evidence) == expected
             and type(evidence['version']) is int and evidence['version'] == 1
             and evidence['kind'] == KIND, 'invalid evidence envelope')
    _require(context == {'read_mode': 'supplied', 'scenario': evidence} and not hypotheses,
             'scenario has independent authority or hypotheses')
    source = _source(evidence['source'])
    shared = _source(evidence['shared']) if evidence['shared'] is not None else None
    _require(source.snapshot_id == evidence['source_snapshot_id'] and
             (shared.snapshot_id if shared is not None else None) == evidence['shared_snapshot_id'],
             'source identity mismatch')
    _require(as_of == source.to_data()['as_of'], 'temporal basis changed')
    derived, overlays, collisions, heads = _derive(source, evidence['selection'], shared)
    _require(digest(document) == digest(derived) and evidence['overlays'] == overlays
             and evidence['collisions'] == collisions and evidence['heads'] == heads,
             'derived computation differs from retained sources')


def assess(snapshot):
    """Only computational dimensions escape this private what-if boundary."""
    from .snapshot import Snapshot
    from .context import CapturedAssessment
    from . import operations
    from .. import provenance as P
    snapshot = Snapshot.from_snapshot(snapshot.to_data())
    data = snapshot.to_data()
    _require('scenario' in data['context'], 'scenario snapshot required')
    report = CapturedAssessment.from_snapshot(snapshot)
    # v2 computational findings deliberately have no history acceptance/support
    # claims. Keep only the corresponding operation projection dimensions.
    projected = operations.findings(report)
    findings = {key: projected[key] for key in ('falsified', 'holes', 'moved')}
    document = operations.bind(P.Record(data['document']), snapshot)
    heads = []
    for name, head in sorted(data['context']['scenario']['heads'].items()):
        expression = head.get('wrong_if')
        if expression is not None:
            truth, result = operations.condition(document, expression)
            heads.append({'name': name, 'truth': truth, 'computation': result})
    return {'findings': findings, 'heads': heads,
            'collisions': data['context']['scenario']['collisions'],
            'source_snapshot_id': data['context']['scenario']['source_snapshot_id'],
            'scenario_id': snapshot.snapshot_id, 'snapshot': snapshot.to_json()}
