"""Atomic first-write bootstrap for a new core/v1 history record.

This is authoring, not migration: the first requested claim is retained with its
actual writer intent and operation time.  There is no legacy source, locator, or
invented import provenance.  Publication uses the ordinary authority-transition
journal so readers see either no record or one complete active generation.
"""
import copy
import datetime
from dataclasses import replace
import glob
from pathlib import Path
import uuid

from . import history_authoring as A, history_contract as C, history_paths as HP
from . import history_store as H, history_transaction as T, provenance as P
from .pending_grounding import identity
from .reasoning import authoring
from .reasoning.contract import capabilities


KIND = 'history-authority-transition/v1'
BOOTSTRAP_KIND = 'new-record-bootstrap/v1'


def _empty_document(as_of):
    document = P.Record({'meta': {'updated': as_of,
                                  'reasoning': copy.deepcopy(authoring.DECLARATION)}})
    document.hypotheses = {}
    return document


def _act(claim, operation, recorded_at, by, kind, because):
    return C.make_object(subject=claim['subject'], kind='act', by=by, on=recorded_at,
        operation=operation, saw=[claim['id']],
        body={'act': kind, 'of': claim['id'], 'over': [], 'because': because})


def _archive(entry):
    return {'path': Path(P.layout(entry)['replaced']).relative_to(entry.parent).as_posix(),
            'sha256': None}


def _verify_environment(entry, mutation=None):
    """A birth may reuse its own staged bytes, but no pre-existing history."""
    layout = P.layout(entry)
    C._require(not entry.is_symlink(), 'invalid_history_path', str(entry))
    allowed = {} if mutation is None else {
        str(T._target(entry.parent, item['path'])): item['after'] for item in mutation.files
        if item['role'] in ('history_object', 'history_commit')}
    C._require(T._read(entry) in ((None,) if mutation is None else
               (None, next(item['after'] for item in mutation.files if item['role'] == 'record'))),
               'history_bootstrap_source_present')
    marker = Path(layout['history_authority'])
    C._require(not marker.is_symlink(), 'invalid_history_path', str(marker))
    marker_after = None if mutation is None else next(
        item['after'] for item in mutation.files if item['role'] == 'history_authority')
    C._require(T._read(marker) in ((None,) if mutation is None else (None, marker_after)),
               'history_bootstrap_authority_present')
    for role in ('history', 'history_commits'):
        directory = Path(layout[role])
        if not directory.exists() and not directory.is_symlink():
            continue
        C._require(directory.is_dir() and not directory.is_symlink(), 'invalid_history_path')
        for path in directory.rglob('*'):
            C._require(not path.is_symlink(), 'invalid_history_path', str(path))
            if path.is_file():
                C._require(str(path) in allowed and path.read_bytes() == allowed[str(path)],
                           'existing_history_evidence', str(path))
    hypotheses = Path(layout['hypotheses'])
    C._require(not hypotheses.is_symlink() and (not hypotheses.exists() or hypotheses.is_dir()),
               'invalid_history_path')
    existing = [path for extension in ('*.yaml', '*.yml')
                for path in glob.glob(glob.escape(str(hypotheses)) + '/' + extension)]
    C._require(not existing, 'existing_hypothesis_requires_record')
    for role in ('replaced',):
        path = Path(layout[role])
        C._require(not path.exists() and not path.is_symlink(), 'existing_record_evidence', layout[role])
    for role in ('history_cancellations',):
        directory = Path(layout[role])
        C._require(not directory.is_symlink() and
                   (not directory.exists() or directory.is_dir() and not any(directory.iterdir())),
                   'existing_history_evidence', str(directory))
    alternate = P.layout(entry.with_name('PROVENANCE.yaml'))
    for role in ('history_authority', 'replaced'):
        path = Path(alternate[role])
        C._require(not path.exists() and not path.is_symlink(), 'existing_record_evidence', alternate[role])
    for role in ('history', 'history_commits', 'history_cancellations'):
        directory = Path(alternate[role])
        C._require(not directory.is_symlink() and
                   (not directory.exists() or directory.is_dir() and not any(directory.rglob('*'))),
                   'existing_history_evidence', str(directory))
    artifacts = entry.parent / '.kpopper-history-migration'
    C._require(not artifacts.is_symlink() and
               (not artifacts.exists() or artifacts.is_dir() and not any(artifacts.rglob('*'))),
               'existing_history_evidence', str(artifacts))


def prepare(entry, action, *, policy, operation=None, recorded_at=None, record_id=None, by=None,
            _replay=False):
    """Prepare a deterministic parentless first commit without touching the tree."""
    entry = Path(entry).resolve()
    C._require(not P.is_legacy(entry), 'legacy_record_birth_unsupported',
               'new records must use GROUNDING.yaml')
    if not _replay:
        _verify_environment(entry)
    action = copy.deepcopy(action)
    C._require(action.get('kind') == 'add' and not action.get('section'),
               'unsupported_history_bootstrap_action')
    C._require(action.get('profile') in (None, 'core/v1'), 'history_profile_migration_required')
    C._require(isinstance(policy, dict), 'invalid_bootstrap_policy')
    operation = operation or 'bootstrap-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    record_id = record_id or 'record-' + uuid.uuid4().hex
    action['as_of'] = action.get('as_of') or datetime.date.today().isoformat()
    intent = copy.deepcopy(action)

    document = _empty_document(action['as_of'])
    before_document = copy.deepcopy(document)
    before_world = A._world(document)
    ids, judgments, fields = A.READER.infer(document)
    fields = {**P.authored_fields(action, fields), **before_world.fields}
    action, notes = P.normalize_authored(action, ids, fields, before_world.raw)
    refusals = P.validate(action, document, ids, judgments, fields, before_world.raw)
    if refusals:
        raise P.Refused('refused - ' + '\n          '.join(refusals))

    body = copy.deepcopy(action['body'])
    collection = P._collection_for(document, ids, judgments, fields,
                                   action['id'], body, action.get('into'))
    authored = {'collection': collection,
                'fields': {key: value for key, value in fields.items() if value},
                'profile': 'core/v1'}
    hypothesis = action.get('hypothesis')
    if hypothesis is not None:
        C.hypothesis_name(hypothesis)
        head = {'born': action['as_of']}
        authored['hypothesis'] = {'version': 1, 'name': hypothesis, 'head': head}
    deps = body.get(fields['deps'], []) if isinstance(body, dict) else []
    C._require(isinstance(deps, list) and all(isinstance(dep, str) for dep in deps),
               'invalid_bootstrap_dependencies')
    blocked = isinstance(body, dict) and bool(P._blocked_text(body))
    C._require(not deps or (hypothesis is not None and blocked),
               'unresolved_history_subject', deps[0] if deps else '')
    gaps = {dependency: 'unavailable' for dependency in deps} if hypothesis is not None else {}

    marker = C.authority(record_id=record_id, authority='history', generation=1)
    initial_state = H.reduce({})
    initial_baseline = H.baseline(marker, {}, initial_state)
    operation_authority = C.authority(record_id=record_id, authority='legacy', generation=0)
    claim = C.make_object(subject=action['id'],
        kind='judgment' if isinstance(body, dict) and fields['deps'] in body else 'reading',
        by=by, on=recorded_at, operation=operation, body=body, authored=authored,
        pins={}, pin_gaps=gaps or None)
    if hypothesis is None:
        disposition = _act(claim, operation, recorded_at, by, 'accept',
                           str(action.get('why') or 'explicit add'))
        semantic_after = copy.deepcopy(document)
        semantic_after.setdefault(collection, {})[action['id']] = copy.deepcopy(body)
        semantic_after = A._destination(semantic_after)
        before = A._evidence(before_document, before_world)
        before['authoring'] = {'version': 1, 'action': intent, 'by': by,
            'recorded_at': recorded_at, 'archive': _archive(entry),
            'baseline': initial_baseline, 'bootstrap': {'version': 1, 'kind': BOOTSTRAP_KIND}}
        after = A._evidence(semantic_after, A._world(semantic_after))
        after['authoring'] = {'objects': sorted([claim['id'], disposition['id']]), 'notes': notes}
    else:
        disposition = _act(claim, operation, recorded_at, by, 'propose',
                           str(action.get('why') or 'explicit named hypothesis'))
        semantic_after = copy.deepcopy(document)
        semantic_after.setdefault(collection, {})[action['id']] = copy.deepcopy(body)
        before = A._evidence(before_document, before_world)
        before['hypothesis_authoring'] = {'version': 1, 'kind': 'edit', 'name': hypothesis,
            'head': copy.deepcopy(authored['hypothesis']['head']), 'action': intent,
            'operation': operation, 'recorded_at': recorded_at, 'by': by,
            'archive': _archive(entry),
            'physical': {}, 'baseline': initial_baseline,
            'bootstrap': {'version': 1, 'kind': BOOTSTRAP_KIND}}
        after = A._evidence(semantic_after, A._world(semantic_after))
        after['hypothesis_authoring'] = {'objects': sorted([claim['id'], disposition['id']])}
    cap = capabilities(semantic_after)
    receipt = T.semantic_receipt(profile='core/v1', capabilities=cap, before=before, after=after)
    objects = [claim, disposition]
    pairs = [(obj, C.encode_document(obj)) for obj in objects]

    template_source = semantic_after if hypothesis is None else document
    template = dict(C.document_template(template_source))
    template.setdefault(collection, {})
    template.setdefault('meta', {})['reasoning'] = copy.deepcopy(
        semantic_after['meta']['reasoning'] if hypothesis is None else document['meta']['reasoning'])
    empty_document = dict(copy.deepcopy(template))
    empty_document['meta']['history'] = initial_baseline
    empty_bytes = C.encode_document(empty_document)
    store = H.Store(entry)
    captured = H.Capture(empty_bytes, empty_document, marker, {}, {}, {}, initial_state,
                         initial_baseline, {})
    requires = HP.commit_requires([C.EXPLICIT_ROOT_DISPOSITION])
    draft = C.make_commit(marker=marker, operation=operation, parents={}, baseline=initial_baseline,
                          objects=pairs, receipt=receipt, view=b'', view_template=template,
                          requires=requires)
    commits = {operation: C.encode_document(draft)}
    selected = {obj['id']: obj for obj in objects}
    rendered = store.render(captured, objects=selected, commits=commits)
    manifest = C.make_commit(marker=marker, operation=operation, parents={}, baseline=initial_baseline,
                             objects=pairs, receipt=receipt, view=rendered, view_template=template,
                             requires=requires)
    manifest_raw = C.encode_document(manifest)
    state = H.reduce(selected)
    completed = replace(captured, entry_bytes=rendered, document=C.decode_document(rendered),
        commits={operation: manifest_raw}, objects=selected, state=state,
        baseline=H.baseline(marker, {operation: manifest_raw}, state),
        object_bytes={(obj['subject'], obj['id']): raw for obj, raw in pairs})
    # Exercise the public history projection before any publication is possible.
    from . import history_adapter
    history_adapter.from_store_capture(completed)

    layout = P.layout(entry)
    files = [{'path': entry.name, 'role': 'record', 'before': None, 'after': rendered},
             {'path': Path(layout['history_authority']).relative_to(entry.parent).as_posix(),
              'role': 'history_authority', 'before': None, 'after': C.encode_document(marker)}]
    for obj, raw in pairs:
        files.append({'path': (Path(layout['history']) / HP.path_for_object(obj)).relative_to(entry.parent).as_posix(),
                      'role': 'history_object', 'before': None, 'after': raw})
    files.append({'path': (Path(layout['history_commits']) / (operation + '.yaml')).relative_to(entry.parent).as_posix(),
                  'role': 'history_commit', 'before': None, 'after': manifest_raw})
    baseline = {'kind': KIND, 'direction': 'activate', 'bootstrap': {'version': 1, 'kind': BOOTSTRAP_KIND,
                    'action': intent, 'by': by, 'recorded_at': recorded_at, 'record_id': record_id,
                    'policy': copy.deepcopy(policy)},
                'transaction_root': str(entry.parent), 'source_absent': True,
                'record_members': {entry.name: None}, 'hypothesis_members': {},
                'retained_files': {}, 'history_baseline': initial_baseline}
    return T.PreparedMutation(operation=operation, authority=operation_authority, baseline=baseline,
        files=files, receipt=receipt, entry=entry.name, transition={'version': 1, 'after': marker})


def verify_prepared(entry, mutation, *, policy):
    """Re-derive every prepared byte from the retained user intent."""
    entry = Path(entry).resolve()
    data = mutation.to_data()
    bootstrap = data.get('baseline', {}).get('bootstrap')
    C._require(isinstance(bootstrap, dict) and bootstrap.get('version') == 1
               and bootstrap.get('kind') == BOOTSTRAP_KIND, 'invalid_history_bootstrap')
    C._require(data['baseline'].get('source_absent') is True
               and data['baseline'].get('record_members') == {entry.name: None}
               and data['baseline'].get('transaction_root') == str(entry.parent)
               and bootstrap.get('policy') == policy, 'history_bootstrap_changed')
    expected = prepare(entry, bootstrap['action'], policy=policy, operation=data['operation'],
                       recorded_at=bootstrap['recorded_at'], record_id=bootstrap['record_id'],
                       by=bootstrap.get('by'), _replay=True)
    C._require(identity(expected.to_data()) == identity(data), 'history_bootstrap_intent_mismatch')
    _verify_environment(entry, mutation)


def _verify_result(entry, mutation):
    captured = H.Store(entry).capture()
    data = mutation.to_data()
    C._require(set(captured.commits) == {data['operation']}
               and captured.entry_bytes == next(item['after'] for item in mutation.files
                                                if item['role'] == 'record'),
               'history_bootstrap_incomplete')


def publish(entry, mutation, *, policy, verify):
    entry = Path(entry).resolve()
    root = entry.parent
    def checked(data):
        verify_prepared(entry, mutation, policy=policy)
        verify(data)
    T.publish_transition(root, T.journal_for(entry), mutation, verify=checked)
    _verify_result(entry, mutation)
    P.forget(entry)
    return {'state': 'bootstrapped', 'operation': mutation.to_data()['operation']}


def recover(entry, *, policy, verify, direction='after'):
    entry = Path(entry).resolve()
    root = entry.parent
    raw = T._read(T._target(root, T.journal_for(entry)))
    C._require(raw is not None, 'no_recovery_pending')
    mutation = T.PreparedMutation.from_bytes(raw)
    def checked(data):
        verify_prepared(entry, mutation, policy=policy)
        verify(data)
    if direction == 'after':
        T.recover_transition(root, T.journal_for(entry), direction=direction, verify=checked)
        _verify_result(entry, mutation)
    else:
        _rollback(root, entry, mutation, checked)
    P.forget(entry)
    return {'state': 'recovered', 'direction': direction,
            'operation': mutation.to_data()['operation']}


def _rollback(root, entry, mutation, verify):
    """Remove an unpublished birth while its durable journal remains recoverable."""
    primary = T._target(root, T.journal_for(entry))
    raw = mutation.to_bytes()
    with T.directory_guards(T.participant_directories(root, mutation), exclusive=True):
        C._require(T._read(primary) == raw, 'concurrent_edit')
        T._journal_path(root, T.journal_for(entry), mutation)
        immutable, mutable = T._transition_targets(root, mutation)
        verify(mutation.to_data())
        replicas = T._prepare_replicas(root, T.journal_for(entry), mutation)
        ready = T._ready_path(primary, mutation)
        C._require(T._read(ready) in (None, mutation._data['digest'].encode('ascii')),
                   'invalid_ready_marker')
        if replicas:
            T.publish_immutable(ready, mutation._data['digest'].encode('ascii'), root=root)
        T._apply_transition(immutable, mutable, 'before', root)
        for item, path in immutable:
            current = T._read(path)
            C._require(current in (None, item['after']), 'history_bootstrap_rollback_changed')
            if current is not None:
                path.unlink()
                T._sync(path.parent)
        T._remove_journals(primary, [*replicas, *([ready] if replicas else [])])
