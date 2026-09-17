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


def prepare(entry, action, *, by=None, operation=None, recorded_at=None, capture=None):
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
        if old is not None:
            new.append(C.make_object(subject=subject, kind='act', by=by, on=recorded_at,
                operation=operation, saw=sorted([*saw, claim['id']]),
                body={'act': 'accept', 'of': claim['id'],
                      'over': sorted(captured.state['subjects'][subject]['heads']),
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
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=b'', view_template=template)
    combined = {**captured.commits, operation: C.encode_document(draft)}
    rendered = store.render(captured, objects={**captured.objects, **{o['id']: o for o in new}}, commits=combined)
    candidate_objects = {**captured.objects, **{o['id']: o for o in new}}
    candidate_state = H.reduce(candidate_objects, captured.state['rules'])
    candidate = replace(captured, objects=candidate_objects, commits=combined,
        object_bytes={**captured.object_bytes, **{(o['subject'], o['id']): raw for o, raw in pairs}},
        state=candidate_state, baseline=H.baseline(captured.marker, combined, candidate_state))
    history_adapter.from_store_capture(candidate)
    manifest = C.make_commit(marker=captured.marker, operation=operation, parents=parents,
        baseline=captured.baseline, objects=pairs, receipt=receipt, view=rendered, view_template=template)
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


def prepare_batch(entry, actions, *, by=None, operation=None, recorded_at=None, capture=None,
                  context=None):
    """Prepare ordered writer intents as one committed generation, without I/O writes.

    Step manifests exist only in detached preparation worlds. They are never
    returned as publishable files or ancestors of the final manifest. Original
    step receipts retain their exact computational evidence and virtual baselines.
    """
    C._require(isinstance(actions, list) and 1 <= len(actions) <= 64, 'invalid_batch')
    store = H.Store(entry)
    original = capture or store.capture()
    operation = operation or 'history-batch-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    C._require(operation not in original.commits, 'operation_already_prepared')
    frozen_archive = _archive(store)
    actions = copy.deepcopy(actions)
    for action in actions:
        action['as_of'] = action.get('as_of') or datetime.date.today().isoformat()
    virtual, steps, files = original, [], []
    for index, action in enumerate(actions):
        step = 'batch-step-' + identity({'operation': operation, 'index': index})
        mutation = prepare(entry, action, by=by, operation=step, recorded_at=recorded_at, capture=virtual)
        data = mutation.to_data()
        steps.append({'operation': step, 'receipt': data['receipt']})
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
    before = copy.deepcopy(steps[0]['receipt']['before'])
    before['authoring'] = {'version': 2, 'kind': 'batch', 'actions': actions, 'by': by,
                           'recorded_at': recorded_at, 'archive': frozen_archive,
                           'baseline': original.baseline, 'context': C.detached(context or {})}
    after_evidence = copy.deepcopy(steps[-1]['receipt']['after'])
    after_evidence['authoring'] = {'steps': steps,
                                 'objects': sorted(set(virtual.objects) - set(original.objects))}
    receipt = T.semantic_receipt(profile=steps[0]['receipt']['profile'],
        capabilities=steps[0]['receipt']['capabilities'], before=before, after=after_evidence)
    pairs = [(C.decode_document(item['after']), item['after']) for item in files]
    parents = C.commit_frontier(original.commits)
    template = store._template(virtual.commits)
    draft = C.make_commit(marker=original.marker, operation=operation, parents=parents,
        baseline=original.baseline, objects=pairs, receipt=receipt, view=b'', view_template=template)
    combined = {**original.commits, operation: C.encode_document(draft)}
    rendered = store.render(original, objects=virtual.objects, commits=combined)
    manifest = C.make_commit(marker=original.marker, operation=operation, parents=parents,
        baseline=original.baseline, objects=pairs, receipt=receipt, view=rendered, view_template=template)
    files += [{'path': store.entry.name, 'role': 'record', 'before': original.entry_bytes, 'after': rendered},
              {'path': (Path(store.layout['history_commits']) / (operation + '.yaml')).relative_to(store.root).as_posix(),
               'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)}]
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
    C._require(isinstance(intent, dict) and intent.get('version') in (1, 2), 'invalid_authoring_receipt')
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
    if intent['version'] == 2:
        C._require(intent.get('kind') == 'batch', 'invalid_authoring_receipt')
        expected = prepare_batch(entry, intent['actions'], by=intent['by'], operation=data['operation'],
            recorded_at=intent['recorded_at'], capture=captured, context=intent['context'])
    else:
        expected = prepare(entry, intent['action'], by=intent['by'], operation=data['operation'],
                           recorded_at=intent['recorded_at'], capture=captured)
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
