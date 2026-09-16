"""Derived page counts, published by an explicit build and read without rendering.

The input binding is the authority for freshness; timestamps are descriptive only.
Measurements do not say the page or its judgments passed verification.
"""
import datetime as dt
import glob
import hashlib
import json
import math
import os
from pathlib import Path
import sys
import tempfile

try:
    from . import provenance as P
except ImportError:
    import provenance as P

DEFAULT = object()
ROOT = Path(__file__).resolve().parent
MAX_BYTES = 256 * 1024
CODE = [ROOT / name for name in ('page_measurements.py', 'provenance.py', 'expressions.py', 'assessment.py',
        'render_page.py', 'page_words.py', 'page_lint.py', 'sameness.py',
        'knowledge_views.py', 'project_modes.py', 'pending_grounding.py', 'pending_publication.py',
        'session/core.py', 'session/model.py', 'session/lean/Main.lean')]
CODE += sorted((ROOT / 'page').glob('*'))
CODE = [path for path in CODE if path.is_file()]


def digest(value):
    return hashlib.sha256(json.dumps(value, ensure_ascii=False, sort_keys=True,
                                    allow_nan=False, separators=(',', ':')).encode()).hexdigest()


def _code_identity():
    return {path.relative_to(ROOT).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest() for path in CODE}


# A long-lived reader must not label old imported code as a newly edited program.
LOADED_CODE = _code_identity()


def _core_identity():
    try:
        cls = P.E._core_type()
    except (ValueError, OSError, ImportError):
        return {'ready': False}
    if getattr(sys.modules[cls.__module__], 'LOADED_SOURCE_HASH', None) != LOADED_CODE['session/core.py']:
        raise ValueError('loaded core transport changed; restart the reader')
    try:
        core = cls()
        return {'ready': True, 'source': core.build['source_sha256'], 'binary': core.build['binary_sha256']}
    except (ValueError, OSError, ImportError):
        return {'ready': False}


def _paths(paths, brief):
    files = list(P._files_of(paths))
    directory = P.hypothesis_dir(paths)
    files += sorted(glob.glob(os.path.join(directory, '*.yaml')) + glob.glob(os.path.join(directory, '*.yml')))
    if brief is not None:
        files.append(brief)
    return list(dict.fromkeys(os.path.abspath(str(path)) for path in files))


def _stamp(path):
    stat = path.stat()
    return stat.st_ino, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns


def snapshot(paths, brief=DEFAULT):
    roots = [os.path.abspath(str(path)) for path in paths]
    if not roots:
        raise ValueError('page measurement needs a record')
    selected = list(roots)
    mode = 'frozen' if P._RAW_READS.get() else os.environ.get('KPOPPER_READ_MODE', 'live')
    if mode not in ('live', 'frozen'):
        raise ValueError('read mode must be live or frozen')
    views = P._peer('knowledge_views')
    context = {'read_mode': mode}
    doc = P.Record()
    if mode == 'live':
        policy = views.project_for(selected).config()
        roots = [os.path.abspath(path) for path in views.write_paths(selected)]
        context['mode'] = policy['mode']
        # Simple aliases select the same graph even when a direct shared-path caller
        # is outside the owning Git project. Owner policy is routing, not graph identity.
        if policy['mode'] == 'advanced':
            context['policy'] = policy
            # Bind the moving overlay independently; checkout documents are hashed below.
            # Do not insert an extra semantic read into callers' measurement boundary.
            doc = views.overlay(roots, P.Record(), read_mode=mode)
            context.update({key: getattr(doc, key, None) for key in (
                'pending_ref', 'contributions', 'publication', 'knowledge_conflicts',
                'target_unavailable', 'private_drafts')})
            context['knowledge_conflicts'] = {nid: [list(variant) for variant in variants]
                for nid, variants in getattr(doc, 'knowledge_conflicts', {}).items()}
    if brief is DEFAULT:
        brief = P._brief_beside(P._first_of(roots))
    brief = os.path.abspath(str(brief)) if brief is not None else None
    scope = {'records': roots, 'brief': brief, 'read_mode': mode}
    code = _code_identity()
    if code != LOADED_CODE:
        raise ValueError('page measurement code changed; restart the reader and build the page again')
    if getattr(P, 'LOADED_SOURCE_HASH', None) != code['provenance.py'] or \
            getattr(P.E, 'LOADED_SOURCE_HASH', None) != code['expressions.py']:
        raise ValueError('loaded record reader changed; restart it before using page measurements')
    if P.assessment_module().LOADED_SOURCE_HASH != code['assessment.py']:
        raise ValueError('loaded assessment changed; restart it before using page measurements')
    paths_read = _paths(roots, brief)
    if mode == 'live' and getattr(doc, 'pending_ref', None):
        paths_read = [name for name in paths_read if Path(name).exists()]
    files, stamps = {}, {}
    for name in paths_read:
        path = Path(name)
        before = _stamp(path)
        data = path.read_bytes()
        after = _stamp(path)
        if before != after:
            raise ValueError('page inputs changed while being read; retry')
        files[name] = {'sha256': hashlib.sha256(data).hexdigest(), 'resolved': str(path.resolve())}
        stamps[name] = after
    current_paths = _paths(roots, brief)
    if mode == 'live' and getattr(doc, 'pending_ref', None):
        current_paths = [name for name in current_paths if Path(name).exists()]
    if paths_read != current_paths:
        raise ValueError('page inputs changed while being read; retry')
    identity = digest({'context': views.G.identity(context), 'resolved_records': roots, 'scope': scope, 'files': files, 'code': code, 'core': _core_identity(),
                       'python': list(sys.version_info[:3]), 'yaml': P.yaml.__version__})
    return {'scope': scope, 'identity': identity, 'stamps': stamps, 'requested_records': selected}


def unchanged(before, after):
    return before['identity'] == after['identity'] and before['stamps'] == after['stamps']


def cache_path(scope):
    base = Path(os.environ.get('XDG_STATE_HOME', ''))
    if not base.is_absolute():
        base = Path.home() / '.local/state'
    return base / 'kpopper/page-measurements' / (digest(scope) + '.json')


def _counts(value):
    if not isinstance(value, dict) or set(value) - set(P.PAGE):
        raise ValueError('invalid page measurement fields')
    for key, count in value.items():
        if key == 'page.drift':
            valid = type(count) in (int, float) and math.isfinite(count) and 0 <= count <= 1
        else:
            valid = type(count) is int and count >= 0
        if not valid:
            raise ValueError('invalid measured count: ' + key)
    return value


def publish(before, counts):
    """Publish the counts from the original build, never a second rendering."""
    counts = _counts({key: value for key, value in counts.items() if value is not None})
    after = snapshot(before.get('requested_records', before['scope']['records']), before['scope']['brief'])
    if not unchanged(before, after):
        raise ValueError('record or view changed during page measurement; build again')
    payload = {'schema': 1, 'scope': before['scope'], 'inputs': before['identity'], 'counts': counts,
               'measured_at': dt.datetime.now(dt.timezone.utc).isoformat()}
    payload['sha256'] = digest(payload)
    encoded = json.dumps(payload, ensure_ascii=False, sort_keys=True, allow_nan=False).encode()
    if len(encoded) > MAX_BYTES:
        raise ValueError('page measurement is too large')
    path = cache_path(before['scope'])
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix='.page-measurement-', dir=str(path.parent))
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    return payload


def read(expected):
    """Read only the cache for the caller's already captured graph/view identity."""
    path = cache_path(expected['scope'])
    try:
        if path.is_symlink():
            raise ValueError('page measurement is not a regular cache file')
        with path.open('rb') as stream:
            encoded = stream.read(MAX_BYTES + 1)
        if len(encoded) > MAX_BYTES:
            raise ValueError('page measurement is too large')
        def unique(pairs):
            value = {}
            for key, item in pairs:
                if key in value:
                    raise ValueError('duplicate page measurement field')
                value[key] = item
            return value
        payload = json.loads(encoded, object_pairs_hook=unique)
        if not isinstance(payload, dict) or set(payload) != {'schema', 'scope', 'inputs', 'counts', 'measured_at', 'sha256'}:
            raise ValueError('invalid page measurement')
        checksum = payload.pop('sha256')
        if checksum != digest(payload) or type(payload['schema']) is not int or payload['schema'] != 1 or payload['scope'] != expected['scope']:
            raise ValueError('page measurement integrity check failed')
        if payload['inputs'] != expected['identity']:
            return {}, 'page measurement is stale; run kpop page or page --verify again'
        return _counts(payload['counts']), 'the selected page did not produce this count'
    except FileNotFoundError:
        return {}, 'no measurement for this record and view; run kpop page or page --verify'
    except (OSError, ValueError, TypeError, RecursionError):
        return {}, 'page measurement is unreadable; build the page again'
