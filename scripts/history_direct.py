"""Routed direct history writes retain one private exact retry envelope."""
import copy
import datetime
import json
import os
from pathlib import Path
import uuid

from . import history_authoring as A, history_adapter, history_contract as C
from . import history_store as H, history_transaction as T, provenance as P, recording
from .pending_grounding import identity


def _intent(action):
    value = copy.deepcopy(action)
    value['evidence'] = {name: C.sha256(raw) for name, raw in (value.get('evidence') or {}).items()}
    return identity(value)


def route(paths, action, *, project, original_paths, expected_policy):
    """Prepare a scoped Advanced contribution while leaving checkout authority intact."""
    scope = action.get('scope', 'unclear')
    scope = copy.deepcopy(scope) if isinstance(scope, dict) else {
        'kind': scope, 'environment': action.get('environment', '')}
    if action.get('commit'):
        scope['commit'] = action['commit']
    if expected_policy['mode'] != 'advanced' or scope.get('kind') not in ('project', 'external'):
        return None
    C._require(not action.get('hypothesis'), 'hypothesis_scope_mismatch',
               'named hypotheses stay local; select feature scope')
    from . import history_bundle as B, pending_grounding as G
    event = action.get('event_id') or uuid.uuid4().hex
    C._require(isinstance(event, str) and G.TOKEN.fullmatch(event), 'invalid_event_id')
    contribution = action.get('contribution_id') or action['id']
    entry = Path(paths[0]).resolve()
    retained = project.state / 'history-contributions' / (event + '.yaml')
    intent_digest = _intent(action)
    with P._locked(str(entry), project=project):
        C._require(project.config() == expected_policy, 'history_routing_changed')
        if retained.exists():
            envelope = C.decode_document(retained.read_bytes())
            C._mapping(envelope, ('version', 'intent_digest', 'event', 'contribution', 'routing',
                                 'inventory', 'manifest', 'files', 'revision'), ('evidence_reads',))
            C._require(envelope['version'] == 1 and envelope['intent_digest'] == intent_digest
                       and envelope['event'] == event and envelope['contribution'] == contribution,
                       'event_identity_mismatch')
        else:
            captured = H.Store(entry).capture()
            document = P.Record(history_adapter.from_store_capture(captured).document)
            document.get('meta', {}).pop('history', None)
            document.hypotheses = P.load_hypotheses(paths)
            private = recording.private_route(paths, action, A.READER, project, document=document)
            if private is not None:
                return private
            if action.get('shareability') != 'project':
                return recording.draft(project, action, document, 'sharing permission is private or unclear')
            candidate_action = copy.deepcopy(action)
            candidate_action['_record_scope'] = scope
            if action['kind'] == 'add':
                C._require(isinstance(candidate_action['body'], dict), 'scoped_body_required')
                C._require(identity(candidate_action['body'].get('scope', scope)) == identity(scope), 'scope_mismatch')
                candidate_action['body']['scope'] = scope
            mutation = A.prepare(entry, candidate_action, operation='contribution-' + event, capture=captured)
            artifact = B.prepare_subset(captured, [action['id']], scope=scope, shareability='project',
                operation='capture-' + event, recorded_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                source_entry=entry.name, disclosed_locators=action.get('disclosed_locators', ()), prepared=mutation)
            scoped = B.adapt(artifact).document
            evidence = copy.deepcopy(action.get('evidence') or {})
            evidence_reads = {}
            if action.get('evidence_root'):
                evidence_root = Path(action['evidence_root']).expanduser().resolve()
                for name in G._files(scoped):
                    G.M.relative_path(name)
                    member = (evidence_root / name).resolve()
                    member.relative_to(evidence_root)
                    evidence[name] = member.read_bytes()
                    evidence_reads[str(member)] = C.sha256(evidence[name])
            bundle = G.prepare(scoped, [action['id']], scope=scope, shareability='project',
                               evidence=evidence, history=artifact)
            envelope = {'version': 1, 'intent_digest': intent_digest, 'event': event,
                'contribution': contribution, 'routing': _routing(original_paths, paths, project),
                'inventory': [{'kind': kind, 'path': path, 'value': value}
                              for (kind, path), value in sorted(captured.inventory.items())],
                'manifest': bundle['manifest'], 'revision': bundle['revision'],
                'evidence_reads': evidence_reads,
                'files': {name: T._blob(raw) for name, raw in bundle['files'].items()}}
            T.publish_immutable(retained, C.encode_document(envelope), root=project.state)
    bundle = {'manifest': envelope['manifest'], 'revision': envelope['revision'],
              'files': {name: T._unblob(raw) for name, raw in envelope['files'].items()}}
    prior = G.Store(project).receipt(event)
    if prior is not None:
        C._require(prior['revision'] == bundle['revision'] and prior['contribution_id'] == contribution,
                   'event_identity_mismatch')
        return prior
    def verify_source():
        _verify_routing(envelope['routing'], original_paths, paths, project)
        current = H.Store(entry).capture()
        observed = [{'kind': kind, 'path': path, 'value': value}
                    for (kind, path), value in sorted(current.inventory.items())]
        C._require(identity(observed) == identity(envelope['inventory']), 'snapshot_changed')
        for path, expected in envelope.get('evidence_reads', {}).items():
            member = Path(path)
            C._require(member.is_file() and C.sha256(member.read_bytes()) == expected,
                       'source_evidence_changed', path)
    return G.Store(project).capture(bundle, event_id=event, contribution_id=contribution,
        shareability='project', expected_policy=expected_policy,
        expected_generation=expected_policy['generation'], verify_source=verify_source)


def active(paths):
    if len(paths) != 1:
        return False
    path = Path(P.layout(paths[0])['history_authority'])
    if not path.exists():
        return False
    raw = path.read_bytes()
    try:
        return C.validate_authority(C.decode_document(raw))['authority'] == 'history'
    except C.HistoryError as error:
        raise P.Refused(error.code + ': ' + str(error)) from None


def journal(entry):
    # The committed manifest is authority while a view lags. A prepared history
    # operation must not trigger the legacy multi-image incomplete-state guard.
    return T.journal_for(entry) + '.history'


def _routing(original_paths, paths, project):
    return {'paths': list(map(os.path.realpath, original_paths)),
            'destination': list(map(os.path.realpath, paths)), 'policy': project.config()}


def _verify_routing(routing, original_paths, paths, project):
    current = P._peer('knowledge_views').write_paths(original_paths)
    C._require(identity(routing) == identity(_routing(original_paths, current, project))
               and list(map(os.path.realpath, paths)) == list(map(os.path.realpath, current)),
               'history_routing_changed')


def _envelope(mutation, routing):
    value = {'version': 1, 'kind': 'direct-history/v1', 'routing': routing,
             'mutation': T._blob(mutation.to_bytes())}
    return {**value, 'digest': identity(value)}


def _read_envelope(raw):
    value = C.decode_document(raw)
    C._mapping(value, ('version', 'kind', 'routing', 'mutation', 'digest'))
    C._require(type(value['version']) is int and value['version'] == 1
               and value['kind'] == 'direct-history/v1', 'invalid_history_journal')
    mutation = T.PreparedMutation.from_bytes(T._unblob(value['mutation']))
    C._require(identity(value) == identity(_envelope(mutation, value['routing'])),
               'invalid_history_journal')
    return mutation, value['routing']


def _writer(mutation):
    before = mutation.to_data()['receipt']['before']
    if 'history_edit' in before:
        return P._peer('history_edits')
    if 'hypothesis_authoring' in before:
        return P._peer('history_hypotheses')
    return A


def recover_report(paths, *, direction='after'):
    """Dispatch before taking a direct lock; report recovery owns its event lock."""
    if len(paths) != 1:
        return None
    entry = Path(paths[0]).resolve()
    pending = T._target(entry.parent, journal(entry))
    if not pending.is_file():
        return None
    value = C.decode_document(pending.read_bytes())
    if value.get('kind') != 'report-history/v1':
        return None
    C._mapping(value, ('version', 'kind', 'record', 'state_dir', 'event_id', 'mutation', 'digest'))
    C._require(type(value['version']) is int and value['version'] == 1
               and value['record'] == str(entry)
               and isinstance(value['state_dir'], str) and Path(value['state_dir']).is_absolute(),
               'invalid_history_journal')
    C._text(value['event_id'])
    C._require(value['digest'] == identity({key: val for key, val in value.items() if key != 'digest'}),
               'invalid_history_journal')
    T.PreparedMutation.from_bytes(T._unblob(value['mutation']))
    return P._peer('ingestion').recover_history(entry, value['state_dir'], value['event_id'], direction=direction)


def apply(paths, action, *, project, original_paths):
    """Caller holds policy plus complete record locks; no caller action is inferred."""
    entry = Path(paths[0]).absolute()
    pending = T._target(entry.parent, journal(entry))
    C._require(not pending.exists(), 'recovery_required', 'run recover for the retained history operation')
    routing = _routing(original_paths, paths, project)
    captured = H.Store(entry).capture()
    document = P.Record(history_adapter.from_store_capture(captured).document)
    document.get('meta', {}).pop('history', None)
    document.hypotheses = P.load_hypotheses(paths)
    action = copy.deepcopy(action)
    explicit_act = action.get('kind') in ('accept', 'refute', 'correct', 'propose', 'retire')
    if explicit_act:
        target = captured.objects.get(action.get('of'))
        if target is not None and recording.private_marker(target):
            result = recording.draft(project, action, {'target': target}, 'private historical claim')
            print(json.dumps(result, ensure_ascii=False))
            return 0
    explicit = recording.ROUTING.intersection(action)
    scope = action.get('scope', 'unclear')
    scope_kind = scope.get('kind') if isinstance(scope, dict) else scope
    # The legacy router cannot publish a history operation as a YAML-only bundle.
    # Local code/feature actions and Simple writes retain its privacy policy.
    if explicit and project.config()['mode'] == 'advanced' and scope_kind in ('project', 'external'):
        raise P.Refused('history_contribution_write_pending: use an explicit complete history contribution')
    routed = recording.route(paths, action, A.READER, project=project,
                             expected_policy=routing['policy'], document=document)
    if routed is not None:
        print(json.dumps(routed, ensure_ascii=False))
        return 0
    if action.get('hypothesis'):
        mutation = P._peer('history_hypotheses').prepare(entry, action['hypothesis'], action, capture=captured)
    else:
        mutation = (A.prepare_act if explicit_act else A.prepare)(entry, action, capture=captured)
    authored_objects = [C.decode_document(item['after']) for item in mutation.files
                        if item['role'] == 'history_object']
    if recording.private_marker(authored_objects):
        result = recording.draft(project, action, {'history': authored_objects}, 'private historical proposal')
        print(json.dumps(result, ensure_ascii=False))
        return 0
    _verify_routing(routing, original_paths, paths, project)
    T.publish_immutable(pending.parent / '.gitignore', b'*\n', root=entry.parent)
    T.publish_immutable(pending, C.encode_document(_envelope(mutation, routing)), root=entry.parent)
    _writer(mutation).commit(entry, mutation, verify=lambda data: _verify_routing(routing, original_paths, paths, project))
    pending.unlink()
    T._sync(pending.parent)
    P.forget(entry)
    print('history committed: ' + mutation.to_data()['operation'] + ' (' + action['kind'] + ' ' + action['id'] + ')')
    return 0


def recover(paths, *, project, original_paths, direction='after'):
    entry = Path(paths[0]).absolute()
    pending = T._target(entry.parent, journal(entry))
    C._require(pending.is_file(), 'no_recovery_pending')
    raw = pending.read_bytes()
    kind = C.decode_document(raw).get('kind')
    if kind == 'history-adoption/v1':
        return _recover_adoption(entry, raw, project, original_paths, paths, direction)
    mutation, routing = _read_envelope(raw)
    _verify_routing(routing, original_paths, paths, project)
    C._require(pending.read_bytes() == raw, 'concurrent_edit')
    if direction == 'before':
        live = H.Store(entry).capture()
        C._require(mutation.to_data()['operation'] not in live.commits,
                   'history_already_committed', 'committed evidence requires an explicit new act')
        _writer(mutation).verify_prepared(entry, mutation)
    else:
        _writer(mutation).commit(entry, mutation, verify=lambda data: _verify_routing(routing, original_paths, paths, project))
    pending.unlink()
    T._sync(pending.parent)
    P.forget(entry)
    return mutation


def record_proposals(paths, subjects, *, because, by=None, project, original_paths):
    """Explicitly retain all selected edited bodies without accepting them."""
    entry = Path(paths[0]).resolve()
    pending = T._target(entry.parent, journal(entry))
    with P._locked(str(entry), project=project):
        C._require(not pending.exists(), 'recovery_required')
        routing = _routing(original_paths, paths, project)
        module = P._peer('history_edits')
        mutation = module.prepare_proposals(entry, subjects=subjects, because=because, by=by,
            operation='view-edit-' + uuid.uuid4().hex,
            recorded_at=datetime.datetime.now(datetime.timezone.utc).isoformat())
        for item in mutation.files:
            if item['role'] == 'history_object':
                obj = C.decode_document(item['after'])
                C._require(not recording.private_marker(obj), 'private_proposal_requires_draft')
        T.publish_immutable(pending.parent / '.gitignore', b'*\n', root=entry.parent)
        T.publish_immutable(pending, C.encode_document(_envelope(mutation, routing)), root=entry.parent)
        module.commit(entry, mutation, verify=lambda data: _verify_routing(routing, original_paths, paths, project))
        pending.unlink()
        T._sync(pending.parent)
        P.forget(entry)
        return {'state': 'proposed', 'operation': mutation.to_data()['operation'], 'subjects': subjects}


def finish_hypotheses(paths, names, *, kind, because, take=(), drops=None, by=None, dry=False):
    """Explicit consolidation has one immutable operation and the shared retry journal."""
    original_paths = list(paths)
    V = P._peer('knowledge_views')
    project = V.project_for(paths)
    policy = project.config()
    paths = V.write_paths(paths)
    entry = Path(paths[0]).resolve()
    pending = T._target(entry.parent, journal(entry))
    module = P._peer('history_hypotheses')
    with P._locked(str(entry), project=project):
        C._require(project.config() == policy, 'history_routing_changed')
        C._require(not pending.exists(), 'recovery_required')
        capture = H.Store(entry).capture()
        if not names:
            adapted = history_adapter.from_store_capture(capture)
            groups, _ = module.layers(adapted.projection, adapted.document)
            names = sorted(groups)
        if not names:
            return {'state': 'unchanged', 'hypotheses': []}
        options = {'because': because, 'by': by, 'capture': capture}
        if kind == 'fold':
            mutation = module.prepare_fold(entry, names, take=take, drops=drops, **options)
        else:
            mutation = module.prepare_refute(entry, names, **options)
        if dry:
            return {'state': 'prepared', 'hypotheses': names, 'action': kind}
        routing = _routing(original_paths, paths, project)
        T.publish_immutable(pending.parent / '.gitignore', b'*\n', root=entry.parent)
        T.publish_immutable(pending, C.encode_document(_envelope(mutation, routing)), root=entry.parent)
        module.commit(entry, mutation, verify=lambda data: _verify_routing(routing, original_paths, paths, project))
        pending.unlink()
        T._sync(pending.parent)
        P.forget(entry)
        return {'state': 'folded' if kind == 'fold' else 'refuted', 'hypotheses': names,
                'operation': mutation.to_data()['operation']}


def adopt(paths, revision, choices, *, by=None, project, original_paths):
    """Adopt one retained scoped contribution using explicitly selected overlaps."""
    from . import history_bundle as B, pending_grounding as G
    bundle = G.Store(project).snapshot()['bundles'].get(revision)
    C._require(bundle is not None, 'unknown_contribution')
    artifact = B.from_contribution(bundle)
    entry = Path(paths[0]).resolve()
    pending = T._target(entry.parent, journal(entry))
    with P._locked(str(entry), project=project):
        C._require(not pending.exists(), 'recovery_required')
        routing = _routing(original_paths, paths, project)
        operation = 'adopt-' + uuid.uuid4().hex
        mutation = B.prepare_adoption(entry, artifact, choices=choices, by=by, operation=operation,
            recorded_at=datetime.datetime.now(datetime.timezone.utc).isoformat())
        value = {'version': 1, 'kind': 'history-adoption/v1', 'routing': routing,
            'mutation': T._blob(mutation.to_bytes()),
            'artifact': {**artifact, 'files': {name: T._blob(raw) for name, raw in artifact['files'].items()}}}
        envelope = {**value, 'digest': identity(value)}
        T.publish_immutable(pending.parent / '.gitignore', b'*\n', root=entry.parent)
        T.publish_immutable(pending, C.encode_document(envelope), root=entry.parent)
        B.commit_adoption(entry, mutation, artifact,
                          verify=lambda data: _verify_routing(routing, original_paths, paths, project))
        pending.unlink()
        T._sync(pending.parent)
        P.forget(entry)
    return {'state': 'adopted', 'revision': revision, 'operation': operation}


def _recover_adoption(entry, raw, project, original_paths, paths, direction):
    from . import history_bundle as B
    envelope = C.decode_document(raw)
    C._mapping(envelope, ('version', 'kind', 'routing', 'mutation', 'artifact', 'digest'))
    C._require(envelope['version'] == 1 and envelope['digest'] == identity(
        {key: value for key, value in envelope.items() if key != 'digest'}), 'invalid_history_journal')
    mutation = T.PreparedMutation.from_bytes(T._unblob(envelope['mutation']))
    artifact = {**envelope['artifact'], 'files': {
        name: T._unblob(data) for name, data in envelope['artifact']['files'].items()}}
    B.validate(artifact)
    _verify_routing(envelope['routing'], original_paths, paths, project)
    if direction == 'before':
        C._require(mutation.to_data()['operation'] not in H.Store(entry).capture().commits,
                   'history_already_committed')
        B.verify_adoption(entry, mutation, artifact)
    else:
        B.commit_adoption(entry, mutation, artifact, verify=lambda data:
                          _verify_routing(envelope['routing'], original_paths, paths, project))
    pending = T._target(entry.parent, journal(entry))
    C._require(pending.read_bytes() == raw, 'concurrent_edit')
    pending.unlink()
    T._sync(pending.parent)
    P.forget(entry)
    return mutation
