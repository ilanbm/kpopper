"""Guarded authority transitions for an explicitly managed single/shared record.

These callables do not select real targets or configure a deployment. A caller
must provide a deployment_guard factory that excludes old managed writers and
source/deployment replacement for the entire guarded operation. Fresh probes
identify selected resolved sources; they do not attest executing bytecode.

Existing pending ledgers remain unchanged across a single-authority transition.
Distinct per-worktree authorities still require an explicit group transition.
"""
import contextlib
import copy
import glob
import os
from pathlib import Path
import uuid
from concurrent.futures import ThreadPoolExecutor

from . import history_paths as HP
from . import history_contract as C, history_transaction as T, history_migration as M
from . import history_store as H, history_adapter as A, history_runtime as R
from . import pending_grounding as G, provenance as P, knowledge_views as V
from .reasoning.snapshot import Snapshot, _observation, _portable, _history_bytes
from .reasoning.contract import capabilities

KIND = 'history-authority-transition/v1'


def _pending(project):
    ledger = G.Store(project).snapshot() if project.git else {'ref': None, 'events': [], 'bundles': {}}
    path = project.state / 'publication.json'
    raw = T._read(path)
    return {'ledger': _portable(ledger, lambda path: path, authored=True), 'publication': T._blob(raw)}


def _project_state(entry, project):
    roots = sorted(str(root.resolve()) for root in project.worktrees())
    try:
        relative = entry.relative_to(project.root)
        records = sorted({str((Path(root) / relative).resolve()) for root in roots})
    except ValueError:
        records = [str(entry)]
    return {'root': str(project.root), 'worktrees': roots, 'records': records,
            'config': project.config(), 'pending': _pending(project),
            'observation': _observation([str(entry)], 'live')}


def _require_group_context(context):
    from .history_group_activation import _GroupContext
    C._require(isinstance(context, _GroupContext), 'group_context_required')


def _settled(state, _group_context=None):
    if _group_context is None:
        C._require(len(state['records']) == 1, 'history_group_transition_required')
    else:
        _require_group_context(_group_context)
        receipt = _group_context.receipt()
        C._mapping(receipt, ('version', 'operation', 'entries'))
        C._require(type(receipt['version']) is int and receipt['version'] == 1 and
                   isinstance(receipt['entries'], list) and receipt['entries'] == sorted(set(receipt['entries'])) and
                   all(isinstance(path, str) and Path(path).is_absolute() for path in receipt['entries']) and
                   set(receipt['entries']) == set(state['records']), 'history_group_membership_mismatch')
        C._text(receipt['operation'])
        _group_context.validate_state(state)
    ledger = state['pending']['ledger']
    C._require(isinstance(ledger['events'], list) and isinstance(ledger['bundles'], dict) and
               all(isinstance(event, dict) and event.get('revision') in ledger['bundles']
                   for event in ledger['events']) and
               {event['revision'] for event in ledger['events']} == set(ledger['bundles']),
               'invalid_transition_pending')
    C._require(all(isinstance(event.get('event_id'), str) and event['event_id'] for event in ledger['events']) and
               len({event['event_id'] for event in ledger['events']}) == len(ledger['events']),
               'invalid_transition_pending')
    for revision, portable in ledger['bundles'].items():
        bundle = {'revision': portable['revision'], 'manifest': portable['manifest'],
                  'files': _history_bytes(portable['files'])}
        C._require(revision == bundle['revision'], 'invalid_transition_pending')
        G.validate_bundle(bundle)



@contextlib.contextmanager
def _guard(entry, inventory, expected_digests, deployment_guard, *, members=(), _group_context=None):
    if _group_context is not None:
        _require_group_context(_group_context)
        # Only the live outer lifecycle can supply this context. No serialized
        # record/receipt grants lock or deployment permission.
        project = _group_context.project(entry, inventory, expected_digests)
        yield project
        return
    C._require(callable(deployment_guard), 'deployment_guard_required')
    project = V.project_for([str(entry)])
    # The callback is an explicit caller authority boundary, not a boolean claim
    # inferred from the record or the runtime declaration.
    with deployment_guard(copy.deepcopy(inventory), copy.deepcopy(expected_digests)):
        with project.lock():
            directories = {entry.parent, *(Path(path).parent for path in members)}
            for root in project.worktrees():
                if root.is_dir():
                    directories.add(root.resolve())
            with T.directory_guards(directories, exclusive=True):
                yield project


def _probe(inventory, expected_digests):
    proof = R.probe_launchers(inventory, expected_digests)
    C._require(proof.get('complete') is True and proof.get('launchers') and
               all(2 in launcher.get('declaration', {}).get('schemas', {}).get('history', {}).get(
                   'prepared_mutation', []) for launcher in proof['launchers']),
               'history_transition_runtime_unsupported')
    return proof


def _deployment(inventory, expected_digests):
    proof = _probe(inventory, expected_digests)
    return {'inventory': copy.deepcopy(inventory), 'expected_digests': copy.deepcopy(expected_digests),
            'prepared_probe': proof, 'exclusion': 'caller_guard_required_through_publication'}


def _role(entry, relative):
    layout = P.layout(entry)
    absolute = str(entry.parent / relative)
    if absolute == str(entry):
        return 'record'
    if absolute == layout['history_authority']:
        return 'history_authority'
    if absolute.startswith(layout['history_commits'] + os.sep):
        return 'history_commit'
    if absolute.startswith(layout['history'] + os.sep):
        return 'history_object'
    return 'history_retained'


def _tree(root, relative):
    directory = T._target(root, relative)
    if not directory.exists():
        return {}
    C._require(directory.is_dir() and not directory.is_symlink(), 'invalid_transition_tree', relative)
    files = {}
    for path in directory.rglob('*'):
        C._require(not path.is_symlink(), 'invalid_transition_tree', str(path))
        if path.is_file():
            C._require(len(files) < C.MAX_OBJECTS and path.stat().st_size <= T.MAX_TRANSACTION_BYTES,
                       'history_limit')
            files[path.relative_to(root).as_posix()] = C.sha256(path.read_bytes())
    return files


def _trees(entry):
    layout = P.layout(entry)
    return [Path(layout[role]).relative_to(entry.parent).as_posix() for role in ('history', 'history_commits', 'history_cancellations')] + [M.ARTIFACTS]


def _source_files(entry, files):
    return {Path(path).relative_to(entry.parent).as_posix(): C.sha256(raw) for path, raw in files.items()}


def prepare_activation(entry, *, inventory, expected_digests, deployment_guard,
                       operation=None, recorded_at=None, record_id=None, _group_context=None):
    """Prepare a same-record legacy -> history transition; no authority is changed."""
    entry = Path(entry).resolve()
    members = [Path(path).resolve() for path in P._files_of([str(entry)])]
    with _guard(entry, inventory, expected_digests, deployment_guard, members=members,
                _group_context=_group_context) as project:
        state = _project_state(entry, project)
        _settled(state, _group_context)
        C._require(not T._target(entry.parent, T.journal_for(entry)).exists(), 'recovery_required')
        marker_path = Path(P.layout(entry)['history_authority'])
        previous_bytes = T._read(marker_path)
        previous = C.validate_authority(C.decode_document(previous_bytes)) if previous_bytes is not None else None
        C._require(previous is None or previous['authority'] == 'legacy', 'history_already_active')
        C._require(not (entry.parent / M.ARTIFACTS).exists() or previous is not None, 'existing_activation_evidence')
        operation = operation or 'activate-' + uuid.uuid4().hex
        record_id = record_id or (previous['record_id'] if previous else 'record-' + uuid.uuid4().hex)
        plan = M.prepare(entry, route=False, read_mode='live' if state['config']['mode'] == 'advanced' else 'frozen',
                         operation=operation, recorded_at=recorded_at, record_id=record_id)
        C._require(not plan.problems, 'incomplete_history_import', '; '.join(plan.problems))
        before_authority = previous or C.authority(record_id=record_id, authority='legacy', generation=0)
        C._require(plan.marker['generation'] == before_authority['generation'] + 1, 'authority_generation_mismatch')
        deployment = _deployment(inventory, expected_digests)
        # The copied rehearsal validates actual loader/capture/replay of the exact
        # candidate before any same-record publication is prepared.
        import tempfile
        with tempfile.TemporaryDirectory(prefix='history-activation-rehearsal-') as temporary:
            _isolated(lambda: plan.publish(Path(temporary) / 'candidate'))
        files, retained = [], {}
        for relative, after in sorted(plan.files.items()):
            before = T._read(T._target(entry.parent, relative))
            if before == after:
                continue
            role = _role(entry, relative)
            if role == 'history_retained':
                C._require(before is None, 'existing_activation_evidence', relative)
                retained[relative] = C.sha256(after)
            files.append({'path': relative, 'role': role, 'before': before, 'after': after})
        manifest = C.decode_document(next(item['after'] for item in files if item['role'] == 'history_commit'))
        baseline = {'kind': KIND, 'direction': 'activate', 'project': state,
            **({'group': _group_context.receipt()} if _group_context is not None else {}),
            'transaction_root': str(entry.parent),
            'deployment': deployment, 'history_baseline': H.baseline(plan.marker, {}, H.reduce({})),
            'record_members': {str(path.relative_to(entry.parent)): C.sha256(path.read_bytes()) for path in members},
            'retained_files': retained, 'source_files': _source_files(entry, plan.source_files),
            'source_storage': copy.deepcopy(plan.originals),
            'reads': [{'kind': kind, 'path': path, 'value': value}
                      for (kind, path), value in sorted(plan.source.inventory.events.items())],
            'managed_trees': {relative: _tree(entry.parent, relative) for relative in _trees(entry)},
            'activation': {'operation': operation, 'recorded_at': plan.recorded_at, 'record_id': record_id,
                           'artifact_root': plan.artifacts,
                           'first_commit_sha256': C.sha256(C.encode_document(manifest))},
            **({'live_observation': plan.observation} if plan.observation else {})}
        plan.verify_source()
        C._require(G.identity(_project_state(entry, project)) == G.identity(state), 'transition_project_changed')
        prepared = T.PreparedMutation(operation=operation, authority=before_authority, baseline=baseline,
            files=files, receipt=manifest['receipt'], entry=entry.name,
            transition={'version': 1, 'after': plan.marker})
        _candidate(entry, prepared)
        return prepared


def _read_witnesses(entry, mutation, cancellation=None):
    data, files = mutation.to_data(), mutation.files
    baseline = data['baseline']
    touched = {str(entry.parent / item['path']): item for item in files}
    trees = baseline['managed_trees']
    for relative, before in trees.items():
        expected = dict(before)
        prefix = relative + '/'
        optional = {}
        for item in files:
            if item['path'].startswith(prefix) and item['role'] in ('history_object', 'history_commit', 'history_retained'):
                optional[item['path']] = C.sha256(item['after'])
        if cancellation and cancellation['receipt_path'].startswith(prefix):
            optional[cancellation['receipt_path']] = C.sha256(cancellation['receipt_bytes'])
        current = _tree(entry.parent, relative)
        C._require(all(current.get(path) == digest for path, digest in expected.items())
                   and all(path in expected or optional.get(path) == digest for path, digest in current.items()),
                   'transition_history_changed', relative)
    for relative, expected in baseline['source_files'].items():
        path = entry.parent / relative
        raw = T._read(path)
        item = touched.get(str(path))
        if item is not None:
            allowed = (item['before'], item['after'])
            if cancellation and item['role'] == 'history_authority':
                allowed += (cancellation['marker_bytes'],)
            C._require(raw in allowed, 'transition_source_changed', relative)
        else:
            C._require(raw is not None and C.sha256(raw) == expected, 'transition_source_changed', relative)
    roots = [str(entry.parent / relative) for relative in trees]
    for event in baseline['reads']:
        kind, path, expected = event['kind'], event['path'], event['value']
        if path in touched:
            continue
        if any(path == root or path.startswith(root + os.sep) or
               path.startswith(glob.escape(root) + os.sep) for root in roots):
            continue  # Complete immutable-tree membership is checked above.
        if kind == 'bytes':
            raw = T._read(Path(path))
            actual = None if raw is None else C.sha256(raw)
        elif kind == 'glob':
            actual = sorted(map(os.path.abspath, glob.glob(path)))
        elif kind == 'directory':
            actual = Path(path).is_dir()
        elif kind == 'exists':
            actual = Path(path).exists()
        else:
            raise C.HistoryError('unsupported_transition_observation', kind)
        C._require(actual == expected, 'transition_source_changed', path)


def _isolated(call):
    # Only detached replay/copy work runs here. Its temporary directory is a new
    # lock namespace and must not inherit caller-held live record lock ordering.
    # The calling thread retains every deployment/project/live record guard.
    with ThreadPoolExecutor(max_workers=1) as executor:
        return executor.submit(call).result()


@HP.replay_mutation
def _candidate(entry, mutation):
    return _isolated(lambda: _candidate_isolated(entry, mutation))


def _candidate_isolated(entry, mutation):
    """Validate exact retained transition bytes without reading a partial authority."""
    data = mutation.to_data()
    baseline = data['baseline']
    artifacts = baseline['activation'].get('artifact_root', M.ARTIFACTS)
    files = mutation.files
    record = next(item for item in files if item['role'] == 'record')
    if baseline['direction'] == 'activate':
        marker = data['transition']['after']
        commits = {data['operation']: next(item['after'] for item in files if item['role'] == 'history_commit')}
        objects = {}
        for item in files:
            if item['role'] == 'history_object':
                obj = C.validate_object(C.decode_document(item['after']))
                objects[(obj['subject'], obj['id'])] = item['after']
        selected = C.committed_objects(marker, commits, objects)
        state = H.reduce(selected)
        captured = H.Capture(record['after'], C.decode_document(record['after']), marker, commits, objects,
                            selected, state, H.baseline(marker, commits, state), {})
        C._require(H.Store(entry).render(captured) == record['after'], 'transition_view_mismatch')
        adapted = A.from_store_capture(captured)
        original_bytes = next(item['after'] for item in files if item['path'] == artifacts + '/original.json')
        original = Snapshot.from_json(original_bytes)
        C._require(M._entry_identity(adapted.document) == M._entry_identity(original.to_data()['document']),
                   'transition_meaning_mismatch')
        C._require(C.sha256(next(iter(commits.values()))) == baseline['activation']['first_commit_sha256'],
                   'transition_commit_mismatch')
        # Re-import the retained before images in an isolated source tree. File
        # locators/object envelopes are portable even though host observation
        # context (and thus complete Snapshot ids) legitimately differs there.
        import tempfile
        touched = {item['path']: item for item in files}
        with tempfile.TemporaryDirectory(prefix='history-transition-verify-') as temporary:
            root = Path(temporary)
            for relative, expected in baseline['source_files'].items():
                item = touched.get(relative)
                raw = item['before'] if item is not None else T._read(entry.parent / relative)
                C._require(raw is not None and C.sha256(raw) == expected, 'transition_source_changed', relative)
                storage = baseline.get('source_storage', {}).get(relative, {
                    'path': artifacts + '/originals/' + relative, 'sha256': expected})
                C._require(storage['sha256'] == expected, 'transition_original_mismatch', relative)
                retained = touched.get(storage['path'])
                retained_raw = retained['after'] if retained is not None else T._read(T._target(entry.parent, storage['path']))
                C._require(retained_raw == raw, 'transition_original_mismatch', relative)
                path = T._target(root, relative)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(raw)
            replay = M.prepare(root / entry.name, operation=baseline['activation']['operation'],
                recorded_at=baseline['activation']['recorded_at'], record_id=baseline['activation']['record_id'])
            C._require(not replay.problems and G.identity(replay.objects) == G.identity(selected),
                       'transition_import_mismatch')
            expected_template = copy.deepcopy(replay.template)
            for member in expected_template['meta']['history_import']['members']:
                if member['path'] == artifacts + '/original.json':
                    member['sha256'] = C.sha256(original_bytes)
            first_commit = C.decode_document(next(iter(commits.values())))
            storage = first_commit['receipt']['before'].get('originals_storage')
            if storage is not None:
                C._require(storage == baseline.get('source_storage') and storage == replay.originals,
                           'transition_original_mismatch')
            observation = baseline.get('live_observation')
            if observation is not None:
                live_item = touched.get(artifacts + '/live.json')
                C._require(live_item is not None and C.sha256(live_item['after']) == observation['sha256'] and
                           first_commit['receipt']['before'].get('observation') == observation,
                           'transition_observation_mismatch')
                live = M._validate_pending(Snapshot.from_json(live_item['after']))
                C._require(live.snapshot_id == observation['snapshot_id'] and
                           M._entry_identity(live.to_data()['document']) == M._entry_identity(original.to_data()['document']),
                           'transition_observation_mismatch')
                pending = live.to_data()['context']['pending']
                ledger = baseline['project']['pending']['ledger']
                C._require(all(G.identity(pending.get(key)) == G.identity(ledger[key])
                               for key in ('ref', 'events', 'bundles')), 'transition_observation_mismatch')
                candidate_item = touched.get(artifacts + '/candidate.json')
                C._require(candidate_item is not None, 'transition_observation_mismatch')
                candidate = M._validate_pending(Snapshot.from_json(candidate_item['after']))
                C._require(G.identity(candidate.to_data()['document']) == G.identity(adapted.document),
                           'transition_observation_mismatch')
                _same_live_context(candidate, live)
                expected_template['meta']['history_import']['members'].append({
                    'path': artifacts + '/live.json', 'sha256': observation['sha256'], 'role': 'retained_original'})
                expected_template['meta']['history_import']['members'].sort(key=lambda item: item['path'])
            else:
                C._require('observation' not in first_commit['receipt']['before'], 'transition_observation_mismatch')
            C._require(G.identity(expected_template) == G.identity(first_commit['view_template']),
                       'transition_template_mismatch')
            C._require(M._entry_identity(replay.original.to_data()['document']) ==
                       M._entry_identity(original.to_data()['document']), 'transition_meaning_mismatch')
            C._require(G.identity(replay.original.to_data()['hypotheses']) == G.identity(original.to_data()['hypotheses']),
                       'transition_hypothesis_mismatch')
    else:
        original = Snapshot.from_json(T._unblob(baseline['original_snapshot']))
        C._require(M._entry_identity(C.decode_document(record['after'])) ==
                   M._entry_identity(C.decode_document(T._unblob(baseline['original_entry']))),
                   'transition_meaning_mismatch')
        C._require(record['after'] == T._unblob(baseline['original_entry']), 'transition_original_mismatch')
        # The original snapshot/bytes must be bound by the still-retained first
        # import commit, not merely asserted by a newly prepared inverse.
        path = Path(P.layout(entry)['history_commits']) / (baseline['activation']['operation'] + '.yaml')
        raw = path.read_bytes()
        C._require(C.sha256(raw) == baseline['activation']['first_commit_sha256'], 'transition_commit_mismatch')
        first = C.validate_commit(C.decode_document(raw))
        C._require(first['receipt']['before']['snapshot_id'] == original.snapshot_id and
                   first['receipt']['before']['source'][entry.name] == C.sha256(record['after']),
                   'transition_original_mismatch')
        Snapshot.from_json(original.to_json())


def _verify(entry, mutation, project, _group_context=None):
    data = mutation.to_data()
    C._require(data['entry'] == entry.name and data['baseline'].get('kind') == KIND,
               'invalid_authority_transition')
    state = _project_state(entry, project)
    _settled(state, _group_context)
    C._require(data['baseline'].get('group') == (_group_context.receipt() if _group_context is not None else None),
               'group_recovery_required')
    C._require(G.identity(state) == G.identity(data['baseline']['project']), 'transition_project_changed')
    _read_witnesses(entry, mutation)
    _candidate(entry, mutation)
    deployment = data['baseline']['deployment']
    _probe(deployment['inventory'], deployment['expected_digests'])
    _read_witnesses(entry, mutation)
    C._require(G.identity(_project_state(entry, project)) == G.identity(state), 'transition_project_changed')


def prepare_deactivation(entry, activation, *, inventory, expected_digests, deployment_guard,
                         operation=None, _group_context=None):
    """Prepare a lossless inverse only when no newer committed/pending evidence exists."""
    entry = Path(entry).resolve()
    C._require(isinstance(activation, T.PreparedMutation) and
               activation.to_data()['baseline'].get('direction') == 'activate', 'activation_receipt_required')
    original_data = activation.to_data()
    members = [entry.parent / path for path in original_data['baseline']['record_members']]
    with _guard(entry, inventory, expected_digests, deployment_guard, members=members,
                _group_context=_group_context) as project:
        state = _project_state(entry, project)
        _settled(state, _group_context)
        C._require(G.identity(state) == G.identity(original_data['baseline']['project']), 'transition_project_changed')
        _read_witnesses(entry, activation)
        _candidate(entry, activation)
        captured = H.Store(entry).capture()
        C._require(captured.marker == original_data['transition']['after'] and
                   set(captured.commits) == {original_data['operation']} and
                   C.sha256(captured.commits[original_data['operation']]) ==
                   original_data['baseline']['activation']['first_commit_sha256'], 'newer_history_not_representable')
        original_entry = next(item['before'] for item in activation.files if item['role'] == 'record')
        original_snapshot = next(item['after'] for item in activation.files
                                 if item['path'] == original_data['baseline']['activation'].get('artifact_root', M.ARTIFACTS) + '/original.json')
        marker = C.authority(record_id=captured.marker['record_id'], authority='legacy',
                             generation=captured.marker['generation'] + 1, cancellations=captured.marker.get('cancellations'))
        after = Snapshot.from_json(original_snapshot)
        receipt = T.semantic_receipt(profile=capabilities(after.to_data()['document'])['profile'],
            capabilities=capabilities(after.to_data()['document']),
            before={'kind': KIND, 'baseline': captured.baseline, 'activation_digest': original_data['digest']},
            after={'kind': 'original-with-inactive-history/v1', 'snapshot_id': after.snapshot_id})
        baseline = {'kind': KIND, 'direction': 'deactivate', 'project': state,
            **({'group': _group_context.receipt()} if _group_context is not None else {}),
            'transaction_root': str(entry.parent),
            'deployment': _deployment(inventory, expected_digests),
            'record_members': {str(path.relative_to(entry.parent)): C.sha256(path.read_bytes()) for path in members},
            'retained_files': {},
            'source_files': {relative: C.sha256((entry.parent / relative).read_bytes())
                             for relative in original_data['baseline']['source_files']
                             if relative != Path(P.layout(entry)['history_authority']).relative_to(entry.parent).as_posix()},
            'reads': copy.deepcopy(original_data['baseline']['reads']),
            'managed_trees': {relative: _tree(entry.parent, relative) for relative in _trees(entry)},
            'activation': original_data['baseline']['activation'],
            **({'live_observation': original_data['baseline']['live_observation']}
               if 'live_observation' in original_data['baseline'] else {}),
            'original_snapshot': T._blob(original_snapshot), 'original_entry': T._blob(original_entry)}
        files = [{'path': entry.name, 'role': 'record', 'before': captured.entry_bytes, 'after': original_entry},
                 {'path': Path(P.layout(entry)['history_authority']).relative_to(entry.parent).as_posix(),
                  'role': 'history_authority', 'before': C.encode_document(captured.marker),
                  'after': C.encode_document(marker)}]
        return T.PreparedMutation(operation=operation or 'deactivate-' + uuid.uuid4().hex,
            authority=captured.marker, baseline=baseline, files=files, receipt=receipt, entry=entry.name,
            transition={'version': 1, 'after': marker})


def _same_live_context(actual, expected):
    left, right = actual.to_data(), expected.to_data()
    for key in ('pending', 'target', 'project', 'history_contributions'):
        C._require(G.identity(left['context'].get(key)) == G.identity(right['context'].get(key)),
                   'transition_live_context_mismatch', key)
    C._require(M._hypothesis_identity(left['hypotheses']) == M._hypothesis_identity(right['hypotheses']),
               'transition_live_context_mismatch', 'hypotheses')


def _verify_live_result(entry, mutation):
    data = mutation.to_data()
    observation = data['baseline'].get('live_observation')
    actual = Snapshot.capture([str(entry)], read_mode='live' if observation else 'frozen')
    if observation is None:
        return actual
    path = T._target(entry.parent, observation['path'])
    raw = T._read(path)
    C._require(raw is not None and C.sha256(raw) == observation['sha256'], 'transition_observation_mismatch')
    expected = M._validate_pending(Snapshot.from_json(raw))
    C._require(expected.snapshot_id == observation['snapshot_id'], 'transition_observation_mismatch')
    _same_live_context(actual, expected)
    C._require(M._entry_identity(actual.to_data()['document']) == M._entry_identity(expected.to_data()['document']),
               'transition_meaning_mismatch')
    return actual


def _finalize(entry, mutation, project):
    try:
        _verify(entry, mutation, project)
    except (ValueError, OSError) as error:
        raise C.HistoryError('transition_unfinalized',
                            'images are durable; recovery verification failed: ' + str(error)) from error


def cancellation_plan(entry, mutation):
    """Deterministic compensation plan; no files or epochs are changed here."""
    entry = Path(entry).resolve()
    data = mutation.to_data()
    C._require(data['entry'] == entry.name and data['baseline'].get('kind') == KIND and
               data['baseline'].get('direction') == 'activate', 'invalid_generation_cancellation')
    reserved = data['transition']['after']
    before = data['authority']
    C._require(reserved['authority'] == 'history' and before['authority'] == 'legacy' and
               reserved['generation'] == before['generation'] + 1, 'invalid_generation_cancellation')
    record = next(item for item in mutation.files if item['role'] == 'record')
    commit_item = next(item for item in mutation.files if item['role'] == 'history_commit')
    manifest = C.validate_commit(C.decode_document(commit_item['after']))
    immutable = [item for item in mutation.files if item['role'] in ('history_object', 'history_commit', 'history_retained')]
    receipt = C.validate_cancellation({'version': 1, 'kind': C.CANCELLATION_CAPABILITY,
        'record_id': reserved['record_id'], 'operation': data['operation'],
        'reserved_generation': reserved['generation'], 'legacy_generation': reserved['generation'] + 1,
        'before_authority_digest': G.identity(before), 'reserved_authority_digest': G.identity(reserved),
        'mutation_digest': data['digest'], 'entry': entry.name, 'original_entry_sha256': C.sha256(record['before']),
        'manifest_sha256': C.sha256(commit_item['after']),
        'objects': {item['id']: {'subject': item['subject'], 'sha256': item['sha256']} for item in manifest['objects']},
        'artifacts': {item['path']: C.sha256(item['after']) for item in immutable if item['role'] == 'history_retained'},
        'visibility': 'never_released_to_readers'})
    receipt_bytes = C.encode_document(receipt)
    filename = data['operation'] + '.yaml'
    cancellations = copy.deepcopy(before.get('cancellations', {}))
    C._require(str(reserved['generation']) not in cancellations, 'cancellation_generation_collision')
    cancellations[str(reserved['generation'])] = {'operation': data['operation'], 'path': filename,
        'sha256': C.sha256(receipt_bytes), 'reserved_generation': reserved['generation'],
        'legacy_generation': reserved['generation'] + 1}
    marker = C.authority(record_id=reserved['record_id'], authority='legacy', generation=reserved['generation'] + 1,
                         cancellations=cancellations)
    path = Path(P.layout(entry)['history_cancellations']) / filename
    plan = {'version': 1, 'operation': data['operation'], 'mutation_digest': data['digest'],
            'receipt_path': path.relative_to(entry.parent).as_posix(), 'receipt_bytes': receipt_bytes,
            'marker_bytes': C.encode_document(marker), 'original_entry': record['before'],
            'immutable_images': [{'path': item['path'], 'after': item['after']} for item in immutable]}
    plan['digest'] = G.identity({'mutation': data['digest'], 'receipt': C.sha256(receipt_bytes),
                               'marker': C.sha256(plan['marker_bytes']), 'entry': C.sha256(record['before'])})
    return plan


def verify_cancellation(entry, mutation, project, plan, _group_context=None, *, completed=False):
    """Bind exact intermediate compensation, or require its complete terminal state."""
    entry = Path(entry).resolve()
    expected = cancellation_plan(entry, mutation)
    C._require(plan == expected, 'cancellation_plan_mismatch')
    data = mutation.to_data()
    state = _project_state(entry, project)
    _settled(state, _group_context)
    C._require(data['baseline'].get('group') == (_group_context.receipt() if _group_context is not None else None),
               'group_recovery_required')
    C._require(G.identity(state) == G.identity(data['baseline']['project']), 'transition_project_changed')
    _read_witnesses(entry, mutation, plan)
    _candidate(entry, mutation)
    deployment = data['baseline']['deployment']
    _probe(deployment['inventory'], deployment['expected_digests'])
    receipt = T._read(T._target(entry.parent, plan['receipt_path']))
    C._require(receipt in (None, plan['receipt_bytes']), 'cancellation_receipt_collision')
    for item in mutation.files:
        raw = T._read(T._target(entry.parent, item['path']))
        if item['role'] == 'record':
            C._require(raw == item['before'] if completed else raw in (item['before'], item['after']),
                       'cancellation_entry_mismatch')
        elif item['role'] == 'history_authority':
            C._require(raw == plan['marker_bytes'] if completed else raw in (item['before'], item['after'], plan['marker_bytes']),
                       'cancellation_authority_mismatch')
        else:
            C._require(raw == item['after'] if completed else raw in (None, item['after']),
                       'cancellation_immutable_mismatch', item['path'])
    marker_bytes = T._read(Path(P.layout(entry)['history_authority']))
    if completed or marker_bytes == plan['marker_bytes']:
        C._require(receipt == plan['receipt_bytes'], 'missing_cancellation_receipt')
        layout = P.layout(entry)
        commits = {path.stem: path.read_bytes() for path in Path(layout['history_commits']).glob('*.yaml')}
        storage = {path.relative_to(Path(layout['history'])).as_posix(): path.read_bytes()
                   for path in Path(layout['history']).glob('*/*.yaml')}
        objects, _ = C.objects_from_storage(commits, storage)
        cancellations = {path.name: path.read_bytes() for path in Path(layout['history_cancellations']).glob('*.yaml')}
        C.committed_generations(C.decode_document(plan['marker_bytes']), commits, objects, cancellations)
        for item in plan['immutable_images']:
            C._require(T._read(T._target(entry.parent, item['path'])) == item['after'], 'cancellation_immutable_mismatch')
    C._require(G.identity(_project_state(entry, project)) == G.identity(state), 'transition_project_changed')
    return plan


def apply_cancellation(entry, mutation, plan):
    """Apply already-guarded compensation; never remove the single/group journal."""
    entry = Path(entry).resolve()
    C._require(plan == cancellation_plan(entry, mutation), 'cancellation_plan_mismatch')
    for item in plan['immutable_images']:
        T.publish_immutable(T._target(entry.parent, item['path']), item['after'], root=entry.parent)
    T.publish_immutable(T._target(entry.parent, plan['receipt_path']), plan['receipt_bytes'], root=entry.parent)
    # All intended immutable bytes are audit evidence only; readers remain
    # excluded by their existing journal until the legacy epoch is durable.
    T._replace(entry, plan['original_entry'])
    T._replace(Path(P.layout(entry)['history_authority']), plan['marker_bytes'])


def publish(entry, mutation, *, deployment_guard):
    entry = Path(entry).resolve()
    data = mutation.to_data()
    C._require('group' not in data['baseline'], 'group_recovery_required')
    deployment = data['baseline']['deployment']
    members = [entry.parent / path for path in data['baseline']['record_members']]
    with _guard(entry, deployment['inventory'], deployment['expected_digests'], deployment_guard,
                members=members) as project:
        T.publish_transition(entry.parent, T.journal_for(entry), mutation,
                             verify=lambda unused: _verify(entry, mutation, project),
                             on_committed=lambda unused: _finalize(entry, mutation, project))
        _verify_live_result(entry, mutation)
    P.forget(entry)
    return {'state': 'activated' if data['baseline']['direction'] == 'activate' else 'deactivated',
            'operation': data['operation'], 'authority': data['transition']['after']}


def recover(entry, *, deployment_guard, direction='after'):
    entry = Path(entry).resolve()
    raw = T._read(T._target(entry.parent, T.journal_for(entry)))
    C._require(raw is not None, 'no_recovery_pending')
    mutation = T.PreparedMutation.from_bytes(raw)
    data = mutation.to_data()
    C._require('group' not in data['baseline'], 'group_recovery_required')
    deployment = data['baseline']['deployment']
    members = [entry.parent / path for path in data['baseline']['record_members']]
    cancellation = None
    with _guard(entry, deployment['inventory'], deployment['expected_digests'], deployment_guard,
                members=members) as project:
        if data['baseline']['direction'] == 'activate' and direction == 'before':
            plan = cancellation_plan(entry, mutation)
            verify_cancellation(entry, mutation, project, plan)
            apply_cancellation(entry, mutation, plan)
            verify_cancellation(entry, mutation, project, plan, completed=True)
            cancellation = {'cancelled_generation': data['transition']['after']['generation'],
                            'authority': C.decode_document(plan['marker_bytes']),
                            'cancellation_receipt': plan['receipt_path']}
            primary = T._target(entry.parent, T.journal_for(entry))
            replicas = T._journal_replicas(entry.parent, T.journal_for(entry), mutation)
            ready = T._ready_path(primary, mutation)
            T._remove_journals(primary, [*replicas, *([ready] if replicas else [])])
        else:
            if data['baseline']['direction'] == 'activate':
                plan = cancellation_plan(entry, mutation)
                C._require(not T._target(entry.parent, plan['receipt_path']).exists(), 'generation_cancellation_recovery_required')
            T.recover_transition(entry.parent, T.journal_for(entry), direction=direction,
                                 verify=lambda unused: _verify(entry, mutation, project),
                                 on_committed=lambda unused: _finalize(entry, mutation, project))
        _verify_live_result(entry, mutation)
    P.forget(entry)
    result = {'state': 'recovered', 'direction': direction, 'operation': data['operation']}
    if cancellation is not None:
        result.update(cancellation)
    return result
