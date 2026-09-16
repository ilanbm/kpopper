"""Pure adaptation of committed claims and a bounded, reader-owned projection.

The caller establishes committed membership and reduces acceptance. This module
neither reads a store nor infers acceptance, temporal precedence or capabilities.
The optional document supplies only retained record headers, never current nodes.
"""
import copy

from . import history_contract as C
from .pending_grounding import identity
from .reasoning.contract import MAX_REQUEST_BYTES, OutputBudget, capabilities, validate_value


_ROLES = ('deps', 'snapshot', 'predicate')
_HEADERS = {'meta', 'schema', 'record', 'also'}


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
    field = authored['fields']['deps']
    pins = obj['pins']
    if field in body:
        deps = body[field]
        if isinstance(deps, dict):
            _require(identity(deps) == identity(pins), 'pin_dependency_mismatch', obj['id'])
            body[field] = sorted(pins)
        else:
            _require(isinstance(deps, list) and all(isinstance(dep, str) for dep in deps)
                     and set(deps) == set(pins), 'pin_dependency_mismatch', obj['id'])
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
        representations = {identity({key: obj.get(key) for key in ('kind', 'body', 'authored', 'pins')})
                           for obj in heads}
        _require(len(representations) == 1, 'ambiguous_accepted_selection', subject)
        selected = min(heads, key=lambda obj: obj['id'])
        authored = _mapping(selected)
        _require(chosen_profile in (None, authored['profile']), 'incompatible_authored_profiles', subject)
        chosen_profile = authored['profile']
        for role, field in authored['fields'].items():
            _require(role not in schema or schema[role] == field, 'incompatible_field_roles', subject)
            schema[role] = field
        for dep, version in selected['pins'].items():
            witness(dep, version)
        document.setdefault(authored['collection'], {})[subject] = _adapt_body(selected)
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
    result['profile'] = authored['profile']
    body, fields = obj['body'], authored['fields']
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
