"""Detached portable observations and strict, read-only source capture.

Capture brackets the entire source closure (including directory membership and failed
reads), routing configuration, pinned pending history and local target observations.
Replay never resolves a source path or grants publication authority.
Use Snapshot.to_json()/from_json() for portable typed JSON; to_data() remains
a Python mapping with authored date/datetime objects intact.
"""
import copy
import datetime
import glob
import hashlib
import json
import math
import os
from pathlib import Path

from .contract import digest, normalize_as_of, MAX_NODES, MAX_COLLECTION, MAX_EDGES, MAX_REQUEST_BYTES, MAX_LIMITS


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


def _nodes(document, *, copy_bodies=True):
    from ..pending_grounding import entries
    fields = _fields(document)
    result = {name: {'body': copy.deepcopy(body) if copy_bodies else body, 'collection': collection,
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


def _validate_authored_revision(revision):
    """Validate byte-concurrency evidence separately from normalized input identity.

    This checks internal consistency, not source authenticity. A writer must still
    revalidate the real files before using an inventory as a concurrency guard.
    """
    if revision is None:
        return
    if not isinstance(revision, dict) or set(revision) != {'files', 'digest'} \
            or not isinstance(revision['files'], list):
        raise SnapshotError('invalid_authored_revision', 'revision needs files and digest')
    origins = []
    for item in revision['files']:
        if not isinstance(item, dict) or not isinstance(item.get('origin'), str) or not item['origin']:
            raise SnapshotError('invalid_authored_revision', 'revision file needs an origin label')
        origins.append(item['origin'])
        if item.get('status') == 'read':
            value = item.get('sha256')
            if set(item) != {'origin', 'status', 'sha256'} or not isinstance(value, str) \
                    or len(value) != 64 or any(char not in '0123456789abcdef' for char in value):
                raise SnapshotError('invalid_authored_revision', 'read file needs a SHA-256 digest')
        elif item.get('status') == 'unreadable':
            if set(item) != {'origin', 'status', 'sha256', 'error'} or item['sha256'] is not None \
                    or not isinstance(item['error'], str) or not item['error']:
                raise SnapshotError('invalid_authored_revision', 'unreadable file needs an error')
        else:
            raise SnapshotError('invalid_authored_revision', 'unknown revision file status')
    if origins != sorted(set(origins)):
        raise SnapshotError('invalid_authored_revision', 'revision files must have sorted unique origins')
    if digest({'files': revision['files']}) != revision['digest']:
        raise SnapshotError('stale_authored_revision', 'revision digest does not match')


def _snapshot_preimage(data):
    # Exact source bytes guard writes, while normalized typed inputs identify reads.
    return {key: value for key, value in data.items() if key not in ('snapshot_id', 'authored_revision')}


def _validate_history(document, context, hypotheses=None):
    # Shared validation applies both to capture and untrusted frozen replay.
    # Legacy snapshots with no history declaration keep their existing shape.
    if 'history' in context:
        from ..history_contract import CapturedHistory, HistoryError
        try:
            CapturedHistory(document, context['history'])
            from .. import history_hypotheses as HH
            derived, index = HH.layers(context['history'], document)
            if derived or 'history_hypotheses' in context:
                if digest(context.get('history_hypotheses')) != digest(index):
                    raise HistoryError('history_hypothesis_index_mismatch')
                normalized = _hypotheses(derived)
                for name, expected in normalized.items():
                    if digest((hypotheses or {}).get(name)) != digest(expected):
                        raise HistoryError('history_hypothesis_layer_mismatch', name)
            for name, hypothesis in (hypotheses or {}).items():
                if hypothesis.get('kind') == HH.KIND and name not in derived:
                    raise HistoryError('unrecorded_history_hypothesis', name)
        except HistoryError as error:
            raise SnapshotError('invalid_history', str(error)) from error
    elif isinstance(document.get('meta'), dict) and 'history' in document['meta']:
        raise SnapshotError('missing_history_context', 'history view requires captured evidence')
    from .. import pending_grounding as G, history_bundle as B, knowledge_views as V
    hypotheses = hypotheses or {}
    bundles = context.get('pending', {}).get('bundles', {})
    revisions = {revision for revision, bundle in bundles.items()
                 if isinstance(bundle, dict) and bundle.get('manifest', {}).get('version') == 3}
    witnesses = context.get('history_contributions', {})
    if set(witnesses) != revisions:
        raise SnapshotError('missing_history_context', 'pending history witnesses disagree with captured bundles')
    if revisions or 'history' in context.get('target', {}).get('snapshot', {}):
        B.C.detached(context, MAX_REQUEST_BYTES)
    for revision in sorted(revisions):
        portable = bundles[revision]
        bundle = {'revision': portable['revision'], 'manifest': portable['manifest'],
                  'files': _history_bytes(portable['files'])}
        G.validate_bundle(bundle)
        if revision != bundle['revision']:
            raise SnapshotError('invalid_history', 'pending revision key disagrees')
        adapted = B.adapt(B.from_contribution(bundle))
        decision = context['pending'].get('observations', {}).get('decisions', {}).get(revision, {})
        retired = decision.get('state') in ('withdrawn', 'rejected', 'superseded')
        expected = {'artifact_revision': bundle['manifest']['history']['revision'],
                    'projection': adapted.projection, 'scope': bundle['manifest']['scope'],
                    'roots': bundle['manifest']['roots'], 'status': 'retired' if retired else 'active'}
        if digest(witnesses[revision]) != digest(expected):
            raise SnapshotError('invalid_history', 'pending projection disagrees with complete captured evidence')
        hypothesis = hypotheses.get('pending-' + revision)
        if retired:
            if hypothesis is not None:
                raise SnapshotError('invalid_history', 'retired history is an active hypothesis')
        elif hypothesis is None or hypothesis.get('kind') != 'contribution' or digest(
                hypothesis.get('document', hypothesis.get('doc'))) != digest(adapted.document):
            raise SnapshotError('invalid_history', 'pending history hypothesis disagrees with capture')
    for name, hypothesis in hypotheses.items():
        body = hypothesis.get('document', hypothesis.get('doc', {}))
        if isinstance(body.get('meta'), dict) and 'history' in body['meta'] and name not in {
                'pending-' + revision for revision in revisions}:
            raise SnapshotError('missing_history_context', 'history hypothesis requires its independent capture')
    target = context.get('target', {}).get('snapshot', {})
    if 'history' in target:
        evidence = copy.deepcopy(target['history'])
        evidence['files'] = _history_bytes(evidence['files'])
        captured = V.validate_history_evidence(evidence)
        if digest(target['doc']) != digest(B.A.from_store_capture(captured).document):
            raise SnapshotError('invalid_history', 'target document disagrees with committed history')
    elif isinstance(target.get('doc', {}).get('meta'), dict) and 'history' in target['doc']['meta']:
        raise SnapshotError('missing_history_context', 'target history lacks captured closure')


def _history_bytes(files):
    """Decode only explicit, checksummed portable bytes; never resolve a path."""
    from .. import history_bundle as B
    if not isinstance(files, dict) or len(files) > 2 * B.C.MAX_OBJECTS + 2:
        raise SnapshotError('limit', 'history file membership exceeds transport bounds')
    result, total = {}, 0
    for path, value in files.items():
        if not isinstance(value, dict) or set(value) != {'encoding', 'data', 'sha256'} or value['encoding'] != 'hex' \
                or not isinstance(value['data'], str):
            raise SnapshotError('invalid_history', 'history files require checksummed bytes')
        total += len(value['data'])
        if total > MAX_REQUEST_BYTES or len(value['data']) % 2:
            raise SnapshotError('limit', 'history bytes exceed snapshot transport bounds')
        try:
            raw = bytes.fromhex(value['data'])
        except ValueError:
            raise SnapshotError('invalid_history', 'malformed captured history bytes') from None
        if raw.hex() != value['data'] or _sha(raw) != value['sha256']:
            raise SnapshotError('invalid_history', 'captured history checksum mismatch')
        result[path] = raw
    return result


def _check_typed_json(value, depth=0):
    """Check exact encoder shapes before the legacy typed decoder can consume them."""
    if depth > 128:
        raise SnapshotError('limit', 'snapshot JSON nesting exceeds 128 levels')
    if not isinstance(value, list) or not value or not isinstance(value[0], str):
        raise SnapshotError('invalid_snapshot_json', 'expected a typed value')
    tag = value[0]
    if tag == 'null':
        if len(value) != 1:
            raise SnapshotError('invalid_snapshot_json', 'null has no payload')
        return
    if len(value) != 2:
        raise SnapshotError('invalid_snapshot_json', 'typed value needs one payload')
    payload = value[1]
    if tag == 'bool':
        if type(payload) is not bool:
            raise SnapshotError('invalid_snapshot_json', 'boolean payload must be boolean')
    elif tag in ('text', 'int', 'float', 'date', 'datetime'):
        if not isinstance(payload, str):
            raise SnapshotError('invalid_snapshot_json', 'scalar payload must be text')
        if tag == 'int' and len(payload) - int(payload.startswith('-')) > MAX_LIMITS['digits']:
            raise SnapshotError('limit', 'snapshot JSON integer exceeds digit limit')
    elif tag == 'list':
        if not isinstance(payload, list):
            raise SnapshotError('invalid_snapshot_json', 'list payload must be a list')
        for child in payload:
            _check_typed_json(child, depth + 1)
    elif tag == 'map':
        if not isinstance(payload, list):
            raise SnapshotError('invalid_snapshot_json', 'map payload must be entry pairs')
        names = set()
        for pair in payload:
            if not isinstance(pair, list) or len(pair) != 2 or not isinstance(pair[0], str) \
                    or pair[0] in names:
                raise SnapshotError('invalid_snapshot_json', 'malformed or duplicate map key')
            names.add(pair[0])
            _check_typed_json(pair[1], depth + 1)
    else:
        raise SnapshotError('invalid_snapshot_json', 'unknown typed value tag')


def _json_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise SnapshotError('invalid_snapshot_json', 'duplicate JSON object key')
        result[key] = value
    return result


def _json_constant(value):
    raise SnapshotError('invalid_snapshot_json', 'nonfinite JSON constant')


class Snapshot:
    __slots__ = ('__data', '__basis')

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
        _validate_history(data['document'], data['context'], data['hypotheses'])
        _validate_authored_revision(data['authored_revision'])
        if digest(_snapshot_preimage(data)) != data['snapshot_id']:
            raise SnapshotError('stale_snapshot', 'snapshot digest does not match')
        if normalize_as_of(data['as_of']) != data['as_of'] or digest(_nodes(data['document'])) != digest(data['nodes']):
            raise SnapshotError('invalid_snapshot', 'snapshot normalization does not match')
        mode = data['context'].get('read_mode')
        if mode not in ('supplied', 'live', 'frozen', 'captured-live'):
            raise SnapshotError('invalid_snapshot', 'unknown captured read mode')
        self.__data = copy.deepcopy(data)
        self.__basis = None

    @classmethod
    def from_data(cls, document, *, context=None, hypotheses=None, as_of=None, authored_revision=None):
        if not isinstance(document, dict):
            raise SnapshotError('invalid_snapshot', 'document must be a mapping')
        # Validate before deepcopy, which otherwise preserves cycles.
        from .contract import _check_finite
        _check_finite(document)
        if context is not None and not isinstance(context, dict) \
                or hypotheses is not None and not isinstance(hypotheses, dict):
            raise SnapshotError('invalid_snapshot', 'context and hypotheses must be mappings')
        context = copy.deepcopy(context) if context is not None else {
            'read_mode': 'supplied', 'source_collection': 'caller-owned'}
        context.setdefault('read_mode', 'supplied')
        normalized_hypotheses = _hypotheses(hypotheses)
        if 'history' in context:
            from .. import history_hypotheses as HH
            derived, index = HH.layers(context['history'], document)
            for name, hypothesis in _hypotheses(derived).items():
                if name in normalized_hypotheses and normalized_hypotheses[name].get('kind') != HH.KIND:
                    raise SnapshotError('hypothesis_authority_collision', name)
                if name not in normalized_hypotheses:
                    normalized_hypotheses[name] = hypothesis
            if derived:
                context.setdefault('history_hypotheses', index)
        _validate_history(document, context, normalized_hypotheses)
        if context['read_mode'] not in ('supplied', 'live', 'frozen', 'captured-live'):
            raise SnapshotError('invalid_snapshot', 'unknown captured read mode')
        _validate_authored_revision(authored_revision)
        derived_conflicts = _hypothesis_conflicts(normalized_hypotheses)
        if derived_conflicts:
            context['conflicts'] = {**derived_conflicts, **context.get('conflicts', {})}
        owned_document = copy.deepcopy(dict(document))
        data = {'schema_version': 1, 'document': owned_document,
                'nodes': _nodes(owned_document, copy_bodies=False), 'hypotheses': normalized_hypotheses,
                'context': context, 'as_of': normalize_as_of(as_of),
                'authored_revision': copy.deepcopy(authored_revision)}
        data['snapshot_id'] = digest(_snapshot_preimage(data))
        # These fields were just normalized, validated and detached here. Public
        # replay still uses __init__ to verify untrusted serialized snapshots;
        # rebuilding the same node map and hashes here would duplicate capture.
        result = object.__new__(cls)
        result.__data = data
        result.__basis = None
        return result

    @classmethod
    def from_snapshot(cls, data):
        """Verify and replay an exported observation without ambient reads."""
        return cls(data)

    @classmethod
    def from_json(cls, serialized):
        """Replay canonical pending-grounding typed JSON (UTF-8 text or bytes).

        The 16MiB transport, 128-level data and 4096-digit integer bounds apply
        before decoding. No source paths, configuration or Git refs are consulted.
        """
        from ..pending_grounding import _decode, _encode
        if not isinstance(serialized, (str, bytes)):
            raise SnapshotError('invalid_snapshot_json', 'expected UTF-8 JSON text or bytes')
        try:
            encoded_bytes = serialized.encode('utf-8') if isinstance(serialized, str) else serialized
            if len(encoded_bytes) > MAX_REQUEST_BYTES:
                raise SnapshotError('limit', 'snapshot JSON exceeds transport byte limit')
            encoded = json.loads(encoded_bytes.decode('utf-8'), object_pairs_hook=_json_object,
                                 parse_constant=_json_constant)
            _check_typed_json(encoded)
            data = _decode(encoded)
            # Reject noncanonical numeric/date encodings and altered decoder shapes.
            if _encode(data) != encoded:
                raise SnapshotError('invalid_snapshot_json', 'noncanonical typed encoding')
            return cls(data)
        except SnapshotError:
            raise
        except RecursionError:
            raise SnapshotError('limit', 'snapshot JSON nesting limit exceeded') from None
        except (ValueError, TypeError, OverflowError):
            raise SnapshotError('invalid_snapshot_json', 'malformed typed snapshot JSON') from None

    def to_json(self):
        """Return portable typed JSON without coercing authored dates into text."""
        from ..pending_grounding import _encode, json_bytes
        typed = _encode(self.__data)
        _check_typed_json(typed)
        encoded = json_bytes(typed)
        if len(encoded) > MAX_REQUEST_BYTES:
            raise SnapshotError('limit', 'snapshot JSON exceeds transport byte limit')
        return encoded.decode('utf-8')

    @classmethod
    def capture(cls, paths, *, read_mode=None, as_of=None):
        return capture(paths, read_mode=read_mode, as_of=as_of)

    @property
    def snapshot_id(self):
        return self.__data['snapshot_id']

    def to_data(self):
        return copy.deepcopy(self.__data)

    def _input_basis(self):
        """Internal derived cache; source data and snapshot identity stay immutable."""
        if self.__basis is None:
            from .basis import InputBasis
            self.__basis = InputBasis(self.__data)
        return self.__basis

    def capture_scope(self, scope_id, *, limits=None):
        return ScopeCapture(self, scope_id, limits=limits)

    def capture_query_scope(self, scope_id, *, limits=None):
        """Capture one complete query row set or refuse incomplete history.

        Ordinary scope summaries deliberately remain available for partial
        review projections.  A collection query is different: its result would
        silently change if an accepted member were absent, so active history
        must prove complete, all-subject coverage before the capture escapes.
        """
        history = self.__data['context'].get('history')
        if history is not None:
            coverage, integrity = history['coverage'], history['integrity']
            if coverage['scope'] != 'all' or not coverage['complete'] \
                    or not integrity['complete'] or integrity['findings']:
                raise SnapshotError('incomplete_history_scope', scope_id)
        capture = ScopeCapture(self, scope_id, limits=limits, query=True)
        if history is not None and not set(capture._member_ids()) <= set(
                history['coverage']['subjects']):
            raise SnapshotError('incomplete_history_scope', scope_id)
        capture._validate_query_fields()
        return capture


def _query_value(value, depth=0):
    """Project an authored literal to the finite typed algebra, without evaluation."""
    from .contract import validate_value
    from .. import expressions as E
    if depth > 128:
        raise ValueError('authored value exceeds query depth')
    if isinstance(value, dict) and 'type' in value:
        # A value that claims to be typed is never reinterpreted as a record
        # when its tagged representation is malformed.
        typed = copy.deepcopy(validate_value(value))
        pending = [typed]
        while pending:
            item = pending.pop()
            if item['type'] == 'number':
                numerator, denominator = int(item['numerator']), int(item['denominator'])
                if math.gcd(abs(numerator), denominator) != 1:
                    raise ValueError('authored exact number is not canonical')
            elif item['type'] == 'list':
                pending.extend(item['items'])
            elif item['type'] == 'record':
                pending.extend(item['fields'].values())
        return typed
    if type(value) is bool:
        return {'type': 'boolean', 'value': value}
    if value is None:
        return {'type': 'null'}
    if isinstance(value, str):
        return {'type': 'text', 'value': value}
    number = E.number(value)
    if number is not None:
        return {'type': 'number', 'numerator': str(number.numerator),
                'denominator': str(number.denominator)}
    if isinstance(value, list):
        return {'type': 'list', 'items': [_query_value(item, depth + 1) for item in value]}
    if isinstance(value, dict) and all(type(key) is str for key in value):
        return {'type': 'record', 'fields': {
            key: _query_value(value[key], depth + 1) for key in sorted(value)}}
    raise TypeError('authored value is outside the finite typed algebra')


class ScopeCapture:
    __slots__ = ('__snapshot', '__data')

    def __init__(self, snapshot, scope_id, *, limits=None, query=False):
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
        fields = definition['fields']
        if fields != sorted(set(fields)):
            raise SnapshotError('invalid_scope', 'scope fields must be sorted and unique')
        if query and any(field in ('rule', 'computed') for field in fields):
            raise SnapshotError('invalid_scope', 'formula and computed fields cannot be query inputs')
        normalized = {'collection': collection, 'fields': fields}
        source = data['document'].get(collection)
        if not isinstance(source, dict) or collection in ('meta', 'schema', 'record', 'also'):
            raise SnapshotError('scope_unavailable', 'collection membership unavailable')
        # Field authority belongs to the collection's schema, including when no
        # member exists. Validate it before projecting any candidate values.
        mapped_seen = _fields(data['document'])['snapshot']
        if any(field == mapped_seen or field in ('assessment', 'current_assessment') for field in fields):
            raise SnapshotError('invalid_scope', 'historical or assessment fields cannot be computational inputs')
        members = sorted(source)
        bound = MAX_COLLECTION
        if limits is not None:
            if not isinstance(limits, dict) or set(limits) - {'members', 'field_cells'}:
                raise SnapshotError('invalid_limits', 'unknown scope limit')
            bound = limits.get('members', bound)
            if type(bound) is not int or not 0 < bound <= MAX_COLLECTION:
                raise SnapshotError('invalid_limits', 'invalid collection bound')
            field_cells = limits.get('field_cells', MAX_EDGES)
            if type(field_cells) is not int or not 0 < field_cells <= MAX_EDGES:
                raise SnapshotError('invalid_limits', 'invalid field cell bound')
        else:
            field_cells = MAX_EDGES
        if len(members) * len(fields) > field_cells:
            raise SnapshotError('limit', 'scope candidate field limit exceeded')
        if len(members) > bound:
            raise SnapshotError('limit', 'collection member limit exceeded')
        candidates = {}
        dependencies = []
        for member in members:
            if member not in nodes or nodes[member]['collection'] != collection:
                raise SnapshotError('scope_unavailable', 'collection member not captured')
            body = nodes[member]['body']
            candidates[member] = {}
            for field in fields:
                conflicts = data['context'].get('conflicts', {}).get(member)
                present = isinstance(body, dict) and field in body
                observation = {'status': 'contested' if conflicts else ('known' if present else 'missing')}
                if query and not conflicts and not present and field == 'v' \
                        and isinstance(body, dict) and 'rule' in body:
                    observation = {'status': 'unavailable', 'reason': 'formula_value'}
                if present:
                    observation['value'] = copy.deepcopy(body[field])
                if conflicts:
                    # Full conflict bodies remain in the snapshot. A scope grant
                    # exposes this field only, including in alternative readings.
                    observation['alternatives'] = [
                        [name, {field: copy.deepcopy(variant[field])}
                         if isinstance(variant, dict) and field in variant else {}]
                        for name, variant in conflicts]
                if not query and field in ('v', 'rule') and isinstance(body, dict) and 'rule' in body:
                    observation['computed_basis'] = snapshot._input_basis().summary(member)
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
        basis = {'version': 1, 'recipe': 'scope-inputs/v2',
                 'profile': 'core/v1', 'modules': ['arithmetic/v1'],
                 'witness': witness, 'members': list(members), 'fields': list(fields), 'dependencies': dependencies,
                 'historical_detail': 'fingerprints_only', 'as_of': data['as_of']}
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

    def _member_ids(self):
        return tuple(self.__data['basis']['members'])

    def _validate_query_fields(self):
        fields = self.__data['definition']['fields']
        if any(field in ('rule', 'computed') for field in fields):
            raise SnapshotError('invalid_scope', 'formula and computed fields cannot be query inputs')

    def query_rows(self):
        """Return canonical authored-only query cells in member-ID order.

        The row adapter never evaluates ``rule`` and never substitutes a
        computed basis for an absent authored value.  Existing ``candidates``
        and ``view`` retain their richer review evidence unchanged.
        """
        self._validate_query_fields()
        result = []
        for member in self.__data['basis']['members']:
            fields = {}
            for field in self.__data['definition']['fields']:
                observation = self.__data['candidates'][member][field]
                if observation['status'] == 'contested':
                    fields[field] = {'status': 'contested'}
                    continue
                if observation['status'] == 'missing':
                    fields[field] = ({'status': 'unavailable', 'reason': 'formula_value'}
                                     if field == 'v' and 'computed_basis' in observation
                                     else {'status': 'missing'})
                    continue
                if observation['status'] == 'unavailable':
                    fields[field] = {'status': 'unavailable', 'reason': observation['reason']}
                    continue
                value = observation.get('value')
                if isinstance(value, (datetime.date, datetime.datetime)):
                    fields[field] = {'status': 'unavailable', 'reason': 'unsupported_type'}
                    continue
                try:
                    fields[field] = {'status': 'known', 'value': _query_value(value)}
                except TypeError:
                    fields[field] = {'status': 'unavailable', 'reason': 'unsupported_type'}
                except (ValueError, RecursionError, OverflowError):
                    fields[field] = {'status': 'unavailable', 'reason': 'invalid_value'}
            result.append({'id': member, 'fields': fields})
        return result

    def view(self):
        return SnapshotView(self.__snapshot, scopes=[self.__data['scope_id']])

    def _read_field(self, field):
        # Internal view boundary: copy only the selected field, never the full scope.
        if field not in self.__data['definition']['fields']:
            raise SnapshotError('undeclared_dependency', self.__data['scope_id'] + ':' + field)
        return [dict(id=member, **copy.deepcopy(values[field]))
                for member, values in self.__data['candidates'].items()]


class SnapshotView:
    """Exact grants; returned values are detached and never mutable snapshot state."""
    __slots__ = ('__snapshot', '__data', '__nodes', '__scopes', '__reads', '__limits',
                 '__node_bases', '__scope_captures')

    def __init__(self, snapshot, *, nodes=(), scopes=(), limits=None):
        self.__snapshot = snapshot
        # Both classes own this module's immutable backing data. Only detached
        # node/field values leave the view; no full copy is needed per request.
        self.__data = snapshot._Snapshot__data
        self.__node_bases = {}
        self.__scope_captures = {}
        self.__nodes = frozenset(nodes)
        self.__scopes = frozenset(scopes)
        self.__reads = {}
        self.__limits = copy.deepcopy(limits)

    def read_node(self, node_id):
        if node_id not in self.__nodes:
            raise SnapshotError('undeclared_dependency', node_id)
        if node_id not in self.__node_bases:
            self.__node_bases[node_id] = self.__snapshot._input_basis().summary(node_id)
        basis = self.__node_bases[node_id]
        witness = {'kind': 'node', 'id': node_id, 'fingerprint': basis['fingerprint']}
        self.__reads[digest(witness)] = witness
        return copy.deepcopy(self.__data['nodes'].get(node_id))

    def read_scope(self, scope_id, field):
        if scope_id not in self.__scopes:
            raise SnapshotError('undeclared_dependency', scope_id)
        if scope_id not in self.__scope_captures:
            self.__scope_captures[scope_id] = self.__snapshot.capture_scope(scope_id, limits=self.__limits)
        capture = self.__scope_captures[scope_id]
        values = capture._read_field(field)
        witness = capture.witness
        self.__reads[digest(witness)] = witness
        return values

    @property
    def executed_reads(self):
        return [copy.deepcopy(self.__reads[key]) for key in sorted(self.__reads)]


def _sha(data):
    return hashlib.sha256(data).hexdigest()


class _Inventory:
    def __init__(self, *, retain_bytes=False):
        self.events = {}
        self.contents = {} if retain_bytes else None

    def __call__(self, kind, path, value):
        if kind == 'bytes' and self.contents is not None:
            self.contents[path] = bytes(value)
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


def _publication_context(status):
    """Bind observed content and disposition, not publisher scheduling telemetry.

    Exact private capture checks still observe the complete publisher state.
    Only the portable semantic context omits repeated verification timestamps.
    Authored documents and immutable events never pass through this projection.
    """
    result = copy.deepcopy(status)
    for key in ('retry_at', 'failures'):
        result.pop(key, None)
    verified = result.get('last_verified')
    if isinstance(verified, dict):
        verified.pop('at', None)
    return result


class CapturedSource:
    """Private exact reader inventory for checked copying; not publication authority."""
    def __init__(self, snapshot, inventory, observation, paths, mode):
        self.snapshot = snapshot
        self.inventory = inventory
        self.observation = copy.deepcopy(observation)
        self.paths = list(paths)
        self.mode = mode

    @property
    def files(self):
        return dict(self.inventory.contents)

    def verify(self):
        self.inventory.verify()
        if digest(self.observation) != digest(_observation(self.paths, self.mode)):
            raise SnapshotError('snapshot_changed', 'routing, pending, or target observation changed')



def _retained_history_members(document, entry):
    """Validate imported source locators without treating originals as authority."""
    import fnmatch
    from .. import history_contract as C, history_transaction as T, provenance as P
    entry = Path(entry).absolute()
    imported = document.get('meta', {}).get('history_import')
    members = {}
    if imported is not None:
        C._mapping(imported, ('version', 'operation', 'recorded_at', 'members'))
        C._require(type(imported['version']) is int and imported['version'] == 1,
                   'unsupported_history_import')
        C._text(imported['operation'])
        C._require(isinstance(imported['recorded_at'], str) and imported['recorded_at'], 'invalid_history_import')
        C._require(isinstance(imported['members'], list) and len(imported['members']) <= C.MAX_OBJECTS,
                   'history_limit')
        total = 0
        for item in imported['members']:
            C._mapping(item, ('path', 'sha256', 'role'))
            name = C.relative_path(item['path'])
            C._text(item['sha256'], C.HEX)
            C._require(item['role'] in ('retained_original', 'replaced') and name not in members
                       and name != entry.name, 'invalid_history_import')
            path = T._target(entry.parent, name)
            P._capture_event('exists', path, path.exists())
            C._require(path.is_file() and path.stat().st_size <= C.MAX_REQUEST_BYTES,
                       'missing_retained_history_file', name)
            raw = path.read_bytes()
            total += len(raw)
            C._require(total <= T.MAX_TRANSACTION_BYTES, 'history_limit')
            P._capture_event('bytes', path, raw)
            C._require(C.sha256(raw) == item['sha256'], 'retained_history_mismatch', name)
            members[name] = item
    for key in ('record', 'also'):
        value = document.get(key)
        values = [value] if isinstance(value, str) else value if isinstance(value, list) else \
            list(value.values()) if isinstance(value, dict) else []
        for pointer in values:
            if isinstance(pointer, str) and pointer.endswith(('.yaml', '.yml')):
                C.relative_path(pointer)
                C._require(any(fnmatch.fnmatchcase(name, pointer) and item['role'] == 'retained_original'
                               for name, item in members.items()), 'history_composite_capture_unsupported',
                           'history template pointer lacks retained member evidence: ' + pointer)
    return members


def _capture_load(paths, mode, initial):
    """Select explicit history authority before the ordinary loader sees a view."""
    from .. import provenance as P, knowledge_views as V, history_contract as C
    routed = V.write_paths(paths) if mode == 'live' else paths
    entries = [str(Path(path).absolute()) for pattern in routed
               for path in sorted(P._capture_glob(glob.escape(pattern) if Path(pattern).exists() else pattern)) or [pattern]]
    active = []
    for entry in entries:
        marker_path = Path(P.layout(entry)['history_authority'])
        exists = marker_path.exists()
        P._capture_event('exists', marker_path, exists)
        if not exists:
            continue
        with marker_path.open('rb') as stream:
            raw = stream.read(C.MAX_OBJECT_BYTES + 1)
        P._capture_event('bytes', marker_path, raw)
        C._require(len(raw) <= C.MAX_OBJECT_BYTES, 'history_limit')
        marker = C.validate_authority(C.decode_document(raw))
        if marker['authority'] == 'history':
            active.append(entry)
    if not active:
        return P.load(paths, read_mode=mode)
    if len(entries) != 1:
        raise SnapshotError('history_composite_capture_unsupported', 'capture one authoritative history entry')
    from ..history_store import Store
    from ..history_adapter import from_store_capture
    store = Store(active[0])
    captured = store.capture()
    rendered = C.decode_document(store.render(captured))
    current = digest(captured.document) == digest(rendered)
    known = store._known_view(captured)
    if not current and not known:
        raise SnapshotError('unresolved_view_edit', 'history entry needs reconciliation')
    adapted = from_store_capture(captured)
    document = adapted.document
    _retained_history_members(document, active[0])
    doc = P.Record(document)
    doc.hypotheses = P.load_hypotheses(routed)
    from .. import history_hypotheses as HH
    doc.hypotheses = HH.active_physical(document, active[0], doc.hypotheses)
    from .. import history_hypotheses as HH
    named, _ = HH.layers(adapted.projection, document)
    C._require(not set(named) & set(doc.hypotheses), 'hypothesis_authority_collision')
    doc.hypotheses.update(named)
    from .contract import capabilities, CapabilityError
    for hypothesis in doc.hypotheses.values():
        body = hypothesis['doc']
        try:
            capabilities(body)
        except CapabilityError as error:
            raise P.Refused(error.code + ': ' + str(error)) from None
        if isinstance(body.get('meta'), dict) and 'history' in body['meta']:
            raise SnapshotError('history_hypothesis_unsupported', 'hypothesis needs its own committed capture')
    doc.history_projection = adapted.projection
    doc.history_view = {'status': 'current' if current else 'stale_generated',
                        'baseline_digest': digest(captured.document['meta']['history'])}
    doc.history_private_roots = [os.path.abspath(store.layout[name])
                                 for name in ('history', 'history_commits')]
    return V.overlay(routed, doc, read_mode=mode)


def capture_source(paths, *, read_mode=None, as_of=None):
    """Capture the reader's exact bytes and observations for a later atomic copy."""
    return capture(paths, read_mode=read_mode, as_of=as_of, _retain_source=True)


def capture(paths, *, read_mode=None, as_of=None, _retain_source=False):
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
        inventory = _Inventory(retain_bytes=_retain_source)
        token = P._CAPTURE_READS.set(inventory)
        core_token = P._CORE_READS.set(True)
        try:
            try:
                documents.append(_capture_load(paths, mode, initial))
            except P._peer('history_contract').HistoryError as error:
                raise SnapshotError(error.code, str(error)) from error
        finally:
            P._CORE_READS.reset(core_token)
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
    from ..pending_publication import scope_identity
    publication = config.get('publication')
    target = copy.deepcopy(getattr(doc, 'knowledge_target', initial.get('target', {})))
    reason = getattr(doc, 'target_unavailable', None)
    observed_target = hasattr(doc, 'knowledge_target') and target.get('revision')
    target.update(status='unavailable' if reason else ('observed' if observed_target else 'unassessed'),
                  reason=reason, ref=target.get('ref'), revision=target.get('revision'))
    if hasattr(doc, 'knowledge_target_snapshot'):
        target['snapshot'] = copy.deepcopy(doc.knowledge_target_snapshot)
    context = {'read_mode': 'captured-live' if mode == 'live' else 'frozen',
               'original_read_mode': mode,
               'project': {'version': config['version'], 'mode': config['mode'], 'generation': config['generation'],
                           'routing_identity': _portable(initial['record'], origin),
                           'publication_identity': scope_identity(publication) if publication else None},
               'pending': _portable(getattr(doc, 'pending_snapshot', {'ref': None, 'bundles': {}, 'events': []}), origin),
               'conflicts': _portable(getattr(doc, 'knowledge_conflicts', {}), origin, authored=True),
               'target': _portable(target, origin)}
    context['pending']['observations'] = _portable(_publication_context(
        getattr(doc, 'publication', initial.get('publication', {}))), origin)
    context['pending']['contributions'] = _portable([
        _publication_context(item) for item in getattr(doc, 'contributions', [])], origin)
    if getattr(doc, 'history_contributions', None):
        context['history_contributions'] = copy.deepcopy(doc.history_contributions)
    if hasattr(doc, 'history_projection'):
        context['history'] = doc.history_projection
        context['history_view'] = doc.history_view
    private_roots = getattr(doc, 'history_private_roots', [])
    def public_revision_path(path):
        absolute = os.path.abspath(path)
        return not any(absolute == root or absolute.startswith(root + os.sep) for root in private_roots)
    files = [{'origin': origin(path), 'sha256': value if kind == 'bytes' else None,
              'status': 'read' if kind == 'bytes' else 'unreadable',
              **({'error': value} if kind == 'unreadable' else {})}
             for (kind, path), value in sorted(inventories[-1].events.items()) if kind in ('bytes', 'unreadable') and public_revision_path(path)]
    revision = {'files': sorted(files, key=lambda item: item['origin'])}
    revision['digest'] = digest(revision)
    hypotheses = copy.deepcopy(doc.hypotheses)
    for hypothesis in hypotheses.values():
        if hypothesis.get('kind') == 'contribution':
            head = hypothesis.get('head', {})
            if isinstance(head.get('publication'), dict):
                head['publication'] = _publication_context(head['publication'])
    snapshot = Snapshot.from_data(doc, context=context, hypotheses=_portable(hypotheses, origin),
                                  as_of=as_of, authored_revision=revision)
    if _retain_source:
        return CapturedSource(snapshot, inventories[-1], initial, paths, mode)
    return snapshot
