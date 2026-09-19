"""Pure adaptation of committed claims and a bounded, reader-owned projection.

The caller establishes committed membership and reduces acceptance. This module
neither reads a store nor infers acceptance, temporal precedence or capabilities.
The optional document supplies only retained record headers, never current nodes.
"""
import copy
import heapq

from . import history_contract as C
from .pending_grounding import identity
from .reasoning.contract import (MAX_REQUEST_BYTES, OperationalLimit, OutputBudget,
                                 capabilities, digest, validate_value)


_ROLES = ('deps', 'snapshot', 'predicate')
_HEADERS = {'meta', 'schema', 'record', 'also'}
MAX_TEMPORAL_OBSERVATIONS = 64
MAX_TEMPORAL_FRONTIER_VISITS = 1_000_000


def _require(condition, code, detail=''):
    if not condition:
        raise C.HistoryError(code, detail)


def _finding(code, subject, version, detail):
    return {'code': code, 'subject': subject, 'object_id': version, 'detail': detail}


def _mapping(obj):
    authored = obj.get('authored')
    _require(isinstance(authored, dict), 'unresolved_authored_mapping', obj['id'])
    fields = authored['fields']
    _require(all(role in fields for role in _ROLES)
             and len({fields[role] for role in _ROLES}) == len(_ROLES),
             'invalid_authored_mapping', obj['id'])
    _require(authored['collection'] not in _HEADERS, 'invalid_authored_mapping', obj['id'])
    return authored


def _adapt_body(obj):
    authored = _mapping(obj)
    body = copy.deepcopy(obj['body'])
    if not isinstance(body, dict):
        return body
    field = authored['fields']['deps']
    pins = obj['pins']
    gaps = obj.get('pin_gaps', {})
    names = set(pins) | set(gaps)
    if field in body:
        deps = body[field]
        if isinstance(deps, dict):
            _require(bool(gaps) or identity(deps) == identity(pins), 'pin_dependency_mismatch', obj['id'])
            body[field] = sorted(names)
        else:
            _require(isinstance(deps, list) and all(isinstance(dep, str) for dep in deps)
                     and set(deps) == names, 'pin_dependency_mismatch', obj['id'])
    elif pins or obj['kind'] == 'judgment':
        body[field] = sorted(pins)
    return body


def capture_history(objects, projection, *, document=None):
    """Return C.CapturedHistory from validated committed objects and reduction.

    ``document`` must be an immutable C.document_template, optionally carrying
    the matching generated baseline. Its capability declaration is retained verbatim; none is synthesized. Every
    selected claim must have a resolved authored mapping, compatible field roles
    and profile. Incomplete evidence stays incomplete, and an accepted subject
    with disagreeing heads refuses instead of choosing a scalar arbitrarily.
    """
    projection = C.validate_projection(projection)
    _require(isinstance(objects, dict) and len(objects) <= C.MAX_OBJECTS, 'history_limit')
    validated = {}
    for version, obj in objects.items():
        obj = C.validate_object(obj)
        _require(version == obj['id'], 'identity_mismatch')
        validated[version] = obj
    document = C.detached({} if document is None else document, MAX_REQUEST_BYTES)
    _require(isinstance(document, dict), 'invalid_document')
    meta = document.setdefault('meta', {})
    _require(isinstance(meta, dict), 'invalid_document')
    if 'history' in meta:
        _require(identity(meta['history']) == identity(projection['baseline']), 'baseline_mismatch')
    meta.pop('history', None)
    _require(identity(C.document_template(document)) == identity(document),
             'invalid_document', 'only an immutable document template may be supplied')
    meta['history'] = copy.deepcopy(projection['baseline'])
    schema = document.setdefault('schema', {})
    _require(isinstance(schema, dict), 'invalid_authored_mapping')
    chosen_profile = None
    projection_budget = OutputBudget(C.MAX_PROJECTION_BYTES, 'history_limit')
    projection_budget.add(projection)

    def incomplete(code, subject, version, detail):
        _require(not projection['integrity']['complete'], code, detail)
        finding = _finding(code, subject, version, detail)
        if finding not in projection['integrity']['findings']:
            projection['integrity']['findings'].append(finding)
        projection['coverage']['complete'] = False

    def witness(subject, version):
        existing = projection['pins'].get(version)
        obj = validated.get(version)
        if obj is not None:
            _require(obj['subject'] == subject and obj['kind'] != 'act', 'reference_mismatch', version)
            for finding in C.pin_gap_findings(obj):
                # These gaps are explicit immutable evidence, so expose missing
                # historical support without changing acceptance or original seen.
                projection['integrity']['complete'] = False
                projection['coverage']['complete'] = False
                if finding not in projection['integrity']['findings']:
                    projection['integrity']['findings'].append(finding)
        if existing is not None:
            _require(existing['subject'] == subject, 'reference_mismatch', version)
            if existing['status'] == 'recorded':
                _require(obj is not None and identity(existing['object']) == identity(obj),
                         'pin_object_mismatch', version)
            else:
                incomplete('pin_' + existing['status'], subject, version, 'pin evidence is unavailable')
            return existing
        result = {'subject': subject, 'version': version,
                  'status': 'recorded' if obj is not None else 'unavailable', 'object': obj}
        if obj is None:
            incomplete('missing_pin', subject, version, 'pin was not captured')
        # Charge before retaining another potentially large immutable body. The
        # final contract validation also accounts for findings added above.
        projection_budget.add({version: result})
        projection['pins'][version] = result
        return result

    # Existing unavailable/corrupt witnesses remain explicit even if not required
    # by the selected current view. A complete claim cannot hide them.
    for version, item in list(projection['pins'].items()):
        witness(item['subject'], version)
    for subject, state in sorted(projection['subjects'].items()):
        heads = []
        for version in state['heads']:
            item = witness(subject, version)
            if item['status'] == 'recorded':
                heads.append(item['object'])
        for version in state['open_acts']:
            act = validated.get(version)
            if act is None:
                incomplete('missing_act', subject, version, 'open act was not captured')
            else:
                _require(act['subject'] == subject and act['kind'] == 'act', 'reference_mismatch', version)
        # Alternatives are immutable witnesses in context, not computational nodes.
        if state['acceptance'] != 'accepted':
            continue
        if not heads or len(heads) != len(state['heads']):
            incomplete('missing_accepted_head', subject, '', 'accepted head evidence is unavailable')
            continue
        representations = {identity(C.claim_meaning(obj))
                           for obj in heads}
        _require(len(representations) == 1, 'ambiguous_accepted_selection', subject)
        for obj in heads:
            C.require_interpretable_claim(obj)
        selected = min(heads, key=lambda obj: obj['id'])
        authored = _mapping(selected)
        _require(chosen_profile in (None, authored['profile']), 'incompatible_authored_profiles', subject)
        chosen_profile = authored['profile']
        for role, field in authored['fields'].items():
            _require(role not in schema or schema[role] == field, 'incompatible_field_roles', subject)
            schema[role] = field
        for dep, version in selected['pins'].items():
            witness(dep, version)
        collection_body = document.setdefault(authored['collection'], {})
        _require(isinstance(collection_body, dict), 'unresolved_mapping', authored['collection'])
        collection_body[subject] = _adapt_body(selected)
    declaration = meta.get('reasoning')
    if declaration is not None:
        _require(isinstance(declaration, dict) and
                 chosen_profile in (None, declaration.get('profile')), 'incompatible_authored_profiles')
    if chosen_profile == 'core/v1':
        _require(declaration is not None, 'missing_reasoning_declaration')
        _require(capabilities(document)['profile'] == chosen_profile, 'incompatible_authored_profiles')
    # No global capability declaration is invented from a claim's profile. The
    # immutable authored profile remains available in each retained head witness.
    return C.CapturedHistory(document, projection)


def _temporal_projection(captured, objects, required):
    """Retain bounded exact replay inputs; truth is checked by the assessor."""
    if C.TEMPORAL_APPLICABILITY not in required:
        return None
    from . import history_store as H, history_transaction as T
    manifests = {operation: C.validate_commit(C.decode_document(raw))
                 for operation, raw in captured.commits.items()}
    children = {operation: [] for operation in manifests}
    remaining = {operation: len(manifest['parents']) for operation, manifest in manifests.items()}
    for child, manifest in manifests.items():
        for parent in manifest['parents']:
            _require(parent in manifests, 'incomplete_commit', parent)
            children[parent].append(child)
    temporal_lineage = {}
    ready = [operation for operation, count in remaining.items() if count == 0]
    heapq.heapify(ready)
    while ready:
        operation = heapq.heappop(ready)
        manifest = manifests[operation]
        temporal_lineage[operation] = (
            C.TEMPORAL_APPLICABILITY in manifest.get('requires', [])
            or any(temporal_lineage[parent] for parent in manifest['parents']))
        for child in children[operation]:
            remaining[child] -= 1
            if remaining[child] == 0:
                heapq.heappush(ready, child)
    _require(len(temporal_lineage) == len(manifests), 'cyclic_commits')
    closure_cache = {}
    frontier_visits = [0]

    def causal_ids(operation, include_self):
        key = operation, include_self
        if key in closure_cache:
            return closure_cache[key]
        pending = [operation] if include_self else list(manifests[operation]['parents'])
        seen, ids = set(), set()
        while pending:
            current = pending.pop()
            if current in seen:
                continue
            _require(current in manifests, 'incomplete_commit', current)
            frontier_visits[0] += 1
            if frontier_visits[0] > MAX_TEMPORAL_FRONTIER_VISITS:
                raise OperationalLimit('temporal_frontier_limit')
            seen.add(current)
            ids.update(item['id'] for item in manifests[current]['objects'])
            pending.extend(manifests[current]['parents'])
            _require(len(seen) <= C.MAX_OBJECTS, 'history_limit', 'temporal causal frontier')
        closure_cache[key] = ids
        return ids

    def causal_operations(operation, include_self):
        pending = [operation] if include_self else list(manifests[operation]['parents'])
        seen = set()
        while pending:
            current = pending.pop()
            if current in seen:
                continue
            _require(current in manifests, 'incomplete_commit', current)
            seen.add(current)
            pending.extend(manifests[current]['parents'])
            _require(len(seen) <= C.MAX_OBJECTS, 'history_limit', 'temporal causal frontier')
        return seen

    def accepted_at(operation, phase):
        selected = {vid: objects[vid] for vid in causal_ids(operation, phase == 'after')}
        state = H.reduce(selected, captured.state['rules'])
        return {subject: item['head'] for subject, item in state['subjects'].items()
                if item.get('acceptance') == 'accepted' and 'head' in item}

    def reconstructed_snapshot(operation, phase, frontier):
        operations = causal_operations(operation, phase == 'after')
        commits = {name: captured.commits[name] for name in operations}
        document = H.Store._template(commits)
        schema = document.setdefault('schema', {})
        _require(isinstance(schema, dict), 'invalid_authored_mapping')
        profile = None
        for subject, version in sorted(frontier.items()):
            obj = objects[version]
            authored = _mapping(obj)
            _require(profile in (None, authored['profile']), 'incompatible_authored_profiles')
            profile = authored['profile']
            for role in _ROLES:
                _require(role not in schema or schema[role] == authored['fields'][role],
                         'incompatible_field_roles', subject)
                schema[role] = authored['fields'][role]
            collection = document.setdefault(authored['collection'], {})
            _require(isinstance(collection, dict) and subject not in collection,
                     'duplicate_projection', subject)
            collection[subject] = _adapt_body(obj)
        _require(profile == 'core/v1', 'incompatible_authored_profiles')
        document.setdefault('meta', {}).pop('history', None)
        from .reasoning.snapshot import Snapshot
        return Snapshot.from_data(document, as_of=None)
    temporal_claims = {vid: obj for vid, obj in objects.items()
                       if obj.get('id_scheme') == C.ID_SCHEME
                       and isinstance(obj.get('body'), dict)
                       and obj['body'].get('temporal') is not None}
    observations, findings = [], []
    temporal_budget = OutputBudget(C.MAX_PROJECTION_BYTES // 2, 'temporal_history_limit')
    temporal_budget.add({'version': 1, 'observations': [], 'complete': True, 'findings': []})
    seen_claims = set()
    truncated = False
    missing_contexts = 0
    missing_claims = {}
    for operation, raw in sorted(captured.commits.items()):
        manifest = manifests[operation]
        receipt = T.validate_receipt(manifest['receipt'])
        for phase in ('before', 'after'):
            evidence = receipt[phase]
            replay = evidence.get('temporal_replay') if isinstance(evidence, dict) else None
            if replay is None:
                lineage_active = (temporal_lineage[operation] if phase == 'after' else
                                  any(temporal_lineage[parent] for parent in manifest['parents']))
                if not lineage_active:
                    continue
                try:
                    frontier = accepted_at(operation, phase)
                except OperationalLimit:
                    truncated = True
                    continue
                expected = [(subject, version) for subject, version in frontier.items()
                            if version in temporal_claims]
                if expected:
                    try:
                        snapshot = reconstructed_snapshot(operation, phase, frontier)
                        replay = {'version': 1, 'snapshot': snapshot.to_json(),
                                  'claims': dict(expected)}
                        assessment = None
                        evidence_kind = 'reconstructed_committed_world'
                    except (ValueError, TypeError, KeyError, OperationalLimit, C.HistoryError):
                        missing_contexts += 1
                        for key in expected:
                            missing_claims[key] = missing_claims.get(key, 0) + 1
                        continue
                else:
                    continue
            else:
                assessment = evidence.get('assessment')
                evidence_kind = 'recorded_receipt'
            _require(isinstance(replay, dict) and set(replay) == {'version', 'snapshot', 'claims'}
                     and replay['version'] == 1 and isinstance(replay['snapshot'], str)
                     and isinstance(replay['claims'], dict),
                     'invalid_temporal_replay', operation)
            _require(assessment is None or isinstance(assessment, dict),
                     'invalid_temporal_replay', operation)
            try:
                frontier = accepted_at(operation, phase)
            except OperationalLimit:
                truncated = True
                continue
            expected_claims = {subject: version for subject, version in frontier.items()
                               if version in temporal_claims}
            _require(replay['claims'] == expected_claims,
                     'temporal_causal_frontier_mismatch', operation + ':' + phase)
            from .reasoning.snapshot import Snapshot
            try:
                snapshot = Snapshot.from_json(replay['snapshot'])
                snapshot_nodes = snapshot.to_data()['nodes']
            except (ValueError, TypeError, KeyError) as error:
                raise C.HistoryError('invalid_temporal_replay', operation) from error
            expected_nodes = {subject: objects[version] for subject, version in frontier.items()}
            _require(set(snapshot_nodes) == set(expected_nodes),
                     'temporal_causal_frontier_mismatch', operation + ':' + phase)
            for subject, obj in expected_nodes.items():
                node = snapshot_nodes[subject]
                _require(digest(node['body']) == digest(_adapt_body(obj))
                         and node['collection'] == obj['authored']['collection']
                         and node['fields'] == {role: obj['authored']['fields'][role]
                                                for role in _ROLES},
                         'temporal_causal_frontier_mismatch', subject)
            claims = []
            for subject, version in sorted(replay['claims'].items()):
                _require(isinstance(subject, str) and isinstance(version, str),
                         'invalid_temporal_replay', operation)
                obj = temporal_claims.get(version)
                _require(obj is not None and obj['subject'] == subject,
                         'temporal_claim_mismatch', version)
                if assessment is not None:
                    node = assessment.get('nodes', {}).get(subject)
                    _require(isinstance(node, dict) and digest(node.get('body')) == digest(obj['body']),
                             'temporal_claim_mismatch', version)
                field = obj['authored']['fields']['predicate']
                claims.append({
                    'subject': subject, 'claim_id': version,
                    'applicability': obj['body']['temporal']['applicability'],
                    'predicate_digest': digest(obj['body'].get(field)),
                    'anchors': {'on': obj.get('on'), 'at': obj.get('at'),
                                'applies': obj.get('applies')},
                })
                seen_claims.add(version)
            if not claims:
                continue
            observation = {'operation': operation, 'phase': phase, 'evidence_kind': evidence_kind,
                           'snapshot': replay['snapshot'], 'assessment': copy.deepcopy(assessment),
                           'claims': claims}
            if len(observations) >= MAX_TEMPORAL_OBSERVATIONS:
                truncated = True
                continue
            try:
                temporal_budget.add(observation)
            except OperationalLimit:
                truncated = True
                continue
            observations.append(observation)
    if truncated:
        findings.append(_finding('temporal_history_limit', '', '',
                                 'retained temporal observations exceed replay bounds'))
    if missing_contexts:
        for (subject, version), count in sorted(missing_claims.items())[:63]:
            findings.append(_finding('temporal_replay_unavailable', subject, version,
                                     f'{count} committed contexts lack exact Snapshot replay evidence'))
        if not missing_claims or len(missing_claims) > 63:
            findings.append(_finding('temporal_replay_unavailable', '', '',
                                     f'{missing_contexts} committed contexts lack exact Snapshot replay evidence'))
    accepted_temporal = {claim['claim_id'] for observation in observations
                         for claim in observation['claims']} | {version for _, version in missing_claims}
    unseen = [(version, temporal_claims[version]) for version in sorted(accepted_temporal)
              if version not in seen_claims and (temporal_claims[version]['subject'], version)
              not in missing_claims]
    for version, obj in unseen[:63]:
        findings.append(_finding('temporal_replay_unavailable', obj['subject'], version,
                                 'claim has no exact retained Snapshot context'))
    if len(unseen) > 63:
        findings.append(_finding('temporal_replay_unavailable', '', '',
                                 f'{len(unseen)} claims have no exact retained Snapshot context'))
    return {'version': 1, 'complete': not findings,
            'observations': observations, 'findings': findings}



def from_store_capture(captured):
    """Adapt detached Store.capture evidence; no mutable view supplies headers.

    The semantic closure binds only committed manifests and reachable objects.
    Orphan staging bytes remain the source capture's private concurrency evidence.
    """
    from .history_store import Store
    objects = captured.objects
    subjects = {subject: {'acceptance': state['acceptance'],
                          'heads': sorted(state['heads']),
                          'open_acts': captured.baseline['open_acts'].get(subject, [])}
                for subject, state in sorted(captured.state['subjects'].items())}
    reviews = {subject: [] for subject in subjects}
    for version, obj in sorted(objects.items()):
        if obj['kind'] == 'act' and obj['body']['act'] == 'review' \
                and obj['body']['of'] in (subjects[obj['subject']]['heads'] +
                                          captured.state['subjects'][obj['subject']]['proposals']):
            reviews[obj['subject']].append(obj)
    dispositions = {subject: {'marks': state['marks'], 'proposals': sorted(state['proposals']),
                              'contested_claims': sorted(state['disputed_acts']),
                              'reviews': reviews[subject], 'implied': state['implied']}
                    for subject, state in sorted(captured.state['subjects'].items())}
    versions = set()
    for subject, state in captured.state['subjects'].items():
        versions.update(state['heads'])
        versions.update(state['proposals'])
        for review in reviews[subject]:
            versions.update(review['body'].get('read', {}).values())
    versions.update(version for version, obj in objects.items()
                    if obj.get('id_scheme') == C.ID_SCHEME
                    and isinstance(obj.get('body'), dict)
                    and obj['body'].get('temporal') is not None)
    pins = {version: {'subject': objects[version]['subject'], 'version': version,
                      'status': 'recorded', 'object': objects[version]}
            for version in sorted(versions)}
    closure = {'authority': captured.marker,
               'commits': {operation: C.sha256(raw) for operation, raw in sorted(captured.commits.items())},
               'objects': {version: {'subject': obj['subject'],
                                    'sha256': C.sha256(captured.object_bytes[(obj['subject'], version)])}
                           for version, obj in sorted(objects.items())}}
    projection = {'projection_version': 1, 'authority': captured.marker,
                  'baseline': captured.baseline,
                  'identity_schemes': sorted({C.ID_SCHEME if 'id_scheme' in obj else C.LEGACY_SCHEME
                                              for obj in objects.values()}),
                  'rules': captured.state['rules'], 'rules_digest': identity(captured.state['rules']),
                  'closure_digest': identity(closure),
                  'coverage': {'scope': 'all', 'subjects': sorted(subjects), 'complete': True},
                  'subjects': subjects, 'pins': pins, 'dispositions': dispositions,
                  'integrity': {'complete': True, 'findings': []}}
    required = {capability for raw in captured.commits.values()
                for capability in C.validate_commit(C.decode_document(raw)).get('requires', [])}
    if any(obj['kind'] == 'act' and obj['body']['act'] in ('propose', 'retire') for obj in objects.values()):
        required.add(C.EXPLICIT_ROOT_DISPOSITION)
    if required:
        projection['requires'] = sorted(required)
    temporal = _temporal_projection(captured, objects, required)
    if temporal is not None:
        projection['temporal'] = temporal
        if not temporal['complete']:
            projection['coverage']['complete'] = False
            projection['integrity']['complete'] = False
            projection['integrity']['findings'].extend(temporal['findings'])
    gap_findings = [finding for version in sorted(versions)
                    for finding in C.pin_gap_findings(objects[version])]
    if gap_findings:
        projection['integrity']['complete'] = False
        projection['integrity']['findings'].extend(
            finding for finding in gap_findings
            if finding not in projection['integrity']['findings'])
        projection['coverage']['complete'] = False
    template = Store._template(captured.commits)
    origin = template.get('meta', {}).get('history_subset')
    if origin is not None:
        origin = C.validate_subset_origin(origin)
        C._require(set(origin['subjects']) == set(subjects), 'invalid_subset_coverage')
        C._require(len(captured.commits) == 1, 'invalid_subset_commits')
        commit = C.decode_document(next(iter(captured.commits.values())))
        C._require(commit['operation'] == origin['operation'] and not commit['parents'] and
                   identity(commit['receipt'].get('after')) == identity({'history_subset': origin}),
                   'invalid_subset_receipt')
        for subject, evidence in origin['subjects'].items():
            inventory = {vid: C.sha256(captured.object_bytes[(subject, vid)])
                         for vid, obj in objects.items() if obj['subject'] == subject}
            C._require(evidence['objects_digest'] == identity(inventory) and
                       evidence['reduction_digest'] == identity(captured.state['subjects'][subject]),
                       'invalid_subset_receipt')
        projection['coverage']['scope'] = 'selected'
        projection['origin'] = origin
        for subject in subjects:
            projection['dispositions'][subject]['source_state'] = origin['subjects'][subject]['source_state']
    return capture_history(objects, projection, document=template)


def _literal(value):
    """Decode a stored scalar without evaluating an expression or coercing text."""
    from . import expressions as E
    if isinstance(value, dict) and 'type' in value:
        return copy.deepcopy(validate_value(value))
    if type(value) is bool:
        return {'type': 'boolean', 'value': value}
    if value is None:
        return {'type': 'null'}
    if isinstance(value, str):
        return {'type': 'text', 'value': value}
    number = E.number(value)
    if number is not None:
        return {'type': 'number', 'numerator': str(number.numerator), 'denominator': str(number.denominator)}
    raise ValueError('unsupported stored value')


def pin_review_evidence(projection, version, *, subject=None):
    """Decode one recorded pin for future consumers, without evaluating its body.

    This internal result is not assessment-v3. ``status`` concerns the immutable
    witness; value/basis availability are independent. It never inserts ``seen``
    or computes a missing historical result. Callers retain original seen as a
    separate evidence source and choose consumer policy outside this module.
    """
    projection = C.validate_projection(projection)
    _require(isinstance(version, str) and C.OBJECT_ID.fullmatch(version), 'invalid_identifier')
    witness = projection['pins'].get(version)
    if witness is not None:
        _require(subject is None or subject == witness['subject'], 'reference_mismatch', version)
        subject = witness['subject']
    result = {'status': 'unavailable', 'evidence_kind': 'version_pin', 'subject': subject,
              'version': version, 'body': None, 'profile': None,
              'value_status': 'unavailable', 'value': None,
              'basis_status': 'not_recorded', 'basis': None,
              'findings': copy.deepcopy([item for item in projection['integrity']['findings']
                                         if item['object_id'] == version])}
    if witness is None or witness['status'] != 'recorded':
        code = 'missing_pin' if witness is None else 'pin_' + witness['status']
        result['findings'].append(_finding(code, subject or '', version, 'pin evidence is unavailable'))
        return result
    obj = witness['object']
    result.update(status='recorded', body=copy.deepcopy(obj['body']))
    authored = obj.get('authored')
    if authored is None:
        result['findings'].append(_finding('unresolved_authored_mapping', subject, version,
                                          'recorded profile and value role are unavailable'))
        return result
    try:
        C.require_interpretable_claim(obj)
    except C.HistoryError as error:
        result['basis_status'] = 'unavailable'
        result['findings'].append(_finding(error.code, subject, version, 'original condition profile is unknown'))
        return result
    result['profile'] = authored['profile']
    body, fields = obj['body'], authored['fields']
    if not isinstance(body, dict):
        try:
            result.update(value_status='recorded', value=_literal(body))
        except (ValueError, TypeError, RecursionError):
            result['findings'].append(_finding('unsupported_history_value', subject, version,
                                              'stored scalar is outside the computation value profile'))
        return result
    field = fields.get('value')
    # Only an explicitly stored computation is a historical formula result.
    stored = body if 'computed' in body else body.get(field) if field else None
    computed = stored.get('computed') if isinstance(stored, dict) else None
    if isinstance(stored, dict) and 'computed' in stored:
        from .reasoning.evaluate import compare_basis
        try:
            if not isinstance(computed, dict) or type(computed.get('version')) is not int \
                    or computed['version'] != 2 or authored['profile'] != 'core/v1' \
                    or not {'version', 'value', 'basis'} <= set(computed) \
                    or set(computed) - {'version', 'value', 'basis', 'rule'}:
                raise ValueError('invalid recorded computation or profile')
            typed = validate_value(computed['value'])
            if compare_basis(computed['basis'], computed['basis']) != 'same':
                raise ValueError('invalid recorded basis')
            result.update(value_status='recorded', value=copy.deepcopy(typed),
                          basis_status='recorded', basis=copy.deepcopy(computed['basis']))
        except (ValueError, TypeError, RecursionError):
            result['basis_status'] = 'unavailable'
            result['findings'].append(_finding('invalid_history', subject, version,
                                              'stored computation cannot be decoded under its profile'))
        return result
    if 'rule' in body or obj['kind'] != 'reading' or field is None or field not in body:
        return result
    try:
        result.update(value_status='recorded', value=_literal(body[field]))
    except (ValueError, TypeError, RecursionError):
        result['findings'].append(_finding('unsupported_history_value', subject, version,
                                          'stored value is not a supported typed scalar'))
    return result
