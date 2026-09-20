"""Private session write evidence and Stop delivery receipts, never admission state."""
import contextlib
import hashlib
import importlib
import io
import json
import os
from pathlib import Path
import re
import tempfile


def _peer(name):
    return importlib.import_module((__package__ + '.' if __package__ else '') + name)


def session_id():
    return os.environ.get('KPOPPER_AGENT_SESSION') or os.environ.get('CODEX_THREAD_ID')


def _home(sid):
    if not isinstance(sid, str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,200}', sid):
        return None
    # Match the shell hooks even when tempfile cached a different directory earlier.
    return Path(os.environ.get('TMPDIR') or tempfile.gettempdir()) / ('kpopper-session-' + sid)


def _key(path):
    return hashlib.sha256(str(Path(path).resolve()).encode('utf-8')).hexdigest()


def _read(path):
    try:
        if path.is_symlink() or path.stat().st_size > 1024 * 1024:
            return {}
        value = json.loads(path.read_text(encoding='utf-8'))
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def _update(sid, name, change):
    home = _home(sid)
    if home is None:
        return None
    home.mkdir(mode=0o700, exist_ok=True)
    if home.is_symlink():
        raise ValueError('session state must not be a symlink')
    path = home / (name + '.json')
    # A separate private lock domain: callbacks touch only this JSON state. Never
    # acquire record locks here (writers already hold theirs; Stop assesses first).
    with _peer('ingestion')._file_lock(home / 'state.lock'):
        state = _read(path)
        result = change(state)
        data = json.dumps(state, ensure_ascii=False, sort_keys=True).encode('utf-8')
        if len(data) > 1024 * 1024:
            raise ValueError('session state limit exceeded')
        fd, temporary = tempfile.mkstemp(prefix='.session-', dir=home)
        try:
            with os.fdopen(fd, 'wb') as output:
                output.write(data)
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, path)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
        return result


def published(reader, root, files, *, subjects=None):
    """Record changed bodies from successfully published images, under the writer lock.

    No scan of the current tree can establish who wrote it. Only the writer calls this,
    after publication; previews, private drafts, no-ops and failed writes add no evidence.
    An unavailable receipt loses attribution, never turns a committed write into failure.
    """
    sid = session_id()
    if _home(sid) is None:
        return
    try:
        identity = _peer('pending_grounding').identity
        for item in files:
            if item['role'] not in ('record', 'record_member', 'hypothesis'):
                continue
            before = reader.bodies(reader.parse(text=item['before'].decode('utf-8')) or {}) if item['before'] else {}
            after = reader.bodies(reader.parse(text=item['after'].decode('utf-8')) or {}) if item['after'] else {}
            changed = {nid: identity(body) for nid, body in after.items()
                       if (subjects is None or nid in subjects)
                       and (nid not in before or identity(before[nid]) != identity(body))}
            removed = (set(before) - set(after)) & (set(before) if subjects is None else subjects)
            if not changed and not removed:
                continue

            def save(state):
                for nid in removed:
                    state.pop(nid, None)
                state.update(changed)

            _update(sid, 'writes-' + _key(Path(root) / item['path']), save)
    except Exception:
        # This is optional attribution evidence, not a second commit boundary.
        return


def owned(reader, doc, sid):
    """Current bodies whose exact source file and fingerprint have a write receipt."""
    home = _home(sid)
    if home is None or home.is_symlink():
        return set()
    identity = _peer('pending_grounding').identity
    raw = reader.bodies(doc)
    origins = {nid: doc.origins.get(section, {}).get(nid)
               for section, members in doc.items() if isinstance(members, dict) for nid in members
               if section not in ('meta', 'schema', 'record', 'also')}
    for hypothesis in doc.hypotheses.values():
        for nid, body in hypothesis['raw'].items():
            if nid not in raw:
                raw[nid], origins[nid] = body, hypothesis['path']
    receipts, result = {}, set()
    for nid, body in raw.items():
        origin = origins.get(nid)
        if not origin:
            continue
        try:
            key = _key(origin)
            if key not in receipts:
                receipts[key] = _read(home / ('writes-' + key + '.json'))
            if receipts[key].get(nid) == identity(body):
                result.add(nid)
        except (OSError, TypeError, ValueError):
            # Unsupported attribution must not prevent reporting integrity findings.
            continue
    return result


def stop(reader, state_path, paths, sid, turns=0, host=None, nudged_at=None):
    """Assess every time; deliver each finding once per session and resolved record."""
    issues = []
    with contextlib.redirect_stdout(io.StringIO()) as output:
        code = reader.gate(state_path, paths, turns, host, nudged_at,
                           _session_id=sid, _issues=issues)
    if code != 2:
        return code
    if not issues:
        message = output.getvalue().strip() or 'The record assessment could not complete.'
        issues.append(('assessment', message, message))
    identity = _peer('pending_grounding').identity
    record = identity(sorted(str(Path(path).resolve()) for path in paths))

    def select(state):
        fresh = []
        for kind, condition, message in issues:
            key = identity([kind, condition])
            if key not in state:
                state[key] = True
                fresh.append(message)
        return fresh

    try:
        fresh = _update(sid, 'delivery-' + record, select)
    except Exception:
        # If once-only delivery cannot be retained, do not trap the host in a loop.
        return 0
    for message in fresh or []:
        print(message)
    return 2 if fresh else 0
