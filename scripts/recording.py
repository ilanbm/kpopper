"""Privacy-first explicit routing at the common record mutation boundary."""
import base64
import copy
import datetime
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


def private_route(paths, action, reader, project=None):
    doc = reader._peer('reasoning.authoring').load(reader, paths)
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
    referenced.update(name for name in G.P._mentioned(value) if name in existing)
    if action.get('source') in existing:
        referenced.add(action['source'])
    selected = G.closure(doc, sorted(referenced)) if referenced else {}
    controls = {k: doc[k] for k in ('meta', 'privacy', 'visibility', 'private', 'shareability') if k in doc}
    if private_marker(controls):
        selected.update(copy.deepcopy(controls))
        return draft(project or V.project_for(paths), action, selected, 'private or unclear record permission')
    if private_marker(action) or private_marker(selected):
        return draft(project or V.project_for(paths), action, selected, 'private or unclear source permission')
    return None


def set_body(old, action):
    """The complete semantic set edit, shared by capture and scoped local writes."""
    body = copy.deepcopy(old)
    field = 'v' if 'v' in body else 'quoted' if 'quoted' in body else None
    if field is None:
        raise ValueError('set needs an entry with v or quoted')
    body[field] = action['value']
    body['of'] = action.get('as_of') or datetime.date.today().isoformat()
    if action.get('source') is not None:
        if action.get('at') is None:
            raise ValueError('a changed source needs its exact at location')
        body['from'], body['at'] = action['source'], action['at']
    if '_record_scope' in action:
        body['scope'] = copy.deepcopy(action['_record_scope'])
    return body


def route(paths, action, reader, project=None, expected_policy=None):
    """Return a durable receipt or None for an intentional local-file mutation.

    Legacy unannotated writes retain local semantics, never implicit publication.
    Explicit routing with missing permission is private. Private closure markers
    override a caller's project-level declaration.
    """
    project = project or V.project_for(paths)
    policy = project.config()
    if expected_policy is not None and policy != expected_policy:
        raise ValueError('project policy or destination changed before routing; retry')
    explicit = bool(ROUTING.intersection(action))
    if action.get('shareability') not in (None, 'project', 'private', 'unclear'):
        raise ValueError('shareability must be project, private or unclear')
    source_hashes = {str(path): hashlib.sha256(Path(path).read_bytes()).hexdigest()
                     for path in G.P._files_of(paths)}
    if not Path(paths[0]).exists():
        source_hashes[str(paths[0])] = None
    doc = reader._peer('reasoning.authoring').load(reader, paths) if Path(paths[0]).exists() else reader.Record()
    if any((hashlib.sha256(Path(path).read_bytes()).hexdigest() if Path(path).is_file() else None) != digest
           for path, digest in source_hashes.items()):
        raise ValueError('record changed while preparing the write; retry')
    existing_private = private_route(paths, action, reader, project) if Path(paths[0]).exists() else None
    if existing_private is not None:
        return existing_private
    scope = action.get('scope', 'unclear')
    scope = copy.deepcopy(scope) if isinstance(scope, dict) else {
        'kind': scope, 'environment': action.get('environment', '')}
    if action.get('commit'):
        scope['commit'] = action['commit']
    if explicit and action.get('shareability') == 'project':
        if scope.get('kind') not in ('project', 'external', 'code', 'feature', 'unclear'):
            raise ValueError('scope must be project, external, code, feature or unclear')
        declared_scope = any(k in action for k in ('scope', 'environment', 'commit'))
        if declared_scope:
            action['_record_scope'] = scope
        if action.get('kind') == 'add':
            if not isinstance(action.get('body'), dict):
                raise ValueError('scoped writes need a complete entry body')
            if declared_scope and 'scope' in action['body'] and G.identity(action['body']['scope']) != G.identity(scope):
                raise ValueError('entry body scope differs from the requested scope')
            if declared_scope:
                action['body']['scope'] = copy.deepcopy(scope)
    candidate = copy.deepcopy(doc)
    nid = action.get('id')
    if action.get('kind') == 'add':
        for group in reader.collections_of(candidate).values():
            group.pop(nid, None)
        collection = action.get('into') or 'known'
        if candidate.get(collection) is None:
            candidate[collection] = {}
        candidate[collection][nid] = copy.deepcopy(action['body'])
    elif action.get('kind') == 'set' and nid in G.entries(candidate):
        collection, old = G.entries(candidate)[nid]
        if isinstance(old, dict):
            if explicit or 'v' in old or 'quoted' in old:
                candidate[collection][nid] = set_body(old, action)
        elif explicit:
            raise ValueError('scoped set needs a complete entry body')
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
    if policy['mode'] == 'simple' or scope['kind'] in ('feature', 'unclear'):
        return None
    if action.get('hypothesis'):
        raise ValueError('named hypotheses stay in the local record; use feature scope')
    if action['kind'] not in ('add', 'set'):
        raise ValueError('a project contribution requires a complete add or set, not a review refresh')
    if reader._peer('reasoning.authoring').selected(doc, action.get('profile')):
        raise ValueError('unsupported_capability: core contribution capture requires the versioned contribution writer')
    # Capture the authored representation, just as the file writer does. In
    # particular readable formulas must be lowered/validated before identity is
    # assigned, and a new judgment needs its initial dependency snapshot.
    base = doc or {'meta': {}}
    ids, judgments, fields = G.P.infer(base)
    fields = G.P.authored_fields(action, fields)
    raw = G.P.with_builtins(base, ids, judgments, fields)
    authored, notes = G.P.normalize_authored(action, ids, fields, raw)
    refusals = G.P.validate(authored, base, ids, judgments, fields, raw)
    if refusals:
        raise ValueError('; '.join(refusals))
    if action['kind'] == 'add':
        body = copy.deepcopy(authored['body'])
        snapshot_field = fields['snapshot'] or 'seen'
        if fields['deps'] in body and snapshot_field not in body:
            body[snapshot_field] = G.P._snapshot(body[fields['deps']], raw, ids, judgments, paths,
                                               G.P._brief_beside(paths[0]))
        collection = G.P._collection_for(base, ids, judgments, fields, nid, body, action.get('into'))
        for members in G.P.collections_of(candidate).values():
            members.pop(nid, None)
        candidate.setdefault(collection, {})[nid] = body
    bundle = G.prepare(candidate, [nid], scope=scope, shareability='project', evidence=action.get('evidence'))
    receipt = G.Store(project).capture(bundle, event_id=action.get('event_id') or uuid.uuid4().hex,
                                   contribution_id=action.get('contribution_id') or nid,
                                   shareability='project', expected_generation=policy['generation'], expected_policy=policy,
                                   expected_sources=source_hashes)
    if notes:
        receipt['diagnostics'] = notes
    return receipt
