"""Immutable, portable contributions with one durable Git acknowledgement boundary.

The store ref is a local ledger, separate from the publisher's reusable PR branch.
Every new ledger commit retains its predecessor. No capture writes a checkout, its
index, a private report into Git, or a second authoritative receipt on disk.
"""
import copy
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import re
import time

try:
    from . import project_modes as M
except ImportError:
    import project_modes as M
I, P = M.I, M.I.P
REF = M.PENDING_REF
TOKEN = re.compile(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,159}')


def _encode(value):
    """Canonical typed JSON: absent/null and bool/int/float/text never coalesce."""
    kind = type(value)
    if value is None:
        return ['null']
    if kind is bool:
        return ['bool', value]
    if kind is int:
        return ['int', str(value)]
    if kind is float:
        if not math.isfinite(value):
            raise ValueError('nonfinite values cannot be shared')
        return ['float', value.hex()]
    if kind is str:
        return ['text', value]
    if kind in (datetime.date, datetime.datetime):
        return [kind.__name__, value.isoformat()]
    if kind is list:
        return ['list', [_encode(x) for x in value]]
    if isinstance(value, dict):
        if any(type(k) is not str for k in value):
            raise ValueError('record mappings need text keys')
        return ['map', [[k, _encode(value[k])] for k in sorted(value)]]
    raise ValueError('unsupported shared value type: ' + kind.__name__)


def _decode(value):
    tag = value[0]
    if tag == 'null':
        return None
    if tag in ('bool', 'text'):
        return value[1]
    if tag == 'int':
        return int(value[1])
    if tag == 'float':
        return float.fromhex(value[1])
    if tag in ('date', 'datetime'):
        return getattr(datetime, tag).fromisoformat(value[1])
    if tag == 'list':
        return [_decode(x) for x in value[1]]
    if tag == 'map':
        return {k: _decode(v) for k, v in value[1]}
    raise ValueError('invalid typed value')


def json_bytes(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'), allow_nan=False).encode('utf-8')


def identity(value):
    return hashlib.sha256(json_bytes(_encode(value))).hexdigest()


def entries(doc):
    result = {}
    for collection, members in P.collections_of(doc).items():
        if collection == 'meta':
            continue
        for name, body in members.items():
            if name in result:
                raise ValueError('duplicate entry across collections: ' + name)
            result[name] = (collection, body)
    return result


def _strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from _strings(item)
    elif isinstance(value, dict):
        for key, item in value.items():
            yield key
            yield from _strings(item)


def _legacy_closure(doc, roots):
    """Retain complete bodies plus every referenced existing entry, recursively.

    Explicit citations and dependencies must resolve. Incidental prose tokens that
    aren't entry references do not create dependencies.
    """
    all_entries = entries(doc)
    if any(root not in all_entries for root in roots):
        raise ValueError('contribution roots must be stored entries, not computed or missing IDs')
    selected, pending = set(), list(roots)
    schema = doc.get('schema') or {}
    while pending:
        name = pending.pop()
        if name in selected or P.is_builtin(name):
            continue
        if name not in all_entries:
            raise ValueError('missing contribution dependency: ' + name)
        selected.add(name)
        body = all_entries[name][1]
        if isinstance(body, dict):
            explicit = body.get(schema.get('deps', 'rests_on'), [])
            if explicit and (not isinstance(explicit, list) or any(not isinstance(x, str) for x in explicit)):
                raise ValueError('contribution dependencies must be entry IDs')
            pending.extend(explicit)
            if 'from' in body:
                source = body['from']
                sources = source if isinstance(source, list) else [source]
                if any(not isinstance(x, str) for x in sources):
                    raise ValueError('contribution sources must be entry IDs')
                # Native records also use prose and filenames (including names
                # such as package.json that look like IDs) as direct citations.
                # Only an existing entry supplies an entry-body dependency here.
                pending.extend(x for x in sources if x in all_entries)
        for text in _strings(body):
            pending.extend(P.refs_in(text))
        # Follow the existing reader's mentions, not arbitrary tokens buried inside
        # provenance metadata. An explicit {{reference}} still resolves everywhere.
        if isinstance(body, dict):
            pending.extend(x for x in P._mentioned(body) if x in all_entries)
    result = {'schema': copy.deepcopy(schema)} if schema else {}
    for name in sorted(selected):
        collection, body = all_entries[name]
        result.setdefault(collection, {})[name] = copy.deepcopy(body)
    return result


def _reasoning_modules():
    # Keep exception/module identities local to the importing package.
    if __package__:
        from .reasoning import contract, language, snapshot, assessment
        return contract, language, snapshot, assessment
    return tuple(P._peer('reasoning.' + name)
                 for name in ('contract', 'language', 'snapshot', 'assessment'))


def _declared_capabilities(document):
    """Read a declaration without authorizing its interpretation."""
    C, _, _, _ = _reasoning_modules()
    meta = document.get('meta')
    if not isinstance(meta, dict) or 'reasoning' not in meta:
        return {'version': 1, 'profile': 'ordinary-reader/v1', 'requires': []}
    value = meta['reasoning']
    if not isinstance(value, dict) or set(value) != {'version', 'profile', 'requires'} \
            or type(value['version']) is not int or not isinstance(value['profile'], str) \
            or not isinstance(value['requires'], list) \
            or any(not isinstance(item, str) for item in value['requires']) \
            or value['requires'] != sorted(set(value['requires'])):
        raise C.CapabilityError('invalid_capability', 'invalid meta.reasoning capability declaration')
    return copy.deepcopy(value)


def document_capabilities(document):
    """Validate declared and actual dependent capabilities, without execution."""
    C, L, S, A = _reasoning_modules()
    meta = document.get('meta')
    if isinstance(meta, dict) and 'history' in meta:
        raise C.CapabilityError('unsupported_history_contribution',
                                'history evidence requires a history-aware contribution format')
    cap = C.capabilities(document)
    fields = S._fields(document) if cap['profile'] == C.PROFILE else None
    # History declarations cannot hide behind an ordinary declaration either.
    if fields is None:
        has_history = any(isinstance(body, dict) and any(
            isinstance(seen, dict) and any(isinstance(old, dict) and
                isinstance(old.get('computed'), dict) and 'version' in old['computed']
                for old in seen.values()) for seen in body.values())
            for _, body in entries(document).values())
        if not has_history:
            return cap
        fields = S._fields(document)
    for nid, (_, body) in entries(document).items():
        if not isinstance(body, dict):
            continue
        seen = body.get(fields['snapshot'])
        for dependency, old in seen.items() if isinstance(seen, dict) else ():
            computed = old.get('computed') if isinstance(old, dict) else None
            if isinstance(computed, dict) and isinstance(computed.get('basis'), dict):
                basis = computed['basis']
                modules = basis.get('modules', [])
                if not isinstance(modules, list) or any(not isinstance(module, str) for module in modules):
                    raise C.CapabilityError('invalid_history', nid + ': invalid historical modules')
                if set(modules) - set(C.MODULES) or basis.get('profile', C.PROFILE) != C.PROFILE:
                    raise C.CapabilityError('unsupported_capability', nid + ': unsupported historical capability')
                if set(modules) - set(cap['requires']):
                    raise C.CapabilityError('invalid_capability', nid + ': undeclared historical modules')
            if isinstance(computed, dict) and 'version' in computed:
                error = A._history(old, document)[2]
                if error:
                    raise C.CapabilityError(error, nid + ': unsupported or malformed core history')
            if cap['profile'] == C.PROFILE and isinstance(computed, dict) and isinstance(computed.get('rule'), dict):
                _historical_rule_capabilities(computed, dependency, nid, C, L)
        if cap['profile'] != C.PROFILE:
            continue
        for field in ('rule', fields['predicate']):
            value = body.get(field)
            if isinstance(value, dict):
                _expression_capabilities(value, nid, C, L)
        deps = body.get(fields['deps'], [])
        if isinstance(deps, list) and any(isinstance(dep, str) and P.is_builtin(dep) for dep in deps):
            raise C.CapabilityError('unsupported_core_builtin', nid + ': computed builtin dependency')
    return cap


def _historical_rule_capabilities(computed, dependency, nid, contract, language):
    rule = computed['rule']
    if set(rule) != {'collection', 'fields'}:
        _expression_capabilities(rule, nid, contract, language)
        return
    # Scope review stores the original definition, not a scalar expression.
    # Authorize its historical witness without looking up today's membership.
    basis = computed.get('basis')
    fields = rule['fields']
    if not isinstance(rule['collection'], str) or not rule['collection'] \
            or not isinstance(fields, list) or any(not isinstance(field, str) or not field for field in fields) \
            or fields != sorted(set(fields)) or not isinstance(basis, dict):
        raise contract.CapabilityError('invalid_history', nid + ': invalid historical scope definition')
    witness = basis.get('witness')
    if witness is not None:
        if not isinstance(witness, dict) or witness.get('kind') != 'scope' \
                or witness.get('scope_id') != dependency \
                or witness.get('definition_digest') != contract.digest(rule):
            raise contract.CapabilityError('invalid_history', nid + ': historical scope definition disagrees with its witness')
    elif 'recipe' in basis or not {'members', 'dependencies'} <= set(basis):
        # Preserve the supported older scope basis which carried membership but
        # no recipe/witness. It remains distinct evidence, never upgraded here.
        raise contract.CapabilityError('invalid_history', nid + ': missing historical scope witness')
    value = computed.get('value')
    if not isinstance(value, dict) or value.get('type') != 'record':
        raise contract.CapabilityError('invalid_history', nid + ': historical scope needs its typed summary')


def _expression_capabilities(value, nid, contract, language):
    try:
        expression = language.lower(value)
        if any(P.is_builtin(ref) for ref in language.references(expression)):
            raise contract.CapabilityError('unsupported_core_builtin', nid + ': computed builtin input')
        return expression
    except contract.CapabilityError:
        raise
    except (ValueError, TypeError, SyntaxError, RecursionError) as error:
        raise contract.CapabilityError('invalid_expression', nid + ': ' + str(error)) from None


def meaning_capabilities(document):
    cap = document_capabilities(document)
    return {key: copy.deepcopy(cap[key]) for key in ('profile', 'requires')}


def closure(doc, roots):
    """Capture complete bodies, core dependencies and named scope authority."""
    C, L, S, _ = _reasoning_modules()
    cap = document_capabilities(doc)
    selected = _legacy_closure(doc, roots)
    if cap['profile'] == C.PROFILE:
        fields = S._fields(doc)
        all_entries = entries(doc)
        scopes, processed = set(), set()
        # The snapshot captures authority and validates scopes without evaluation.
        snapshot = None
        while set(entries(selected)) - processed:
            for nid in sorted(set(entries(selected)) - processed):
                processed.add(nid)
                body = all_entries[nid][1]
                if not isinstance(body, dict):
                    continue
                dependencies = []
                for field in ('rule', fields['predicate']):
                    if isinstance(body.get(field), dict):
                        dependencies.extend(L.references(L.lower(body[field])))
                if 'collection_scope' in body:
                    if snapshot is None:
                        snapshot = S.Snapshot.from_data(doc)
                    captured = snapshot.capture_scope(nid).to_data()
                    collection = captured['definition']['collection']
                    scopes.add(collection)
                    dependencies.extend(doc[collection])
                if dependencies:
                    extra = _legacy_closure(doc, dependencies)
                    for collection, members in extra.items():
                        if collection != 'schema':
                            selected.setdefault(collection, {}).update(members)
        for collection in scopes:
            selected.setdefault(collection, {})
    meta = doc.get('meta')
    if isinstance(meta, dict) and 'reasoning' in meta:
        selected['meta'] = {'reasoning': copy.deepcopy(meta['reasoning'])}
    return selected


def validate_bundle(bundle, *, supported=True):
    """Check frozen identity/evidence; optionally authorize its capabilities.

    Raw archival reads preserve unknown declarations but never accept a v2
    declaration which disagrees with the document bound by the same identity.
    Raw reads and every v1 identity check use the stored closure. Supported v2
    interpretation also checks complete current closure, scope and evidence.
    """
    manifest = bundle['manifest']
    if type(manifest.get('version')) is not int or manifest['version'] not in (1, 2) \
            or identity(manifest) != bundle['revision']:
        raise ValueError('stored contribution failed its complete identity check')
    cap = _declared_capabilities(manifest['document'])
    if manifest['version'] == 2 and identity(manifest.get('reasoning')) != identity(cap):
        raise ValueError('contribution manifest and document capabilities disagree')
    _privacy(manifest['document'])
    _privacy(manifest['scope'])
    files = bundle['files']
    _portable_files(files)
    if set(files) != set(manifest['evidence']) or any(
            hashlib.sha256(files[path]).hexdigest() != digest for path, digest in manifest['evidence'].items()):
        raise ValueError('stored contribution evidence failed its complete identity check')
    if not supported:
        return cap
    cap = document_capabilities(manifest['document'])
    if manifest['version'] == 2:
        checked = _prepare(manifest['document'], manifest['roots'], scope=manifest['scope'],
                           shareability='project', evidence=files, version=2)
        if checked['revision'] != bundle['revision']:
            raise ValueError('contribution is not its complete validated closure')
    return cap


def _privacy(value):
    if isinstance(value, dict):
        if ('private' in value and value['private'] is not False) \
                or value.get('shareability', 'project') != 'project' \
                or any(value.get(key) in ('private', 'unclear', 'unknown', 'personal')
                       for key in ('privacy', 'visibility')):
            raise ValueError('private or unclear sharing permission belongs in a private draft')
        for item in value.values():
            _privacy(item)
    elif isinstance(value, list):
        for item in value:
            _privacy(item)


def _files(value):
    if isinstance(value, dict):
        if 'file' in value:
            yield M.relative_path(value['file'])
        for item in value.values():
            yield from _files(item)
    elif isinstance(value, list):
        for item in value:
            yield from _files(item)


def _portable_files(files):
    paths = set(files)
    for path, data in files.items():
        M.relative_path(path)
        if any(str(parent) in paths for parent in Path(path).parents):
            raise ValueError('evidence file path collides with another evidence directory')
        if not isinstance(data, bytes):
            raise ValueError('portable evidence must be supplied as bytes')


def prepare(doc, roots, *, scope, shareability, evidence=None):
    return _prepare(doc, roots, scope=scope, shareability=shareability, evidence=evidence, version=2)


def _prepare(doc, roots, *, scope, shareability, evidence=None, version):
    """Create a validated bundle in memory, before any publishable object is written.

    Evidence is an explicit {portable record-relative path: bytes} allowlist. The
    caller's shareability declaration covers the entire selected closure and bytes;
    no private filesystem path is implicitly read, copied or rewritten.
    """
    if shareability != 'project':
        raise ValueError('private or unclear sharing permission belongs in a private draft')
    if not isinstance(scope, dict) or scope.get('kind') not in ('project', 'external', 'code'):
        raise ValueError('contribution needs an explicit project, external or code scope')
    if not isinstance(scope.get('environment'), str) or not scope['environment'].strip():
        raise ValueError('contribution scope needs an exact environment')
    if scope['kind'] == 'code' and not re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', str(scope.get('commit', ''))):
        raise ValueError('code-scoped knowledge needs its exact commit')
    if not isinstance(roots, (list, tuple)) or not roots or any(not isinstance(x, str) for x in roots):
        raise ValueError('contribution needs explicit root entry IDs')
    _privacy({k: doc[k] for k in ('meta', 'privacy', 'visibility', 'private', 'shareability') if k in doc})
    document = (closure if version == 2 else _legacy_closure)(doc, roots)
    if scope['kind'] == 'code':
        for root in roots:
            body = entries(document)[root][1]
            if not isinstance(body, dict) or identity(body.get('scope')) != identity(scope):
                raise ValueError('code-scoped root entries must retain their exact scope in the portable body')
    _privacy(document)
    _privacy(scope)
    files = copy.deepcopy(evidence or {})
    required = set(_files(document))
    if set(files) != required:
        raise ValueError('evidence allowlist must contain exactly the files referenced by the selected closure')
    _portable_files(files)
    for root in roots:
        body = entries(document)[root][1]
        if not isinstance(body, dict) or identity(body.get('scope')) != identity(scope):
            raise ValueError('contribution roots must retain their exact scope in the portable body')
    manifest = {'version': version, 'roots': sorted(set(roots)), 'document': document,
                'scope': copy.deepcopy(scope),
                'evidence': {path: hashlib.sha256(data).hexdigest() for path, data in sorted(files.items())}}
    if version == 2:
        manifest['reasoning'] = document_capabilities(document)
    return {'revision': identity(manifest), 'manifest': manifest, 'files': files}

def semantic_roles(document):
    """Infer roles consistently for a full graph or an explicit-schema closure."""
    try:
        _, judgments, fields = P.infer(document)
        return judgments, fields
    except SystemExit:
        # A selected source/fact closure can keep its parent record's explicit
        # schema without including any downstream judgment using that schema.
        deps = (document.get('schema') or {}).get('deps')
        if deps and not any(isinstance(body, dict) and deps in body
                            for _, body in entries(document).values()):
            return {}, {}
        return None


def equivalent(bundle, doc, evidence):
    """Content-based acceptance: shared subset must match; extra local IDs are fine."""
    validate_bundle(bundle)
    expected = bundle['manifest']
    meaning = meaning_capabilities(expected['document'])
    if meaning != meaning_capabilities(doc):
        return False
    # A captured named scope binds membership, including empty collections.
    scoped = entries(expected['document']).values() if meaning['profile'] == 'core/v1' else ()
    for _, body in scoped:
        definition = body.get('collection_scope') if isinstance(body, dict) else None
        if isinstance(definition, dict):
            collection = definition.get('collection')
            if identity(expected['document'].get(collection)) != identity(doc.get(collection)):
                return False
    actual = entries(doc)
    for root in expected['roots']:
        body = actual.get(root, (None, None))[1]
        if not isinstance(body, dict) or identity(body.get('scope')) != identity(expected['scope']):
            return False
    if identity({k: expected['document'][k] for k in ('schema',) if k in expected['document']}) != \
            identity({k: doc[k] for k in ('schema',) if k in doc}):
        return False
    for name, body in entries(expected['document']).items():
        if name not in actual or identity(list(body)) != identity(list(actual[name])):
            return False
    source_roles, target_roles = semantic_roles(expected['document']), semantic_roles(doc)
    if source_roles is None or target_roles is None:
        return False
    for name in entries(expected['document']):
        source_judgment = name in source_roles[0]
        if source_judgment != (name in target_roles[0]):
            return False
        if source_judgment and identity(source_roles[1]) != identity(target_roles[1]):
            return False
    return all(path in evidence and hashlib.sha256(evidence[path]).hexdigest() == digest
               for path, digest in expected['evidence'].items())


class Store:
    def __init__(self, project):
        self.project = project if isinstance(project, M.Project) else M.Project(project)
        if not self.project.git:
            raise ValueError('durable contribution store needs Git')
        self.root = self.project.root

    def head(self):
        value = M.git(self.root, 'rev-parse', '--verify', REF, check=False)
        return value.stdout.decode().strip() if value.returncode == 0 else None

    def tree(self, ref=None):
        head = ref or self.head()
        if not head:
            return {}
        result = {}
        for line in M.git(self.root, 'ls-tree', '-r', '-z', head).stdout.split(b'\0'):
            if line:
                info, path = line.split(b'\t', 1)
                mode, kind, oid = info.split()
                if mode != b'100644' or kind != b'blob':
                    raise ValueError('pending ledger contains an unexpected object')
                result[path.decode('utf-8')] = oid.decode()
        return result

    def blob(self, oid):
        return M.git(self.root, 'cat-file', 'blob', oid).stdout

    def _write_blob(self, data):
        return M.git(self.root, 'hash-object', '-w', '--stdin', data=data).stdout.decode().strip()

    def _write_tree(self, files):
        nested = {}
        for path, oid in files.items():
            parts, node = path.split('/'), nested
            for part in parts[:-1]:
                node = node.setdefault(part, {})
                if not isinstance(node, dict):
                    raise ValueError('pending tree has a file/directory collision')
            if isinstance(node.get(parts[-1]), dict):
                raise ValueError('pending tree has a directory/file collision')
            node[parts[-1]] = oid
        def build(node):
            rows = []
            for name, value in sorted(node.items()):
                if isinstance(value, dict):
                    rows.append(('040000 tree ' + build(value) + '\t' + name).encode() + b'\0')
                else:
                    rows.append(('100644 blob ' + value + '\t' + name).encode() + b'\0')
            return M.git(self.root, 'mktree', '-z', data=b''.join(rows)).stdout.decode().strip()
        return build(nested)

    def _commit(self, files, old):
        tree = self._write_tree(files)
        env = os.environ.copy()
        for role in ('AUTHOR', 'COMMITTER'):
            env['GIT_' + role + '_NAME'] = 'Knowledge record'
            env['GIT_' + role + '_EMAIL'] = 'knowledge@localhost'
        args = ['commit-tree', '--no-gpg-sign', tree] + (['-p', old] if old else [])
        return M.git(self.root, *args, data=b'Retain project knowledge contribution\n', env=env).stdout.decode().strip()

    def _cas(self, old, new):
        zero = '0' * len(new)
        return M.git(self.root, 'update-ref', REF, new, old or zero, check=False).returncode == 0

    def capture(self, bundle, *, event_id, contribution_id, shareability, expected_generation=None, expected_policy=None,
                expected_sources=None):
        receipt = self._capture(bundle, event_id=event_id, contribution_id=contribution_id,
                                shareability=shareability, expected_generation=expected_generation,
                                expected_policy=expected_policy, expected_sources=expected_sources)
        # The durable acknowledgement is established and the policy lock released
        # before a publisher is even started. Publication failure cannot erase or
        # turn a successfully retained contribution into a failed capture.
        try:
            if __package__:
                from .pending_publication import trigger_after_capture
            else:
                from pending_publication import trigger_after_capture
            receipt['publication_attempt'] = trigger_after_capture(self.project)
        except Exception as error:
            receipt['publication_attempt'] = {'started': False, 'reason': str(error)}
        return receipt

    def _capture(self, bundle, *, event_id, contribution_id, shareability, expected_generation=None, expected_policy=None,
                 expected_sources=None):
        # Validate again at the object-write boundary, including supplied identity.
        if shareability != 'project':
            raise ValueError('private or unclear sharing permission belongs in a private draft')
        for value in (event_id, contribution_id):
            if not isinstance(value, str) or not TOKEN.fullmatch(value):
                raise ValueError('event and contribution IDs must be portable nonempty tokens')
        manifest = bundle['manifest']
        validate_bundle(bundle)
        verified = _prepare(manifest['document'], manifest['roots'], scope=manifest['scope'],
                            shareability=shareability, evidence=bundle['files'], version=manifest['version'])
        if identity(manifest) != verified['revision'] or bundle['revision'] != verified['revision']:
            raise ValueError('contribution integrity check failed before capture')
        revision = verified['revision']
        event = {'event_id': event_id, 'contribution_id': contribution_id, 'revision': revision}
        event_path = 'events/' + event_id + '.json'
        with self.project.lock():
            # Coordinated source writers also hold this policy lock. A prepared
            # contribution cannot outlive a permission/body change in its inputs.
            if expected_sources is not None and any(
                    (hashlib.sha256(Path(path).read_bytes()).hexdigest() if Path(path).is_file() else None) != digest
                    for path, digest in expected_sources.items()):
                raise ValueError('record source changed before capture; reread its current scope and permissions')
            if expected_policy is not None and self.project.config() != expected_policy:
                raise ValueError('project policy or destination changed before capture; retry')
            if expected_generation is not None and self.project.config()['generation'] != expected_generation:
                raise ValueError('project policy changed before capture; retry with the current mode and record')
            if self.project.config()['mode'] == 'simple':
                raise ValueError('Simple uses its shared record, not a Git contribution queue')
            # Freeze legacy/new defaults before acknowledging the first Git capture.
            if not self.project.config_path.exists():
                I._save(self.project.config_path, self.project.config())
            for attempt in range(8):
                old = self.head()
                files = self.tree(old)
                if event_path in files:
                    existing = json.loads(self.blob(files[event_path]))
                    if {k: existing.get(k) for k in event} != event:
                        raise ValueError('event ID was already used with different content or target')
                    return self.receipt(event_id, old)
                prefix = 'contributions/' + revision + '/'
                retained_event = dict(event, sequence=len([p for p in files if p.startswith('events/')]) + 1,
                                      captured_at_ns=time.time_ns())
                additions = {event_path: json_bytes(retained_event), prefix + 'manifest.json': json_bytes(_encode(manifest))}
                additions.update({prefix + 'evidence/' + path: data for path, data in verified['files'].items()})
                for path, data in additions.items():
                    if path in files and self.blob(files[path]) != data:
                        raise ValueError('pending ledger contains conflicting immutable content')
                    files[path] = self._write_blob(data)
                new = self._commit(files, old)
                if self._cas(old, new):
                    # This ref is the receipt; crashing now only causes an identical replay.
                    return dict(retained_event, state='captured', commit=new, ledger_commit=new, ref=REF, replay=False)
            raise ValueError('pending ref kept changing; retry capture with the same event ID')

    def receipt(self, event_id, ref=None):
        """Recover a durable acknowledgement without another capture or publication."""
        if not isinstance(event_id, str) or not TOKEN.fullmatch(event_id):
            raise ValueError('invalid contribution event ID')
        head = ref or self.head()
        if not head:
            return None
        path = 'events/' + event_id + '.json'
        files = self.tree(head)
        if path not in files:
            return None
        event = json.loads(self.blob(files[path]))
        self.read_bundle(event['revision'], head)
        first = M.git(self.root, 'log', '--diff-filter=A', '--format=%H', head, '--', path).stdout.decode().splitlines()
        return dict(event, state='captured', commit=first[-1], ledger_commit=head, ref=REF, replay=True)

    def events(self, ref=None):
        head = ref or self.head()
        files = self.tree(head)
        return sorted([json.loads(self.blob(oid)) for path, oid in files.items() if path.startswith('events/')],
                      key=lambda event: event['sequence'])

    def read_bundle(self, revision, ref=None):
        if not re.fullmatch(r'[0-9a-f]{64}', revision):
            raise ValueError('invalid contribution revision')
        files = self.tree(ref)
        prefix = 'contributions/' + revision + '/'
        path = prefix + 'manifest.json'
        if path not in files:
            raise ValueError('contribution is unavailable at the selected ledger revision')
        manifest = _decode(json.loads(self.blob(files[path])))
        evidence = {name: self.blob(files[prefix + 'evidence/' + name])
                    for name in manifest['evidence'] if prefix + 'evidence/' + name in files}
        bundle = {'revision': revision, 'manifest': manifest, 'files': evidence}
        validate_bundle(bundle, supported=False)
        return bundle

    def snapshot(self, ref=None):
        head = ref or self.head()
        if head is None:
            return {'ref': None, 'events': [], 'bundles': {}}
        events = self.events(head)
        return {'ref': head, 'events': events,
                'bundles': {event['revision']: self.read_bundle(event['revision'], head) for event in events}}
