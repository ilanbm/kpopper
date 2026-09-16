"""Prepared multi-file mutations and guarded, recoverable local publication.

The prepared envelope includes exact before/after bytes and semantic evidence.
Legacy readers must use reader_guard before observing its files. History readers
use complete immutable commit closure as authority; the view may lag a commit.
Nothing here selects knowledge acceptance or grants activation permission.
"""
import base64
import contextlib
import contextvars
import json
import os
from pathlib import Path
import tempfile
import threading

try:
    from . import history_contract as C
    from .pending_grounding import _encode, _decode, json_bytes, identity
except ImportError:
    import history_contract as C
    from pending_grounding import _encode, _decode, json_bytes, identity

MAX_TRANSACTION_BYTES = 64 * 1024 * 1024
ROLES = ('record', 'record_member', 'view', 'replaced', 'history_authority', 'history_object', 'history_commit')
_LOCKS = contextvars.ContextVar('history_directory_locks', default=())


def journal_for(entry, *, root=None):
    """Private journal relative to the chosen lock root (normally entry.parent)."""
    try:
        from .provenance import layout
    except ImportError:
        from provenance import layout
    entry = Path(entry).absolute()
    root = entry.parent if root is None else Path(root).absolute()
    home = Path(layout(entry)['home'])
    token = C.sha256(entry.name.encode('utf-8'))[:24]
    try:
        entry.relative_to(root)
        relative = (home / '.history-local' / (token + '.json')).relative_to(root).as_posix()
    except ValueError:
        raise C.HistoryError('invalid_path', 'entry must be below the journal root') from None
    return C.relative_path(relative)


def legacy_authority(entry):
    """Local identity of an unactivated record, never a persisted history marker."""
    token = C.sha256(str(Path(entry).resolve()).encode('utf-8'))[:32]
    return C.authority(record_id='legacy-' + token, authority='legacy', generation=0)


def _private_journal_home(root, journal, mutation):
    # Integrated writers use journal_for; raw fixture callers may select another
    # already-private location. Never alter a caller's existing ignore policy.
    if journal == journal_for(Path(root) / mutation._data['entry'], root=root):
        ignore = _target(root, journal).parent / '.gitignore'
        if ignore.exists():
            C._require(_read(ignore) == b'*\n', 'journal_ignore_mismatch')
        else:
            publish_immutable(ignore, b'*\n', root=root)


def _blob(value):
    C._require(value is None or type(value) is bytes, 'invalid_bytes')
    return None if value is None else {'sha256': C.sha256(value),
                                      'data': base64.b64encode(value).decode('ascii')}


def _unblob(value):
    if value is None:
        return None
    C._mapping(value, ('sha256', 'data'))
    C._require(isinstance(value['data'], str), 'invalid_bytes')
    try:
        raw = base64.b64decode(value['data'], validate=True)
    except (ValueError, TypeError):
        raise C.HistoryError('invalid_bytes') from None
    C._require(_blob(raw) == value, 'byte_hash_mismatch')
    return raw


def semantic_receipt(*, profile, capabilities, before, after):
    """Bind caller-validated assessment/capability evidence without re-evaluation."""
    C._require(profile in ('ordinary-reader/v1', 'checked-reader/v1', 'core/v1'), 'unsupported_profile')
    C._require(isinstance(capabilities, dict), 'invalid_capabilities')
    C._require(isinstance(before, dict) and isinstance(after, dict), 'invalid_evidence')
    payload = {'version': 1, 'profile': profile, 'capabilities': capabilities,
               'before': before, 'after': after}
    payload = C.detached(payload, MAX_TRANSACTION_BYTES)
    return {**payload, 'digest': identity(payload)}


def validate_receipt(value):
    C._mapping(value, ('version', 'profile', 'capabilities', 'before', 'after', 'digest'))
    C._require(type(value['version']) is int and value['version'] == 1, 'invalid_receipt')
    expected = semantic_receipt(profile=value['profile'], capabilities=value['capabilities'],
                                before=value['before'], after=value['after'])
    C._require(identity(expected) == identity(value), 'invalid_receipt')
    return expected


class PreparedMutation:
    """Immutable-by-copy, serializable operation; safe preparation performs no I/O.

    `files` lists mappings with relative path, role, before bytes (None means
    absent), and after bytes. Immutable history files require an absent before
    image. Their presence on retry is allowed only with identical bytes.

    Legacy record_member paths must occur in baseline.record_members, including
    the entry and captured SHA256 values. This proves internal consistency only:
    the publication verifier must re-resolve reader membership and its hashes.
    """
    def __init__(self, *, operation, authority, baseline, files, receipt, entry='GROUNDING.yaml'):
        C._text(operation)
        C.relative_path(entry)
        try:
            from .provenance import layout
        except ImportError:
            from provenance import layout
        paths = layout('/' + entry)
        marker = C.validate_authority(authority)
        C._require(isinstance(baseline, dict), 'invalid_baseline')
        members = baseline.get('record_members')
        if 'record_members' in baseline:
            C._require(isinstance(members, dict) and entry in members, 'invalid_record_members')
            for path, digest in members.items():
                C.relative_path(path)
                C._text(digest, C.HEX)
        receipt = validate_receipt(receipt)
        C._require(isinstance(files, list) and files, 'empty_mutation')
        encoded = []
        for item in files:
            C._mapping(item, ('path', 'role', 'before', 'after'))
            C.relative_path(item['path'])
            C._require(item['role'] in ROLES, 'invalid_file_role')
            role = item['role']
            if role == 'record_member':
                C._require(marker['authority'] == 'legacy', 'authority_transition_required')
                C._require(item['path'] != entry, 'role_path_mismatch', item['path'])
                C._require(members is not None and item['path'] in members,
                           'invalid_record_members', item['path'])
                C._require(type(item['before']) is bytes
                           and C.sha256(item['before']) == members[item['path']],
                           'record_member_mismatch', item['path'])
                expected = '/' + item['path']
            elif role == 'history_object':
                obj = C.validate_object(C.decode_document(item['after']))
                expected = paths['history'] + '/' + obj['subject'] + '/' + obj['id'] + '.yaml'
            elif role == 'history_commit':
                expected = paths['history_commits'] + '/' + operation + '.yaml'
            else:
                expected = paths['entry' if role == 'record' else role]
            C._require(item['path'] == expected.lstrip('/'), 'role_path_mismatch', item['path'])
            if role == 'record' and members is not None:
                C._require(type(item['before']) is bytes
                           and C.sha256(item['before']) == members[entry],
                           'record_member_mismatch', entry)
            if item['role'] in ('history_object', 'history_commit'):
                C._require(item['before'] is None and item['after'] is not None,
                           'immutable_mutation')
            encoded.append({**item, 'before': _blob(item['before']), 'after': _blob(item['after'])})
        encoded.sort(key=lambda item: item['path'])
        C._require(len({item['path'] for item in encoded}) == len(encoded), 'duplicate_path')
        history = marker['authority'] == 'history'
        if history:
            C.bind_authority(marker, baseline)
            C._require(sum(item['role'] == 'history_commit' for item in encoded) == 1,
                       'missing_commit')
            C._require(not any(item['role'] in ('replaced', 'history_authority') for item in encoded),
                       'authority_transition_required')
            # File roles are not assertions: bind them to the actual commit.
            commit_item = next(item for item in files if item['role'] == 'history_commit')
            commit = C.validate_commit(C.decode_document(commit_item['after']))
            C._require(commit['operation'] == operation and commit['record_id'] == marker['record_id']
                       and commit['authority_generation'] == marker['generation']
                       and commit['baseline_digest'] == identity(baseline)
                       and identity(commit['receipt']) == identity(receipt), 'commit_mismatch')
            inventory = []
            for item in files:
                if item['role'] == 'history_object':
                    obj = C.validate_object(C.decode_document(item['after']))
                    inventory.append({'subject': obj['subject'], 'id': obj['id'],
                                      'sha256': C.sha256(item['after'])})
            C._require(sorted(inventory, key=lambda item: item['id']) == commit['objects'],
                       'commit_inventory_mismatch')
            views = [item for item in files if item['role'] == 'record']
            C._require(len(views) == 1 and views[0]['after'] is not None
                       and C.sha256(views[0]['after']) == commit['view_sha256'], 'commit_view_mismatch')
        else:
            C._require(not any(item['role'] in ('history_object', 'history_commit', 'history_authority')
                               for item in encoded), 'authority_transition_required')
        payload = {'version': 1, 'operation': operation, 'entry': entry, 'authority': marker,
                   'baseline': baseline, 'files': encoded, 'receipt': receipt}
        payload = C.detached(payload, MAX_TRANSACTION_BYTES)
        self._data = {**payload, 'digest': identity(payload)}

    def to_data(self):
        return C.detached(self._data, MAX_TRANSACTION_BYTES)

    def to_bytes(self):
        raw = json_bytes(_encode(self._data))
        C._require(len(raw) <= MAX_TRANSACTION_BYTES, 'history_limit')
        return raw

    @classmethod
    def from_bytes(cls, raw):
        C._require(type(raw) is bytes and len(raw) <= MAX_TRANSACTION_BYTES, 'history_limit')
        try:
            # Reuse the strict portable typed-value parser, without constructing a snapshot.
            try:
                from .reasoning.snapshot import _json_object, _json_constant, _check_typed_json
            except ImportError:
                from reasoning.snapshot import _json_object, _json_constant, _check_typed_json
            encoded = json.loads(raw.decode('utf-8'), object_pairs_hook=_json_object,
                                 parse_constant=_json_constant)
            _check_typed_json(encoded)
            value = _decode(encoded)
            C._require(_encode(value) == encoded, 'invalid_journal')
            C._mapping(value, ('version', 'operation', 'entry', 'authority', 'baseline', 'files', 'receipt', 'digest'))
            C._require(type(value['version']) is int and value['version'] == 1, 'invalid_journal')
            files = [{**item, 'before': _unblob(item['before']), 'after': _unblob(item['after'])}
                     for item in value['files']]
            result = cls(operation=value['operation'], authority=value['authority'],
                         baseline=value['baseline'], files=files, receipt=value['receipt'], entry=value['entry'])
            C._require(identity(result._data) == identity(value), 'invalid_journal')
            return result
        except (ValueError, TypeError, KeyError, RecursionError, AttributeError) as error:
            if isinstance(error, C.HistoryError):
                raise
            raise C.HistoryError('invalid_journal') from error

    @property
    def files(self):
        return [{**item, 'before': _unblob(item['before']), 'after': _unblob(item['after'])}
                for item in self._data['files']]


def _target(root, relative):
    C.relative_path(relative)
    root = Path(root).resolve(strict=True)
    # The caller chooses a root, possibly via /tmp or another path alias.
    # Links below that root cannot redirect a relative record member.
    path = root / relative
    for candidate in (path, *path.parents):
        if candidate == root:
            break
        C._require(not candidate.is_symlink(), 'symlink_path', str(candidate))
    return path


def _read(path):
    if not path.exists():
        return None
    C._require(path.is_file(), 'invalid_path', str(path))
    return path.read_bytes()


def _sync(directory):
    fd = os.open(str(directory), os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def _replace(path, data):
    if data is None:
        if path.exists():
            path.unlink()
            _sync(path.parent)
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temp = tempfile.mkstemp(prefix='.history-', dir=path.parent)
    try:
        if path.exists():
            os.fchmod(fd, path.stat().st_mode & 0o7777)
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temp, path)
        _sync(path.parent)
    finally:
        if os.path.exists(temp):
            os.unlink(temp)


def publish_immutable(path, data, *, root=None):
    """Exclusive atomic publication; retries verify exact bytes, including races."""
    path = Path(path).absolute()
    C._require(type(data) is bytes, 'invalid_bytes')
    if root is None:
        # Standalone calls select an exact destination, not a relative capability.
        # Resolve its parent alias but never follow a final-file symlink.
        path = path.parent.resolve() / path.name
        C._require(not path.is_symlink(), 'symlink_path', str(path))
    else:
        base = Path(root).absolute()
        try:
            relative = path.relative_to(base).as_posix()
        except ValueError:
            # Internal callers may already hold the canonical path from _target.
            try:
                relative = path.relative_to(base.resolve(strict=True)).as_posix()
            except ValueError:
                raise C.HistoryError('invalid_path', str(path)) from None
        path = _target(root, relative)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temp = tempfile.mkstemp(prefix='.history-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        try:
            os.link(temp, path)
        except FileExistsError:
            C._require(_read(path) == data, 'immutable_collision', str(path))
        _sync(path.parent)
    finally:
        os.unlink(temp)


@contextlib.contextmanager
def _lock(root, exclusive):
    try:
        import fcntl
    except ImportError:
        if not exclusive:
            # Read-only installations remain portable. This host cannot run our
            # writers; the reader still brackets its read with the journal guard.
            yield
            return
        raise C.HistoryError('locking_unavailable') from None
    fd = os.open(str(root), os.O_RDONLY)
    stat = os.fstat(fd)
    key = (stat.st_dev, stat.st_ino, os.getpid(), threading.get_ident())
    held = next((mode for lock, mode in _LOCKS.get() if lock == key), None)
    if held is not None:
        os.close(fd)
        C._require(held or not exclusive, 'lock_upgrade_refused')
        yield
        return
    try:
        fcntl.flock(fd, fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH)
        token = _LOCKS.set((*_LOCKS.get(), (key, exclusive)))
        try:
            yield
        finally:
            _LOCKS.reset(token)
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)


@contextlib.contextmanager
def writer_guard(root):
    """Share one directory lock with direct writers and nested captured reads."""
    with _lock(root, True):
        yield


@contextlib.contextmanager
def reader_guard(root, journal):
    """A reader sees a complete legacy generation or an explicit recovery refusal."""
    with _lock(root, False):
        C._require(not _target(root, journal).exists(), 'recovery_required')
        yield
        C._require(not _target(root, journal).exists(), 'recovery_required')


def _preflight(root, mutation, *, recovery):
    targets, states = [], []
    for item in mutation.files:
        path = _target(root, item['path'])
        current = _read(path)
        targets.append((item, path))
        states.append(current)
    if not recovery and any(current != item['before'] for current, (item, _) in zip(states, targets)) \
            and all(current == item['after'] for current, (item, _) in zip(states, targets)):
        # Equal bytes are observable evidence, not proof that this operation ran.
        raise C.HistoryError('after_images_match', 'completion requires a retained operation receipt')
    for current, (item, _) in zip(states, targets):
        allowed = (item['before'], item['after']) if recovery else (item['before'],)
        C._require(current in allowed, 'concurrent_edit', item['path'])
    return targets


def _journal_path(root, journal, mutation):
    path = _target(root, journal)
    candidate = Path(journal)
    for item in mutation.files:
        member = Path(item['path'])
        C._require(candidate != member and candidate not in member.parents
                   and member not in candidate.parents, 'invalid_journal_path')
    return path


def _apply_legacy(targets, direction):
    for item, path in targets:
        if _read(path) != item[direction]:
            _replace(path, item[direction])


def publish_legacy(root, journal, mutation, *, verify, on_committed=None):
    """Validate under a writer lock, persist the journal, then apply all files.

    `verify` must recheck routing/capabilities and the complete captured baseline,
    including actual reader-resolved membership and hashes for record_members.
    It is mandatory and called before any journal or file publication. Recovery
    reuses the stored envelope. Readers must share reader_guard at integration.
    """
    C._require(isinstance(mutation, PreparedMutation), 'invalid_mutation')
    C._require(mutation._data['authority']['authority'] == 'legacy', 'invalid_authority')
    C._require(callable(verify), 'missing_verifier')
    C._require(on_committed is None or callable(on_committed), 'invalid_completion_callback')
    with _lock(root, True):
        journal_path = _journal_path(root, journal, mutation)
        C._require(not journal_path.exists(), 'recovery_required')
        targets = _preflight(root, mutation, recovery=False)
        verify(mutation.to_data())
        _private_journal_home(root, journal, mutation)
        publish_immutable(journal_path, mutation.to_bytes(), root=root)
        _apply_legacy(targets, 'after')
        if on_committed is not None:
            on_committed(mutation.to_data())
        journal_path.unlink()
        _sync(journal_path.parent)


def recover_legacy(root, journal, *, verify, direction='after', on_committed=None):
    """Resume or undo exact prepared bytes, refusing unrelated concurrent edits."""
    C._require(direction in ('before', 'after') and callable(verify), 'invalid_recovery')
    C._require(on_committed is None or callable(on_committed), 'invalid_completion_callback')
    with _lock(root, True):
        journal_path = _target(root, journal)
        C._require(journal_path.is_file(), 'no_recovery_pending')
        mutation = PreparedMutation.from_bytes(journal_path.read_bytes())
        C._require(mutation._data['authority']['authority'] == 'legacy', 'invalid_authority')
        _journal_path(root, journal, mutation)
        targets = _preflight(root, mutation, recovery=True)
        verify(mutation.to_data())
        _apply_legacy(targets, direction)
        if direction == 'after' and on_committed is not None:
            on_committed(mutation.to_data())
        journal_path.unlink()
        _sync(journal_path.parent)
        return mutation
