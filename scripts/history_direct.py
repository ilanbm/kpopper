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
    if 'identity_authoring' in before:
        return P._peer('history_identity')
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
    # Only the explicitly authored subject belongs to this direct write.
    P._peer('session_activity').published(P, entry.parent, mutation.files, subjects={action['id']})
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
    if kind == 'history-branch-adoption/v1':
        return _recover_branch_adoption(entry, raw, project, original_paths, paths, direction)
    if kind == 'history-branch-adoption-set/v1':
        return _recover_branch_adoption_set(entry, raw, project, original_paths, paths, direction)
    mutation, routing = _read_envelope(raw)
    _verify_routing(routing, original_paths, paths, project)
    C._require(pending.read_bytes() == raw, 'concurrent_edit')
    if direction == 'before':
        if T.auxiliary_view(mutation) is not None:
            H.Store(entry).cancel_auxiliary(mutation, verify=lambda data:
                _verify_routing(routing, original_paths, paths, project))
        else:
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
        source = document = None
        if dry:
            from .reasoning.snapshot import capture_source
            from .reasoning import operations
            source = capture_source(original_paths)
            capture = source.history_capture
            C._require(capture is not None, 'history_authority_changed')
            document = source.document
            document._operation_source = source
            operations.bind(document, source.snapshot)
        else:
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
        assessment = {}
        if document is not None and operations.selected(document):
            candidate = operations.prepared(document, mutation)
            before = operations.findings(operations.world(document).context)
            after = operations.findings(operations.world(candidate).context)
            assessment = {'source_snapshot': source.snapshot.snapshot_id,
                          'candidate_snapshot': operations.snapshot_for(candidate).snapshot_id,
                          'base': before, 'candidate': after}
        if source is not None:
            source.verify()
        if dry:
            return {'state': 'prepared', 'hypotheses': names, 'action': kind,
                    **({'assessment': assessment} if assessment else {})}
        routing = _routing(original_paths, paths, project)
        T.publish_immutable(pending.parent / '.gitignore', b'*\n', root=entry.parent)
        T.publish_immutable(pending, C.encode_document(_envelope(mutation, routing)), root=entry.parent)
        module.commit(entry, mutation, verify=lambda data: _verify_routing(routing, original_paths, paths, project))
        pending.unlink()
        T._sync(pending.parent)
        P.forget(entry)
        return {'state': 'folded' if kind == 'fold' else 'refuted', 'hypotheses': names,
                'operation': mutation.to_data()['operation'],
                **({'assessment': assessment} if assessment else {})}


def identity_write(paths, a, b, *, kind, keep=None, because=None, as_of=None):
    original_paths = list(paths)
    V = P._peer('knowledge_views')
    project = V.project_for(paths)
    policy = project.config()
    paths = V.write_paths(paths)
    entry = Path(paths[0]).resolve()
    pending = T._target(entry.parent, journal(entry))
    module = P._peer('history_identity')
    with P._locked(str(entry), project=project):
        C._require(project.config() == policy, 'history_routing_changed')
        C._require(not pending.exists(), 'recovery_required')
        options = {} if as_of is None else {'as_of': as_of}
        mutation = module.prepare_same(entry, a, b, keep=keep, **options) if kind == 'same' else \
            module.prepare_distinct(entry, a, b, because, **options)
        authored = [C.decode_document(item['after']) for item in mutation.files if item['role'] == 'history_object']
        if recording.private_marker(authored):
            receipt = recording.draft(project, {'kind': kind, 'ids': [a, b]}, {'history': authored},
                                       'private identity change retained for review')
            print(json.dumps(receipt, ensure_ascii=False))
            return 0
        routing = _routing(original_paths, paths, project)
        T.publish_immutable(pending.parent / '.gitignore', b'*\n', root=entry.parent)
        T.publish_immutable(pending, C.encode_document(_envelope(mutation, routing)), root=entry.parent)
        module.commit(entry, mutation, verify=lambda data: _verify_routing(routing, original_paths, paths, project))
        pending.unlink()
        T._sync(pending.parent)
        P.forget(entry)
    print('history committed: ' + mutation.to_data()['operation'] + ' (' + kind + ' ' + a + ', ' + b + ')')
    return 0


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


def _branch_destination_clean(entry, captured, project):
    """Branch folding starts from committed target knowledge, as legacy folding does."""
    relative = set()
    candidates = [entry, Path(P.layout(entry)['view'])]
    candidates.extend(Path(path) for kind, path in captured.inventory if kind in ('file', 'bytes'))
    for path in candidates:
        try:
            relative.add(path.resolve().relative_to(project.root).as_posix())
        except ValueError:
            raise C.HistoryError('branch_target_outside_checkout', str(path)) from None
    status = P._peer('project_modes').git(project.root, '--literal-pathspecs', 'status', '--porcelain=v1', '-z',
        '--untracked-files=all', '--ignored=matching', '--', *sorted(relative)).stdout
    C._require(not status, 'branch_target_uncommitted',
               'commit the target record and its history before adopting a branch')


def _base64_length(size):
    return 4 * ((size + 2) // 3)


def _adoption_preflight(source_bytes):
    """Reject a captured source that cannot fit in the private write envelope."""
    C._require(isinstance(source_bytes, (list, tuple)) and source_bytes and
               all(type(raw) is bytes for raw in source_bytes), 'invalid_branch_capture')
    encoded_sources = sum(_base64_length(len(raw)) for raw in source_bytes)
    minimum = encoded_sources + _base64_length(encoded_sources)
    C._require(minimum <= C.MAX_REQUEST_BYTES, 'branch_adoption_limit',
               'capture fits its branch limit but not the private write envelope')


def _serialize_adoption_envelope(envelope):
    """Name the narrower private-journal limit without hiding other failures."""
    try:
        return C.encode_document(envelope)
    except C.HistoryError as error:
        if error.code == 'history_limit':
            raise C.HistoryError('branch_adoption_limit',
                                 'capture fits its branch limit but not the private write envelope') from None
        raise


def _prepare_branch_mutation(prepare, *args, **kwargs):
    try:
        return prepare(*args, **kwargs)
    except C.HistoryError as error:
        if error.code == 'history_limit':
            raise C.HistoryError('branch_adoption_limit',
                                 'captured source exceeds target write limits: ' + str(error)) from error
        raise


def adopt_branch(paths, ref, *, choices=None, by=None, preview=False, as_of=None,
                 expected_source_revision=None):
    """Capture one local ref once, then preview or retain one exact local operation."""
    from . import history_branch as Branch
    original_paths = list(paths)
    views = P._peer('knowledge_views')
    project = views.project_for(original_paths)
    policy = project.config()
    paths = views.write_paths(original_paths)
    C._require(project.git and len(paths) == 1, 'branch_history_requires_git_record')
    entry = Path(paths[0]).resolve()
    try:
        source_entry = entry.relative_to(project.root).as_posix()
    except ValueError:
        raise C.HistoryError('branch_target_outside_checkout', str(entry)) from None
    C._require(active(paths), 'history_not_active')
    source = Branch.capture(project.root, ref, entry=source_entry, as_of=as_of)
    if expected_source_revision is not None:
        C._require(source['revision'] == expected_source_revision, 'branch_source_changed')
    routing = _routing(original_paths, paths, project)
    C._require(routing['policy'] == policy, 'history_routing_changed')
    pending = T._target(entry.parent, journal(entry))
    if preview:
        captured = H.Store(entry).capture()
        result = Branch.preview_adoption(captured, source, entry=entry)
        _verify_routing(routing, original_paths, paths, project)
        C._require(H.Store(entry).capture().inventory == captured.inventory, 'stale_baseline')
        return {**result, 'state': 'prepared', 'admission': 'requires_explicit_choices',
                'source_ref': ref}
    source_raw = Branch.to_bytes(source)
    _adoption_preflight([source_raw])
    with P._locked(str(entry), project=project):
        _verify_routing(routing, original_paths, paths, project)
        C._require(not pending.exists(), 'recovery_required')
        captured = H.Store(entry).capture()
        _branch_destination_clean(entry, captured, project)
        operation = 'branch-adopt-' + uuid.uuid4().hex
        mutation = _prepare_branch_mutation(Branch.prepare_adoption, entry, source,
            choices={} if choices is None else choices, by=by,
            operation=operation, recorded_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
            capture=captured)
        value = {'version': 1, 'kind': 'history-branch-adoption/v1', 'routing': routing,
                 'mutation': T._blob(mutation.to_bytes()), 'source': T._blob(source_raw),
                 'source_ref': ref}
        envelope = {**value, 'digest': identity(value)}
        encoded = _serialize_adoption_envelope(envelope)
        T.publish_immutable(pending.parent / '.gitignore', b'*\n', root=entry.parent)
        T.publish_immutable(pending, encoded, root=entry.parent)
        Branch.commit_adoption(entry, mutation, source, verify=lambda data:
            _verify_routing(routing, original_paths, paths, project))
        pending.unlink()
        T._sync(pending.parent)
        P.forget(entry)
    return {'state': 'adopted', 'source_revision': source['revision'],
            'source_commit': source['manifest']['source']['commit'], 'operation': operation}


def adopt_branches(paths, refs, *, choices=None, by=None, preview=False, as_of=None,
                   expected_source_revision=None):
    """Atomically adopt a bounded, pinned set of local branch captures.

    The retained journal contains every captured source byte.  Recovery therefore
    never needs the named refs (or Git) again.
    """
    from . import history_branch as Branch
    C._require(isinstance(refs, (list, tuple)) and refs, 'invalid_branch_source_set')
    if len(refs) == 1:
        return adopt_branch(paths, refs[0], choices=choices, by=by, preview=preview,
                            as_of=as_of, expected_source_revision=expected_source_revision)
    C._require(2 <= len(refs) <= 16 and all(isinstance(ref, str) and ref for ref in refs),
               'invalid_branch_source_set')
    original_paths = list(paths)
    views = P._peer('knowledge_views')
    project = views.project_for(original_paths)
    policy = project.config()
    paths = views.write_paths(original_paths)
    C._require(project.git and len(paths) == 1, 'branch_history_requires_git_record')
    entry = Path(paths[0]).resolve()
    try:
        source_entry = entry.relative_to(project.root).as_posix()
    except ValueError:
        raise C.HistoryError('branch_target_outside_checkout', str(entry)) from None
    C._require(active(paths), 'history_not_active')

    # Check the aggregate as each pinned source arrives, before admitting the
    # next source into the operation.  _source_set then validates and orders the
    # exact same bytes used by preview, preparation and replay.
    sources, total = [], 0
    for ref in refs:
        source = Branch.capture(project.root, ref, entry=source_entry, as_of=as_of)
        total += sum(len(raw) for raw in source['files'].values())
        C._require(total <= Branch.MAX_BYTES, 'branch_capture_limit')
        sources.append((ref, source))
    ordered, _, source_set_revision = Branch._source_set([source for _, source in sources])
    by_revision = {source['revision']: ref for ref, source in sources}
    source_bytes = [Branch.to_bytes(source) for source in ordered]
    source_list = [{'ref': by_revision[source['revision']], 'revision': source['revision'],
                    'source': T._blob(raw)} for source, raw in zip(ordered, source_bytes)]
    if expected_source_revision is not None:
        C._require(source_set_revision == expected_source_revision, 'branch_source_changed')
    routing = _routing(original_paths, paths, project)
    C._require(routing['policy'] == policy, 'history_routing_changed')
    pending = T._target(entry.parent, journal(entry))
    envelopes = ordered
    if preview:
        captured = H.Store(entry).capture()
        result = Branch.preview_adoption_set(captured, envelopes, entry=entry)
        _verify_routing(routing, original_paths, paths, project)
        C._require(H.Store(entry).capture().inventory == captured.inventory, 'stale_baseline')
        return {**result, 'state': 'prepared', 'admission': 'requires_explicit_choices',
                'source_refs': [item['ref'] for item in source_list]}
    _adoption_preflight(source_bytes)
    with P._locked(str(entry), project=project):
        _verify_routing(routing, original_paths, paths, project)
        C._require(not pending.exists(), 'recovery_required')
        captured = H.Store(entry).capture()
        _branch_destination_clean(entry, captured, project)
        operation = 'branch-adopt-set-' + uuid.uuid4().hex
        mutation = _prepare_branch_mutation(Branch.prepare_adoption_set, entry, envelopes,
            choices={} if choices is None else choices,
            by=by, operation=operation, recorded_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
            capture=captured)
        value = {'version': 1, 'kind': 'history-branch-adoption-set/v1', 'routing': routing,
                 'mutation': T._blob(mutation.to_bytes()), 'sources': source_list}
        envelope = {**value, 'digest': identity(value)}
        encoded = _serialize_adoption_envelope(envelope)
        T.publish_immutable(pending.parent / '.gitignore', b'*\n', root=entry.parent)
        T.publish_immutable(pending, encoded, root=entry.parent)
        Branch.commit_adoption_set(entry, mutation, envelopes, verify=lambda data:
            _verify_routing(routing, original_paths, paths, project))
        pending.unlink()
        T._sync(pending.parent)
        P.forget(entry)
    return {'state': 'adopted', 'source_set_revision': source_set_revision,
            'source_revisions': [source['revision'] for source in envelopes],
            'source_commits': [source['manifest']['source']['commit'] for source in envelopes],
            'operation': operation}


def _recover_branch_adoption(entry, raw, project, original_paths, paths, direction):
    from . import history_branch as Branch
    envelope = C.decode_document(raw)
    C._mapping(envelope, ('version', 'kind', 'routing', 'mutation', 'source', 'source_ref', 'digest'))
    C._require(type(envelope['version']) is int and envelope['version'] == 1 and
               envelope['kind'] == 'history-branch-adoption/v1' and
               envelope['digest'] == identity({key: value for key, value in envelope.items() if key != 'digest'}),
               'invalid_history_journal')
    mutation = T.PreparedMutation.from_bytes(T._unblob(envelope['mutation']))
    source = Branch.from_bytes(T._unblob(envelope['source']))
    _verify_routing(envelope['routing'], original_paths, paths, project)
    pending = T._target(entry.parent, journal(entry))
    C._require(pending.read_bytes() == raw, 'concurrent_edit')
    if direction == 'before':
        C._require(mutation.to_data()['operation'] not in H.Store(entry).capture().commits,
                   'history_already_committed')
        Branch.verify_adoption(entry, mutation, source)
    else:
        Branch.commit_adoption(entry, mutation, source, verify=lambda data:
            _verify_routing(envelope['routing'], original_paths, paths, project))
    C._require(pending.read_bytes() == raw, 'concurrent_edit')
    pending.unlink()
    T._sync(pending.parent)
    P.forget(entry)
    return mutation


def _recover_branch_adoption_set(entry, raw, project, original_paths, paths, direction):
    """Replay or cancel a retained multi-source branch operation without Git."""
    from . import history_branch as Branch
    envelope = C.decode_document(raw)
    C._mapping(envelope, ('version', 'kind', 'routing', 'mutation', 'sources', 'digest'))
    C._require(type(envelope['version']) is int and envelope['version'] == 1 and
               envelope['kind'] == 'history-branch-adoption-set/v1' and
               envelope['digest'] == identity({key: value for key, value in envelope.items() if key != 'digest'}),
               'invalid_history_journal')
    C._require(isinstance(envelope['sources'], list) and 2 <= len(envelope['sources']) <= 16,
               'invalid_history_journal')
    sources, revisions = [], []
    for item in envelope['sources']:
        C._mapping(item, ('ref', 'revision', 'source'))
        C._require(isinstance(item['ref'], str) and item['ref'], 'invalid_history_journal')
        source = Branch.from_bytes(T._unblob(item['source']))
        C._require(source['revision'] == item['revision'], 'invalid_history_journal')
        sources.append(source)
        revisions.append(item['revision'])
    ordered, _, revision = Branch._source_set(sources)
    C._require([item['revision'] for item in ordered] == revisions, 'invalid_history_journal')
    mutation = T.PreparedMutation.from_bytes(T._unblob(envelope['mutation']))
    adoption = mutation.to_data()['receipt']['after'].get('history_branch_adoption', {})
    C._require(adoption.get('version') == 2 and adoption.get('source_set_revision') == revision,
               'invalid_history_journal')
    _verify_routing(envelope['routing'], original_paths, paths, project)
    pending = T._target(entry.parent, journal(entry))
    C._require(pending.read_bytes() == raw, 'concurrent_edit')
    if direction == 'before':
        C._require(mutation.to_data()['operation'] not in H.Store(entry).capture().commits,
                   'history_already_committed')
        Branch.verify_adoption_set(entry, mutation, ordered)
    else:
        Branch.commit_adoption_set(entry, mutation, ordered, verify=lambda data:
            _verify_routing(envelope['routing'], original_paths, paths, project))
    C._require(pending.read_bytes() == raw, 'concurrent_edit')
    pending.unlink()
    T._sync(pending.parent)
    P.forget(entry)
    return mutation


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
