"""Explicit proposal capture for body edits of the current generated history view.

Preview remains Store.prepare_reconciliation: it creates no knowledge. This
writer supports the complete set of body additions/changes in one current view.
Stale views, merge alternatives, deletion, collection moves and template edits
need separate dispositions and are refused without changing the source. Comments
and formatting are retained verbatim in immutable, receipt-bound edit evidence.
"""
import copy
import datetime
from dataclasses import replace
from pathlib import Path
import uuid

from . import history_authoring as A, history_contract as C
from . import history_store as H, history_transaction as T
from .pending_grounding import entries, identity


def _selection(store, captured, subjects):
    C._require(captured.commits, 'history_bootstrap_required')
    C._require(identity(captured.document.get('meta', {}).get('history')) ==
               identity(captured.baseline), 'stale_edit_baseline')
    original = store.render(captured)
    document = C.decode_document(original)
    C._require(identity(C.document_template(document)) ==
               identity(C.document_template(captured.document)),
               'template_disposition_required')
    before, after = entries(document), entries(captured.document)
    C._require(set(before) <= set(after), 'deletion_disposition_required')
    changed = []
    for subject, (collection, body) in after.items():
        if subject in before:
            C._require(before[subject][0] == collection, 'collection_disposition_required')
            if identity(before[subject][1]) == identity(body):
                continue
        changed.append(subject)
    C._require(changed, 'no_body_proposals')
    if subjects is None:
        subjects = sorted(changed)
    C._require(isinstance(subjects, (list, tuple)) and all(isinstance(s, str) for s in subjects)
               and len(set(subjects)) == len(subjects), 'invalid_edit_subjects')
    C._require(set(subjects) == set(changed), 'unhandled_view_edits')
    C._require(len(subjects) <= 64, 'history_limit', 'edited subjects')
    canonical = replace(captured, entry_bytes=original, document=document)
    return canonical, sorted(subjects), after


def _prepare(entry, captured, *, because, by, operation, recorded_at, subjects):
    C._require(isinstance(because, str) and bool(because.strip()), 'act_reason_required')
    C._text(operation)
    store = H.Store(entry)
    C._require(operation not in captured.commits, 'operation_already_prepared')
    C._require(len(captured.entry_bytes) <= C.MAX_REQUEST_BYTES, 'history_limit', 'edited view')
    canonical, subjects, authored = _selection(store, captured, subjects)
    frozen_archive = A._archive(store)
    files, steps, first_receipt = [], [], None
    selected = dict(canonical.objects)
    for index, subject in enumerate(subjects):
        collection, body = authored[subject]
        step = 'edit-step-' + identity({'operation': operation, 'index': index})
        mutation = A.prepare_proposal(entry, subject, copy.deepcopy(body), collection,
            because=because, by=by, operation=step,
            recorded_at=recorded_at, capture=canonical)
        receipt = mutation.to_data()['receipt']
        first_receipt = first_receipt or receipt
        steps.append({'operation': step, 'receipt_digest': receipt['digest']})
        for item in mutation.files:
            if item['role'] == 'history_object':
                files.append(item)
                obj = C.decode_document(item['after'])
                selected[obj['id']] = obj
    evidence_path = (Path(store.layout['home']) / 'evidence' / 'view-edits' /
                     (operation + '.yaml')).relative_to(store.root).as_posix()
    before = copy.deepcopy(first_receipt['before'])
    before['authoring'] = {'version': 1, 'kind': 'view-edit-proposals',
        'because': because, 'by': by, 'recorded_at': recorded_at, 'subjects': subjects,
        'baseline': captured.baseline, 'archive': frozen_archive,
        'original_view_sha256': C.sha256(canonical.entry_bytes),
        'edited_view_sha256': C.sha256(captured.entry_bytes),
        'edited_view_utf8': captured.entry_bytes.decode('utf-8'),
        'evidence': {evidence_path: C.sha256(captured.entry_bytes)},
        'template_disposition': 'unchanged'}
    before['history_edit'] = {'version': 1, 'kind': 'view-edit-proposals'}
    after = copy.deepcopy(first_receipt['before'])
    after['authoring'] = {'steps': steps, 'objects': sorted(set(selected) - set(canonical.objects)),
                          'disposition': 'proposed', 'complete_body_capture': True}
    receipt = T.semantic_receipt(profile=first_receipt['profile'],
        capabilities=first_receipt['capabilities'], before=before, after=after)
    pairs = [(C.decode_document(item['after']), item['after']) for item in files]
    arguments = dict(marker=captured.marker, operation=operation,
        parents=C.commit_frontier(captured.commits), baseline=captured.baseline,
        objects=pairs, receipt=receipt, view_template=store._template(captured.commits),
        requires=[C.EXPLICIT_ROOT_DISPOSITION])
    draft = C.make_commit(**arguments, view=b'')
    rendered = store.render(canonical, objects=selected,
                           commits={**captured.commits, operation: C.encode_document(draft)})
    manifest = C.make_commit(**arguments, view=rendered)
    files.extend([
        {'path': store.entry.name, 'role': 'record', 'before': captured.entry_bytes, 'after': rendered},
        {'path': (Path(store.layout['history_commits']) / (operation + '.yaml')).relative_to(store.root).as_posix(),
         'role': 'history_commit', 'before': None, 'after': C.encode_document(manifest)},
        {'path': evidence_path, 'role': 'history_evidence', 'before': None, 'after': captured.entry_bytes}])
    C._require(A._archive(store) == frozen_archive, 'concurrent_archive_edit')
    return T.PreparedMutation(operation=operation, authority=captured.marker, baseline=captured.baseline,
                              files=files, receipt=receipt, entry=store.entry.name)


def prepare_proposals(entry, *, because, by=None, operation=None, recorded_at=None, subjects=None):
    """Prepare explicit record-as-proposal intent; never accept or write the edit.

    Omitted subjects selects all changed bodies. A partial selection refuses:
    replacing the view must never discard an unhandled body or template edit.
    Retain returned bytes for exact retry; re-preparation is a fresh observation.
    """
    store = H.Store(entry)
    captured = store.capture_reconciliation()
    description = store.prepare_reconciliation(captured)
    C._require(not description['conflicted'], 'conflict_disposition_required')
    result = _prepare(entry, captured, because=because, by=by,
        operation=operation or 'history-edit-' + uuid.uuid4().hex,
        recorded_at=recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat(), subjects=subjects)
    C._require(store.capture().inventory == captured.inventory, 'stale_baseline')
    return result


def raw_edit_evidence(receipt):
    """Verify and return exact retained edit bytes without opening any source.

    This checks receipt evidence consistency; it does not grant write authority.
    Full closure validation separately establishes committed object membership.
    """
    receipt = T.validate_receipt(receipt)
    C._require(receipt['before'].get('history_edit') ==
               {'version': 1, 'kind': 'view-edit-proposals'}, 'invalid_edit_receipt')
    intent = receipt['before'].get('authoring', {})
    text = intent.get('edited_view_utf8')
    C._require(isinstance(text, str), 'invalid_edit_evidence')
    raw = text.encode('utf-8')
    C._require(len(raw) <= C.MAX_REQUEST_BYTES, 'history_limit', 'edited view')
    C._require(C.sha256(raw) == intent.get('edited_view_sha256') and
               isinstance(intent.get('evidence'), dict) and len(intent['evidence']) == 1 and
               list(intent['evidence'].values()) == [C.sha256(raw)], 'edit_evidence_mismatch')
    return raw


def verify_prepared(entry, mutation):
    """Replay original edited bytes against its exact committed parent closure.

    Valid both before and after manifest publication. The receipt is not a
    permission: commit additionally requires caller-owned routing verification.
    """
    C._require(isinstance(mutation, T.PreparedMutation), 'invalid_mutation')
    store = H.Store(entry)
    live = store.capture()
    data = mutation.to_data()
    intent = data['receipt']['before'].get('authoring', {})
    C._require(intent.get('kind') == 'view-edit-proposals' and intent.get('version') == 1,
               'invalid_edit_receipt')
    C._require(A._archive(store) == intent['archive'], 'concurrent_archive_edit')
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
    C._require(raw_edit_evidence(data['receipt']) == before, 'edit_evidence_mismatch')
    captured = replace(live, entry_bytes=before, document=C.decode_document(before),
        commits=commits, objects=objects, state=state, baseline=H.baseline(live.marker, commits, state))
    expected = _prepare(entry, captured, because=intent['because'], by=intent['by'],
        operation=data['operation'], recorded_at=intent['recorded_at'], subjects=intent['subjects'])
    C._require(expected.to_bytes() == mutation.to_bytes(), 'edit_receipt_mismatch')


def commit(entry, mutation, *, verify):
    """Publish with mandatory external routing verification; also the recovery API."""
    C._require(callable(verify), 'missing_verifier')
    def checked(data):
        verify_prepared(entry, mutation)
        verify(data)
        C._require(A._archive(H.Store(entry)) == data['receipt']['before']['authoring']['archive'],
                   'concurrent_archive_edit')
    return H.Store(entry).commit(mutation, verify=checked)
