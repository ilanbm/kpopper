"""Privacy-first explicit routing at the common record mutation boundary."""
import base64
import copy
import hashlib
import os
from pathlib import Path
import uuid

try:
    from . import knowledge_views as V, pending_grounding as G
except ImportError:
    import knowledge_views as V
    import pending_grounding as G

ROUTING = {'shareability', 'scope', 'environment', 'commit', 'event_id', 'contribution_id', 'evidence', 'evidence_root'}


def private_drafts(project):
    """Discover retained private obligations without reading or exposing their bodies."""
    key = hashlib.sha256(str(project.common or project.root).encode()).hexdigest()
    home = Path(os.environ.get('KPOPPER_PRIVATE_HOME', Path.home() / '.local/share/kpopper/private'))
    return [{'event_id': path.stem, 'state': 'private draft', 'path': str(path)}
            for path in sorted((home / key).glob('*.json')) if path.is_file()]


def private_marker(value):
    if isinstance(value, dict):
        for key, item in value.items():
            if key == 'shareability' and item != 'project':
                return True
            if key in ('privacy', 'visibility') and item in ('private', 'unclear', 'unknown', 'personal'):
                return True
            if key == 'private' and item is not False:
                return True
            if private_marker(item):
                return True
    elif isinstance(value, (list, tuple)):
        return any(private_marker(item) for item in value)
    return False


def private_locator(value):
    if isinstance(value, dict):
        for key, item in value.items():
            if key in ('file', 'path', 'location', 'source_file', 'uri', 'url', 'from') and isinstance(item, str) and (
                    item.startswith(('file:', '/', '~/')) or (len(item) > 2 and item[1:3] in (':/', ':\\'))):
                return True
            if private_locator(item):
                return True
    elif isinstance(value, (list, tuple)):
        return any(private_locator(item) for item in value)
    return False


def draft(project, action, doc, reason):
    """Retain complete structured intent privately, including explicitly supplied bytes."""
    key = hashlib.sha256(str(project.common or project.root).encode()).hexdigest()
    home = Path(os.environ.get('KPOPPER_PRIVATE_HOME', Path.home() / '.local/share/kpopper/private'))
    # Private state cannot be redirected into the project or another Git checkout.
    probe = home
    while not probe.exists():
        probe = probe.parent
    if G.M.Project(probe).git:
        raise ValueError('private draft home must be outside Git')
    try:
        home.resolve().relative_to(project.root)
    except ValueError:
        pass
    else:
        raise ValueError('private draft home must be outside the project')
    ident = action.get('event_id') or uuid.uuid4().hex
    if not G.TOKEN.fullmatch(ident):
        raise ValueError('private draft event ID must be a portable token')
    payload = copy.deepcopy(action)
    evidence = payload.pop('evidence', {}) or {}
    body = {'version': 1, 'state': 'private draft', 'reason': reason,
            'action': payload, 'document': dict(doc),
            'evidence': {p: base64.b64encode(b).decode('ascii') for p, b in evidence.items()}}
    data = G.json_bytes(G._encode(body))
    path = home / key / (ident + '.json')
    with G.I._file_lock(home / key / 'drafts.lock'):
        if path.exists() and path.read_bytes() != data:
            raise ValueError('private draft event ID already has different content')
        G.I._atomic(path, data)
    return {'state': 'private draft', 'path': str(path), 'reason': reason}


def private_route(paths, action, reader):
    doc = reader.load(paths, read_mode='frozen')
    # This locked check is conservative: entry/source privacy cannot be weakened by
    # an edit while a routed local writer waits to acquire the directory.
    value = copy.deepcopy(action.get('body', {}))
    existing = G.entries(doc)
    nid = action.get('id')
    referenced = set()
    if nid in existing:
        referenced.add(nid)
    for text in G._strings(value):
        if text in existing:
            referenced.add(text)
    if action.get('source') in existing:
        referenced.add(action['source'])
    selected = G.closure(doc, sorted(referenced)) if referenced else {}
    if private_marker(action) or private_marker(selected):
        return draft(V.project_for(paths), action, selected, 'private or unclear source permission')
    return None


def route(paths, action, reader):
    """Return a durable receipt or None for an intentional local-file mutation.

    Legacy unannotated writes retain local semantics, never implicit publication.
    Explicit routing with missing permission is private. Private closure markers
    override a caller's project-level declaration.
    """
    project = V.project_for(paths)
    explicit = bool(ROUTING.intersection(action))
    if action.get('shareability') not in (None, 'project', 'private', 'unclear'):
        raise ValueError('shareability must be project, private or unclear')
    doc = reader.load(paths, read_mode='frozen') if Path(paths[0]).exists() else reader.Record()
    existing_private = private_route(paths, action, reader) if Path(paths[0]).exists() else None
    if existing_private is not None:
        return existing_private
    candidate = copy.deepcopy(doc)
    nid = action.get('id')
    if action.get('kind') == 'add':
        for group in reader.collections_of(candidate).values():
            group.pop(nid, None)
        candidate.setdefault(action.get('into') or 'known', {})[nid] = copy.deepcopy(action['body'])
    elif action.get('kind') == 'set' and nid in G.entries(candidate):
        collection, old = G.entries(candidate)[nid]
        if isinstance(old, dict):
            body = copy.deepcopy(old)
            body['v'] = action['value']
            if action.get('source'):
                body['from'] = action['source']
            candidate[collection][nid] = body
    # Inspect only the selected closure, so unrelated private entries do not taint a
    # permitted independent contribution. Failure to resolve a closure cannot allow it.
    selected = candidate
    if nid in G.entries(candidate):
        try:
            selected = G.closure(candidate, [nid])
        except ValueError:
            if explicit:
                raise
    if private_marker(action) or private_marker(selected):
        return draft(project, action, selected, 'private or unclear source permission')
    if not explicit:
        return None
    if private_locator(selected):
        return draft(project, action, selected, 'private source locator needs explicit portable evidence reconciliation')
    if action.get('shareability') != 'project':
        return draft(project, action, selected, 'sharing permission is private or unclear')
    scope = action.get('scope', 'unclear')
    scope = copy.deepcopy(scope) if isinstance(scope, dict) else {
        'kind': scope, 'environment': action.get('environment', '')}
    if action.get('commit'):
        scope['commit'] = action['commit']
    if scope.get('kind') not in ('project', 'external', 'code', 'feature', 'unclear'):
        raise ValueError('scope must be project, external, code, feature or unclear')
    if action.get('evidence_root'):
        root = Path(action['evidence_root']).expanduser().resolve()
        files = {}
        for name in G._files(selected):
            G.M.relative_path(name)
            path = (root / name).resolve()
            path.relative_to(root)
            files[name] = path.read_bytes()
        action['evidence'] = files
    if scope['kind'] == 'code':
        # Validation happens before the local write, retaining code-world in the body.
        if action['kind'] != 'add' or not isinstance(action.get('body'), dict):
            raise ValueError('code scope needs an add with a complete body and exact commit')
        action['body']['scope'] = scope
        collection, body = G.entries(candidate)[nid]
        body['scope'] = scope
        G.prepare(candidate, [nid], scope=scope, shareability='project', evidence=action.get('evidence'))
        return None
    if project.config()['mode'] == 'simple' or scope['kind'] in ('feature', 'unclear'):
        return None
    if action.get('hypothesis'):
        raise ValueError('named hypotheses stay in the local record; use feature scope')
    if action['kind'] not in ('add', 'set'):
        raise ValueError('a project contribution requires a complete add or set, not a review refresh')
    bundle = G.prepare(candidate, [nid], scope=scope, shareability='project', evidence=action.get('evidence'))
    return G.Store(project).capture(bundle, event_id=action.get('event_id') or uuid.uuid4().hex,
                                   contribution_id=action.get('contribution_id') or nid,
                                   shareability='project')
