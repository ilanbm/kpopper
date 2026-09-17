"""Portable physical keys for exact authored subject text.

This module does not validate object contents or make history authoritative.
Callers must validate immutable IDs/manifest membership separately. A hashed
path deliberately cannot recover its subject: resolve it from a validated
manifest or object envelope, never by normalizing a filesystem directory.
"""
from dataclasses import dataclass
import contextlib
import contextvars
import functools
import hashlib
import re

CAPABILITY = 'subject-paths/v2'
HASHED = 'hashed-subject/v1'
LEGACY = 'legacy-subject/v1'
DOMAIN = b'kpopper-history-subject-path/v1\x00'
MAX_SUBJECT_BYTES = 1024 * 1024
LEGACY_DIRECTORY = re.compile(r'[A-Za-z0-9_][A-Za-z0-9_.-]*\Z')
HASHED_DIRECTORY = re.compile(r'~[0-9a-f]{64}\Z')
OBJECT_ID = re.compile(r'(?:[0-9a-f]{40}|[0-9a-f]{64})\Z')
_SCHEME = contextvars.ContextVar('history_object_path_scheme', default=HASHED)


class HistoryPathError(ValueError):
    def __init__(self, code, detail=''):
        self.code = code
        super().__init__(code + (': ' + detail if detail else ''))


def _require(condition, code, detail=''):
    if not condition:
        raise HistoryPathError(code, detail)


def validate_subject(subject):
    """Return exact supported text, with no normalization, case folding or trim.

    Empty/control/path-like text is permitted as logical data because existing
    Snapshot input preserves it. It never becomes an interpolated native path.
    Invalid UTF-8 scalar sequences and oversized names refuse explicitly.
    """
    _require(type(subject) is str, 'invalid_history_subject')
    try:
        raw = subject.encode('utf-8', errors='strict')
    except UnicodeError as error:
        raise HistoryPathError('invalid_subject_encoding') from error
    _require(len(raw) <= MAX_SUBJECT_BYTES, 'history_subject_limit')
    return subject


def subject_directory(subject, *, scheme=None):
    validate_subject(subject)
    scheme = _SCHEME.get() if scheme is None else scheme
    if scheme == LEGACY:
        _require(LEGACY_DIRECTORY.fullmatch(subject) is not None, 'invalid_legacy_subject_path')
        return subject
    _require(scheme == HASHED, 'unsupported_subject_path_scheme')
    return '~' + hashlib.sha256(DOMAIN + subject.encode('utf-8')).hexdigest()


def object_path(subject, version, *, scheme=None):
    """Path relative to a reader-owned history/ or portable objects/ directory.

    New files use HASHED. LEGACY is an explicit compatibility mode for retained
    old manifests/mutations; callers must never guess it from subject spelling.
    """
    _require(isinstance(version, str) and OBJECT_ID.fullmatch(version) is not None,
             'invalid_history_object_id')
    return subject_directory(subject, scheme=scheme) + '/' + version + '.yaml'


@dataclass(frozen=True)
class ObjectPath:
    scheme: str
    directory: str
    version: str

    @property
    def legacy_subject(self):
        """Only legacy raw directories encode the logical subject verbatim."""
        return self.directory if self.scheme == LEGACY else None


def parse_object_path(path):
    """Parse a strict portable two-component path without consulting disk."""
    _require(isinstance(path, str) and '\\' not in path and '\x00' not in path,
             'invalid_history_object_path')
    pieces = path.split('/')
    _require(len(pieces) == 2 and all(pieces), 'invalid_history_object_path')
    directory, filename = pieces
    _require(filename.endswith('.yaml'), 'invalid_history_object_path')
    version = filename[:-5]
    _require(OBJECT_ID.fullmatch(version) is not None, 'invalid_history_object_path')
    if HASHED_DIRECTORY.fullmatch(directory):
        scheme = HASHED
    else:
        _require(LEGACY_DIRECTORY.fullmatch(directory) is not None, 'invalid_history_object_path')
        scheme = LEGACY
    return ObjectPath(scheme, directory, version)


def validate_object_path(path, subject, version):
    """Bind stored bytes to the exact subject and immutable filename identity."""
    parsed = parse_object_path(path)
    expected = object_path(subject, version, scheme=parsed.scheme)
    _require(path == expected, 'history_object_path_mismatch')
    return parsed.scheme


def resolve_object_path(paths, subject, version):
    """Resolve one declared manifest object without decoding orphan staged files.

    ``paths`` is a captured path-membership set. Unknown staged paths/bytes stay
    reader-private inventory. Two physical representations of one logical object
    refuse so migration cannot silently drop one of the retained originals.
    """
    validate_subject(subject)
    # Reading probes both layouts independently of a surrounding writer replay.
    candidates = [object_path(subject, version, scheme=HASHED)]
    if LEGACY_DIRECTORY.fullmatch(subject):
        candidates.append(object_path(subject, version, scheme=LEGACY))
    matches = [path for path in candidates if path in paths]
    _require(matches, 'missing_history_object_path')
    _require(len(matches) == 1, 'duplicate_history_object_path')
    validate_object_path(matches[0], subject, version)
    return matches[0]


def validate_path_capability(paths, requires):
    """New physical paths cannot silently masquerade as old reader capability."""
    _require(isinstance(requires, (list, tuple, set)) and all(isinstance(item, str) for item in requires),
             'invalid_history_path_capabilities')
    if any(parse_object_path(path).scheme == HASHED for path in paths):
        _require(CAPABILITY in requires, 'subject_path_capability_required')


def commit_requires(requires=None):
    """Fresh path layout is explicit; old replay preserves None versus []."""
    if _SCHEME.get() == LEGACY:
        return requires
    return sorted(set(requires or []) | {CAPABILITY})


@contextlib.contextmanager
def replay_layout(manifest):
    """Retained commit capability chooses layout without changing any receipt."""
    _require(isinstance(manifest, dict), 'invalid_history_path_capabilities')
    requires = manifest.get('requires', [])
    known = {CAPABILITY, 'explicit-root-disposition/v1', 'history-closure/v1',
             'history-generations/v1', 'generation-cancellation/v1', 'history-subset/v1'}
    _require(isinstance(requires, list) and all(isinstance(item, str) for item in requires)
             and len(requires) == len(set(requires)) and set(requires) <= known,
             'invalid_history_path_capabilities')
    token = _SCHEME.set(HASHED if CAPABILITY in requires else LEGACY)
    try:
        yield
    finally:
        _SCHEME.reset(token)


def replay_mutation(function):
    """Use exact old writer layout through recursive replay, including batches."""
    @functools.wraps(function)
    def wrapped(entry, mutation, *args, **kwargs):
        try:
            from . import history_contract as C
        except ImportError:
            import history_contract as C
        manifests = [item for item in mutation.files if item['role'] == 'history_commit']
        if not manifests:
            return function(entry, mutation, *args, **kwargs)
        _require(len(manifests) == 1, 'invalid_history_path_manifest')
        manifest = C.validate_commit(C.decode_document(manifests[0]['after']))
        with replay_layout(manifest):
            return function(entry, mutation, *args, **kwargs)
    return wrapped


def path_for_object(obj, captured=None):
    """Reused immutable objects retain their observed path; new ones use context."""
    key = obj['subject'], obj['id']
    path = getattr(captured, 'object_paths', {}).get(key) if captured is not None else None
    if path is not None:
        validate_object_path(path, *key)
        return path
    return object_path(*key)
