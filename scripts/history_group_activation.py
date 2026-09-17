"""Explicit all-selected-worktree authority transitions, with one retained group.

Targets are caller-selected; this module neither discovers activation intent nor
manages a real deployment. Every member keeps its exact independently verified
v2 transition. One external deployment guard and ordered project/directory locks
cover the complete group. Publication uses the shared grouped journal primitive.
Cancelling a started activation fences its reserved generation and restores the
authored bytes at a new legacy epoch; cancelled generations are never selected.
"""
import contextlib
import contextvars
import copy
import datetime
import json
import os
import threading
from pathlib import Path
import uuid

from . import history_activation as A, history_contract as C, history_transaction as T
from . import knowledge_views as V, pending_grounding as G, provenance as P

KIND = 'history-authority-group/v1'
MAX_MEMBERS = 32
_ACTIVE_CONTEXT = contextvars.ContextVar("history_authority_group_context", default=None)


def _entries(entries):
    C._require(isinstance(entries, (list, tuple)) and 2 <= len(entries) <= MAX_MEMBERS,
               'invalid_group_entries')
    result = sorted(str(Path(entry).resolve()) for entry in entries)
    C._require(len(set(result)) == len(result), 'duplicate_group_entry')
    # Distinct authorities sharing a directory cannot safely have overlapping
    # reader-owned history roles. Support one selected entry per directory.
    C._require(len({str(Path(path).parent) for path in result}) == len(result),
               'overlapping_group_entries')
    return result


class GroupPrepared:
    """Bounded exact retry envelope, including all private participant before bytes."""
    def __init__(self, *, operation, entries, mutations, inventory, expected_digests):
        C._text(operation)
        entries = _entries(entries)
        C._require(isinstance(mutations, dict) and set(mutations) == set(entries), 'group_membership_mismatch')
        members, directions = [], set()
        group = {'version': 1, 'operation': operation, 'entries': entries}
        for entry in entries:
            mutation = mutations[entry]
            C._require(isinstance(mutation, T.PreparedMutation), 'invalid_authority_transition')
            data = mutation.to_data()
            baseline = data['baseline']
            C._require(data['version'] == 2 and baseline.get('kind') == A.KIND and
                       baseline.get('group') == group and data['entry'] == Path(entry).name and
                       baseline.get('transaction_root') == str(Path(entry).parent), 'group_member_mismatch')
            deployment = baseline['deployment']
            C._require(G.identity(deployment['inventory']) == G.identity(inventory) and
                       G.identity(deployment['expected_digests']) == G.identity(expected_digests),
                       'group_deployment_mismatch')
            directions.add(baseline['direction'])
            members.append({'entry': entry, 'mutation': T._blob(mutation.to_bytes())})
        C._require(len(directions) == 1 and directions <= {'activate', 'deactivate'}, 'group_direction_mismatch')
        payload = {'version': 1, 'kind': KIND, 'operation': operation, 'entries': entries,
            'direction': next(iter(directions)), 'members': members,
            'inventory': copy.deepcopy(inventory), 'expected_digests': copy.deepcopy(expected_digests)}
        self._data = C.detached({**payload, 'digest': G.identity(payload)}, T.MAX_TRANSACTION_BYTES)
        self.to_bytes()

    @property
    def entries(self):
        return list(self._data['entries'])

    @property
    def mutations(self):
        return {item['entry']: T.PreparedMutation.from_bytes(T._unblob(item['mutation']))
                for item in self._data['members']}

    def to_data(self):
        return copy.deepcopy(self._data)

    def to_bytes(self):
        raw = G.json_bytes(G._encode(self._data))
        C._require(len(raw) <= T.MAX_TRANSACTION_BYTES, 'history_limit')
        return raw

    @classmethod
    def from_bytes(cls, raw):
        C._require(type(raw) is bytes and len(raw) <= T.MAX_TRANSACTION_BYTES, 'history_limit')
        from .reasoning.snapshot import _json_object, _json_constant, _check_typed_json
        try:
            encoded = json.loads(raw.decode('utf-8'), object_pairs_hook=_json_object, parse_constant=_json_constant)
            _check_typed_json(encoded)
            data = G._decode(encoded)
            C._require(G._encode(data) == encoded, 'invalid_group_journal')
            C._mapping(data, ('version', 'kind', 'operation', 'entries', 'direction', 'members',
                              'inventory', 'expected_digests', 'digest'))
            C._require(type(data['version']) is int and data['version'] == 1 and data['kind'] == KIND,
                       'invalid_group_journal')
            C._require(isinstance(data['members'], list) and
                       [item['entry'] for item in data['members']] == data['entries'], 'group_membership_mismatch')
            mutations = {}
            for item in data['members']:
                C._mapping(item, ('entry', 'mutation'))
                mutations[item['entry']] = T.PreparedMutation.from_bytes(T._unblob(item['mutation']))
            result = cls(operation=data['operation'], entries=data['entries'], mutations=mutations,
                         inventory=data['inventory'], expected_digests=data['expected_digests'])
            C._require(result.to_bytes() == raw, 'invalid_group_journal')
            return result
        except (KeyError, TypeError, ValueError, AttributeError, RecursionError) as error:
            if isinstance(error, C.HistoryError):
                raise
            raise C.HistoryError('invalid_group_journal') from error


class _GroupContext:
    """An active outer guard with verified exact selected routing, never a bypass flag."""
    def __init__(self, entries, projects, operation, inventory, expected_digests):
        self.entries = entries
        self.projects = projects
        self.operation = operation
        self.inventory = copy.deepcopy(inventory)
        self.expected_digests = copy.deepcopy(expected_digests)
        self.active = False

    def receipt(self):
        C._require(self.active and _ACTIVE_CONTEXT.get() is self, 'inactive_group_context')
        held = {path for pid, thread, path in T._LOCK_PATHS.get()
                if (pid, thread) == (os.getpid(), threading.get_ident())}
        C._require({str(Path(entry).parent) for entry in self.entries} <= held, 'group_locks_required')
        return {'version': 1, 'operation': self.operation, 'entries': list(self.entries)}

    def validate_state(self, state):
        C._require(self.active and _ACTIVE_CONTEXT.get() is self, 'inactive_group_context')
        matching = [entry for entry, project in self.projects.items()
                    if str(project.root) == state.get('root')]
        C._require(len(matching) == 1 and state.get('observation', {}).get('record') == matching[0],
                   'group_routing_mismatch')
        current = V.project_for([matching[0]])
        saved = self.projects[matching[0]]
        C._require(current.git and current.common == saved.common and current.root == saved.root,
                   'group_project_changed')
        C._require(set(state['records']) == set(self.entries), 'group_inventory_mismatch')

    def project(self, entry, inventory, expected_digests):
        entry = str(Path(entry).resolve())
        C._require(self.active and _ACTIVE_CONTEXT.get() is self and entry in self.projects, 'invalid_group_context')
        C._require(G.identity(inventory) == G.identity(self.inventory) and
                   G.identity(expected_digests) == G.identity(self.expected_digests), 'group_deployment_mismatch')
        project = self.projects[entry]
        self.validate_state(A._project_state(Path(entry), project))
        held = {path for pid, thread, path in T._LOCK_PATHS.get()
                if (pid, thread) == (os.getpid(), threading.get_ident())}
        members = {str(Path(path).resolve().parent) for path in P._files_of([entry])}
        C._require(members <= held, 'group_member_locks_changed')
        return project


@contextlib.contextmanager
def _guard(entries, operation, inventory, expected_digests, deployment_guard, *, mutations=None):
    C._require(callable(deployment_guard), 'deployment_guard_required')
    entries = _entries(entries)
    projects = {entry: V.project_for([entry]) for entry in entries}
    C._require(all(project.git for project in projects.values()) and
               len({str(project.common.resolve()) for project in projects.values()}) == 1,
               'group_requires_one_git_project')
    with deployment_guard(copy.deepcopy(inventory), copy.deepcopy(expected_digests)):
        with contextlib.ExitStack() as stack:
            # Worktrees share a project policy lock: acquire each namespace once.
            unique = {str(project.state.resolve()): project for project in projects.values()}
            for key in sorted(unique):
                stack.enter_context(unique[key].lock())
            directories = {Path(entry).parent for entry in entries}
            for entry, project in projects.items():
                directories.update(root.resolve() for root in project.worktrees() if root.is_dir())
                if mutations is None:
                    directories.update(Path(path).resolve().parent for path in P._files_of([entry]))
                else:
                    directories.update(T.participant_directories(Path(entry).parent, mutations[entry]))
            stack.enter_context(T.directory_guards(directories, exclusive=True))
            context = _GroupContext(entries, projects, operation, inventory, expected_digests)
            context.active = True
            token = _ACTIVE_CONTEXT.set(context)
            try:
                for entry, project in projects.items():
                    context.validate_state(A._project_state(Path(entry), project))
                yield context
            finally:
                _ACTIVE_CONTEXT.reset(token)
                context.active = False


def prepare_group(entries, *, inventory, expected_digests, deployment_guard,
                  operation=None, recorded_at=None, activations=None):
    """Prepare all explicitly selected entries for activation or a lossless inverse."""
    entries = _entries(entries)
    operation = operation or 'authority-group-' + uuid.uuid4().hex
    recorded_at = recorded_at or datetime.datetime.now(datetime.timezone.utc).isoformat()
    if activations is not None:
        C._require(isinstance(activations, GroupPrepared) and activations.entries == entries and
                   activations.to_data()['direction'] == 'activate', 'group_activation_receipt_required')
        originals = activations.mutations
    else:
        originals = None
    with _guard(entries, operation, inventory, expected_digests, deployment_guard, mutations=originals) as context:
        mutations = {}
        for entry in entries:
            member_operation = 'group-member-' + G.identity({'operation': operation, 'entry': entry})
            kwargs = dict(inventory=inventory, expected_digests=expected_digests,
                          deployment_guard=deployment_guard, operation=member_operation, _group_context=context)
            if originals is None:
                mutations[entry] = A.prepare_activation(entry, recorded_at=recorded_at, **kwargs)
            else:
                mutations[entry] = A.prepare_deactivation(entry, originals[entry], **kwargs)
        # Preparing a later participant must not stale an earlier source observation.
        for entry, mutation in mutations.items():
            A._verify(Path(entry), mutation, context.projects[entry], _group_context=context)
        return GroupPrepared(operation=operation, entries=entries, mutations=mutations,
                             inventory=inventory, expected_digests=expected_digests)


def journal_for(group):
    """One canonical private coordinator; never infer or enlarge target selection."""
    C._require(isinstance(group, GroupPrepared), 'invalid_group_journal')
    entry = Path(group.entries[0])
    primary = T._target(entry.parent, T.journal_for(entry))
    token = C.sha256(group.to_data()['operation'].encode('utf-8'))
    result = primary.parent / ('group-' + token + '.json')
    mutation = group.mutations[str(entry)]
    for target in (result, result.with_suffix('.ready'), result.with_suffix('.complete'), result.with_suffix('.cancel')):
        T._journal_path(entry.parent, target.relative_to(entry.parent).as_posix(), mutation)
    return result


def _group_paths(group):
    coordinator = journal_for(group)
    return coordinator, coordinator.with_suffix('.ready'), coordinator.with_suffix('.complete')


def _local_guards(group):
    coordinator = journal_for(group)
    data = group.to_data()
    guards = {}
    for entry, mutation in group.mutations.items():
        path = Path(entry)
        primary = T._journal_path(path.parent, T.journal_for(path), mutation)
        guard = {'version': 1, 'kind': 'history-authority-group-guard/v1',
            'operation': data['operation'], 'digest': data['digest'], 'entry': entry,
            'coordinator': str(coordinator)}
        guards[primary] = (path.parent, G.json_bytes(G._encode(guard)))
        for replica in T._journal_replicas(path.parent, T.journal_for(path), mutation):
            T._journal_path(path.parent, replica.relative_to(path.parent).as_posix(), mutation)
            raw = G.json_bytes(T.member_guard(mutation, path.parent, replica.parent.parent))
            C._require(replica not in guards, 'overlapping_group_guards')
            guards[replica] = (path.parent, raw)
    return guards


def _ready_bytes(group):
    data = group.to_data()
    return G.json_bytes({'kind': 'history-authority-group-ready/v1', 'digest': data['digest'],
                         'guards': {str(path): C.sha256(raw) for path, (_, raw) in sorted(_local_guards(group).items())}})


def _complete_bytes(group, direction, *, cancelled=False, compensated=False):
    C._require(not (cancelled and compensated), 'invalid_group_completion')
    return G.json_bytes({'kind': 'history-authority-group-complete/v1',
                         'digest': group.to_data()['digest'], 'direction': direction,
                         'phase': 'cancelled-before-ready' if cancelled else 'compensated' if compensated else 'applied'})


def _verify(group, context):
    for entry, mutation in group.mutations.items():
        A._verify(Path(entry), mutation, context.projects[entry], _group_context=context)


def _targets(group, *, initial=False):
    result = []
    # Reuse strict v2 transition preflight for every participant before applying
    # any participant. No ordinary per-entry publish/cleanup operation is used.
    for entry, mutation in group.mutations.items():
        root = Path(entry).parent
        immutable, mutable = T._transition_targets(root, mutation)
        if initial:
            C._require(all(T._read(path) == item['before'] for item, path in mutable), 'concurrent_edit')
        result.append((root, immutable, mutable))
    return result


def _validate_guards(group, *, complete=False):
    for path, (_, raw) in _local_guards(group).items():
        C._require(T._read(path) in ((None, raw) if complete else (raw,)),
                   'group_guard_mismatch', str(path))


def _install_guards(group):
    for path, (root, raw) in sorted(_local_guards(group).items()):
        C._require(T._read(path) in (None, raw), 'group_guard_mismatch', str(path))
        T.publish_immutable(path.parent / '.gitignore', b'*\n', root=root)
        T.publish_immutable(path, raw, root=root)


def _cleanup(group):
    coordinator, ready, _ = _group_paths(group)
    # A durable completion receipt precedes ANY guard removal. A crash during
    # this loop cannot expose an incomplete participant generation.
    T._remove_journals(coordinator, [*sorted(_local_guards(group)), ready, coordinator.with_suffix('.cancel')])


def _cancellation_plans(group):
    C._require(group.to_data()['direction'] == 'activate', 'invalid_group_cancellation')
    return {entry: A.cancellation_plan(Path(entry), mutation)
            for entry, mutation in group.mutations.items()}


def _cancellation_bytes(group, plans):
    return G.json_bytes({'version': 1, 'kind': 'history-authority-group-cancellation/v1',
        'digest': group.to_data()['digest'], 'direction': 'before',
        'members': {entry: {'plan_digest': plan['digest'], 'receipt_sha256': C.sha256(plan['receipt_bytes'])}
                    for entry, plan in plans.items()}})


def _verify_compensation(group, context, plans, *, completed=False):
    for entry, mutation in group.mutations.items():
        try:
            A.verify_cancellation(Path(entry), mutation, context.projects[entry], plans[entry],
                                  _group_context=context, completed=completed)
        except C.HistoryError as error:
            # Recovery never overwrites an independent edit. Save that edit
            # separately, restore this member's retained exact before image, and
            # rerun before-recovery with the same coordinator and guards intact.
            record = next(item for item in mutation.files if item['role'] == 'record')
            raw = T._read(Path(entry))
            detail = 'member=' + entry + '; ' + str(error)
            if raw not in (record['before'], record['after']):
                digest = C.sha256(raw) if raw is not None else 'absent'
                detail += ('; divergent entry SHA-256=' + digest +
                    '; expected original SHA-256=' + C.sha256(record['before']) +
                    '; expected generated SHA-256=' + C.sha256(record['after']) +
                    '; save the divergent bytes separately, restore the exact record.before image '
                    'from this member in ' + str(journal_for(group)) +
                    ', verify its original SHA-256, then repeat group recovery before; keep all guards')
            raise C.HistoryError(error.code, detail) from error


def _finalize_compensation(group, context, plans):
    try:
        _verify_compensation(group, context, plans, completed=True)
    except (ValueError, OSError) as error:
        raise C.HistoryError('transition_unfinalized',
            'group=' + group.to_data()['operation'] + '; compensation terminal verification failed; ' + str(error)) from error


def _compensate(group, context, *, completed=False):
    """Fence every reserved generation before releasing any participant guard."""
    coordinator, ready, completion = _group_paths(group)
    plans = _cancellation_plans(group)
    phase = coordinator.with_suffix('.cancel')
    expected = _cancellation_bytes(group, plans)
    if completed:
        # Cleanup may have removed the phase after the terminal proof became
        # durable. The member terminal states remain mandatory in either case.
        C._require(T._read(phase) in (None, expected), 'group_cancellation_mismatch')
        _finalize_compensation(group, context, plans)
        _cleanup(group)
        return
    C._require(T._read(ready) == _ready_bytes(group), 'invalid_group_ready_marker')
    _validate_guards(group)
    C._require(T._read(phase) in (None, expected), 'group_cancellation_mismatch')
    _verify_compensation(group, context, plans)
    T.publish_immutable(phase, expected, root=Path(group.entries[0]).parent)
    # All member source/routing/runtime proofs pass before the first metadata
    # epoch changes. Each helper retains the complete cancelled audit closure.
    for entry, mutation in group.mutations.items():
        A.apply_cancellation(Path(entry), mutation, plans[entry])
    _finalize_compensation(group, context, plans)
    T.publish_immutable(completion, _complete_bytes(group, 'before', compensated=True),
                        root=Path(group.entries[0]).parent)
    _cleanup(group)


def _finish(group, context, direction):
    if direction == 'before' and group.to_data()['direction'] == 'activate':
        _compensate(group, context)
        return
    targets = _targets(group)
    for root, immutable, mutable in targets:
        T._apply_transition(immutable, mutable, direction, root)
    try:
        _verify(group, context)
        # Callbacks cannot waive exact final-byte checks.
        for _, _, mutable in _targets(group):
            C._require(all(T._read(path) == item[direction] for item, path in mutable), 'concurrent_edit')
    except (ValueError, OSError) as error:
        raise C.HistoryError('transition_unfinalized',
            'group=' + group.to_data()['operation'] + '; images are durable; ' + str(error)) from error
    coordinator, _, completed = _group_paths(group)
    T.publish_immutable(completed, _complete_bytes(group, direction), root=Path(group.entries[0]).parent)
    _cleanup(group)


def publish(group, *, deployment_guard):
    """Publish all selected authorities while every integrated reader is guarded."""
    C._require(isinstance(group, GroupPrepared), 'invalid_group_journal')
    data = group.to_data()
    mutations = group.mutations
    with _guard(group.entries, data['operation'], data['inventory'], data['expected_digests'],
                deployment_guard, mutations=mutations) as context:
        coordinator, ready, completed = _group_paths(group)
        if T._read(completed) is not None:
            C._require(T._read(completed) == _complete_bytes(group, 'after'), 'group_operation_collision')
            _verify(group, context)
            C._require(all(T._read(path) == item['after'] for _, _, mutable in _targets(group)
                           for item, path in mutable), 'concurrent_edit')
            C._require(not coordinator.exists(), 'group_recovery_required', str(coordinator))
        else:
            C._require(not coordinator.exists() and not ready.exists() and
                       not coordinator.with_suffix('.cancel').exists(), 'group_recovery_required', str(coordinator))
            for path in _local_guards(group):
                C._require(not path.exists(), 'recovery_required', str(path))
            _targets(group, initial=True)
            _verify(group, context)
            _targets(group, initial=True)
            root = Path(group.entries[0]).parent
            T.publish_immutable(coordinator.parent / '.gitignore', b'*\n', root=root)
            T.publish_immutable(coordinator, group.to_bytes(), root=root)
            _install_guards(group)
            _validate_guards(group)
            _targets(group, initial=True)
            T.publish_immutable(ready, _ready_bytes(group), root=root)
            _finish(group, context, 'after')
        for entry, mutation in mutations.items():
            A._verify_live_result(Path(entry), mutation)
            P.forget(entry)
    return {'state': 'activated' if data['direction'] == 'activate' else 'deactivated',
            'operation': data['operation'], 'entries': group.entries, 'journal': str(journal_for(group))}


def _cancel_unready(group, context):
    """Cancel unstarted preparation without overwriting later unguarded edits."""
    guards = _local_guards(group)
    for path, (_, expected) in guards.items():
        C._require(T._read(path) in (None, expected), 'group_guard_mismatch', str(path))
    for entry, mutation in group.mutations.items():
        root = Path(entry).parent
        primary = T._target(root, T.journal_for(Path(entry)))
        guarded = T._read(primary) is not None
        # Missing ready is not itself proof publication never started: exact
        # after images or newly staged immutable bytes forbid cancellation.
        for item in mutation.files:
            path = T._target(root, item['path'])
            current = T._read(path)
            if current != item['before']:
                C._require(current != item['after'], 'incomplete_group_readiness', str(path))
                C._require(not guarded, 'concurrent_edit', str(path))
        project = context.projects[entry]
        state = A._project_state(Path(entry), project)
        context.validate_state(state)
        C._require(G.identity(state) == G.identity(mutation.to_data()['baseline']['project']),
                   'transition_project_changed')
    A._probe(group.to_data()['inventory'], group.to_data()['expected_digests'])
    coordinator, _, completed = _group_paths(group)
    T.publish_immutable(completed, _complete_bytes(group, 'before', cancelled=True), root=Path(group.entries[0]).parent)
    _cleanup(group)


def recover(journal, *, deployment_guard, direction='after'):
    """Recover only an explicitly named complete group; never a single member."""
    C._require(direction in ('before', 'after'), 'invalid_recovery')
    journal = Path(journal).absolute()
    C._require(not journal.exists() or journal.stat().st_size <= T.MAX_TRANSACTION_BYTES, 'history_limit')
    raw = T._read(journal)
    C._require(raw is not None, 'no_recovery_pending')
    group = GroupPrepared.from_bytes(raw)
    C._require(journal == journal_for(group), 'group_coordinator_mismatch')
    data = group.to_data()
    with _guard(group.entries, data['operation'], data['inventory'], data['expected_digests'],
                deployment_guard, mutations=group.mutations) as context:
        coordinator, ready, completed = _group_paths(group)
        C._require(T._read(coordinator) == raw, 'concurrent_edit')
        done = T._read(completed)
        cancellation = T._read(coordinator.with_suffix('.cancel'))
        C._require(cancellation is None or direction == 'before', 'group_cancellation_in_progress')
        if done is not None:
            cancelled = direction == 'before' and done == _complete_bytes(group, 'before', cancelled=True)
            compensated = direction == 'before' and done == _complete_bytes(group, 'before', compensated=True)
            C._require(cancelled or compensated or done == _complete_bytes(group, direction), 'group_completion_direction_mismatch')
            _validate_guards(group, complete=True)
            if compensated:
                _compensate(group, context, completed=True)
            elif cancelled:
                # A completion file alone cannot grant permission to expose a
                # partial authority: recheck the no-publication cancellation proof.
                _cancel_unready(group, context)
            else:
                C._require(not (direction == 'before' and data['direction'] == 'activate'),
                           'group_compensation_required')
                _verify(group, context)
                C._require(all(T._read(path) == item[direction] for _, _, mutable in _targets(group)
                               for item, path in mutable), 'concurrent_edit')
                _cleanup(group)
        else:
            proof = T._read(ready)
            C._require(proof in (None, _ready_bytes(group)), 'invalid_group_ready_marker')
            if direction == 'before' and data['direction'] == 'activate' and (proof is not None or cancellation is not None):
                _compensate(group, context)
            elif proof is None and direction == 'before':
                _cancel_unready(group, context)
            else:
                if proof is None:
                    _targets(group, initial=True)
                    for path, (_, expected) in _local_guards(group).items():
                        C._require(T._read(path) in (None, expected), 'group_guard_mismatch')
                else:
                    _validate_guards(group)
                _verify(group, context)
                _targets(group, initial=proof is None)
                _install_guards(group)
                T.publish_immutable(ready, _ready_bytes(group), root=Path(group.entries[0]).parent)
                _finish(group, context, direction)
        for entry in group.entries:
            P.forget(entry)
    return {'state': 'recovered', 'direction': direction, 'operation': data['operation'],
            'entries': group.entries}
