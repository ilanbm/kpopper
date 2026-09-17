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
    from . import history_paths as HP
except ImportError:
    import history_paths as HP

try:
    from . import history_contract as C
    from .pending_grounding import _encode, _decode, json_bytes, identity
except ImportError:
    import history_contract as C
    from pending_grounding import _encode, _decode, json_bytes, identity

MAX_TRANSACTION_BYTES = 64 * 1024 * 1024
ROLES = ('record', 'record_member', 'hypothesis', 'view', 'replaced', 'history_authority', 'history_object', 'history_commit', 'history_retained', 'history_evidence')
_LOCKS = contextvars.ContextVar('history_directory_locks', default=())
_LOCK_PATHS = contextvars.ContextVar('history_directory_lock_paths', default=())
_AUXILIARY_READS = contextvars.ContextVar('history_auxiliary_reads', default=())
AUXILIARY_KIND = 'history-auxiliary/v1'


def journal_for(entry, *, root=None):
    """Private journal relative to the chosen lock root (normally entry.parent)."""
    try:
        from .provenance import layout
    except ImportError:
        from provenance import layout
    entry = Path(entry).absolute()
    if root is None:
        root = entry.parent
    else:
        entry = entry.parent.resolve() / entry.name
        root = Path(root).resolve()
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
    def __init__(self, *, operation, authority, baseline, files, receipt, entry='GROUNDING.yaml', transition=None):
        C._text(operation)
        C.relative_path(entry)
        try:
            from .provenance import layout
        except ImportError:
            from provenance import layout
        paths = layout('/' + entry)
        marker = C.validate_authority(authority)
        C._require(isinstance(baseline, dict), 'invalid_baseline')
        next_marker = None
        if transition is not None:
            C._mapping(transition, ('version', 'after'))
            C._require(type(transition['version']) is int and transition['version'] == 1,
                       'invalid_authority_transition')
            next_marker = C.validate_authority(transition['after'])
            C._require(next_marker['record_id'] == marker['record_id'] and
                       next_marker['generation'] == marker['generation'] + 1 and
                       next_marker['authority'] != marker['authority'] and
                       baseline.get('kind') == 'history-authority-transition/v1' and
                       baseline.get('direction') == ('activate' if next_marker['authority'] == 'history' else 'deactivate'),
                       'invalid_authority_transition')
        members = baseline.get('record_members')
        if 'record_members' in baseline:
            C._require(isinstance(members, dict) and entry in members, 'invalid_record_members')
            for path, digest in members.items():
                C.relative_path(path)
                if digest is None:
                    C._require(path == entry and baseline.get('source_absent') is True and
                               marker['authority'] == 'legacy', 'invalid_record_members')
                else:
                    C._text(digest, C.HEX)
        if baseline.get('source_absent') is True:
            C._require(members == {entry: None} and marker['authority'] == 'legacy',
                       'invalid_record_members')
        hypotheses = baseline.get('hypothesis_members', {})
        C._require(isinstance(hypotheses, dict), 'invalid_hypothesis_members')
        for path, digest in hypotheses.items():
            C.relative_path(path)
            candidate = Path('/' + path)
            C._require(candidate.parent == Path(paths['hypotheses']) and candidate.suffix in ('.yaml', '.yml')
                       and bool(candidate.stem),
                       'invalid_hypothesis_members', path)
            if digest is not None:
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
            elif role == 'hypothesis':
                C._require(marker['authority'] == 'legacy' and item['path'] in hypotheses,
                           'invalid_hypothesis_members', item['path'])
                before_hash = C.sha256(item['before']) if item['before'] is not None else None
                C._require(before_hash == hypotheses[item['path']], 'hypothesis_member_mismatch')
                expected = '/' + item['path']
            elif role == 'history_object':
                obj = C.validate_object(C.decode_document(item['after']))
                prefix = paths['history'].lstrip('/') + '/'
                C._require(item['path'].startswith(prefix), 'role_path_mismatch', item['path'])
                try:
                    HP.validate_object_path(item['path'][len(prefix):], obj['subject'], obj['id'])
                except HP.HistoryPathError as error:
                    raise C.HistoryError('role_path_mismatch', str(error)) from error
                expected = '/' + item['path']
            elif role == 'history_commit':
                expected = paths['history_commits'] + '/' + operation + '.yaml'
            elif role == 'history_evidence':
                candidate = Path('/' + item['path'])
                C._require(marker['authority'] == 'history' and next_marker is None and
                           ((candidate.parent == Path(paths['home']) / 'evidence' / 'reports' and candidate.suffix == '.txt') or
                            (candidate.parent == Path(paths['home']) / 'evidence' / 'view-edits' and candidate.suffix == '.yaml'
                             and receipt['before'].get('history_edit') == {'version': 1, 'kind': 'view-edit-proposals'}
                             and receipt['before'].get('authoring', {}).get('kind') == 'view-edit-proposals')),
                           'invalid_history_evidence')
                C._text(candidate.stem)
                expected = '/' + item['path']
            elif role == 'history_retained':
                retained = baseline.get('retained_files', {})
                C._require(next_marker is not None and next_marker['authority'] == 'history' and
                           isinstance(retained, dict) and item['path'] in retained and
                           item['before'] is None and type(item['after']) is bytes and
                           C.sha256(item['after']) == retained[item['path']], 'invalid_retained_file')
                # Explicit evidence inventory is confined to its dedicated import
                # artifact directory; it is never arbitrary filesystem authority.
                home = str(Path(paths['entry']).parent / '.kpopper-history-migration')
                C._require(('/' + item['path']).startswith(home.rstrip('/') + '/'), 'invalid_retained_file')
                expected = '/' + item['path']
            else:
                expected = paths['entry' if role == 'record' else role]
            C._require(item['path'] == expected.lstrip('/'), 'role_path_mismatch', item['path'])
            if role == 'record' and members is not None:
                C._require((C.sha256(item['before']) if item['before'] is not None else None) == members[entry],
                           'record_member_mismatch', entry)
            if item['role'] in ('history_object', 'history_commit', 'history_retained', 'history_evidence'):
                C._require(item['before'] is None and item['after'] is not None,
                           'immutable_mutation')
            encoded.append({**item, 'before': _blob(item['before']), 'after': _blob(item['after'])})
        encoded.sort(key=lambda item: item['path'])
        C._require(len({item['path'] for item in encoded}) == len(encoded), 'duplicate_path')
        if next_marker is not None:
            C._require(all(item['role'] in ('record', 'history_authority', 'history_object',
                                          'history_commit', 'history_retained') for item in files),
                       'invalid_transition_role')
            authorities = [item for item in files if item['role'] == 'history_authority']
            C._require(len(authorities) == 1, 'missing_authority_transition')
            authority_file = authorities[0]
            C._require(C.validate_authority(C.decode_document(authority_file['after'])) == next_marker,
                       'authority_transition_mismatch')
            if authority_file['before'] is None:
                C._require(marker['authority'] == 'legacy' and marker['generation'] == 0,
                           'authority_transition_mismatch')
            else:
                C._require(C.validate_authority(C.decode_document(authority_file['before'])) == marker,
                           'authority_transition_mismatch')
            C._require(sum(item['role'] == 'record' for item in files) == 1,
                       'missing_transition_record')
            retained = {item['path']: C.sha256(item['after']) for item in files if item['role'] == 'history_retained'}
            C._require(retained == baseline.get('retained_files', {}), 'invalid_retained_file')
        history = (next_marker or marker)['authority'] == 'history'
        if history:
            evidence = {item['path']: C.sha256(item['after']) for item in files if item['role'] == 'history_evidence'}
            declared_evidence = receipt['before'].get('authoring', {}).get('evidence', {})
            C._require(isinstance(declared_evidence, dict) and evidence == declared_evidence,
                       'history_evidence_mismatch')
            commit_marker = next_marker or marker
            commit_baseline = baseline.get('history_baseline') if next_marker else baseline
            C.bind_authority(commit_marker, commit_baseline)
            C._require(sum(item['role'] == 'history_commit' for item in encoded) == 1,
                       'missing_commit')
            C._require(next_marker is not None or not any(item['role'] in ('replaced', 'history_authority', 'history_retained') for item in encoded),
                       'authority_transition_required')
            # File roles are not assertions: bind them to the actual commit.
            commit_item = next(item for item in files if item['role'] == 'history_commit')
            commit = C.validate_commit(C.decode_document(commit_item['after']))
            try:
                HP.validate_path_capability([item['path'][len(paths['history'].lstrip('/')) + 1:]
                    for item in files if item['role'] == 'history_object'], commit.get('requires', []))
            except HP.HistoryPathError as error:
                raise C.HistoryError(error.code, str(error)) from error
            C._require(commit['operation'] == operation and commit['record_id'] == commit_marker['record_id']
                       and commit['authority_generation'] == commit_marker['generation']
                       and commit['baseline_digest'] == identity(commit_baseline)
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
            if next_marker is not None:
                C._require(not commit['parents'], 'transition_requires_initial_commit')
                objects = {(C.decode_document(item['after'])['subject'], C.decode_document(item['after'])['id']): item['after']
                           for item in files if item['role'] == 'history_object'}
                C.committed_objects(commit_marker, {operation: commit_item['after']}, objects)
        else:
            C._require(not any(item['role'] in ('history_object', 'history_commit', 'history_retained')
                               or item['role'] == 'history_authority' and next_marker is None
                               for item in encoded), 'authority_transition_required')
        payload = {'version': 2 if next_marker else 1, 'operation': operation, 'entry': entry, 'authority': marker,
                   'baseline': baseline, 'files': encoded, 'receipt': receipt}
        if next_marker is not None:
            payload['transition'] = {'version': 1, 'after': next_marker}
        payload = C.detached(payload, MAX_TRANSACTION_BYTES)
        self._data = {**payload, 'digest': identity(payload)}
        if history and next_marker is None:
            auxiliary_view(self)

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
            if isinstance(value, dict) and value.get('kind') == 'history-authority-group-guard/v1':
                raise C.HistoryError('group_recovery_required', str(value.get('coordinator', '')))
            if isinstance(value, dict) and value.get('kind') == AUXILIARY_KIND:
                raise C.HistoryError('history_auxiliary_recovery_required')
            C._require(_encode(value) == encoded, 'invalid_journal')
            C._mapping(value, ('version', 'operation', 'entry', 'authority', 'baseline', 'files', 'receipt', 'digest'), ('transition',))
            C._require(type(value['version']) is int and value['version'] in (1, 2) and
                       ('transition' in value) == (value['version'] == 2), 'invalid_journal')
            files = [{**item, 'before': _unblob(item['before']), 'after': _unblob(item['after'])}
                     for item in value['files']]
            result = cls(operation=value['operation'], authority=value['authority'],
                         baseline=value['baseline'], files=files, receipt=value['receipt'], entry=value['entry'],
                         transition=value.get('transition'))
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


def auxiliary_view(mutation):
    """Bind the sole supported history auxiliary file to its exact identity receipt."""
    data = mutation.to_data()
    items = [item for item in mutation.files if item['role'] == 'view']
    intent = data['receipt']['before'].get('identity_authoring', {})
    if not items:
        C._require(intent.get('version') != 2, 'missing_identity_brief')
        return None
    C._require(data['authority']['authority'] == 'history' and 'transition' not in data and
               len(items) == 1 and intent.get('version') == 2 and intent.get('kind') == 'same',
               'invalid_history_auxiliary')
    item = items[0]
    brief = intent.get('brief')
    C._mapping(brief, ('path', 'before_utf8', 'before_sha256', 'after_sha256'))
    C._require(type(item['before']) is bytes and type(item['after']) is bytes and
               item['before'] != item['after'] and isinstance(brief['before_utf8'], str), 'invalid_history_auxiliary')
    C._require(brief['path'] == item['path'] and brief['before_utf8'].encode('utf-8') == item['before'] and
               brief['before_sha256'] == C.sha256(item['before']) == intent.get('view_sha256') and
               brief['after_sha256'] == C.sha256(item['after']) ==
               data['receipt']['after'].get('identity_authoring', {}).get('view_sha256'),
               'identity_brief_mismatch')
    return item


def auxiliary_envelope(mutation):
    C._require(auxiliary_view(mutation) is not None, 'invalid_history_auxiliary')
    body = {'version': 1, 'kind': AUXILIARY_KIND, 'mutation': _blob(mutation.to_bytes())}
    raw = json_bytes(_encode({**body, 'digest': identity(body)}))
    C._require(len(raw) <= MAX_TRANSACTION_BYTES, 'history_limit')
    return raw


def _exclusive_owned(root):
    stat = Path(root).stat()
    key = (stat.st_dev, stat.st_ino, os.getpid(), threading.get_ident())
    return any(lock == key and exclusive for lock, exclusive in _LOCKS.get())


def _owned_auxiliary(root, path, raw):
    return _exclusive_owned(root) and (str(path), C.sha256(raw), os.getpid(), threading.get_ident()) in _AUXILIARY_READS.get()


@contextlib.contextmanager
def auxiliary_owner(root, journal, mutation):
    """Allow only this exact locked writer to read its own pending auxiliary journal."""
    C._require(_exclusive_owned(root), 'auxiliary_writer_lock_required')
    C._require(journal == journal_for(Path(root) / mutation.to_data()['entry'], root=root), 'invalid_auxiliary_journal')
    primary = _journal_path(root, journal, mutation)
    raw = auxiliary_envelope(mutation)
    C._require(_read(primary) in (None, raw), 'recovery_required')
    key = (str(primary), C.sha256(raw), os.getpid(), threading.get_ident())
    token = _AUXILIARY_READS.set((*_AUXILIARY_READS.get(), key))
    try:
        yield
    finally:
        _AUXILIARY_READS.reset(token)


def publish_auxiliary_journal(root, journal, mutation):
    primary = _journal_path(root, journal, mutation)
    raw = auxiliary_envelope(mutation)
    C._require(_owned_auxiliary(root, primary, raw), 'auxiliary_writer_lock_required')
    _private_journal_home(root, journal, mutation)
    publish_immutable(primary, raw, root=root)


def clear_auxiliary_journal(root, journal, mutation):
    primary = _journal_path(root, journal, mutation)
    raw = auxiliary_envelope(mutation)
    C._require(_owned_auxiliary(root, primary, raw), 'auxiliary_writer_lock_required')
    C._require(_read(primary) in (None, raw), 'recovery_required')
    if primary.exists():
        primary.unlink()
        _sync(primary.parent)


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
    canonical = str(Path(root).resolve())
    owner = (os.getpid(), threading.get_ident())
    previous = [path for pid, thread, path in _LOCK_PATHS.get() if (pid, thread) == owner]
    if previous and canonical < max(previous):
        os.close(fd)
        raise C.HistoryError('lock_order_refused', canonical)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH)
        token = _LOCKS.set((*_LOCKS.get(), (key, exclusive)))
        paths_token = _LOCK_PATHS.set((*_LOCK_PATHS.get(), (*owner, canonical)))
        try:
            yield
        finally:
            _LOCK_PATHS.reset(paths_token)
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
def directory_guards(roots, *, exclusive):
    """Acquire a complete participant set in canonical order; never upgrade it."""
    with contextlib.ExitStack() as stack:
        for root in sorted({str(Path(root).resolve()) for root in roots}):
            stack.enter_context(_lock(root, exclusive))
        yield


def transaction_root(entry, members):
    """The common path is a path namespace, not permission to touch other files."""
    parents = [str(Path(path).parent.resolve()) for path in [entry, *members]]
    return Path(os.path.commonpath(parents))


def _participant_members(mutation):
    baseline = mutation._data['baseline']
    return set(baseline.get('record_members', {})) | set(baseline.get('hypothesis_members', {}))


def participant_directories(root, mutation):
    members = _participant_members(mutation)
    return sorted({*(_target(root, path).parent for path in members),
                   _target(root, mutation._data['entry']).parent})


def _journal_replicas(root, journal, mutation):
    """A durable local guard protects independently opened external members."""
    baseline = mutation._data['baseline']
    if 'transaction_root' not in baseline:
        return []
    C._require(str(Path(root).absolute()) == baseline['transaction_root'], 'transaction_root_mismatch')
    primary = _target(root, journal)
    directories = {_target(root, path).parent for path in _participant_members(mutation)}
    if len(directories) < 2:
        return []
    name = mutation._data['digest'] + '.json'
    entry_directory = _target(root, mutation._data['entry']).parent
    return [directory / '.history-local' / name for directory in sorted(directories)
            if directory != entry_directory]


def member_guard(mutation, root, directory):
    members = _participant_members(mutation)
    return {'version': 1, 'kind': 'member_guard', 'operation': mutation._data['operation'],
            'digest': mutation._data['digest'],
            'members': sorted({_target(root, path).name for path in members
                               if _target(root, path).parent == directory})}


def validate_member_guard(value):
    C._mapping(value, ('version', 'kind', 'operation', 'digest', 'members'))
    C._require(type(value['version']) is int and value['version'] == 1
               and value['kind'] == 'member_guard', 'invalid_member_guard')
    C._text(value['operation'])
    C._text(value['digest'], C.HEX)
    C._require(isinstance(value['members'], list) and value['members']
               and all(isinstance(v, str) for v in value['members'])
               and value['members'] == sorted(set(value['members'])), 'invalid_member_guard')
    for name in value['members']:
        C.relative_path(name)
        C._require('/' not in name, 'invalid_member_guard')
    return value


def _prepare_replicas(root, journal, mutation):
    paths = _journal_replicas(root, journal, mutation)
    guards = [(path, json_bytes(member_guard(mutation, root, path.parent.parent))) for path in paths]
    for path, raw in guards:
        checked = _journal_path(root, path.relative_to(Path(root).resolve()).as_posix(), mutation)
        existing = _read(checked)
        C._require(existing is None or existing == raw, 'journal_replica_mismatch', str(path))
    for path, raw in guards:
        publish_immutable(path.parent / '.gitignore', b'*\n', root=root)
        publish_immutable(path, raw, root=root)
    return paths


def _ready_path(primary, mutation):
    return primary.with_name(primary.name + '.' + mutation._data['digest'] + '.ready')


def _remove_journals(primary, replicas):
    for path in [*replicas, primary]:
        if path.exists():
            path.unlink()
            _sync(path.parent)


@contextlib.contextmanager
def reader_guard(root, journal):
    """A reader sees a complete legacy generation or an explicit recovery refusal."""
    with _lock(root, False):
        path = _target(root, journal)
        def check():
            if path.exists():
                C._require(path.stat().st_size <= MAX_TRANSACTION_BYTES, 'history_limit')
                raw = _read(path)
                C._require(_owned_auxiliary(root, path, raw), 'recovery_required')
        check()
        yield
        check()


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
    C._require(mutation._data['authority']['authority'] == 'legacy' and
               'transition' not in mutation._data, 'invalid_authority')
    C._require(callable(verify), 'missing_verifier')
    C._require(on_committed is None or callable(on_committed), 'invalid_completion_callback')
    directories = participant_directories(root, mutation)
    for directory in directories:
        directory.mkdir(parents=True, exist_ok=True)
    with directory_guards(directories, exclusive=True):
        journal_path = _journal_path(root, journal, mutation)
        C._require(not journal_path.exists(), 'recovery_required')
        targets = _preflight(root, mutation, recovery=False)
        verify(mutation.to_data())
        _private_journal_home(root, journal, mutation)
        publish_immutable(journal_path, mutation.to_bytes(), root=root)
        replicas = _prepare_replicas(root, journal, mutation)
        ready = _ready_path(journal_path, mutation)
        if replicas:
            publish_immutable(ready, mutation._data['digest'].encode('ascii'), root=root)
        _apply_legacy(targets, 'after')
        if on_committed is not None:
            on_committed(mutation.to_data())
        _remove_journals(journal_path, [*replicas, *([ready] if replicas else [])])


def recover_legacy(root, journal, *, verify, direction='after', on_committed=None, verify_cancel=None):
    """Resume or undo exact prepared bytes, refusing unrelated concurrent edits."""
    C._require(direction in ('before', 'after') and callable(verify), 'invalid_recovery')
    C._require(on_committed is None or callable(on_committed), 'invalid_completion_callback')
    C._require(verify_cancel is None or callable(verify_cancel), 'missing_verifier')
    journal_path = _target(root, journal)
    C._require(journal_path.is_file(), 'no_recovery_pending')
    try:
        raw = journal_path.read_bytes()
    except FileNotFoundError:
        raise C.HistoryError('no_recovery_pending') from None
    mutation = PreparedMutation.from_bytes(raw)
    with directory_guards(participant_directories(root, mutation), exclusive=True):
        C._require(_read(journal_path) == raw, 'concurrent_edit', str(journal_path))
        C._require(mutation._data['authority']['authority'] == 'legacy' and
               'transition' not in mutation._data, 'invalid_authority')
        _journal_path(root, journal, mutation)
        replicas = _journal_replicas(root, journal, mutation)
        ready = _ready_path(journal_path, mutation)
        ready_bytes = _read(ready)
        C._require(ready_bytes in (None, mutation._data['digest'].encode('ascii')), 'invalid_ready_marker')
        if replicas and ready_bytes is None and direction == 'before':
            # No image is ever published before this durable marker. Cancelling
            # preparation leaves any independent newer member write untouched.
            (verify_cancel or verify)(mutation.to_data())
            guarded = {_target(root, mutation._data['entry']).parent,
                       *(path.parent.parent for path in replicas if path.exists())}
            for item in mutation.files:
                path = _target(root, item['path'])
                current = _read(path)
                if current != item['before']:
                    C._require(current != item['after'], 'incomplete_readiness', str(path))
                    C._require(item['role'] == 'record_member' and path.parent not in guarded,
                               'concurrent_edit', str(path))
            for path in replicas:
                expected = json_bytes(member_guard(mutation, root, path.parent.parent))
                C._require(_read(path) in (None, expected), 'journal_replica_mismatch', str(path))
            _remove_journals(journal_path, replicas)
            return mutation
        targets = _preflight(root, mutation, recovery=True)
        verify(mutation.to_data())
        replicas = _prepare_replicas(root, journal, mutation)
        if replicas:
            publish_immutable(ready, mutation._data['digest'].encode('ascii'), root=root)
        _apply_legacy(targets, direction)
        if direction == 'after' and on_committed is not None:
            on_committed(mutation.to_data())
        _remove_journals(journal_path, [*replicas, *([ready] if replicas else [])])
        return mutation


def _transition_targets(root, mutation):
    C._require(mutation._data.get('version') == 2 and 'transition' in mutation._data,
               'invalid_authority_transition')
    immutable, mutable = [], []
    for item in mutation.files:
        path = _target(root, item['path'])
        current = _read(path)
        if item['role'] in ('history_object', 'history_commit', 'history_retained'):
            C._require(current in (None, item['after']), 'immutable_collision', item['path'])
            immutable.append((item, path))
        else:
            C._require(current in (item['before'], item['after']), 'concurrent_edit', item['path'])
            mutable.append((item, path))
    return immutable, mutable


def _apply_transition(immutable, mutable, direction, root):
    if direction == 'after':
        for item, path in immutable:
            publish_immutable(path, item['after'], root=root)
    # The journal protects the entire marker/view switch; standalone snapshots
    # and legacy readers must refuse until its final completion callback succeeds.
    for item, path in sorted(mutable, key=lambda pair: pair[0]['role'] == 'history_authority'):
        if _read(path) != item[direction]:
            _replace(path, item[direction])


def publish_transition(root, journal, mutation, *, verify, on_committed=None):
    """Publish an explicit v2 authority transition under the shared reader guard.

    The mandatory verifier authenticates runtime/policy and retained source and
    candidate meaning. Immutable history/evidence is exclusively created. It is
    retained inactive after rollback, never deleted by the transition primitive.
    """
    C._require(isinstance(mutation, PreparedMutation) and callable(verify), 'missing_verifier')
    C._require('group' not in mutation._data['baseline'], 'group_recovery_required')
    C._require(on_committed is None or callable(on_committed), 'invalid_completion_callback')
    with directory_guards(participant_directories(root, mutation), exclusive=True):
        primary = _journal_path(root, journal, mutation)
        C._require(not primary.exists(), 'recovery_required')
        immutable, mutable = _transition_targets(root, mutation)
        C._require(all(_read(path) == item['before'] for item, path in mutable), 'concurrent_edit')
        verify(mutation.to_data())
        # Callbacks cannot waive optimistic byte preconditions.
        immutable, mutable = _transition_targets(root, mutation)
        C._require(all(_read(path) == item['before'] for item, path in mutable), 'concurrent_edit')
        _private_journal_home(root, journal, mutation)
        publish_immutable(primary, mutation.to_bytes(), root=root)
        replicas = _prepare_replicas(root, journal, mutation)
        ready = _ready_path(primary, mutation)
        if replicas:
            publish_immutable(ready, mutation._data['digest'].encode('ascii'), root=root)
        _apply_transition(immutable, mutable, 'after', root)
        if on_committed is not None:
            on_committed(mutation.to_data())
        _remove_journals(primary, [*replicas, *([ready] if replicas else [])])


def recover_transition(root, journal, *, verify, direction='after', on_committed=None):
    """Resume retained v2 bytes or restore marker/view while keeping immutable evidence."""
    C._require(direction in ('before', 'after') and callable(verify), 'invalid_recovery')
    C._require(on_committed is None or callable(on_committed), 'invalid_completion_callback')
    primary = _target(root, journal)
    raw = _read(primary)
    C._require(raw is not None, 'no_recovery_pending')
    mutation = PreparedMutation.from_bytes(raw)
    C._require('group' not in mutation._data['baseline'], 'group_recovery_required')
    with directory_guards(participant_directories(root, mutation), exclusive=True):
        C._require(_read(primary) == raw, 'concurrent_edit')
        _journal_path(root, journal, mutation)
        cancellation_receipt = None
        if mutation._data['baseline'].get('direction') == 'activate':
            try:
                from .provenance import layout
            except ImportError:
                from provenance import layout
            entry = Path(root).resolve() / mutation._data['entry']
            receipt = Path(layout(entry)['history_cancellations']) / (mutation._data['operation'] + '.yaml')
            receipt = _target(root, receipt.relative_to(Path(root).resolve()).as_posix())
            cancellation_receipt = receipt
            C._require(not cancellation_receipt.exists(), 'cancellation_recovery_required')
        immutable, mutable = _transition_targets(root, mutation)
        verify(mutation.to_data())
        C._require(cancellation_receipt is None or not cancellation_receipt.exists(),
                   'cancellation_recovery_required')
        immutable, mutable = _transition_targets(root, mutation)
        replicas = _prepare_replicas(root, journal, mutation)
        ready = _ready_path(primary, mutation)
        C._require(_read(ready) in (None, mutation._data['digest'].encode('ascii')), 'invalid_ready_marker')
        if replicas:
            publish_immutable(ready, mutation._data['digest'].encode('ascii'), root=root)
        _apply_transition(immutable, mutable, direction, root)
        if direction == 'after' and on_committed is not None:
            on_committed(mutation.to_data())
        _remove_journals(primary, [*replicas, *([ready] if replicas else [])])
    return mutation
