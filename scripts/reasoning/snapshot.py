"""Detached portable observations and strict, read-only source capture.

Capture brackets the entire source closure (including directory membership and failed
reads), routing configuration, pinned pending history and local target observations.
Replay never resolves a source path or grants publication authority.
"""
import copy
import glob
import hashlib
import os
from pathlib import Path

from .contract import digest, normalize_as_of, MAX_NODES, MAX_COLLECTION, MAX_EDGES


class SnapshotError(ValueError):
    def __init__(self, code, detail=''):
        self.code = code
        super().__init__(code + (': ' + detail if detail else ''))


def _fields(document):
    """Infer roles only; do not run builtins, assessment, or historical evaluation."""
    from .. import provenance as P
    schema = document.get('schema') or {}
    if not isinstance(schema, dict):
        raise SnapshotError('invalid_snapshot', 'schema must be a mapping')
    # A core source/scope fixture need not carry judgments. Explicit roles remain
    # meaningful even when the collection contains no instance of that role yet.
    roles = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
    if schema:
        for role in roles:
            if role in schema:
                if not isinstance(schema[role], str) or not schema[role]:
                    raise SnapshotError('invalid_snapshot', 'field roles must name literal fields')
                roles[role] = schema[role]
    try:
        _, _, inferred = P.infer(document)
        return {role: inferred.get(role) or value for role, value in roles.items()}
    except SystemExit as error:
        # No dependency declaration is a legitimate literal-only core snapshot.
        missing_declared_role = schema.get('deps') and not any(
            isinstance(body, dict) and schema['deps'] in body
            for collection, members in P.collections_of(document).items() if collection != 'meta'
            for body in members.values())
        if str(error).startswith('no dependency field found:') or (
                missing_declared_role and str(error).startswith('schema names ')):
            return roles
        raise SnapshotError('invalid_snapshot', str(error)) from None


def _nodes(document):
    from ..pending_grounding import entries
    fields = _fields(document)
    result = {name: {'body': copy.deepcopy(body), 'collection': collection,
                     'fields': dict(fields)}
              for name, (collection, body) in entries(document).items()}
    if len(result) > MAX_NODES:
        raise SnapshotError('limit', 'captured node limit exceeded')
    if any(not isinstance(name, str) for name in result):
        raise SnapshotError('invalid_snapshot', 'node ids must be text')
    return result


def _hypotheses(value):
    result = {}
    for name, item in (value or {}).items():
        if not isinstance(item, dict):
            raise SnapshotError('invalid_snapshot', 'hypothesis must be a mapping')
        # Derived legacy caches (ids/raw) and host paths are not authored evidence.
        result[name] = {'document': copy.deepcopy(item.get('document', item.get('doc', {}))),
                        'head': copy.deepcopy(item.get('head', {})), 'error': item.get('error')}
        if item.get('kind'):
            result[name]['kind'] = item['kind']
    return result


def _hypothesis_conflicts(hypotheses):
    """Two disagreeing alternatives contest; a single alternative stays a proposal.

    Compare authored claims with the typed identity encoder. Legacy claim selection
    is reused without evaluating a rule or coercing bool/number/text equality.
    """
    from .. import provenance as P
    from ..pending_grounding import entries
    holders = {}
    for name, hyp in sorted(hypotheses.items()):
        if hyp['error']:
            continue
        for nid, (_, body) in entries(hyp['document']).items():
            holders.setdefault(nid, []).append([name, copy.deepcopy(body)])
    return {nid: variants for nid, variants in holders.items()
            if len({digest(P.claim_of(body)) for _, body in variants}) > 1}


class Snapshot:
    __slots__ = ('__data',)

    def __init__(self, data):
        # Construction through this public entry point validates replay too.
        if not isinstance(data, dict) or set(data) != {
                'schema_version', 'document', 'nodes', 'hypotheses', 'context',
                'as_of', 'authored_revision', 'snapshot_id'}:
            raise SnapshotError('invalid_snapshot', 'unexpected snapshot schema')
        if type(data['schema_version']) is not int or data['schema_version'] != 1:
            raise SnapshotError('invalid_snapshot', 'unsupported snapshot schema')
        if not isinstance(data['document'], dict) or not isinstance(data['context'], dict) \
                or not isinstance(data['hypotheses'], dict):
            raise SnapshotError('invalid_snapshot', 'invalid snapshot structure')
        body = {key: value for key, value in data.items() if key != 'snapshot_id'}
        if digest(body) != data['snapshot_id']:
            raise SnapshotError('stale_snapshot', 'snapshot digest does not match')
        if normalize_as_of(data['as_of']) != data['as_of'] or digest(_nodes(data['document'])) != digest(data['nodes']):
            raise SnapshotError('invalid_snapshot', 'snapshot normalization does not match')
        mode = data['context'].get('read_mode')
        if mode not in ('supplied', 'live', 'frozen', 'captured-live'):
            raise SnapshotError('invalid_snapshot', 'unknown captured read mode')
        self.__data = copy.deepcopy(data)

    @classmethod
    def from_data(cls, document, *, context=None, hypotheses=None, as_of=None, authored_revision=None):
        if not isinstance(document, dict):
            raise SnapshotError('invalid_snapshot', 'document must be a mapping')
        # Validate before deepcopy, which otherwise preserves cycles.
        digest(document)
        context = copy.deepcopy(context) if context is not None else {
            'read_mode': 'supplied', 'source_collection': 'caller-owned'}
        context.setdefault('read_mode', 'supplied')
        normalized_hypotheses = _hypotheses(hypotheses)
        derived_conflicts = _hypothesis_conflicts(normalized_hypotheses)
        if derived_conflicts:
            context['conflicts'] = {**derived_conflicts, **context.get('conflicts', {})}
        data = {'schema_version': 1, 'document': copy.deepcopy(dict(document)),
                'nodes': _nodes(document), 'hypotheses': normalized_hypotheses,
                'context': context, 'as_of': normalize_as_of(as_of),
                'authored_revision': copy.deepcopy(authored_revision)}
        data['snapshot_id'] = digest(data)
        return cls(data)

    @classmethod
    def from_snapshot(cls, data):
        """Verify and replay an exported observation without ambient reads."""
        return cls(data)

    @classmethod
    def capture(cls, paths, *, read_mode=None, as_of=None):
        return capture(paths, read_mode=read_mode, as_of=as_of)

    @property
    def snapshot_id(self):
        return self.__data['snapshot_id']

    def to_data(self):
        return copy.deepcopy(self.__data)

    def capture_scope(self, scope_id, *, limits=None):
        return ScopeCapture(self, scope_id, limits=limits)


class ScopeCapture:
    __slots__ = ('__snapshot', '__data')

    def __init__(self, snapshot, scope_id, *, limits=None):
        data = snapshot.to_data()
        nodes = data['nodes']
        node = nodes.get(scope_id)
        definition = node['body'].get('collection_scope') if node and isinstance(node['body'], dict) else None
        if not isinstance(definition, dict) or set(definition) != {'collection', 'fields'} \
                or not isinstance(definition['collection'], str) or not isinstance(definition['fields'], list) \
                or any(not isinstance(field, str) or not field for field in definition['fields']):
            raise SnapshotError('invalid_scope', 'scope needs a collection and literal fields')
        if data['context'].get('conflicts', {}).get(scope_id):
            raise SnapshotError('scope_unavailable', 'scope definition is contested')
        collection = definition['collection']
        fields = sorted(set(definition['fields']))
        normalized = {'collection': collection, 'fields': fields}
        source = data['document'].get(collection)
        if not isinstance(source, dict) or collection in ('meta', 'schema', 'record', 'also'):
            raise SnapshotError('scope_unavailable', 'collection membership unavailable')
        members = sorted(source)
        bound = MAX_COLLECTION
        if limits is not None:
            if not isinstance(limits, dict) or set(limits) - {'members'}:
                raise SnapshotError('invalid_limits', 'unknown scope limit')
            bound = limits.get('members', bound)
            if type(bound) is not int or not 0 < bound <= MAX_COLLECTION:
                raise SnapshotError('invalid_limits', 'invalid collection bound')
        if len(members) * len(fields) > MAX_EDGES:
            raise SnapshotError('limit', 'scope candidate field limit exceeded')
        if len(members) > bound:
            raise SnapshotError('limit', 'collection member limit exceeded')
        candidates = {}
        dependencies = []
        for member in members:
            if member not in nodes or nodes[member]['collection'] != collection:
                raise SnapshotError('scope_unavailable', 'collection member not captured')
            body = nodes[member]['body']
            mapped_seen = nodes[member]['fields']['snapshot']
            candidates[member] = {}
            for field in fields:
                if field == mapped_seen or field in ('assessment', 'current_assessment'):
                    raise SnapshotError('invalid_scope', 'historical or assessment fields cannot be computational inputs')
                conflicts = data['context'].get('conflicts', {}).get(member)
                present = isinstance(body, dict) and field in body
                observation = {'status': 'contested' if conflicts else ('known' if present else 'missing')}
                if present:
                    observation['value'] = copy.deepcopy(body[field])
                if conflicts:
                    observation['alternatives'] = copy.deepcopy(conflicts)
                if field in ('v', 'rule') and isinstance(body, dict) and 'rule' in body:
                    from .contract import node_basis
                    observation['computed_basis'] = node_basis(data, member)
                fingerprint_input = {key: value for key, value in observation.items() if key != 'alternatives'}
                if conflicts:
                    # Retain full alternatives as evidence, but only projected
                    # computational fields contribute to the field fingerprint.
                    fingerprint_input['alternatives'] = [
                        {'name': name, 'present': isinstance(variant, dict) and field in variant,
                         **({'value': variant[field]} if isinstance(variant, dict) and field in variant else {})}
                        for name, variant in conflicts]
                fingerprint = digest(fingerprint_input)
                candidates[member][field] = dict(observation, fingerprint=fingerprint)
                dependencies.append({'id': member, 'field': field, 'fingerprint': fingerprint})
        witness = {'kind': 'scope', 'scope_id': scope_id,
                   'definition_digest': digest(normalized), 'membership_digest': digest(members),
                   'projected_inputs_digest': digest(dependencies)}
        basis = {'version': 1, 'profile': 'core/v1', 'modules': ['arithmetic/v1'],
                 'dependencies': dependencies, 'historical_detail': 'fingerprints_only', 'as_of': data['as_of']}
        basis['digest'] = digest(basis)
        value = {'type': 'record', 'fields': {'member_count': {
            'type': 'number', 'numerator': str(len(members)), 'denominator': '1'}}}
        self.__snapshot = snapshot
        self.__data = {'scope_id': scope_id, 'definition': normalized, 'value': value,
                       'witness': witness, 'basis': basis, 'candidates': candidates}

    def to_data(self):
        return copy.deepcopy(self.__data)

    @property
    def value(self):
        return copy.deepcopy(self.__data['value'])

    @property
    def witness(self):
        return copy.deepcopy(self.__data['witness'])

    @property
    def basis(self):
        return copy.deepcopy(self.__data['basis'])

    @property
    def candidates(self):
        return copy.deepcopy(self.__data['candidates'])

    def view(self):
        return SnapshotView(self.__snapshot, scopes=[self.__data['scope_id']])


class SnapshotView:
    """Exact grants; returned values are detached and never mutable snapshot state."""
    __slots__ = ('__snapshot', '__nodes', '__scopes', '__reads', '__limits')

    def __init__(self, snapshot, *, nodes=(), scopes=(), limits=None):
        self.__snapshot = snapshot
        self.__nodes = frozenset(nodes)
        self.__scopes = frozenset(scopes)
        self.__reads = {}
        self.__limits = copy.deepcopy(limits)

    def read_node(self, node_id):
        if node_id not in self.__nodes:
            raise SnapshotError('undeclared_dependency', node_id)
        data = self.__snapshot.to_data()
        from .contract import node_basis
        basis = node_basis(data, node_id)
        witness = {'kind': 'node', 'id': node_id, 'fingerprint': basis['fingerprint']}
        self.__reads[digest(witness)] = witness
        return copy.deepcopy(data['nodes'].get(node_id))

    def read_scope(self, scope_id, field):
        if scope_id not in self.__scopes:
            raise SnapshotError('undeclared_dependency', scope_id)
        capture = self.__snapshot.capture_scope(scope_id, limits=self.__limits)
        data = capture.to_data()
        if field not in data['definition']['fields']:
            raise SnapshotError('undeclared_dependency', scope_id + ':' + field)
        self.__reads[digest(data['witness'])] = data['witness']
        return [dict(id=member, **values[field]) for member, values in data['candidates'].items()]

    @property
    def executed_reads(self):
        return [copy.deepcopy(self.__reads[key]) for key in sorted(self.__reads)]


def _sha(data):
    return hashlib.sha256(data).hexdigest()


class _Inventory:
    def __init__(self):
        self.events = {}

    def __call__(self, kind, path, value):
        value = _sha(value) if kind == 'bytes' else value
        key = (kind, path)
        if key in self.events and self.events[key] != value:
            raise SnapshotError('snapshot_changed', 'input changed during load')
        self.events[key] = value

    def verify(self):
        for (kind, path), value in self.events.items():
            if kind == 'bytes':
                try:
                    current = _sha(Path(path).read_bytes())
                except OSError:
                    raise SnapshotError('snapshot_changed', 'source became unreadable') from None
            elif kind == 'unreadable':
                try:
                    Path(path).read_bytes()
                    current = None
                except OSError as error:
                    current = type(error).__name__
            elif kind == 'glob':
                current = sorted(map(os.path.abspath, glob.glob(path)))
            elif kind == 'directory':
                current = os.path.isdir(path)
            else:
                current = os.path.exists(path)
            if current != value:
                raise SnapshotError('snapshot_changed', 'source inventory changed')


def _observation(paths, mode):
    from .. import knowledge_views as V, project_modes as M
    project = V.project_for(paths)
    config = project.config()
    result = {'config': config, 'config_exists': project.config_path.exists(),
              'root': str(project.root), 'record': str(project.record())}
    if mode == 'live' and project.git and config['mode'] == 'advanced':
        from ..pending_grounding import Store
        from ..pending_publication import Publisher, scope_identity
        result['pending_ref'] = Store(project).head()
        result['publication'] = Publisher(project).status()
        publication = config.get('publication')
        if publication:
            observed = result['publication'].get('last_verified') or {}
            ref = 'refs/remotes/' + publication['remote'] + '/' + publication['target']
            if observed.get('scope') == scope_identity(publication) and observed.get('target'):
                ref = observed['target']
            got = M.git(project.root, 'rev-parse', '--verify', ref + '^{commit}', check=False)
            result['target'] = {'ref': ref, 'revision': got.stdout.decode().strip() if got.returncode == 0 else None}
    return result


def _portable(value, origin, *, authored=False):
    """Observation metadata only: preserve bytes; remove standing authority and host paths."""
    if isinstance(value, bytes):
        return {'encoding': 'hex', 'data': value.hex(), 'sha256': _sha(value)}
    if isinstance(value, dict):
        return {key: _portable(item, origin, authored=authored or key in ('doc', 'document', 'manifest'))
                for key, item in value.items() if authored or key != 'standing_permission'}
    if isinstance(value, (list, tuple)):
        return [_portable(item, origin, authored=authored) for item in value]
    if not authored and isinstance(value, str) and os.path.isabs(value):
        return origin(value)
    return copy.deepcopy(value)


def capture(paths, *, read_mode=None, as_of=None):
    from .. import provenance as P
    paths = [str(path) for path in ([paths] if isinstance(paths, (str, Path)) else paths)]
    if not paths:
        raise SnapshotError('invalid_snapshot', 'capture needs source paths')
    mode = read_mode or ('frozen' if P._RAW_READS.get() else os.environ.get('KPOPPER_READ_MODE', 'live'))
    if mode not in ('live', 'frozen'):
        raise SnapshotError('invalid_snapshot', 'capture read mode must be live or frozen')
    normalize_as_of(as_of)
    initial = _observation(paths, mode)
    inventories = []
    documents = []
    # Discovery reads the complete actual closure; the second load is bracketed by
    # its verified inventory. Equal merged YAML alone is never sufficient.
    for _ in range(2):
        inventory = _Inventory()
        token = P._CAPTURE_READS.set(inventory)
        try:
            documents.append(P.load(paths, read_mode=mode))
        finally:
            P._CAPTURE_READS.reset(token)
        inventory.verify()
        loaded = documents[-1]
        if hasattr(loaded, 'capture_config') and digest(loaded.capture_config) != digest(initial['config']):
            raise SnapshotError('snapshot_changed', 'loaded configuration does not match capture')
        if hasattr(loaded, 'pending_ref') and loaded.pending_ref != initial.get('pending_ref'):
            raise SnapshotError('snapshot_changed', 'loaded pending ref does not match capture')
        if hasattr(loaded, 'knowledge_target') and loaded.knowledge_target != initial.get('target'):
            raise SnapshotError('snapshot_changed', 'loaded target does not match capture')
        if digest(initial) != digest(_observation(paths, mode)):
            raise SnapshotError('snapshot_changed', 'routing, pending, or target observation changed')
        inventories.append(inventory)
    if inventories[0].events != inventories[1].events:
        raise SnapshotError('snapshot_changed', 'source closure changed')
    doc = documents[-1]
    base = Path(paths[0]).absolute().parent
    def origin(path):
        path = Path(path).absolute()
        try:
            return 'origin:' + path.relative_to(base).as_posix()
        except ValueError:
            return 'external:' + _sha(str(path).encode())[:24] + '/' + path.name
    config = initial['config']
    target = copy.deepcopy(getattr(doc, 'knowledge_target', initial.get('target', {})))
    reason = getattr(doc, 'target_unavailable', None)
    observed_target = hasattr(doc, 'knowledge_target') and target.get('revision')
    target.update(status='unavailable' if reason else ('observed' if observed_target else 'unassessed'),
                  reason=reason, ref=target.get('ref'), revision=target.get('revision'))
    context = {'read_mode': 'captured-live' if mode == 'live' else 'frozen',
               'original_read_mode': mode,
               'project': {'mode': config['mode'], 'generation': config['generation'],
                           'routing_identity': _portable(initial['record'], origin),
                           'configuration': _portable(config, origin)},
               'pending': _portable(getattr(doc, 'pending_snapshot', {'ref': None, 'bundles': {}, 'events': []}), origin),
               'conflicts': _portable(getattr(doc, 'knowledge_conflicts', {}), origin, authored=True),
               'target': _portable(target, origin)}
    context['pending']['observations'] = _portable(getattr(doc, 'publication', initial.get('publication', {})), origin)
    context['pending']['contributions'] = _portable(getattr(doc, 'contributions', []), origin)
    files = [{'origin': origin(path), 'sha256': value if kind == 'bytes' else None,
              'status': 'read' if kind == 'bytes' else 'unreadable',
              **({'error': value} if kind == 'unreadable' else {})}
             for (kind, path), value in sorted(inventories[-1].events.items()) if kind in ('bytes', 'unreadable')]
    revision = {'files': sorted(files, key=lambda item: item['origin'])}
    revision['digest'] = digest(revision)
    return Snapshot.from_data(doc, context=context, hypotheses=_portable(doc.hypotheses, origin),
                              as_of=as_of, authored_revision=revision)
