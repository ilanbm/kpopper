"""Pure import of captured physical hypothesis layers into named proposals.

Only supplied bytes/typed values are read. File membership, actual path access,
publication, and verification of the inactive physical mapping belong to callers.
Import time is a new capture event; it is never historical review or observation
provenance. Original bodies, seen and heads remain unchanged.
"""
import copy
from pathlib import PurePosixPath
import re

from . import history_contract as C, history_store as H, provenance as P
from .pending_grounding import entries, identity
from .reasoning.contract import capabilities, CapabilityError
from .reasoning.snapshot import _fields, SnapshotError

KIND = 'legacy-hypothesis-import/v1'
MAPPING_KEY = 'history_hypothesis_import'
NAME = re.compile(r'[A-Za-z0-9][A-Za-z0-9_-]*\Z')


def validate_mapping(value, *, entry='GROUNDING.yaml'):
    """Validate a template-bound inactive physical map without reading files."""
    value = C.detached(value)
    C._mapping(value, ('version', 'physical'))
    C._require(type(value['version']) is int and value['version'] == 1, 'invalid_hypothesis_import_mapping')
    C.relative_path(entry)
    C._require(PurePosixPath(entry).name == entry, 'invalid_hypothesis_import_entry')
    directory = PurePosixPath(P.layout('/' + entry)['hypotheses'].lstrip('/'))
    C._require(isinstance(value['physical'], list), 'invalid_hypothesis_import_mapping')
    names, paths = [], []
    for item in value['physical']:
        C._mapping(item, ('name', 'path', 'sha256'))
        C.hypothesis_name(item['name'])
        C.relative_path(item['path'])
        path = PurePosixPath(item['path'])
        C._require(path.parent == directory and path.stem == item['name'] and path.suffix in ('.yaml', '.yml'),
                   'invalid_hypothesis_import_path', item['path'])
        C._text(item['sha256'], C.HEX)
        names.append(item['name'])
        paths.append(item['path'])
    C._require(names == sorted(set(names)) and len(paths) == len(set(paths)), 'duplicate_hypothesis_import')
    return value


def _normalized(hypotheses, entry):
    """Accept a named map or an explicit file sequence (which can expose collisions)."""
    if isinstance(hypotheses, dict):
        items = []
        for name, item in hypotheses.items():
            C._require(isinstance(item, dict), 'invalid_hypothesis_import_source')
            C._require('name' not in item or item['name'] == name, 'hypothesis_name_mismatch')
            items.append({**item, 'name': name})
    else:
        C._require(isinstance(hypotheses, list), 'invalid_hypothesis_import_source')
        items = hypotheses
    C._require(len(items) <= C.MAX_OBJECTS, 'history_limit')
    result, total = {}, 0
    for item in items:
        C._mapping(item, ('name', 'document', 'head', 'path', 'bytes'), ('profile', 'fields', 'error'))
        name, raw = item['name'], item['bytes']
        C.hypothesis_name(name)
        C._require(name not in result, 'duplicate_hypothesis_import', name)
        C._require(not item.get('error'), 'unreadable_hypothesis', name)
        C._require(type(raw) is bytes and len(raw) <= C.MAX_REQUEST_BYTES, 'history_limit')
        total += len(raw)
        C._require(total <= C.MAX_REQUEST_BYTES, 'history_limit')
        # Legacy aliases/duplicate keys are explicitly refused by the strict
        # import decoder rather than silently selecting one conflicting source.
        parsed = C.decode_document(raw)
        head = parsed.pop('hypothesis', None)
        C._require(head is None or isinstance(head, dict), 'invalid_hypothesis_head', name)
        head = head or {}
        C._require(isinstance(item['document'], dict) and isinstance(item['head'], dict)
                   and identity(parsed) == identity(item['document']) and identity(head) == identity(item['head']),
                   'hypothesis_source_mismatch', name)
        validate_mapping({'version': 1, 'physical': [{'name': name, 'path': item['path'],
                          'sha256': C.sha256(raw)}]}, entry=entry)
        try:
            cap = capabilities(parsed, profile=item.get('profile'))
        except CapabilityError as error:
            raise C.HistoryError('unsupported_hypothesis_profile', name + ': ' + str(error)) from error
        C._require(not cap.get('experimental_override'), 'hypothesis_profile_promotion_refused', name)
        result[name] = {**item, 'document': C.detached(parsed), 'head': C.detached(head),
                        'profile': cap['profile'], 'sha256': C.sha256(raw),
                        'profile_evidence': 'supplied_capture_profile' if 'profile' in item else 'original_document'}
    return result


def _roles(source, base_document):
    declared = source['document'].get('schema', {})
    C._require(isinstance(declared, dict), 'invalid_hypothesis_field_roles')
    if 'fields' in source:
        fields = C.detached(source['fields'])
        C._require(isinstance(fields, dict) and set(fields) == {'deps', 'snapshot', 'predicate'}
                   and all(isinstance(field, str) and field for field in fields.values())
                   and len(set(fields.values())) == 3, 'invalid_hypothesis_field_roles')
        C._require(all(role not in declared or declared[role] == field for role, field in fields.items()),
                   'hypothesis_field_role_mismatch')
        return fields
    # A sparse layer's names resolve in its captured base. Explicit layer schema
    # is retained for its original interpretation, not inferred from a body hash.
    layered = P.layered(P.Record(copy.deepcopy(base_document)), {'doc': source['document']})
    if declared:
        layered['schema'] = copy.deepcopy(declared)
    try:
        return _fields(layered)
    except (SnapshotError, SystemExit, ValueError) as error:
        raise C.HistoryError('ambiguous_hypothesis_field_roles', source['name']) from error


def prepare(base_objects, hypotheses, *, base_document, entry='GROUNDING.yaml', operation, recorded_at):
    """Return new named proposal objects, expected layers, and inactive-file mapping.

    Each source item supplies name/document/head/path/bytes, optionally an already
    captured named profile and deps/snapshot/predicate field map. Original pin IDs
    are not guessed from current values. `pins` stays empty and `pin_gaps` records
    unavailable or not-recorded history; separate locator observations bind what
    this import actually read from the base or this particular sparse layer.
    """
    C._text(operation)
    C._require(isinstance(recorded_at, str) and bool(recorded_at), 'invalid_recorded_time')
    C._require(isinstance(base_document, dict), 'invalid_document')
    base_document = C.detached(base_document, C.MAX_REQUEST_BYTES)
    base_objects = C.validate_closure(base_objects)
    sources = _normalized(hypotheses, entry)
    state = H.reduce(base_objects)
    known_base = entries(base_document)
    for subject, (collection, body) in known_base.items():
        current = state['subjects'].get(subject)
        C._require(current is not None and current['acceptance'] == 'accepted' and 'head' in current,
                   'hypothesis_import_base_mismatch', subject)
        obj = base_objects[current['head']]
        C._require(obj['authored']['collection'] == collection and identity(obj['body']) == identity(body),
                   'hypothesis_import_base_mismatch', subject)
    for subject, current in state['subjects'].items():
        C._require(current['acceptance'] != 'accepted' or subject in known_base,
                   'hypothesis_import_base_mismatch', subject)
    objects, groups, physical, diagnostics, collections = {}, {}, [], [], set()
    for name, source in sorted(sources.items()):
        document, head = source['document'], source['head']
        try:
            known = entries(document)
        except ValueError as error:
            raise C.HistoryError('ambiguous_hypothesis_entries', name) from error
        C._require(bool(known), 'empty_hypothesis_requires_group_anchor', name)
        fields = _roles(source, base_document)
        headers = {key: copy.deepcopy(value) for key, value in document.items() if key not in P.collections_of(document) or key == 'meta'}
        group = {'version': 1, 'name': name, 'head': copy.deepcopy(head)}
        group_versions = {}
        for subject, (collection, body) in sorted(known.items()):
            C._text(subject, C.SUBJECT)
            C._require(collection not in ('meta', 'schema', 'record', 'also'), 'invalid_hypothesis_collection')
            collections.add(collection)
            deps = body.get(fields['deps'], []) if isinstance(body, dict) else []
            C._require(isinstance(deps, list) and all(isinstance(dep, str) for dep in deps),
                       'unsupported_legacy_hypothesis_dependencies', subject)
            kind = 'judgment' if isinstance(body, dict) and fields['deps'] in body else 'reading'
            previous = [obj for obj in base_objects.values() if obj['subject'] == subject and obj['kind'] != 'act']
            C._require(all(obj['kind'] == kind for obj in previous), 'hypothesis_kind_conflict', subject)
            C._require(all(obj['kind'] == 'act' or obj['subject'] != subject or obj['kind'] == kind
                           for obj in objects.values()), 'hypothesis_kind_conflict', subject)
            gaps, observations = {}, {}
            for dependency in deps:
                C._text(dependency, C.SUBJECT)
                if dependency in known:
                    dep_collection, dep_body = known[dependency]
                    observations[dependency] = {'kind': 'hypothesis_entry', 'name': name,
                        'path': source['path'], 'sha256': source['sha256'],
                        'collection': dep_collection, 'subject': dependency, 'body_digest': identity(dep_body)}
                    gaps[dependency] = 'not_recorded'
                elif dependency in known_base:
                    selected = state['subjects'][dependency]['head']
                    observations[dependency] = {'kind': 'base_version', 'subject': dependency,
                                               'version': selected, 'object_digest': identity(base_objects[selected])}
                    gaps[dependency] = 'not_recorded'
                else:
                    observations[dependency] = {'kind': 'unavailable', 'subject': dependency}
                    gaps[dependency] = 'unavailable'
                diagnostics.append({'code': 'historical_pin_' + gaps[dependency], 'hypothesis': name,
                                    'subject': subject, 'dependency': dependency})
            locator = {'version': 1, 'kind': KIND, 'path': source['path'], 'sha256': source['sha256'],
                'collection': collection, 'subject': subject,
                'document_headers': headers,
                'original': {'writer': None, 'operation': None, 'recorded_at': None, 'pin_versions': None},
                'import': {'operation': operation, 'recorded_at': recorded_at,
                           'observations': observations},
                'profile': {'name': source['profile'],
                            'evidence': source['profile_evidence']}}
            # Only the imported base is a causal predecessor. Peer hypotheses
            # remain independent alternatives, never silently superseding one another.
            saw = sorted(version for version, obj in base_objects.items() if obj['subject'] == subject)
            claim = C.make_object(subject=subject, kind=kind, by=None, on=recorded_at, operation=operation,
                body=body, saw=saw, pins={}, pin_gaps=gaps,
                authored={'collection': collection, 'fields': fields, 'profile': source['profile'],
                          'hypothesis': group, 'locator': locator})
            propose = C.make_object(subject=subject, kind='act', by=None, on=recorded_at, operation=operation,
                saw=sorted([*saw, claim['id']]), body={'act': 'propose', 'of': claim['id'], 'over': [],
                    'because': 'imported physical hypothesis; no base acceptance is recorded'})
            for obj in (claim, propose):
                C._require(obj['id'] not in base_objects and obj['id'] not in objects, 'duplicate_hypothesis_import')
                objects[obj['id']] = obj
            group_versions[subject] = claim['id']
        groups[name] = {'document': copy.deepcopy(document), 'head': copy.deepcopy(head),
                        'profile': source['profile'], 'fields': fields, 'versions': group_versions}
        physical.append({'name': name, 'path': source['path'], 'sha256': source['sha256']})
    C.validate_closure({**base_objects, **objects})
    result_state = H.reduce({**base_objects, **objects})
    for name, group in groups.items():
        C._require(all(version in result_state['subjects'][subject]['proposals']
                       for subject, version in group['versions'].items()), 'hypothesis_import_disposition_mismatch', name)
    for subject, original in state['subjects'].items():
        C._require(result_state['subjects'][subject]['heads'] == original['heads'], 'hypothesis_import_changed_base', subject)
    return {'version': 1, 'objects': objects, 'physical': validate_mapping({'version': 1, 'physical': physical}, entry=entry),
            'groups': groups, 'collections': sorted(collections), 'diagnostics': diagnostics,
            'historical_support_complete': not diagnostics}
