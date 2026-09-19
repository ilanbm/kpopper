"""Prepared direct authoring for one active, frozen history entry.

The caller owns routing/policy locks. Preparation is read-only; publication uses
Store's immutable manifest boundary. Retain PreparedMutation.to_bytes() for exact
retry. Re-preparing is a fresh observation, including equal-valued readings.

Computational receipts deliberately assess the authored document projection,
separately binding history baseline and objects. They are not public combined
history assessments and do not recursively hash their own commit receipt.
"""
import copy
import contextvars
import datetime
from dataclasses import replace
from pathlib import Path
import uuid

from . import history_paths as HP
from . import history_contract as C, history_store as H, history_transaction as T
from . import provenance as P, recording, history_adapter
from .pending_grounding import identity, entries
from .reasoning import authoring
from .reasoning.contract import capabilities, digest as reasoning_digest
from .reasoning.snapshot import _fields, _retained_history_members


def _document(document):
    document = P.Record(copy.deepcopy(document))
    document.get('meta', {}).pop('history', None)
    return document


def _archive(store):
    path = store._path(store.layout['replaced'])
    C._require(not path.exists() or path.is_file(), 'invalid_archive')
    if not path.exists():
        return {'path': path.relative_to(store.root.resolve()).as_posix(), 'sha256': None}
    C._require(path.stat().st_size <= C.MAX_REQUEST_BYTES, 'history_limit', 'frozen archive')
    return {'path': path.relative_to(store.root.resolve()).as_posix(), 'sha256': C.sha256(path.read_bytes())}


class _Reader(P._Reader):
    @staticmethod
    def infer(document):
        if capabilities(document)['profile'] != 'core/v1':
            return P.infer(document)
        fields = _fields(document)
        known = entries(document)
        judgments = {}
        for subject, (_, body) in known.items():
            if isinstance(body, dict) and fields['deps'] in body:
                seen = body.get(fields['snapshot']) or {}
                judgments[subject] = {'deps': list(body[fields['deps']]),
                    'seen': set(seen), 'snap': dict(seen),
                    'pred': P.predicate_of(body, fields), 'body': body}
        return set(known), judgments, fields


READER = _Reader()


def _world(document):
    return authoring.World(READER, document) if capabilities(document)['profile'] == 'core/v1' else None


def _destination(document):
    """New writer metadata; unchanged scalar declarations retain exact bytes."""
    if capabilities(document)['profile'] == 'core/v1':
        desired = authoring.declaration(document)
        if set(desired['requires']) - set(capabilities(document)['requires']):
            document = copy.deepcopy(document)
            document['meta']['reasoning'] = desired
    return document


def _declare_template(template, document):
    if capabilities(document)['profile'] == 'core/v1':
        template.setdefault('meta', {})['reasoning'] = copy.deepcopy(document['meta']['reasoning'])


_REPLAY_AUDIT = contextvars.ContextVar('history_retained_adapter_audit', default=None)


def _computations(report):
    for node in report.get('nodes', {}).values():
        yield node.get('computation')
        state = node.get('state', {})
        yield state.get('falsifier', {}).get('computation')
        for dependency in state.get('basis', {}).get('dependencies', {}).values():
            yield dependency.get('computation')


def _native_audit_key(implementation):
    return reasoning_digest({key: value for key, value in implementation.items()
                             if key != 'adapter_source_sha256'})


def _recorded_adapter_audit(receipt):
    """Validate internal audit hashes; this alone does not establish provenance."""
    audits = {}
    pending = [receipt]
    while pending:
        current = T.validate_receipt(pending.pop())
        for role in ('before', 'after'):
            evidence = current[role]
            reports = [evidence.get('assessment'), evidence.get('proposal', {}).get('assessment')]
            for report in reports:
                if report is None:
                    continue
                C._require(isinstance(report, dict) and report.get('schema_version') == 2
                           and report.get('assessment_revision') == reasoning_digest({
                               key: value for key, value in report.items() if key != 'assessment_revision'}),
                           'invalid_retained_assessment')
                for result in _computations(report):
                    if result is None or result.get('implementation') is None:
                        continue
                    implementation = result['implementation']
                    C._require(isinstance(implementation, dict) and
                               isinstance(implementation.get('adapter_source_sha256'), str) and
                               C.HEX.fullmatch(implementation['adapter_source_sha256']) and
                               result.get('assurance', {}).get('implementation') == reasoning_digest(implementation),
                               'invalid_retained_implementation')
                    key = _native_audit_key(implementation)
                    C._require(key not in audits or audits[key] == implementation, 'ambiguous_retained_adapter_audit')
                    audits[key] = copy.deepcopy(implementation)
        for step in current['after'].get('authoring', {}).get('steps', []):
            if isinstance(step, dict) and 'receipt' in step:
                pending.append(step['receipt'])
    return audits



def _witnessed_adapter_audit(receipt, parents):
    """Trust only the current adapter or audit already recorded by causal parents.

    The incoming operation, its own committed retry manifest, sibling commits,
    and caller-supplied hashes cannot establish historical adapter provenance.
    A first pending operation from an otherwise unwitnessed older adapter must
    finish using its original verified runtime; it is never silently regenerated.
    """
    from .reasoning import adapter_identity
    actual = adapter_identity()
    witnessed = set()
    for raw in parents.values():
        parent = C.validate_commit(C.decode_document(raw))
        prior = parent['receipt']
        # Legacy low-level bootstrap receipts may carry no semantic assessment.
        # They remain readable, but cannot witness an adapter they never named.
        if not {'before', 'after', 'capabilities', 'profile', 'digest'} <= set(prior):
            continue
        witnessed.update(reasoning_digest(audit) for audit in _recorded_adapter_audit(prior).values())
    recorded = _recorded_adapter_audit(receipt)
    for audit in recorded.values():
        C._require(audit['adapter_source_sha256'] == actual or reasoning_digest(audit) in witnessed,
                   'unknown_retained_adapter_audit',
                   'no current or committed causal-parent witness; use the original verified runtime')
    return recorded


def _retain_adapter_audit(report):
    recorded = _REPLAY_AUDIT.get()
    if recorded is None:
        return report
    report = copy.deepcopy(report)
    for result in _computations(report):
        if result is None or result.get('implementation') is None:
            continue
        implementation = result['implementation']
        key = _native_audit_key(implementation)
        C._require(key in recorded, 'authoring_native_audit_mismatch')
        # Only the adapter source fingerprint can differ. The actual evaluator
        # has run; all values, capabilities, reads, bases, diagnostics and costs
        # remain freshly derived and the whole mutation must still byte-match.
        # This scratch reconstruction retains original audit provenance, and
        # does not replace the current runtime's reported implementation.
        result['implementation'] = copy.deepcopy(recorded[key])
        result['assurance']['implementation'] = reasoning_digest(recorded[key])
    report['assessment_revision'] = reasoning_digest({key: value for key, value in report.items()
                                                     if key != 'assessment_revision'})
    return report


def _temporal_bodies(document):
    return {subject: body['temporal'] for subject, (_, body) in entries(document).items()
            if isinstance(body, dict) and isinstance(body.get('temporal'), dict)}


def _accepted_versions(captured):
    return {subject: state['head'] for subject, state in captured.state['subjects'].items()
            if state.get('acceptance') == 'accepted' and 'head' in state}


def _temporal_requires(document, base):
    required = list(base or [])
    if _temporal_bodies(document):
        required.append(C.TEMPORAL_APPLICABILITY)
    return HP.commit_requires(sorted(set(required)))


def _evidence(document, world, *, versions=None):
    evidence = {'kind': 'authored-computational-projection/v1', 'document': dict(document)}
    if world is not None:
        evidence['assessment'] = _retain_adapter_audit(world.assessment())
        _attach_temporal_replay(evidence, document, world, versions or {})
    return evidence


def _attach_temporal_replay(evidence, document, world, versions):
    temporal = {subject: metadata for subject, metadata in _temporal_bodies(document).items()
                if subject in versions}
    if temporal:
        evidence['temporal_replay'] = {
            'version': 1, 'snapshot': world.snapshot.to_json(),
            'claims': {subject: versions[subject] for subject in sorted(temporal)}}


def _head(captured, subject):
    state = captured.state['subjects'].get(subject)
    C._require(state is not None and state['acceptance'] == 'accepted' and 'head' in state,
               'unresolved_history_subject', subject)
    return captured.objects[state['head']]


def _pins(captured, deps, *, allow_missing=False):
    pins = {}
    for subject in deps:
        if subject not in captured.state['subjects'] and allow_missing:
            continue
        pins[subject] = _head(captured, subject)['id']
    return pins


def _pins_and_gaps(captured, deps, *, allow_missing=False):
    pins, gaps = {}, {}
    for subject in deps:
        if subject not in captured.state['subjects']:
            C._require(allow_missing, 'unresolved_history_subject', subject)
            gaps[subject] = 'unavailable'
        else:
            pins[subject] = _head(captured, subject)['id']
    return pins, gaps


def prepare(entry, action, *, by=None, operation=None, recorded_at=None, capture=None, _strict=True,
            _receipt_version=None):
    """Validate add/set/review and return its complete immutable retry envelope.

    `capture` may supply detached Store evidence. An existing operation must be
    retried from serialized bytes, never regenerated from the current entry.
    Authoring validation retains existing replacement permission and conditions;
    an accept act records the explicit successful writer request, not a temporal
    transition inferred by the history reducer.
    """
    store = H.Store(entry)
    captured = capture or store.capture()
    C._require(captured.commits, 'history_bootstrap_required')
    C._require(identity(captured.document['meta']['history']) == identity(captured.baseline), 'stale_view')
    C._require(captured.entry_bytes == store.render(captured), 'unresolved_view_edit')
    _retained_history_members(captured.document, store.entry)
    action = copy.deepcopy(action)
    C._require(action.get('kind') in ('add', 'set', 'review'), 'unsupported_history_action')
    C._require(not action.get('hypothesis') and not action.get('section'), 'history_hypothesis_write_unsupported')
    operation = operation or 'history-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    C._require(operation not in captured.commits, 'operation_already_prepared', 'retry retained mutation bytes')
    action['as_of'] = action.get('as_of') or datetime.date.today().isoformat()
    intent = copy.deepcopy(action)
    frozen_archive = _archive(store)
    document = _document(history_adapter.from_store_capture(captured).document)
    before_document = copy.deepcopy(document)
    profiles = {captured.objects[state['head']]['authored']['profile']
                for state in captured.state['subjects'].values() if 'head' in state}
    C._require(len(profiles) <= 1, 'incompatible_authored_profiles')
    cap = capabilities(document, profile=next(iter(profiles), None))
    C._require(action.get('profile') in (None, cap['profile']), 'history_profile_migration_required')
    authoring.validate_declared(document)
    before_world = _world(document)
    ids, judgments, fields = READER.infer(document)
    fields = P.authored_fields(action, fields)
    if before_world is not None:
        fields = {**fields, **before_world.fields}
        raw = before_world.raw
    else:
        raw = P.with_builtins(document, ids, judgments, fields)
    action, notes = P.normalize_authored(action, ids, fields, raw)
    refusals = P.validate(action, document, ids, judgments, fields, raw)
    if refusals:
        raise P.Refused('refused - ' + '\n          '.join(refusals))
    subject, kind = action['id'], action['kind']
    receipt_version = _receipt_version or 1
    existing = entries(document)
    old = _head(captured, subject) if subject in captured.state['subjects'] else None
    saw = sorted(vid for vid, obj in captured.objects.items() if obj['subject'] == subject)
    new = []
    if kind == 'review':
        C._require(old is not None and old['kind'] == 'judgment', 'invalid_review')
        C._require('_record_scope' not in action or
                   identity(action['_record_scope']) == identity(old['body'].get('scope')),
                   'history_review_scope_change_requires_claim')
        pins = _pins(captured, judgments[subject]['deps'], allow_missing=bool(P._blocked_text(old['body'])))
        body = {'act': 'review', 'of': old['id'], 'over': [],
                'because': str(action.get('why') or 'explicit review'), 'read': pins}
        new.append(C.make_object(subject=subject, kind='act', by=by, on=recorded_at,
                                 operation=operation, body=body, saw=saw))
    else:
        if kind == 'set':
            C._require(old is not None, 'unresolved_history_subject', subject)
            body = recording.set_body(old['body'], action)
            collection = old['authored']['collection']
            authored = copy.deepcopy(old['authored'])
        else:
            body = copy.deepcopy(action['body'])
            collection = existing[subject][0] if subject in existing else P._collection_for(
                document, ids, judgments, fields, subject, body, action.get('into'))
            authored = {'collection': collection, 'fields': {k: v for k, v in fields.items() if v},
                        'profile': cap['profile']}
        deps = body.get(fields['deps'], []) if isinstance(body, dict) else []
        is_judgment = isinstance(body, dict) and fields['deps'] in body
        capture_seen = is_judgment and cap['profile'] == 'core/v1'
        receipt_version = _receipt_version if _receipt_version is not None else (7 if capture_seen else 1)
        C._require(receipt_version in (1, 7), 'invalid_authoring_receipt')
        if capture_seen and receipt_version == 7:
            body[fields['snapshot']] = P._snapshot(
                list(deps), raw, ids, judgments, [], None)
            action['body'] = copy.deepcopy(body)
        pins, gaps = _pins_and_gaps(captured, deps, allow_missing=bool(P._blocked_text(body)))
        claim = C.make_object(subject=subject, kind='judgment' if is_judgment else 'reading',
                              by=by, on=recorded_at, operation=operation, body=body, saw=saw,
                              pins=pins, pin_gaps=gaps or None, authored=authored)
        new.append(claim)
        document.setdefault(collection, {})[subject] = copy.deepcopy(body)
        if old is not None or _strict:
            new.append(C.make_object(subject=subject, kind='act', by=by, on=recorded_at,
                operation=operation, saw=sorted([*saw, claim['id']]),
                body={'act': 'accept', 'of': claim['id'],
                      'over': sorted(captured.state['subjects'][subject]['heads']) if old is not None else [],
                      'because': str(action.get('why') or 'explicit ' + kind)}))
    document = _destination(document)
    cap = capabilities(document, profile=cap['profile'])
    # Updates belong to a new immutable template, not to an old claim body.
    if isinstance(document.get('meta'), dict) and 'updated' in document['meta']:
        document['meta']['updated'] = action['as_of']
    template = store._template(captured.commits)
    for collection in P.collections_of(document):
        if collection != 'meta':
            template.setdefault(collection, {})
    if 'updated' in document.get('meta', {}):
        template['meta']['updated'] = document['meta']['updated']
    _declare_template(template, document)
    after_world = _world(document)
    before_versions = _accepted_versions(captured)
    after_versions = dict(before_versions)
    if kind != 'review':
        after_versions[subject] = claim['id']
    before = _evidence(before_document, before_world, versions=before_versions)
    before['authoring'] = {'version': receipt_version, 'action': intent, 'by': by, 'recorded_at': recorded_at,
                           'archive': frozen_archive, 'baseline': captured.baseline}
    after = _evidence(document, after_world, versions=after_versions)
    after['authoring'] = {'objects': sorted(obj['id'] for obj in new), 'notes': notes}
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before=before, after=after)
    pairs = [(obj, C.encode_document(obj)) for obj in new]
    parents = C.commit_frontier(captured.commits)
    draft = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=b'', view_template=template,
        requires=_temporal_requires(document, [C.EXPLICIT_ROOT_DISPOSITION] if _strict else None))
    combined = {**captured.commits, operation: C.encode_document(draft)}
    rendered = store.render(captured, objects={**captured.objects, **{o['id']: o for o in new}}, commits=combined)
    candidate_objects = {**captured.objects, **{o['id']: o for o in new}}
    candidate_state = H.reduce(candidate_objects, captured.state['rules'])
    candidate = replace(captured, objects=candidate_objects, commits=combined,
        object_bytes={**captured.object_bytes, **{(o['subject'], o['id']): raw for o, raw in pairs}},
        state=candidate_state, baseline=H.baseline(captured.marker, combined, candidate_state))
    history_adapter.from_store_capture(candidate)
    manifest = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=rendered, view_template=template,
        requires=_temporal_requires(document, [C.EXPLICIT_ROOT_DISPOSITION] if _strict else None))
    files = [{'path': store.entry.name, 'role': 'record', 'before': captured.entry_bytes, 'after': rendered}]
    for obj, raw in pairs:
        path = Path(store.layout['history']) / HP.path_for_object(obj, captured)
        files.append({'path': path.relative_to(store.root).as_posix(), 'role': 'history_object',
                      'before': None, 'after': raw})
    manifest_path = Path(store.layout['history_commits']) / (operation + '.yaml')
    files.append({'path': manifest_path.relative_to(store.root).as_posix(), 'role': 'history_commit',
                  'before': None, 'after': C.encode_document(manifest)})
    C._require(_archive(store) == frozen_archive, 'concurrent_archive_edit')
    return T.PreparedMutation(operation=operation, authority=captured.marker, baseline=captured.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)



def prepare_act(entry, action, *, by=None, operation=None, recorded_at=None, capture=None, _strict=True):
    """Record an explicitly targeted acceptance, refutation or correction.

    The caller supplies kind, subject id, exact claim of, explicit over list and
    nonempty because. Refutation has no replacement relation and requires an
    empty over list. No condition outcome grants acceptance or revives a claim;
    the act is the explicit writer intent. Existing claim bodies/pins/seen stay
    immutable. Evaluator results are independent computational evidence.
    """
    C._mapping(action, ('kind', 'id', 'of', 'over', 'because'))
    action = C.detached(action)
    C._require(action['kind'] in ('accept', 'refute', 'correct', 'propose', 'retire'), 'invalid_history_act')
    C._text(action['id'], C.SUBJECT)
    C._text(action['of'], C.OBJECT_ID)
    C._require(isinstance(action['over'], list), 'invalid_act_over')
    for target in action['over']:
        C._text(target, C.OBJECT_ID)
    C._require(len(set(action['over'])) == len(action['over']) and action['of'] not in action['over'],
               'invalid_act_over')
    C._require(action['kind'] not in ('refute', 'propose', 'retire') or not action['over'],
               'invalid_refute_over' if action['kind'] == 'refute' else 'invalid_act_over')
    C._require(_strict or action['kind'] not in ('propose', 'retire'), 'strict_history_capability_required')
    C._require(isinstance(action['because'], str) and bool(action['because'].strip()), 'act_reason_required')
    action['over'] = sorted(action['over'])
    store = H.Store(entry)
    captured = capture or store.capture()
    C._require(captured.commits, 'history_bootstrap_required')
    C._require(identity(captured.document['meta']['history']) == identity(captured.baseline), 'stale_view')
    C._require(captured.entry_bytes == store.render(captured), 'unresolved_view_edit')
    for version in [action['of'], *action['over']]:
        target = captured.objects.get(version)
        C._require(target is not None, 'missing_act_target', version)
        C._require(target['kind'] != 'act', 'act_target_must_be_claim', version)
        C._require(target['subject'] == action['id'], 'act_subject_mismatch', version)
    operation = operation or 'history-act-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    C._require(operation not in captured.commits, 'operation_already_prepared', 'retry retained mutation bytes')
    frozen_archive = _archive(store)
    before_document = _document(history_adapter.from_store_capture(captured).document)
    target = captured.objects[action['of']]
    if action['kind'] in ('accept', 'correct'):
        C.require_interpretable_claim(target)
    cap = capabilities(before_document, profile=target['authored']['profile'])
    obj = C.make_object(subject=action['id'], kind='act', by=by, on=recorded_at,
        operation=operation, body={'act': action['kind'], 'of': action['of'],
                                  'over': action['over'], 'because': action['because']},
        saw=sorted(version for version, known in captured.objects.items() if known['subject'] == action['id']))
    raw = C.encode_document(obj)
    pairs = [(obj, raw)]
    parents = C.commit_frontier(captured.commits)
    template = store._template(captured.commits)
    # The effect digest excludes the receipt and generated-view hash, so this
    # provisional envelope can derive the exact accepted projection without a
    # cycle. Capability declarations follow that projection; a proposal's
    # hypothetical declaration is not promoted until an explicit act selects it.
    placeholder = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before={}, after={})
    draft = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=placeholder, view=b'', view_template=template,
        requires=_temporal_requires(before_document,
                                    [C.EXPLICIT_ROOT_DISPOSITION] if _strict else None))
    combined = {**captured.commits, operation: C.encode_document(draft)}
    selected = {**captured.objects, obj['id']: obj}
    state = H.reduce(selected, captured.state['rules'])
    candidate = replace(captured, objects=selected, commits=combined, state=state,
        object_bytes={**captured.object_bytes, (obj['subject'], obj['id']): raw},
        baseline=H.baseline(captured.marker, combined, state))
    projected = _destination(_document(history_adapter.from_store_capture(candidate).document))
    _declare_template(template, projected)
    draft = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=placeholder, view=b'', view_template=template,
        requires=_temporal_requires(projected,
                                    [C.EXPLICIT_ROOT_DISPOSITION] if _strict else None))
    combined = {**captured.commits, operation: C.encode_document(draft)}
    rendered = store.render(captured, objects=selected, commits=combined)
    candidate = replace(captured, entry_bytes=rendered, document=C.decode_document(rendered),
        objects=selected, commits=combined, state=state,
        object_bytes={**captured.object_bytes, (obj['subject'], obj['id']): raw},
        baseline=H.baseline(captured.marker, combined, state))
    after_document = _document(history_adapter.from_store_capture(candidate).document)
    cap = capabilities(after_document, profile=target['authored']['profile'])
    before = _evidence(before_document, _world(before_document),
                       versions=_accepted_versions(captured))
    before['authoring'] = {'version': 4, 'kind': 'act', 'action': action, 'by': by,
        'recorded_at': recorded_at, 'archive': frozen_archive, 'baseline': captured.baseline}
    after = _evidence(after_document, _world(after_document),
                      versions=_accepted_versions(candidate))
    after['authoring'] = {'objects': [obj['id']], 'subject': action['id'],
                         'acceptance': state['subjects'][action['id']]['acceptance']}
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before=before, after=after)
    manifest = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=rendered, view_template=template,
        requires=_temporal_requires(after_document,
                                    [C.EXPLICIT_ROOT_DISPOSITION] if _strict else None))
    C._require(store.render(captured, objects=selected,
        commits={**captured.commits, operation: C.encode_document(manifest)}) == rendered,
        'view_projection_mismatch')
    files = [
        {'path': store.entry.name, 'role': 'record', 'before': captured.entry_bytes, 'after': rendered},
        {'path': (Path(store.layout['history']) / HP.path_for_object(obj, captured)).relative_to(store.root).as_posix(),
         'role': 'history_object', 'before': None, 'after': raw},
        {'path': (Path(store.layout['history_commits']) / (operation + '.yaml')).relative_to(store.root).as_posix(),
         'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)}]
    C._require(_archive(store) == frozen_archive, 'concurrent_archive_edit')
    return T.PreparedMutation(operation=operation, authority=captured.marker, baseline=captured.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)



def prepare_proposal(entry, subject, body=None, collection=None, *, because=None, by=None, operation=None,
                     recorded_at=None, capture=None, hypothesis=None, _receipt_version=9):
    """Prepare an explicitly requested claim plus propose act, never an edited-view import.

    The body is retained exactly. Existing resolvable dependencies are pinned;
    uncertainty in historical dependency versions needs a separate import path.
    Proposed conditions are assessed as hypothetical computational evidence and
    do not grant standing even when true, false, unknown or erroneous.
    """
    if isinstance(subject, dict):
        C._mapping(subject, ('id', 'body', 'into', 'because'))
        C._require(body is None and collection is None and because is None, 'invalid_proposal_action')
        subject, body, collection, because = subject['id'], subject['body'], subject['into'], subject['because']
    C._text(subject, C.SUBJECT)
    C._text(collection, C.SUBJECT)
    C._require(collection not in ('meta', 'schema', 'record', 'also'), 'invalid_proposal_collection')
    C._require(isinstance(because, str) and bool(because.strip()), 'act_reason_required')
    store = H.Store(entry)
    captured = capture or store.capture()
    C._require(captured.commits, 'history_bootstrap_required')
    C._require(identity(captured.document['meta']['history']) == identity(captured.baseline), 'stale_view')
    C._require(captured.entry_bytes == store.render(captured), 'unresolved_view_edit')
    operation = operation or 'history-proposal-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    C._require(operation not in captured.commits, 'operation_already_prepared')
    frozen_archive = _archive(store)
    document = _document(history_adapter.from_store_capture(captured).document)
    fields = _fields(document)
    current = captured.state['subjects'].get(subject, {})
    profile = captured.objects[current['head']]['authored']['profile'] if 'head' in current else None
    cap = capabilities(document, profile=profile)
    body = C.detached(body)
    intent_body = copy.deepcopy(body)
    deps = body.get(fields['deps'], []) if isinstance(body, dict) else []
    C._require(isinstance(deps, list) and all(isinstance(dep, str) for dep in deps), 'invalid_proposal_dependencies')
    C._require(_receipt_version in (5, 9), 'invalid_authoring_receipt')
    if isinstance(body, dict) and fields['deps'] in body:
        C._require(fields['snapshot'] not in body, 'authored_snapshot_forbidden')
    authored = {'collection': collection, 'fields': fields, 'profile': cap['profile']}
    if hypothesis is not None:
        authored['hypothesis'] = C.detached(hypothesis)
    hypothetical = copy.deepcopy(document)
    for name in list(P.collections_of(hypothetical)):
        if name != 'meta':
            hypothetical[name].pop(subject, None)
    hypothetical.setdefault(collection, {})[subject] = body
    hypothetical = _destination(hypothetical)
    hypothetical_world = _world(hypothetical)
    receipt_version = 9 if _receipt_version == 9 and hypothetical_world is not None else 5
    if receipt_version == 9 and isinstance(body, dict) and fields['deps'] in body:
        hyp_ids, hyp_judgments, _ = READER.infer(hypothetical)
        body[fields['snapshot']] = P._snapshot(
            list(deps), hypothetical_world.raw, hyp_ids, hyp_judgments, [], None)
        hypothetical[collection][subject] = copy.deepcopy(body)
        hypothetical_world = _world(hypothetical)
    saw = sorted(version for version, obj in captured.objects.items() if obj['subject'] == subject)
    claim = C.make_object(subject=subject,
        kind='judgment' if isinstance(body, dict) and fields['deps'] in body else 'reading',
        by=by, on=recorded_at, operation=operation, body=body, saw=saw,
        pins=_pins(captured, deps), authored=authored)
    propose = C.make_object(subject=subject, kind='act', by=by, on=recorded_at, operation=operation,
        saw=sorted([*saw, claim['id']]),
        body={'act': 'propose', 'of': claim['id'], 'over': [], 'because': because})
    objects = {**captured.objects, claim['id']: claim, propose['id']: propose}
    C.validate_closure(objects)
    current_versions = _accepted_versions(captured)
    before = _evidence(document, _world(document), versions=current_versions)
    before['authoring'] = {'version': receipt_version, 'kind': 'proposal', 'subject': subject,
        'body': intent_body, 'collection': collection, 'because': because, 'hypothesis': hypothesis,
        'by': by, 'recorded_at': recorded_at, 'archive': frozen_archive, 'baseline': captured.baseline}
    # The accepted computational view is unchanged. Hypothetical assessment is
    # labelled separately and is never used as a reducer acceptance decision.
    accepted_document = copy.deepcopy(document)
    cap = capabilities(accepted_document, profile=cap['profile'])
    after = _evidence(accepted_document, _world(accepted_document), versions=current_versions)
    after['proposal'] = _evidence(hypothetical, hypothetical_world, versions=current_versions)
    after['authoring'] = {'objects': sorted([claim['id'], propose['id']]), 'subject': subject,
                          'proposal': claim['id']}
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before=before, after=after)
    pairs = [(obj, C.encode_document(obj)) for obj in (claim, propose)]
    template = store._template(captured.commits)
    template.setdefault(collection, {})
    _declare_template(template, accepted_document)
    parents = C.commit_frontier(captured.commits)
    draft = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=b'', view_template=template,
        requires=_temporal_requires(hypothetical, [C.EXPLICIT_ROOT_DISPOSITION]))
    commits = {**captured.commits, operation: C.encode_document(draft)}
    rendered = store.render(captured, objects=objects, commits=commits)
    state = H.reduce(objects, captured.state['rules'])
    candidate = replace(captured, objects=objects, state=state, commits=commits,
        object_bytes={**captured.object_bytes, **{(obj['subject'], obj['id']): raw for obj, raw in pairs}},
        baseline=H.baseline(captured.marker, commits, state))
    history_adapter.from_store_capture(candidate)
    manifest = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=rendered, view_template=template,
        requires=_temporal_requires(hypothetical, [C.EXPLICIT_ROOT_DISPOSITION]))
    files = [{'path': store.entry.name, 'role': 'record', 'before': captured.entry_bytes, 'after': rendered},
        {'path': (Path(store.layout['history_commits']) / (operation + '.yaml')).relative_to(store.root).as_posix(),
         'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)}]
    for obj, raw in pairs:
        files.append({'path': (Path(store.layout['history']) / HP.path_for_object(obj, captured)).relative_to(store.root).as_posix(),
                      'role': 'history_object', 'before': None, 'after': raw})
    C._require(_archive(store) == frozen_archive, 'concurrent_archive_edit')
    return T.PreparedMutation(operation=operation, authority=captured.marker, baseline=captured.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)



class _BatchAdmissionWorld:
    """One final computation world plus one action's actual preceding claim.

    Replacement/source admission must compare the preceding value, not mistake
    the final candidate's value for prior agreement. Predicate and dependency
    computation still use the one fully staged final snapshot.
    """
    def __init__(self, final, document, subject):
        self.final, self.document, self.subject = final, document, subject
        self.raw = authoring.Readings(self, P.bodies(document))
        self._prior = None

    def __getattr__(self, name):
        return getattr(self.final, name)

    def candidate(self, action):
        return self.final

    def result(self, nid):
        if nid != self.subject:
            return self.final.result(nid)
        if self._prior is None:
            from .reasoning.language import node_expression, references
            body = self.raw.get(nid)
            rule = body.get('rule') if isinstance(body, dict) else None
            expression = authoring._query_expression(self.document, rule)
            query_scope = expression['query']['scope'] if expression is not None else None
            expression = expression if expression is not None else node_expression({'body': body})
            if 'unavailable' in expression:
                self._prior = {'status': 'unknown', 'diagnostics': [{'code': expression['unavailable']}]}
            else:
                self._prior = self.final.engine.evaluate(
                    expression, declared=[query_scope] if query_scope is not None else references(expression))
        return copy.deepcopy(self._prior)

    def same_value(self, nid, candidate):
        return authoring.World.same_value(self, nid, candidate)

    def value(self, nid):
        result = self.result(nid)
        if result['status'] == 'operational_error':
            self.require(result)
        if result['status'] != 'ok':
            return None
        return authoring.authored_value(result['value'])

    def validate(self, action, document, ids, judgments, fields):
        return authoring.World.validate(self, action, document, ids, judgments, fields)


def _batch_stage(document, actions):
    """Stage authored bodies only; admission and computation run after staging."""
    document = copy.deepcopy(document)
    steps = []
    for index, action in enumerate(actions):
        C._require(isinstance(action, dict) and action.get('kind') in ('add', 'set', 'review'), 'invalid_batch_action')
        C._require(not action.get('hypothesis') and not action.get('section'), 'history_hypothesis_write_unsupported')
        subject = action.get('id')
        C._text(subject, C.SUBJECT)
        existing = entries(document)
        prior = copy.deepcopy(existing.get(subject))
        ids, judgments, fields = READER.infer(document)
        fields = P.authored_fields(action, fields)
        if action['kind'] == 'add':
            body = copy.deepcopy(action['body'])
            collection = prior[0] if prior is not None else P._collection_for(
                document, ids, judgments, fields, subject, body, action.get('into'))
            document.setdefault(collection, {})[subject] = body
        else:
            C._require(prior is not None, 'unknown_batch_subject', subject)
            collection, body = prior[0], copy.deepcopy(prior[1])
            if action['kind'] == 'set':
                C._require(isinstance(body, dict), 'history_body_mapping_required')
                body = recording.set_body(body, action)
                document[collection][subject] = body
            elif '_record_scope' in action:
                C._require(isinstance(body, dict) and identity(action['_record_scope']) == identity(body.get('scope')),
                           'history_review_scope_change_requires_claim')
        steps.append({'index': index, 'subject': subject, 'action': action, 'prior': prior,
                      'collection': collection, 'body': copy.deepcopy(body)})
    if 'updated' in document.get('meta', {}):
        document['meta']['updated'] = actions[-1]['as_of']
    return document, steps


def _admission_document(final_document, step):
    document = copy.deepcopy(final_document)
    for name in P.collections_of(document):
        if name != 'meta':
            document[name].pop(step['subject'], None)
    if step['prior'] is not None:
        collection, body = step['prior']
        document.setdefault(collection, {})[step['subject']] = copy.deepcopy(body)
    return document


def _prepare_final_batch(entry, actions, *, by=None, operation=None, recorded_at=None,
                         capture=None, context=None, evidence=None, _receipt_version=8):
    """Stage all bodies, validate one final world, then bind final dependency pins."""
    C._require(isinstance(actions, list) and 1 <= len(actions) <= 64, 'invalid_batch')
    store = H.Store(entry)
    original = capture or store.capture()
    C._require(original.commits, 'history_bootstrap_required')
    C._require(identity(original.document['meta']['history']) == identity(original.baseline), 'stale_view')
    C._require(original.entry_bytes == store.render(original), 'unresolved_view_edit')
    _retained_history_members(original.document, store.entry)
    operation = operation or 'history-batch-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    C._require(operation not in original.commits, 'operation_already_prepared')
    frozen_archive = _archive(store)
    before_document = _document(history_adapter.from_store_capture(original).document)
    actions = copy.deepcopy(actions)
    for action in actions:
        C._require(isinstance(action, dict), 'invalid_batch_action')
        action['as_of'] = action.get('as_of') or datetime.date.today().isoformat()
    intent_actions = copy.deepcopy(actions)
    authoring.validate_declared(before_document)
    staged, raw_steps = _batch_stage(before_document, actions)
    staged = _destination(staged)
    normalization = _world(staged)
    normalized, notes = [], []
    for step in raw_steps:
        doc = _admission_document(staged, step)
        ids, judgments, fields = READER.infer(doc)
        fields = P.authored_fields(step['action'], fields)
        if normalization is not None:
            action, observed = normalization.normalize(step['action'], previous_raw=P.bodies(doc))
        else:
            action, observed = P.normalize_authored(step['action'], ids, fields,
                P.with_builtins(doc, ids, judgments, fields))
        normalized.append(action)
        notes.append(observed)
    final_document, steps = _batch_stage(before_document, normalized)
    final_document = _destination(final_document)
    profiles = {original.objects[state['head']]['authored']['profile']
                for state in original.state['subjects'].values() if 'head' in state}
    C._require(len(profiles) <= 1, 'incompatible_authored_profiles')
    cap = capabilities(final_document, profile=next(iter(profiles), None))
    final_world = _world(final_document)
    admission_witnesses = []
    for step in steps:
        action, subject = step['action'], step['subject']
        C._require(action.get('profile') in (None, cap['profile']), 'history_profile_migration_required')
        if subject in original.state['subjects']:
            _head(original, subject)
        doc = _admission_document(final_document, step)
        ids, judgments, fields = READER.infer(doc)
        fields = P.authored_fields(action, fields)
        if final_world is not None:
            admission = _BatchAdmissionWorld(final_world, doc, subject)
            fields = {**fields, **final_world.fields}
            raw = admission.raw
        else:
            raw = P.with_builtins(doc, ids, judgments, fields)
        refusals = P.validate(action, doc, ids, judgments, fields, raw)
        if refusals:
            raise P.Refused('refused - ' + '\n          '.join(refusals))
        admission_witnesses.append({'index': step['index'], 'subject': subject,
            'action_digest': identity(action),
            'prior_body_digest': identity(step['prior'][1]) if step['prior'] is not None else None,
            'notes': notes[step['index']]})
    fields = _fields(final_document)
    C._require(_receipt_version in (6, 8), 'invalid_authoring_receipt')
    receipt_version = 8 if _receipt_version == 8 and final_world is not None else 6
    if receipt_version == 8:
        final_ids, final_judgments, _ = READER.infer(final_document)
        for step in steps:
            body = step['body']
            if step['action']['kind'] == 'add' and isinstance(body, dict) and fields['deps'] in body:
                body[fields['snapshot']] = P._snapshot(
                    list(body[fields['deps']]), final_world.raw, final_ids,
                    final_judgments, [], None)
                step['action']['body'] = copy.deepcopy(body)
                final_document[step['collection']][step['subject']] = copy.deepcopy(body)
        final_world = _world(final_document)
    # Final-world assessment is computed once and reused in the receipt. Per-action
    # admission above uses the pre-snapshot authored world, never an intermediate state.
    after_evidence = _evidence(final_document, final_world)
    by_subject = {}
    for step in steps:
        by_subject.setdefault(step['subject'], []).append(step)
    producing = {subject for subject, sequence in by_subject.items()
                 if any(step['action']['kind'] != 'review' for step in sequence)}
    final_versions = {subject: state['head'] for subject, state in original.state['subjects'].items()
                      if state['acceptance'] == 'accepted' and 'head' in state and subject not in producing}
    objects, produced_by_action, remaining = {}, {}, set(by_subject)
    while remaining:
        progressed = False
        for subject in sorted(remaining):
            sequence = by_subject[subject]
            dependencies = {dep for step in sequence if isinstance(step['body'], dict)
                for dep in step['body'].get(fields['deps'], [])
                if not (P._blocked_text(step['body'])
                        and dep not in original.state['subjects'] and dep not in producing)}
            if any(dep in producing and dep not in final_versions for dep in dependencies):
                continue
            for dep in dependencies:
                C._require(dep in final_versions, 'unresolved_history_subject', dep)
            saw = sorted(version for version, obj in original.objects.items() if obj['subject'] == subject)
            current = _head(original, subject) if subject in original.state['subjects'] else None
            heads = list(original.state['subjects'][subject]['heads']) if current is not None else []
            for step in sequence:
                action, body = step['action'], step['body']
                op = 'batch-step-' + identity({'operation': operation, 'index': step['index']})
                declared_deps = body.get(fields['deps'], []) if isinstance(body, dict) else []
                pins = {dep: final_versions[dep] for dep in declared_deps if dep in final_versions}
                gaps = {dep: 'unavailable' for dep in declared_deps if dep not in final_versions}
                generated = []
                if action['kind'] == 'review':
                    C._require(current is not None and current['kind'] == 'judgment', 'invalid_review')
                    generated.append(C.make_object(subject=subject, kind='act', by=by, on=recorded_at,
                        operation=op, saw=saw, body={'act': 'review', 'of': current['id'], 'over': [],
                            'because': str(action.get('why') or 'explicit review'), 'read': pins}))
                else:
                    authored = copy.deepcopy(current['authored']) if action['kind'] == 'set' else {
                        'collection': step['collection'], 'fields': fields, 'profile': cap['profile']}
                    claim = C.make_object(subject=subject,
                        kind='judgment' if isinstance(body, dict) and fields['deps'] in body else 'reading',
                        by=by, on=recorded_at, operation=op, body=body, saw=saw, pins=pins,
                        pin_gaps=gaps or None, authored=authored)
                    generated += [claim, C.make_object(subject=subject, kind='act', by=by, on=recorded_at,
                        operation=op, saw=sorted([*saw, claim['id']]), body={'act': 'accept', 'of': claim['id'],
                            'over': sorted(heads), 'because': str(action.get('why') or 'explicit ' + action['kind'])})]
                    current, heads = claim, [claim['id']]
                for obj in generated:
                    objects[obj['id']] = obj
                    saw.append(obj['id'])
                saw.sort()
                produced_by_action[step['index']] = sorted(obj['id'] for obj in generated)
            final_versions[subject] = current['id']
            remaining.remove(subject)
            progressed = True
        C._require(progressed, 'cyclic_batch_pin_dependencies',
                   'new immutable final dependency pins cannot form a cycle: ' + ', '.join(sorted(remaining)))
    C.validate_closure({**original.objects, **objects})
    evidence = copy.deepcopy(evidence or {})
    for path, raw in evidence.items():
        C.relative_path(path)
        C._require(type(raw) is bytes, 'invalid_bytes')
    _attach_temporal_replay(after_evidence, final_document, final_world, final_versions)
    before = _evidence(before_document, _world(before_document),
                       versions=_accepted_versions(original))
    before['authoring'] = {'version': receipt_version, 'kind': 'batch', 'actions': intent_actions, 'by': by,
        'recorded_at': recorded_at, 'archive': frozen_archive, 'baseline': original.baseline,
        'context': C.detached(context or {}), 'evidence': {path: C.sha256(raw) for path, raw in sorted(evidence.items())}}
    after_evidence['authoring'] = {'steps': [{**item, 'objects': produced_by_action[item['index']]}
                                             for item in admission_witnesses], 'objects': sorted(objects),
                                  'validation_world': 'final', 'final_dependency_pins': True}
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before=before, after=after_evidence)
    pairs = [(obj, C.encode_document(obj)) for obj in objects.values()]
    template = store._template(original.commits)
    for collection in P.collections_of(final_document):
        if collection != 'meta':
            template.setdefault(collection, {})
    if 'updated' in final_document.get('meta', {}):
        template['meta']['updated'] = final_document['meta']['updated']
    _declare_template(template, final_document)
    args = dict(marker=original.marker, operation=operation, parents=C.commit_frontier(original.commits),
        baseline=original.baseline, objects=pairs, receipt=receipt, view_template=template,
        requires=_temporal_requires(final_document, [C.EXPLICIT_ROOT_DISPOSITION]))
    draft = C.make_commit(**args, view=b'')
    selected = {**original.objects, **objects}
    commits = {**original.commits, operation: C.encode_document(draft)}
    rendered = store.render(original, objects=selected, commits=commits)
    state = H.reduce(selected, original.state['rules'])
    candidate = replace(original, objects=selected, state=state, commits=commits,
        object_bytes={**original.object_bytes, **{(obj['subject'], obj['id']): raw for obj, raw in pairs}},
        baseline=H.baseline(original.marker, commits, state))
    adapted = _document(history_adapter.from_store_capture(candidate).document)
    C._require(identity(adapted) == identity(final_document), 'batch_final_projection_mismatch')
    manifest = C.make_commit(**args, view=rendered)
    files = [{'path': store.entry.name, 'role': 'record', 'before': original.entry_bytes, 'after': rendered},
        {'path': (Path(store.layout['history_commits']) / (operation + '.yaml')).relative_to(store.root).as_posix(),
         'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)}]
    files.extend({'path': (Path(store.layout['history']) / HP.path_for_object(obj, original)).relative_to(store.root).as_posix(),
                  'role': 'history_object', 'before': None, 'after': raw} for obj, raw in pairs)
    files.extend({'path': path, 'role': 'history_evidence', 'before': None, 'after': raw} for path, raw in sorted(evidence.items()))
    C._require(_archive(store) == frozen_archive, 'concurrent_archive_edit')
    _retained_history_members(original.document, store.entry)
    return T.PreparedMutation(operation=operation, authority=original.marker, baseline=original.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)


def prepare_batch(entry, actions, *, by=None, operation=None, recorded_at=None, capture=None,
                  context=None, evidence=None, _receipt_version=8, _strict=None):
    """Prepare one final-world generation; retain v2/v3 sequential retry semantics.

    New v6 operations stage all bodies and final dependency versions before
    validation. The old paths below are immutable-envelope compatibility only;
    their virtual step manifests and recorded receipt shapes are not rewritten.
    """
    if _receipt_version in (6, 8):
        return _prepare_final_batch(entry, actions, by=by, operation=operation, recorded_at=recorded_at,
                                    capture=capture, context=context, evidence=evidence,
                                    _receipt_version=_receipt_version)
    C._require(isinstance(actions, list) and 1 <= len(actions) <= 64, 'invalid_batch')
    _strict = _receipt_version >= 3 if _strict is None else _strict
    store = H.Store(entry)
    original = capture or store.capture()
    operation = operation or 'history-batch-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    C._require(operation not in original.commits, 'operation_already_prepared')
    frozen_archive = _archive(store)
    actions = copy.deepcopy(actions)
    for action in actions:
        action['as_of'] = action.get('as_of') or datetime.date.today().isoformat()
    C._require(_receipt_version in (2, 3), 'invalid_authoring_receipt')
    evidence = copy.deepcopy(evidence or {})
    for path, raw in evidence.items():
        C.relative_path(path)
        C._require(type(raw) is bytes, 'invalid_bytes')
    C._require(not evidence or _receipt_version == 3, 'invalid_authoring_receipt')
    virtual, steps, files = original, [], []
    first_receipt = last_receipt = None
    for index, action in enumerate(actions):
        step = 'batch-step-' + identity({'operation': operation, 'index': index})
        mutation = prepare(entry, action, by=by, operation=step, recorded_at=recorded_at, capture=virtual, _strict=_strict)
        data = mutation.to_data()
        first_receipt = first_receipt or data['receipt']
        last_receipt = data['receipt']
        steps.append({'operation': step, **({'receipt': data['receipt']} if _receipt_version == 2 else
                                           {'receipt_digest': data['receipt']['digest']})})
        selected = dict(virtual.objects)
        raw_objects = dict(virtual.object_bytes)
        commits = dict(virtual.commits)
        for item in mutation.files:
            if item['role'] == 'history_object':
                obj = C.decode_document(item['after'])
                selected[obj['id']] = obj
                raw_objects[(obj['subject'], obj['id'])] = item['after']
                files.append(item)
            elif item['role'] == 'history_commit':
                commits[step] = item['after']
            else:
                after = item['after']
        state = H.reduce(selected, original.state['rules'])
        virtual = replace(virtual, entry_bytes=after, document=C.decode_document(after),
                          objects=selected, object_bytes=raw_objects, commits=commits,
                          state=state, baseline=H.baseline(original.marker, commits, state))
    before = copy.deepcopy(first_receipt['before'])
    before['authoring'] = {'version': _receipt_version, 'kind': 'batch', 'actions': actions, 'by': by,
                           'recorded_at': recorded_at, 'archive': frozen_archive,
                           'baseline': original.baseline, 'context': C.detached(context or {})}
    if _receipt_version == 3:
        before['authoring']['evidence'] = {path: C.sha256(raw) for path, raw in sorted(evidence.items())}
    after_evidence = copy.deepcopy(last_receipt['after'])
    after_evidence['authoring'] = {'steps': steps,
                                 'objects': sorted(set(virtual.objects) - set(original.objects))}
    receipt = T.semantic_receipt(profile=first_receipt['profile'],
        capabilities=first_receipt['capabilities'], before=before, after=after_evidence)
    pairs = [(C.decode_document(item['after']), item['after']) for item in files]
    parents = C.commit_frontier(original.commits)
    template = store._template(virtual.commits)
    draft = C.make_commit(marker=original.marker, operation=operation, parents=parents,
        baseline=original.baseline, objects=pairs, receipt=receipt, view=b'', view_template=template,
        requires=_temporal_requires(last_receipt['after']['document'],
                                    [C.EXPLICIT_ROOT_DISPOSITION] if _strict else None))
    combined = {**original.commits, operation: C.encode_document(draft)}
    rendered = store.render(original, objects=virtual.objects, commits=combined)
    manifest = C.make_commit(marker=original.marker, operation=operation, parents=parents,
        baseline=original.baseline, objects=pairs, receipt=receipt, view=rendered, view_template=template,
        requires=_temporal_requires(last_receipt['after']['document'],
                                    [C.EXPLICIT_ROOT_DISPOSITION] if _strict else None))
    files += [{'path': store.entry.name, 'role': 'record', 'before': original.entry_bytes, 'after': rendered},
              {'path': (Path(store.layout['history_commits']) / (operation + '.yaml')).relative_to(store.root).as_posix(),
               'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)}]
    files.extend({'path': path, 'role': 'history_evidence', 'before': None, 'after': raw}
                 for path, raw in sorted(evidence.items()))
    C._require(_archive(store) == frozen_archive, 'concurrent_archive_edit')
    return T.PreparedMutation(operation=operation, authority=original.marker, baseline=original.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)


@HP.replay_mutation
def verify_prepared(entry, mutation):
    """Revalidate retained intent/evaluator evidence against its original closure.

    Reconstructing the parent closure makes verification valid after the manifest
    committed but before its generated view was refreshed. No new operation, clock,
    review snapshot, or dependency pin is allocated during this replay.
    """
    C._require(isinstance(mutation, T.PreparedMutation), 'invalid_mutation')
    store = H.Store(entry)
    live = store.capture()
    data = mutation.to_data()
    intent = data['receipt']['before'].get('authoring')
    C._require(isinstance(intent, dict) and intent.get('version') in (1, 2, 3, 4, 5, 6, 7, 8, 9),
               'invalid_authoring_receipt')
    C._require(_archive(store) == intent['archive'], 'concurrent_archive_edit')
    manifest = C.decode_document(next(i['after'] for i in mutation.files if i['role'] == 'history_commit'))
    commits, todo = {}, list(manifest['parents'])
    while todo:
        operation = todo.pop()
        if operation in commits:
            continue
        C._require(operation in live.commits, 'incomplete_closure', operation)
        commits[operation] = live.commits[operation]
        todo.extend(C.decode_document(commits[operation])['parents'])
    objects = C.committed_objects(live.marker, commits, live.object_bytes)
    state = H.reduce(objects, live.state['rules'])
    before = next(i['before'] for i in mutation.files if i['role'] == 'record')
    captured = replace(live, entry_bytes=before, document=C.decode_document(before), commits=commits,
                       objects=objects, state=state, baseline=H.baseline(live.marker, commits, state))
    token = _REPLAY_AUDIT.set(_witnessed_adapter_audit(data['receipt'], commits))
    try:
        strict = C.EXPLICIT_ROOT_DISPOSITION in manifest.get('requires', [])
        if intent['version'] in (5, 9):
            C._require(intent.get('kind') == 'proposal' and strict, 'invalid_authoring_receipt')
            expected = prepare_proposal(entry, intent['subject'], intent['body'], intent['collection'],
                because=intent['because'], hypothesis=intent['hypothesis'], by=intent['by'],
                operation=data['operation'], recorded_at=intent['recorded_at'], capture=captured,
                _receipt_version=intent['version'])
        elif intent['version'] == 4:
            C._require(intent.get('kind') == 'act', 'invalid_authoring_receipt')
            expected = prepare_act(entry, intent['action'], by=intent['by'], operation=data['operation'],
                                   recorded_at=intent['recorded_at'], capture=captured, _strict=strict)
        elif intent['version'] in (2, 3, 6, 8):
            C._require(intent.get('kind') == 'batch', 'invalid_authoring_receipt')
            expected = prepare_batch(entry, intent['actions'], by=intent['by'], operation=data['operation'],
                recorded_at=intent['recorded_at'], capture=captured, context=intent['context'],
                evidence={item['path']: item['after'] for item in mutation.files if item['role'] == 'history_evidence'},
                _receipt_version=intent['version'], _strict=strict)
        else:
            expected = prepare(entry, intent['action'], by=intent['by'], operation=data['operation'],
                               recorded_at=intent['recorded_at'], capture=captured, _strict=strict,
                               _receipt_version=intent['version'])
    finally:
        _REPLAY_AUDIT.reset(token)
    C._require(expected.to_bytes() == mutation.to_bytes(), 'authoring_receipt_mismatch')


def commit(entry, mutation, *, verify):
    """Publish only after semantic replay and caller-owned routing/policy checks."""
    C._require(callable(verify), 'missing_verifier')
    def checked(data):
        verify_prepared(entry, mutation)
        verify(data)
        C._require(_archive(H.Store(entry)) == data['receipt']['before']['authoring']['archive'],
                   'concurrent_archive_edit')
        _retained_history_members(H.Store(entry).capture().document, H.Store(entry).entry)
    return H.Store(entry).commit(mutation, verify=checked)
