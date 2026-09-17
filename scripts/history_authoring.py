"""Prepared direct authoring for one active, frozen history entry.

The caller owns routing/policy locks. Preparation is read-only; publication uses
Store's immutable manifest boundary. Retain PreparedMutation.to_bytes() for exact
retry. Re-preparing is a fresh observation, including equal-valued readings.

Computational receipts deliberately assess the authored document projection,
separately binding history baseline and objects. They are not public combined
history assessments and do not recursively hash their own commit receipt.
"""
import copy
import datetime
from dataclasses import replace
from pathlib import Path
import uuid

from . import history_contract as C, history_store as H, history_transaction as T
from . import provenance as P, recording, history_adapter
from .pending_grounding import identity, entries
from .reasoning import authoring
from .reasoning.contract import capabilities
from .reasoning.snapshot import _fields


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


def _evidence(document, world):
    evidence = {'kind': 'authored-computational-projection/v1', 'document': dict(document)}
    if world is not None:
        evidence['assessment'] = world.assessment()
    return evidence


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


def prepare(entry, action, *, by=None, operation=None, recorded_at=None, capture=None, _strict=True):
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
    C._require(not captured.document.get('also'), 'history_composite_write_unsupported')
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
        pins = _pins(captured, deps)
        claim = C.make_object(subject=subject, kind='judgment' if isinstance(body, dict) and fields['deps'] in body else 'reading',
                              by=by, on=recorded_at, operation=operation, body=body, saw=saw,
                              pins=pins, authored=authored)
        new.append(claim)
        document.setdefault(collection, {})[subject] = copy.deepcopy(body)
        if old is not None or _strict:
            new.append(C.make_object(subject=subject, kind='act', by=by, on=recorded_at,
                operation=operation, saw=sorted([*saw, claim['id']]),
                body={'act': 'accept', 'of': claim['id'],
                      'over': sorted(captured.state['subjects'][subject]['heads']) if old is not None else [],
                      'because': str(action.get('why') or 'explicit ' + kind)}))
    # Updates belong to a new immutable template, not to an old claim body.
    if isinstance(document.get('meta'), dict) and 'updated' in document['meta']:
        document['meta']['updated'] = action['as_of']
    template = store._template(captured.commits)
    for collection in P.collections_of(document):
        if collection != 'meta':
            template.setdefault(collection, {})
    if 'updated' in document.get('meta', {}):
        template['meta']['updated'] = document['meta']['updated']
    after_world = _world(document)
    before = _evidence(before_document, before_world)
    before['authoring'] = {'version': 1, 'action': intent, 'by': by, 'recorded_at': recorded_at,
                           'archive': frozen_archive, 'baseline': captured.baseline}
    after = _evidence(document, after_world)
    after['authoring'] = {'objects': sorted(obj['id'] for obj in new), 'notes': notes}
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before=before, after=after)
    pairs = [(obj, C.encode_document(obj)) for obj in new]
    parents = C.commit_frontier(captured.commits)
    draft = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=b'', view_template=template,
        requires=[C.EXPLICIT_ROOT_DISPOSITION] if _strict else None)
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
        requires=[C.EXPLICIT_ROOT_DISPOSITION] if _strict else None)
    files = [{'path': store.entry.name, 'role': 'record', 'before': captured.entry_bytes, 'after': rendered}]
    for obj, raw in pairs:
        path = Path(store.layout['history']) / obj['subject'] / (obj['id'] + '.yaml')
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
    # provisional envelope can render the exact after projection without a cycle.
    placeholder = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before={}, after={})
    draft = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=placeholder, view=b'', view_template=template,
        requires=[C.EXPLICIT_ROOT_DISPOSITION] if _strict else None)
    combined = {**captured.commits, operation: C.encode_document(draft)}
    selected = {**captured.objects, obj['id']: obj}
    state = H.reduce(selected, captured.state['rules'])
    rendered = store.render(captured, objects=selected, commits=combined)
    candidate = replace(captured, entry_bytes=rendered, document=C.decode_document(rendered),
        objects=selected, commits=combined, state=state,
        object_bytes={**captured.object_bytes, (obj['subject'], obj['id']): raw},
        baseline=H.baseline(captured.marker, combined, state))
    after_document = _document(history_adapter.from_store_capture(candidate).document)
    before = _evidence(before_document, _world(before_document))
    before['authoring'] = {'version': 4, 'kind': 'act', 'action': action, 'by': by,
        'recorded_at': recorded_at, 'archive': frozen_archive, 'baseline': captured.baseline}
    after = _evidence(after_document, _world(after_document))
    after['authoring'] = {'objects': [obj['id']], 'subject': action['id'],
                         'acceptance': state['subjects'][action['id']]['acceptance']}
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before=before, after=after)
    manifest = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=rendered, view_template=template,
        requires=[C.EXPLICIT_ROOT_DISPOSITION] if _strict else None)
    C._require(store.render(captured, objects=selected,
        commits={**captured.commits, operation: C.encode_document(manifest)}) == rendered,
        'view_projection_mismatch')
    files = [
        {'path': store.entry.name, 'role': 'record', 'before': captured.entry_bytes, 'after': rendered},
        {'path': (Path(store.layout['history']) / obj['subject'] / (obj['id'] + '.yaml')).relative_to(store.root).as_posix(),
         'role': 'history_object', 'before': None, 'after': raw},
        {'path': (Path(store.layout['history_commits']) / (operation + '.yaml')).relative_to(store.root).as_posix(),
         'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)}]
    C._require(_archive(store) == frozen_archive, 'concurrent_archive_edit')
    return T.PreparedMutation(operation=operation, authority=captured.marker, baseline=captured.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)



def prepare_proposal(entry, subject, body=None, collection=None, *, because=None, by=None, operation=None,
                     recorded_at=None, capture=None, hypothesis=None):
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
    deps = body.get(fields['deps'], []) if isinstance(body, dict) else []
    C._require(isinstance(deps, list) and all(isinstance(dep, str) for dep in deps), 'invalid_proposal_dependencies')
    authored = {'collection': collection, 'fields': fields, 'profile': cap['profile']}
    if hypothesis is not None:
        authored['hypothesis'] = C.detached(hypothesis)
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
    hypothetical = copy.deepcopy(document)
    for name in list(P.collections_of(hypothetical)):
        if name != 'meta':
            hypothetical[name].pop(subject, None)
    hypothetical.setdefault(collection, {})[subject] = body
    before = _evidence(document, _world(document))
    before['authoring'] = {'version': 5, 'kind': 'proposal', 'subject': subject,
        'body': body, 'collection': collection, 'because': because, 'hypothesis': hypothesis,
        'by': by, 'recorded_at': recorded_at, 'archive': frozen_archive, 'baseline': captured.baseline}
    # The accepted computational view is unchanged. Hypothetical assessment is
    # labelled separately and is never used as a reducer acceptance decision.
    after = _evidence(document, _world(document))
    after['proposal'] = _evidence(hypothetical, _world(hypothetical))
    after['authoring'] = {'objects': sorted([claim['id'], propose['id']]), 'subject': subject,
                          'proposal': claim['id']}
    receipt = T.semantic_receipt(profile=cap['profile'], capabilities=cap, before=before, after=after)
    pairs = [(obj, C.encode_document(obj)) for obj in (claim, propose)]
    template = store._template(captured.commits)
    template.setdefault(collection, {})
    parents = C.commit_frontier(captured.commits)
    draft = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=b'', view_template=template,
        requires=[C.EXPLICIT_ROOT_DISPOSITION])
    commits = {**captured.commits, operation: C.encode_document(draft)}
    rendered = store.render(captured, objects=objects, commits=commits)
    state = H.reduce(objects, captured.state['rules'])
    candidate = replace(captured, objects=objects, state=state, commits=commits,
        object_bytes={**captured.object_bytes, **{(obj['subject'], obj['id']): raw for obj, raw in pairs}},
        baseline=H.baseline(captured.marker, commits, state))
    history_adapter.from_store_capture(candidate)
    manifest = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=rendered, view_template=template,
        requires=[C.EXPLICIT_ROOT_DISPOSITION])
    files = [{'path': store.entry.name, 'role': 'record', 'before': captured.entry_bytes, 'after': rendered},
        {'path': (Path(store.layout['history_commits']) / (operation + '.yaml')).relative_to(store.root).as_posix(),
         'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)}]
    for obj, raw in pairs:
        files.append({'path': (Path(store.layout['history']) / obj['subject'] / (obj['id'] + '.yaml')).relative_to(store.root).as_posix(),
                      'role': 'history_object', 'before': None, 'after': raw})
    C._require(_archive(store) == frozen_archive, 'concurrent_archive_edit')
    return T.PreparedMutation(operation=operation, authority=captured.marker, baseline=captured.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)


def prepare_batch(entry, actions, *, by=None, operation=None, recorded_at=None, capture=None,
                  context=None, evidence=None, _receipt_version=3, _strict=None):
    """Prepare ordered writer intents as one committed generation, without I/O writes.

    Step manifests exist only in detached preparation worlds. They are never
    returned as publishable files or ancestors of the final manifest. Original
    step receipts retain their exact computational evidence and virtual baselines.
    """
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
        requires=[C.EXPLICIT_ROOT_DISPOSITION] if _strict else None)
    combined = {**original.commits, operation: C.encode_document(draft)}
    rendered = store.render(original, objects=virtual.objects, commits=combined)
    manifest = C.make_commit(marker=original.marker, operation=operation, parents=parents,
        baseline=original.baseline, objects=pairs, receipt=receipt, view=rendered, view_template=template,
        requires=[C.EXPLICIT_ROOT_DISPOSITION] if _strict else None)
    files += [{'path': store.entry.name, 'role': 'record', 'before': original.entry_bytes, 'after': rendered},
              {'path': (Path(store.layout['history_commits']) / (operation + '.yaml')).relative_to(store.root).as_posix(),
               'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)}]
    files.extend({'path': path, 'role': 'history_evidence', 'before': None, 'after': raw}
                 for path, raw in sorted(evidence.items()))
    C._require(_archive(store) == frozen_archive, 'concurrent_archive_edit')
    return T.PreparedMutation(operation=operation, authority=original.marker, baseline=original.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)


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
    C._require(isinstance(intent, dict) and intent.get('version') in (1, 2, 3, 4, 5), 'invalid_authoring_receipt')
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
    strict = C.EXPLICIT_ROOT_DISPOSITION in manifest.get('requires', [])
    if intent['version'] == 5:
        C._require(intent.get('kind') == 'proposal' and strict, 'invalid_authoring_receipt')
        expected = prepare_proposal(entry, intent['subject'], intent['body'], intent['collection'],
            because=intent['because'], hypothesis=intent['hypothesis'], by=intent['by'],
            operation=data['operation'], recorded_at=intent['recorded_at'], capture=captured)
    elif intent['version'] == 4:
        C._require(intent.get('kind') == 'act', 'invalid_authoring_receipt')
        expected = prepare_act(entry, intent['action'], by=intent['by'], operation=data['operation'],
                               recorded_at=intent['recorded_at'], capture=captured, _strict=strict)
    elif intent['version'] in (2, 3):
        C._require(intent.get('kind') == 'batch', 'invalid_authoring_receipt')
        expected = prepare_batch(entry, intent['actions'], by=intent['by'], operation=data['operation'],
            recorded_at=intent['recorded_at'], capture=captured, context=intent['context'],
            evidence={item['path']: item['after'] for item in mutation.files if item['role'] == 'history_evidence'},
            _receipt_version=intent['version'], _strict=strict)
    else:
        expected = prepare(entry, intent['action'], by=intent['by'], operation=data['operation'],
                           recorded_at=intent['recorded_at'], capture=captured, _strict=strict)
    C._require(expected.to_bytes() == mutation.to_bytes(), 'authoring_receipt_mismatch')


def commit(entry, mutation, *, verify):
    """Publish only after semantic replay and caller-owned routing/policy checks."""
    C._require(callable(verify), 'missing_verifier')
    def checked(data):
        verify_prepared(entry, mutation)
        verify(data)
        C._require(_archive(H.Store(entry)) == data['receipt']['before']['authoring']['archive'],
                   'concurrent_archive_edit')
    return H.Store(entry).commit(mutation, verify=checked)
