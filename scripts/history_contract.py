"""Versioned history boundaries, independent of storage and evaluation.

These functions validate detached evidence. A valid digest establishes internal
consistency, never source authenticity, acceptance, or permission to publish.
Readers choose authority through the record's marker, not a directory's presence.
"""
import copy
import datetime
from collections import deque
import hashlib
import re
from pathlib import PurePosixPath

import yaml

try:
    from .pending_grounding import identity
    from .reasoning.contract import _check_finite, OutputBudget, MAX_REQUEST_BYTES
except ImportError:
    from pending_grounding import identity
    from reasoning.contract import _check_finite, OutputBudget, MAX_REQUEST_BYTES


ID_SCHEME = 'typed-history/v2'
LEGACY_SCHEME = 'prototype/v1'
EXPLICIT_ROOT_DISPOSITION = 'explicit-root-disposition/v1'
PROFILE = 'history/v1'
TOKEN = re.compile(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,159}\Z')
SUBJECT = re.compile(r'[A-Za-z0-9_][A-Za-z0-9_.-]*\Z')
HEX = re.compile(r'[0-9a-f]{64}\Z')
OBJECT_ID = re.compile(r'(?:[0-9a-f]{40}|[0-9a-f]{64})\Z')
MAX_OBJECT_BYTES = 1024 * 1024
MAX_PROJECTION_BYTES = MAX_REQUEST_BYTES // 2
MAX_OBJECTS = 20000
MAX_VALUE_VISITS = 100000


class HistoryError(ValueError):
    def __init__(self, code, detail=''):
        self.code = code
        super().__init__(code + (': ' + detail if detail else ''))


def _require(condition, code, detail=''):
    if not condition:
        raise HistoryError(code, detail)


def _mapping(value, required, optional=()):
    _require(isinstance(value, dict) and set(required) <= set(value)
             and not set(value) - set(required) - set(optional), 'invalid_schema')


def _text(value, pattern=TOKEN):
    _require(isinstance(value, str) and pattern.fullmatch(value) is not None, 'invalid_identifier')


def _integer(value):
    _require(type(value) is int and value >= 0, 'invalid_generation')


def _ids(value, *, unique=True):
    _require(isinstance(value, list), 'invalid_references')
    for item in value:
        _text(item, OBJECT_ID)
    _require(value == sorted(set(value) if unique else value), 'invalid_references',
             'references must be sorted' + (' and unique' if unique else ''))


def _pins(value):
    _require(isinstance(value, dict), 'invalid_pins')
    for subject, vid in value.items():
        _text(subject, SUBJECT)
        _text(vid, OBJECT_ID)


def detached(value, maximum=MAX_OBJECT_BYTES):
    """Bound and validate before copying/hashing potentially recursive input."""
    # A shared Python DAG can expand exponentially even without a YAML alias.
    # Count occurrences before the existing recursive finite/type validation.
    pending, visits = [(value, 0)], 0
    while pending:
        item, depth = pending.pop()
        visits += 1
        _require(visits <= MAX_VALUE_VISITS and depth <= 128, 'history_limit')
        if isinstance(item, (dict, list)):
            children = item.values() if isinstance(item, dict) else item
            _require(len(item) <= MAX_VALUE_VISITS - visits, 'history_limit')
            pending.extend((child, depth + 1) for child in children)
    try:
        _check_finite(value)
        OutputBudget(maximum, 'history_limit').add(value)
    except ValueError as error:
        raise HistoryError(getattr(error, 'code', 'invalid_history_value'), str(error)) from error
    return copy.deepcopy(value)


def sha256(data):
    _require(type(data) is bytes, 'invalid_bytes')
    return hashlib.sha256(data).hexdigest()


def object_identity(value):
    """Dispatch by declared scheme; a bad new object never falls back to v1."""
    value = detached(value)
    _require(isinstance(value, dict), 'invalid_object')
    fields = {key: item for key, item in value.items() if key != 'id'}
    if value.get('id_scheme') == ID_SCHEME:
        return identity(fields)
    _require('id_scheme' not in value, 'unsupported_identity')
    try:
        from .versions import ident
    except ImportError:
        from versions import ident
    return ident(value)


def _authored(value):
    _mapping(value, ('collection', 'fields', 'profile'), ('locator', 'hypothesis'))
    _text(value['collection'], SUBJECT)
    _require(isinstance(value['fields'], dict) and all(
        isinstance(k, str) and isinstance(v, str) and v
        for k, v in value['fields'].items()), 'invalid_field_roles')
    _require(value['profile'] in ('ordinary-reader/v1', 'checked-reader/v1', 'core/v1'),
             'unsupported_profile')
    if 'locator' in value:
        _require(isinstance(value['locator'], dict), 'invalid_locator')
    if 'hypothesis' in value:
        group = value['hypothesis']
        _mapping(group, ('version', 'name', 'head'))
        _require(type(group['version']) is int and group['version'] == 1 and
                 isinstance(group['name'], str) and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_-]*', group['name'])
                 and isinstance(group['head'], dict), 'invalid_history_hypothesis')


def validate_pin_gaps(value):
    """Declared missing historical evidence is not a positive version reference."""
    gaps = value.get('pin_gaps', {})
    _require(isinstance(gaps, dict), 'invalid_pin_gaps')
    for subject, reason in gaps.items():
        _text(subject, SUBJECT)
        _require(reason in ('not_recorded', 'unavailable'), 'invalid_pin_gaps')
    pins = value['pins']
    _require(not set(pins) & set(gaps), 'overlapping_pin_gap')
    if not gaps:
        return gaps
    body = value['body']
    field = value['authored']['fields'].get('deps')
    _require(isinstance(body, dict) and field in body, 'pin_dependency_mismatch')
    deps = body[field]
    _require(isinstance(deps, (list, dict)), 'pin_dependency_mismatch')
    _require(all(isinstance(dep, str) for dep in deps) and
             set(deps) == set(pins) | set(gaps), 'pin_dependency_mismatch')
    if isinstance(deps, dict):
        for dep, original in deps.items():
            if dep in pins:
                _require(original == pins[dep], 'pin_dependency_mismatch')
            else:
                # A retained actual ID cannot be relabelled unknown to bypass
                # positive-reference closure checking.
                _require(not isinstance(original, str) or not OBJECT_ID.fullmatch(original),
                         'positive_reference_pin_gap', dep)
    return gaps


def pin_gap_findings(value):
    """Evidence-coverage findings; these never change recorded acceptance."""
    return [{'code': 'dependency_pin_' + reason, 'subject': value['subject'],
             'object_id': value['id'], 'detail': dependency}
            for dependency, reason in sorted(value.get('pin_gaps', {}).items())]


def validate_object(value):
    """Validate a v2 claim/act, or preserve an existing prototype identity."""
    value = detached(value)
    _mapping(value, ('id', 'subject', 'kind', 'by', 'on', 'op', 'body', 'saw'),
             ('id_scheme', 'schema_version', 'authored', 'pins', 'pin_gaps', 'at', 'applies'))
    _text(value['subject'], SUBJECT)
    _text(value['id'], OBJECT_ID)
    _text(value['op'])
    _require(value['kind'] in ('reading', 'judgment', 'act'), 'invalid_kind')
    _require(value['by'] is None or isinstance(value['by'], str), 'invalid_actor')
    _require(isinstance(value['on'], str) and value['on'], 'invalid_recorded_time')
    scalar_reading = value.get('id_scheme') == ID_SCHEME and value['kind'] == 'reading'
    _require(isinstance(value['body'], dict) or scalar_reading and (
        value['body'] is None or isinstance(value['body'],
            (str, bool, int, float, datetime.date, datetime.datetime))), 'invalid_body')
    _ids(value['saw'], unique='id_scheme' in value)
    if 'id_scheme' in value:
        _require(value['id_scheme'] == ID_SCHEME and type(value.get('schema_version')) is int
                 and value['schema_version'] == 2, 'unsupported_identity')
        if value['kind'] != 'act':
            _require('authored' in value and 'pins' in value, 'missing_authored_mapping')
            _authored(value['authored'])
            _pins(value['pins'])
            validate_pin_gaps(value)
            if not isinstance(value['body'], dict):
                _require(not value['pins'] and not value.get('pin_gaps'), 'pin_dependency_mismatch')
        else:
            _require(not set(value) & {'authored', 'pins', 'pin_gaps'}, 'invalid_act')
    else:
        _require(not set(value) & {'schema_version', 'authored', 'pins', 'pin_gaps'}, 'unsupported_identity')
        if value['kind'] == 'judgment':
            _pins(value['body'].get('rests_on', {}))
    if value['kind'] == 'act':
        body = value['body']
        _mapping(body, ('act', 'of', 'over', 'because'), ('read',))
        _require(body['act'] in (('accept', 'refute', 'review', 'correct', 'propose', 'retire')
                                if value.get('id_scheme') == ID_SCHEME else
                                ('accept', 'refute', 'review', 'correct')), 'invalid_act')
        if body['act'] in ('propose', 'retire'):
            _require(body['over'] == [] and isinstance(body['because'], str) and bool(body['because'].strip()),
                     'invalid_act')
        _text(body['of'], OBJECT_ID)
        _ids(body['over'], unique='id_scheme' in value)
        _require(isinstance(body['because'], str), 'invalid_act')
        if 'read' in body:
            _pins(body['read'])
    _require(value['id'] == object_identity(value), 'identity_mismatch')
    return value


def make_object(*, subject, kind, by, on, operation, body, saw=(), authored=None,
                pins=None, pin_gaps=None, at=None, applies=None):
    """The caller captures operation/time once and retains this envelope for retries."""
    value = {'schema_version': 2, 'id_scheme': ID_SCHEME, 'subject': subject, 'kind': kind,
             'by': by, 'on': on, 'op': operation, 'body': body, 'saw': sorted(saw)}
    if kind != 'act':
        value.update(authored=authored, pins={} if pins is None else pins)
    if pin_gaps is not None:
        value['pin_gaps'] = pin_gaps
    if at is not None:
        value['at'] = at
    if applies is not None:
        value['applies'] = applies
    value['id'] = object_identity(value)
    return validate_object(value)


def references(value):
    """(subject, id, expected kind) references, without interpreting a condition."""
    value = validate_object(value)
    subject = value['subject']
    result = [(subject, vid, 'any') for vid in value['saw']]
    pins = value.get('pins', value['body'].get('rests_on', {}) if value['kind'] == 'judgment' else {})
    result += [(name, vid, 'claim') for name, vid in pins.items()]
    if value['kind'] == 'act':
        body = value['body']
        result += [(subject, vid, 'claim') for vid in [body['of'], *body['over']]]
        result += [(name, vid, 'claim') for name, vid in body.get('read', {}).items()]
    return result


def validate_closure(objects):
    """A complete object closure or refusal; no partial valid subset is returned."""
    _require(isinstance(objects, dict) and len(objects) <= MAX_OBJECTS, 'history_limit')
    result = {}
    for vid, value in objects.items():
        validated = validate_object(value)
        _require(vid == validated['id'], 'identity_mismatch')
        result[vid] = validated
    for value in result.values():
        for subject, vid, kind in references(value):
            target = result.get(vid)
            _require(target is not None, 'incomplete_closure', vid)
            _require(target['subject'] == subject and (kind != 'claim' or target['kind'] != 'act'),
                     'reference_mismatch', vid)
    return result


class _UniqueLoader(yaml.SafeLoader):
    def compose_node(self, parent, index):
        _require(not self.check_event(yaml.AliasEvent), 'invalid_history_yaml',
                 'history YAML aliases are unsupported')
        return super().compose_node(parent, index)


class _NoAliasDumper(yaml.SafeDumper):
    def ignore_aliases(self, data):
        return True


def _unique_mapping(loader, node, deep=False):
    loader.flatten_mapping(node)
    result = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=deep)
        _require(isinstance(key, str) and key not in result, 'invalid_yaml_key')
        result[key] = loader.construct_object(value_node, deep=deep)
    return result


_UniqueLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _unique_mapping)


def decode_document(raw):
    """Strict mapping YAML for object/manifest files; duplicate keys are refused."""
    _require(type(raw) is bytes and len(raw) <= MAX_REQUEST_BYTES, 'history_limit')
    try:
        value = yaml.load(raw.decode('utf-8'), Loader=_UniqueLoader)
        _require(isinstance(value, dict), 'invalid_schema')
        return detached(value, MAX_REQUEST_BYTES)
    except HistoryError:
        raise
    except (yaml.YAMLError, UnicodeError, RecursionError, ValueError) as error:
        raise HistoryError('invalid_history_yaml') from error


def encode_document(value):
    value = detached(value, MAX_REQUEST_BYTES)
    _require(isinstance(value, dict), 'invalid_schema')
    raw = yaml.dump(value, Dumper=_NoAliasDumper, sort_keys=True, allow_unicode=True).encode('utf-8')
    _require(identity(decode_document(raw)) == identity(value), 'serialization_changed')
    return raw


def authority(*, record_id, authority, generation, profile=PROFILE):
    return validate_authority({'version': 1, 'record_id': record_id, 'authority': authority,
                               'generation': generation, 'profile': profile})


def validate_authority(value):
    value = detached(value)
    _mapping(value, ('version', 'record_id', 'authority', 'generation', 'profile'))
    _require(type(value['version']) is int and value['version'] == 1, 'unsupported_authority')
    _text(value['record_id'])
    _integer(value['generation'])
    _require(value['authority'] in ('legacy', 'history') and value['profile'] == PROFILE,
             'unsupported_authority')
    return value


def validate_baseline(value):
    value = detached(value, MAX_PROJECTION_BYTES)
    _mapping(value, ('version', 'record_id', 'authority_generation', 'committed_set_digest',
                     'heads', 'open_acts'))
    _require(type(value['version']) is int and value['version'] == 1, 'invalid_baseline')
    _text(value['record_id'])
    _integer(value['authority_generation'])
    _text(value['committed_set_digest'], HEX)
    for role in ('heads', 'open_acts'):
        _require(isinstance(value[role], dict), 'invalid_baseline')
        for subject, ids in value[role].items():
            _text(subject, SUBJECT)
            _ids(ids)
    return value


def bind_authority(marker, baseline):
    marker, baseline = validate_authority(marker), validate_baseline(baseline)
    _require(marker['authority'] == 'history'
             and marker['record_id'] == baseline['record_id']
             and marker['generation'] == baseline['authority_generation'], 'authority_mismatch')
    return marker, baseline


def relative_path(value):
    _require(isinstance(value, str) and value and '\\' not in value and '\x00' not in value,
             'invalid_path')
    path = PurePosixPath(value)
    _require(not path.is_absolute() and value == path.as_posix()
             and all(part not in ('.', '..') for part in value.split('/')), 'invalid_path')
    return value


def validate_commit(value):
    value = detached(value, MAX_REQUEST_BYTES)
    _mapping(value, ('version', 'record_id', 'authority_generation', 'operation', 'parents',
                     'baseline_digest', 'objects', 'receipt', 'view_sha256'), ('view_template', 'requires'))
    _require(type(value['version']) is int and value['version'] == 1, 'unsupported_commit')
    if 'requires' in value:
        validate_history_requires(value['requires'])
    _text(value['record_id'])
    _integer(value['authority_generation'])
    _text(value['operation'])
    _require(isinstance(value['parents'], dict), 'invalid_parents')
    for operation, content_hash in value['parents'].items():
        _text(operation)
        _text(content_hash, HEX)
        _require(operation != value['operation'], 'invalid_parents')
    _text(value['baseline_digest'], HEX)
    _text(value['view_sha256'], HEX)
    _require(isinstance(value['receipt'], dict) and value['receipt'], 'missing_receipt')
    _require(isinstance(value['objects'], list) and len(value['objects']) <= MAX_OBJECTS,
             'history_limit')
    ids = []
    for item in value['objects']:
        _mapping(item, ('subject', 'id', 'sha256'))
        _text(item['subject'], SUBJECT)
        _text(item['id'], OBJECT_ID)
        _text(item['sha256'], HEX)
        ids.append(item['id'])
    _require(ids == sorted(set(ids)), 'invalid_object_inventory')
    if 'view_template' in value:
        _require(identity(document_template(value['view_template'])) == identity(value['view_template']),
                 'invalid_view_template', 'template must contain headers and empty collections only')
    return value



def validate_history_requires(value):
    _require(isinstance(value, list) and all(isinstance(item, str) for item in value)
             and value == sorted(set(value)) and all(item == EXPLICIT_ROOT_DISPOSITION for item in value), 'unsupported_history_capability')
    return value


def validate_root_dispositions(manifest, objects, *, prior_ids=()):
    """A strict contribution carries each new root's explicit standing choice.

    Existing roots reachable through its parents keep their original compatible
    interpretation. A strict independently imported root is new to that authority
    and must carry its recorded accept/propose/retire act; no import invents one.
    """
    manifest = validate_commit(manifest)
    if EXPLICIT_ROOT_DISPOSITION not in manifest.get('requires', []):
        return
    members = {item['id']: objects[item['id']] for item in manifest['objects']}
    disposed = {obj['body']['of'] for obj in members.values() if obj['kind'] == 'act'
                and obj['body']['act'] in ('accept', 'propose', 'retire')}
    prior_ids = set(prior_ids)
    for version, obj in members.items():
        if obj['kind'] != 'act' and not obj['saw'] and version not in prior_ids:
            _require(version in disposed, 'missing_root_disposition', version)


def make_commit(*, marker, operation, parents, baseline, objects, receipt, view, view_template=None, requires=None):
    """Bind exact new object bytes and intended view; references need closure validation.

    `objects` is an iterable of (validated object, serialized bytes). A storage
    reader must parse those same bytes and verify complete parent/object closure.
    The receipt is evaluator/capability evidence, not a publication permission.
    """
    marker, baseline = bind_authority(marker, baseline)
    inventory = []
    for obj, raw in objects:
        obj = validate_object(obj)
        _require(identity(validate_object(decode_document(raw))) == identity(obj), 'object_bytes_mismatch')
        inventory.append({'subject': obj['subject'], 'id': obj['id'], 'sha256': sha256(raw)})
    return validate_commit({'version': 1, 'record_id': marker['record_id'],
                            'authority_generation': marker['generation'], 'operation': operation,
                            'parents': parents, 'baseline_digest': identity(baseline),
                            'objects': sorted(inventory, key=lambda item: item['id']),
                            'receipt': receipt, 'view_sha256': sha256(view),
                            **({'view_template': view_template} if view_template is not None else {}),
                            **({'requires': requires} if requires is not None else {})})


def document_template(document):
    """Keep immutable render headers and empty collections, never generated heads."""
    document = detached(document, MAX_REQUEST_BYTES)
    _require(isinstance(document, dict), 'invalid_document')
    try:
        from .provenance import collections_of
    except ImportError:
        from provenance import collections_of
    for collection in collections_of(document):
        if collection != 'meta':
            document[collection] = {}
    if 'meta' in document:
        _require(isinstance(document['meta'], dict), 'invalid_document')
        document['meta'].pop('history', None)
    return document


def committed_set_digest(commits):
    """Non-circular render baseline for the known set of committed effects.

    View bytes contain this digest, and a semantic receipt may contain prepared
    view evidence, so neither downstream field can be in its preimage. Complete
    raw manifest bytes remain separately bound by captured closure identity.
    Parents are already committed manifest hashes, not a hash of this commit.
    """
    _require(isinstance(commits, dict) and len(commits) <= MAX_OBJECTS, 'history_limit')
    effects = {}
    for operation, raw in commits.items():
        manifest = validate_commit(decode_document(raw)) if isinstance(raw, bytes) else validate_commit(raw)
        _require(manifest['operation'] == operation, 'operation_mismatch')
        effects[operation] = {key: item for key, item in manifest.items()
                              if key not in ('receipt', 'view_sha256')}
    return identity(effects)


def commit_frontier(commits):
    """The complete captured set's causal frontier, with exact parent bytes."""
    _require(isinstance(commits, dict) and len(commits) <= MAX_OBJECTS, 'history_limit')
    parents = set()
    for operation, raw in commits.items():
        manifest = validate_commit(decode_document(raw))
        _require(operation == manifest['operation'], 'operation_mismatch')
        parents.update(manifest['parents'])
    return {operation: sha256(raw) for operation, raw in sorted(commits.items()) if operation not in parents}


def claim_meaning(obj):
    """Authored interpretation affects agreement; an evidence locator does not."""
    value = {key: copy.deepcopy(obj.get(key)) for key in ('kind', 'body', 'pins', 'authored')}
    if obj.get('pin_gaps'):
        value['pin_gaps'] = copy.deepcopy(obj['pin_gaps'])
    if isinstance(value['authored'], dict):
        value['authored'].pop('locator', None)
        value['authored'].pop('hypothesis', None)
    return value


def committed_objects(marker, commits, objects):
    """Pure all-or-refuse visibility rule over captured manifest/object bytes.

    Objects unreferenced by a manifest are staging residue and do not affect
    authority. All supplied manifests and their parents must be complete. The
    caller captures file membership before using this function; omissions from
    that capture cannot be proved by a digest. Existing immutable object ids may
    be imported by a new operation without rewriting original operation/time.
    """
    marker = validate_authority(marker)
    _require(marker['authority'] == 'history', 'authority_mismatch')
    _require(isinstance(commits, dict) and isinstance(objects, dict), 'invalid_closure')
    _require(len(commits) <= MAX_OBJECTS, 'history_limit')
    manifests, selected = {}, {}
    for operation, raw in commits.items():
        manifest = validate_commit(decode_document(raw))
        _require(operation == manifest['operation'], 'operation_mismatch')
        _require(manifest['record_id'] == marker['record_id']
                 and manifest['authority_generation'] == marker['generation'], 'authority_mismatch')
        manifests[operation] = manifest
        for item in manifest['objects']:
            key = (item['subject'], item['id'])
            data = objects.get(key)
            _require(data is not None, 'incomplete_commit', item['id'])
            _require(sha256(data) == item['sha256'], 'object_bytes_mismatch', item['id'])
            obj = validate_object(decode_document(data))
            _require((obj['subject'], obj['id']) == key, 'reference_mismatch')
            selected[obj['id']] = obj
    for operation, manifest in manifests.items():
        for parent, expected in manifest['parents'].items():
            _require(parent in commits, 'incomplete_commit', parent)
            _require(sha256(commits[parent]) == expected, 'parent_bytes_mismatch')
    # Cyclic commit ancestry is malformed even when every file is present.
    children = {operation: [] for operation in manifests}
    remaining = {operation: len(manifest['parents']) for operation, manifest in manifests.items()}
    for operation, manifest in manifests.items():
        for parent in manifest['parents']:
            children[parent].append(operation)
    ready = deque(operation for operation, count in remaining.items() if count == 0)
    visited = 0
    while ready:
        operation = ready.popleft()
        visited += 1
        for child in children[operation]:
            remaining[child] -= 1
            if not remaining[child]:
                ready.append(child)
    _require(visited == len(manifests), 'cyclic_commits')
    membership = {op: {item['id'] for item in manifest['objects']} for op, manifest in manifests.items()}
    work = 0
    for operation, manifest in manifests.items():
        if EXPLICIT_ROOT_DISPOSITION not in manifest.get('requires', []):
            continue
        disposed = {selected[vid]['body']['of'] for vid in membership[operation]
                    if selected[vid]['kind'] == 'act' and
                    selected[vid]['body']['act'] in ('accept', 'propose', 'retire')}
        unresolved = {vid for vid in membership[operation] if selected[vid]['kind'] != 'act'
                      and not selected[vid]['saw'] and vid not in disposed}
        prior, examined, pending = set(), set(), list(manifest['parents'])
        while unresolved - prior and pending:
            parent = pending.pop()
            if parent in examined:
                continue
            examined.add(parent)
            work += 1
            _require(work <= 4_000_000, 'history_limit', 'root disposition ancestry')
            prior.update(unresolved & membership[parent])
            pending.extend(manifests[parent]['parents'])
        validate_root_dispositions(manifest, selected, prior_ids=prior)
    return validate_closure(selected)


def validate_subset_origin(value):
    value = detached(value, MAX_PROJECTION_BYTES)
    _mapping(value, ('version', 'kind', 'operation', 'recorded_at', 'source_authority',
                     'source_capture_digest', 'source_entry', 'roots', 'disclosed_locators',
                     'prepared_digest', 'subjects'))
    _require(type(value['version']) is int and value['version'] == 1 and
             value['kind'] == 'selected-subject-observation', 'invalid_subset_origin')
    _text(value['operation'])
    _require(isinstance(value['recorded_at'], str) and value['recorded_at'], 'invalid_subset_origin')
    _require(validate_authority(value['source_authority'])['authority'] == 'history', 'invalid_subset_origin')
    _text(value['source_capture_digest'], HEX)
    relative_path(value['source_entry'])
    _require(isinstance(value['roots'], list) and value['roots'] and
             value['roots'] == sorted(set(value['roots'])), 'invalid_subset_origin')
    _require(isinstance(value['subjects'], dict) and set(value['roots']) <= set(value['subjects']),
             'invalid_subset_origin')
    for subject, item in value['subjects'].items():
        _text(subject, SUBJECT)
        _mapping(item, ('source_state', 'objects_digest', 'reduction_digest'))
        _require(item['source_state'] in ('committed', 'prepared_candidate'), 'invalid_source_state')
        _text(item['objects_digest'], HEX)
        _text(item['reduction_digest'], HEX)
    _require(isinstance(value['disclosed_locators'], list), 'invalid_subset_origin')
    for item in value['disclosed_locators']:
        _mapping(item, ('path', 'sha256'))
        relative_path(item['path'])
        if item['sha256'] is not None:
            _text(item['sha256'], HEX)
    if value['prepared_digest'] is not None:
        _text(value['prepared_digest'], HEX)
    _require(not any(item['source_state'] == 'prepared_candidate' for item in value['subjects'].values())
             or value['prepared_digest'] is not None, 'invalid_source_state')
    return value


def validate_projection(value):
    """Bounded history evidence that participates in Snapshot.context identity.

    Full file inventories and audit artifacts belong to CapturedSource. This
    projection contains selected claims/pins only, never an implicit live lookup.
    """
    value = detached(value, MAX_PROJECTION_BYTES)
    _mapping(value, ('projection_version', 'authority', 'baseline', 'identity_schemes', 'rules',
                     'rules_digest', 'closure_digest', 'coverage', 'subjects', 'pins', 'integrity'),
             ('dispositions', 'origin', 'requires'))
    _require(type(value['projection_version']) is int and value['projection_version'] == 1,
             'unsupported_projection')
    bind_authority(value['authority'], value['baseline'])
    if 'requires' in value:
        validate_history_requires(value['requires'])
    schemes = value['identity_schemes']
    _require(isinstance(schemes, list) and all(s in (ID_SCHEME, LEGACY_SCHEME) for s in schemes)
             and schemes == sorted(set(schemes)), 'unsupported_identity')
    _require(isinstance(value['rules'], dict) and identity(value['rules']) == value['rules_digest'],
             'rules_mismatch')
    _text(value['closure_digest'], HEX)
    coverage = value['coverage']
    _mapping(coverage, ('scope', 'subjects', 'complete'))
    _require(coverage['scope'] in ('all', 'selected') and type(coverage['complete']) is bool,
             'invalid_coverage')
    _require(isinstance(coverage['subjects'], list), 'invalid_coverage')
    for subject in coverage['subjects']:
        _text(subject, SUBJECT)
    _require(coverage['subjects'] == sorted(set(coverage['subjects'])), 'invalid_coverage')
    _require(isinstance(value['subjects'], dict)
             and sorted(value['subjects']) == coverage['subjects'], 'invalid_coverage')
    if coverage['scope'] == 'all':
        _require(set(value['baseline']['heads']) | set(value['baseline']['open_acts'])
                 <= set(coverage['subjects']), 'invalid_coverage')
    if 'origin' in value:
        origin = validate_subset_origin(value['origin'])
        _require(coverage['scope'] == 'selected' and set(origin['subjects']) == set(value['subjects']),
                 'invalid_subset_coverage')
    for subject, result in value['subjects'].items():
        _mapping(result, ('acceptance', 'heads', 'open_acts'))
        _require(result['acceptance'] in ('accepted', 'proposed', 'contested', 'refuted',
                                         'corrected', 'retired', 'unreviewed', 'unavailable'), 'invalid_acceptance')
        _ids(result['heads'])
        _ids(result['open_acts'])
        _require(result['heads'] == value['baseline']['heads'].get(subject, [])
                 and result['open_acts'] == value['baseline']['open_acts'].get(subject, []),
                 'baseline_mismatch')
    _require(isinstance(value['pins'], dict), 'invalid_pins')
    for vid, witness in value['pins'].items():
        _mapping(witness, ('subject', 'version', 'status', 'object'))
        _text(vid, OBJECT_ID)
        _text(witness['subject'], SUBJECT)
        _require(witness['version'] == vid and witness['status'] in ('recorded', 'unavailable', 'corrupt'),
                 'invalid_pin_witness')
        if witness['status'] == 'recorded':
            obj = validate_object(witness['object'])
            _require((ID_SCHEME if 'id_scheme' in obj else LEGACY_SCHEME) in schemes,
                     'unsupported_identity')
            _require(obj['id'] == vid and obj['subject'] == witness['subject'] and obj['kind'] != 'act',
                     'reference_mismatch')
        else:
            _require(witness['object'] is None, 'invalid_pin_witness')
    dispositions = value.get('dispositions', {})
    _require(isinstance(dispositions, dict) and set(dispositions) <= set(value['subjects']),
             'invalid_dispositions')
    for subject, disposition in dispositions.items():
        _mapping(disposition, ('marks', 'proposals', 'contested_claims', 'reviews', 'implied'), ('source_state',))
        if 'origin' in value:
            _require(disposition.get('source_state') == value['origin']['subjects'][subject]['source_state'],
                     'invalid_source_state')
        elif 'source_state' in disposition:
            _require(False, 'missing_subset_origin')
        _require(isinstance(disposition['marks'], dict), 'invalid_dispositions')
        for vid, mark in disposition['marks'].items():
            _text(vid, OBJECT_ID)
            _require(mark in ('corrected', 'refuted', 'retired', 'replaced', 'superseded'), 'invalid_dispositions')
        _ids(disposition['proposals'])
        _ids(disposition['contested_claims'])
        _require(isinstance(disposition['reviews'], list), 'invalid_dispositions')
        review_ids = []
        for review in disposition['reviews']:
            review = validate_object(review)
            _require(review['subject'] == subject and review['kind'] == 'act'
                     and review['body']['act'] == 'review'
                     and review['body']['of'] in (value['subjects'][subject]['heads'] + disposition['proposals']),
                     'invalid_review_scope')
            _require((ID_SCHEME if 'id_scheme' in review else LEGACY_SCHEME) in schemes,
                     'unsupported_identity')
            for dependency, vid in review['body'].get('read', {}).items():
                witness = value['pins'].get(vid)
                _require(witness is not None and witness['subject'] == dependency, 'missing_pin_witness')
            review_ids.append(review['id'])
        _require(review_ids == sorted(set(review_ids)), 'invalid_references')
        _require(isinstance(disposition['implied'], list), 'invalid_dispositions')
        for finding in disposition['implied']:
            _mapping(finding, ('rule', 'subject', 'superseded', 'by', 'why'))
            _require(finding['rule'] == 'source_clock' and finding['subject'] == subject
                     and isinstance(finding['why'], str), 'invalid_dispositions')
            _text(finding['superseded'], OBJECT_ID)
            _text(finding['by'], OBJECT_ID)
    if 'origin' in value:
        _require(set(dispositions) == set(value['subjects']), 'invalid_subset_coverage')
    integrity = value['integrity']
    _mapping(integrity, ('complete', 'findings'))
    _require(type(integrity['complete']) is bool and isinstance(integrity['findings'], list),
             'invalid_integrity')
    for finding in integrity['findings']:
        _mapping(finding, ('code', 'subject', 'object_id', 'detail'))
        _require(all(isinstance(finding[k], str) for k in finding), 'invalid_integrity')
    for witness in value['pins'].values():
        if witness['status'] == 'recorded':
            for finding in pin_gap_findings(witness['object']):
                _require(not integrity['complete'] and not coverage['complete'] and
                         finding in integrity['findings'], 'unreported_pin_gap')
    _require(not integrity['complete'] or (not integrity['findings'] and coverage['complete']),
             'invalid_integrity')
    return value


class CapturedHistory:
    """Pure detached adapter result; original seen and profile remain in the document."""
    def __init__(self, document, projection):
        self._document = detached(document, MAX_REQUEST_BYTES)
        _require(isinstance(document, dict), 'invalid_document')
        self._projection = validate_projection(projection)
        meta = document.get('meta', {})
        _require(isinstance(meta, dict) and 'history' in meta, 'baseline_mismatch')
        actual = validate_baseline(meta['history'])
        _require(identity(actual) == identity(self._projection['baseline']), 'baseline_mismatch')
        origin = meta.get('history_subset')
        if origin is not None or 'origin' in self._projection:
            _require(origin is not None and identity(validate_subset_origin(origin)) ==
                     identity(self._projection.get('origin')), 'subset_origin_mismatch')

    @property
    def document(self):
        return copy.deepcopy(self._document)

    @property
    def projection(self):
        return copy.deepcopy(self._projection)

    def snapshot(self, *, context=None, hypotheses=None, as_of=None, authored_revision=None):
        try:
            from .reasoning.snapshot import Snapshot
        except ImportError:
            from reasoning.snapshot import Snapshot
        context = copy.deepcopy(context) if context is not None else {}
        _require('history' not in context, 'duplicate_history_context')
        context['history'] = self.projection
        return Snapshot.from_data(self.document, context=context, hypotheses=hypotheses,
                                  as_of=as_of, authored_revision=authored_revision)
