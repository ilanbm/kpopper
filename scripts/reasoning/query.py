"""Pure query/v1 authored IR and canonical KP4 data helpers.

This module deliberately contains no query evaluator.  It validates and lowers
authored row expressions, adapts a detached ``ScopeCapture`` into wire rows,
constructs a bound request/basis template, and validates the structure of a KR4
response.  Meaning remains owned by the packaged query kernel.
"""
import copy
import datetime
from fractions import Fraction
import json
import math
import re

from .contract import PROFILE, digest, validate_value
from .transport import DIAGNOSTICS_V3, NUMBER


MODULE = 'query/v1'
PROTOCOL = 'KP4'
RESOURCE_VERSION = 'resources/v4'
MAX_BYTES = 16 * 1024 * 1024
MAX_RESPONSE_BYTES = 64 * 1024 * 1024
HEX64 = re.compile(r'[0-9a-f]{64}\Z', re.ASCII)
INTEGER = re.compile(r'0|-?[1-9][0-9]*\Z', re.ASCII)
POSITIVE = re.compile(r'[1-9][0-9]*\Z', re.ASCII)
NUMBER_PARTS = re.compile(
    r'(?P<sign>-?)(?P<integer>0|[1-9][0-9]*)'
    r'(?:\.(?P<fraction>[0-9]+))?(?:[eE](?P<exponent>[+-]?[0-9]+))?\Z', re.ASCII)

RESOURCE_MAXIMA = {
    'steps': 1_000_000,
    'depth': 128,
    'digits': 256,
    'value_nodes': 10_000,
    'value_depth': 128,
    'value_bytes': 16_777_216,
    'candidates': 10_000,
    'field_reads': 100_000,
}

OP_KEYS = {
    'filter': frozenset(('version', 'scope', 'op', 'where')),
    'project': frozenset(('version', 'scope', 'op', 'value')),
    'select': frozenset(('version', 'scope', 'op', 'where', 'value')),
    'count': frozenset(('version', 'scope', 'op', 'where')),
    'sum': None,  # one of two exact shapes, checked below
    'all': frozenset(('version', 'scope', 'op', 'where')),
    'any': frozenset(('version', 'scope', 'op', 'where')),
}
ARITHMETIC = frozenset(('add', 'sub', 'mul', 'div', 'eq', 'ne', 'lt', 'le', 'gt', 'ge'))
BOOLEAN = frozenset(('and', 'or', 'not'))
UNAVAILABLE_REASONS = frozenset(('unsupported_type', 'formula_value', 'invalid_value'))
QUERY_DIAGNOSTICS = frozenset((
    'missing_column', 'contested_column', 'column_type_error', 'where_unknown',
    'where_error', 'value_error', 'incomplete_history_scope', 'candidate_limit',
    'field_read_limit', 'value_limit', 'digit_limit', 'step_limit',
    'malformed_wire', 'invalid_response',
))
DIAGNOSTIC_CODES = DIAGNOSTICS_V3 | QUERY_DIAGNOSTICS
QUERY_COUNT_KEYS = (
    'definite_match_count', 'error_count', 'input_count',
    'unknown_membership_count', 'unknown_value_count',
)
COST_KEYS = (
    'candidates', 'evaluated_field_reads', 'field_reads',
    'node_evaluations', 'preflight_steps', 'steps',
)


def _identifier(value, label='identifier'):
    if not isinstance(value, str) or not value or len(value) > 500:
        raise ValueError('invalid ' + label)
    _unicode_scalar(value)
    return value


def _unicode_scalar(value):
    if not isinstance(value, str) or any(0xD800 <= ord(char) <= 0xDFFF for char in value):
        raise ValueError('text contains a non-Unicode-scalar value')
    return value


def _rational(value):
    if not isinstance(value, dict) or set(value) != {'type', 'numerator', 'denominator'} \
            or value.get('type') != 'number':
        raise ValueError('invalid exact numeric value')
    numerator, denominator = value['numerator'], value['denominator']
    if not isinstance(numerator, str) or not isinstance(denominator, str) \
            or len(numerator) > 4097 or len(denominator) > 4096 \
            or not INTEGER.fullmatch(numerator) or not POSITIVE.fullmatch(denominator) \
            or math.gcd(abs(int(numerator)), int(denominator)) != 1:
        raise ValueError('noncanonical exact numeric value')
    return value


def _typed_value(value, depth=0, active=None):
    """Strict finite typed algebra, including reduced positive rationals."""
    if depth > RESOURCE_MAXIMA['value_depth'] or not isinstance(value, dict):
        raise ValueError('invalid typed value')
    active = set() if active is None else active
    if id(value) in active:
        raise ValueError('cyclic typed value')
    active.add(id(value))
    try:
        kind = value.get('type')
        if kind == 'number':
            _rational(value)
        elif kind == 'boolean' and set(value) == {'type', 'value'} and type(value['value']) is bool:
            pass
        elif kind == 'text' and set(value) == {'type', 'value'} and isinstance(value['value'], str):
            _unicode_scalar(value['value'])
        elif kind == 'null' and set(value) == {'type'}:
            pass
        elif kind == 'list' and set(value) == {'type', 'items'} and isinstance(value['items'], list) \
                and len(value['items']) <= RESOURCE_MAXIMA['value_nodes']:
            for item in value['items']:
                _typed_value(item, depth + 1, active)
        elif kind == 'record' and set(value) == {'type', 'fields'} and isinstance(value['fields'], dict) \
                and len(value['fields']) <= RESOURCE_MAXIMA['value_nodes']:
            for key in value['fields']:
                _unicode_scalar(key)
            for item in value['fields'].values():
                _typed_value(item, depth + 1, active)
        else:
            raise ValueError('invalid typed value shape')
    finally:
        active.remove(id(value))
    # Keep the common validator in the loop so query values cannot drift from
    # generic consumer expectations.  The checks above deliberately strengthen
    # its rational and Unicode requirements.
    validate_value(value)
    return value


def _json_value(value, active=None, depth=0):
    if depth > 256:
        raise ValueError('JSON nesting exceeds canonical bounds')
    if value is None or type(value) is bool or type(value) is int:
        return
    if isinstance(value, str):
        _unicode_scalar(value)
        return
    if type(value) is float:
        raise ValueError('canonical KP4 JSON does not contain floating point numbers')
    if not isinstance(value, (list, dict)):
        raise ValueError('value is outside canonical KP4 JSON')
    active = set() if active is None else active
    if id(value) in active:
        raise ValueError('canonical JSON value contains a cycle')
    active.add(id(value))
    try:
        if isinstance(value, list):
            for item in value:
                _json_value(item, active, depth + 1)
        else:
            if any(not isinstance(key, str) for key in value):
                raise ValueError('canonical JSON object keys must be text')
            for key in value:
                _unicode_scalar(key)
            if value.get('type') == 'number':
                _rational(value)
            for item in value.values():
                _json_value(item, active, depth + 1)
    finally:
        active.remove(id(value))


def canonical_json_bytes(value):
    """Encode the exact KP4 JSON subset, with raw Unicode scalar values."""
    _json_value(value)
    try:
        encoded = json.dumps(value, ensure_ascii=False, allow_nan=False,
                             sort_keys=True, separators=(',', ':')).encode('utf-8')
    except (TypeError, ValueError, UnicodeError, RecursionError) as error:
        raise ValueError('invalid canonical JSON value') from error
    if len(encoded) > MAX_RESPONSE_BYTES:
        raise ValueError('canonical JSON exceeds transport bounds')
    return encoded


def _pairs(pairs):
    keys = [key for key, _ in pairs]
    if len(keys) != len(set(keys)):
        raise ValueError('duplicate JSON object key')
    return dict(pairs)


def _integer(token):
    if len(token.lstrip('-')) > 4096 or not INTEGER.fullmatch(token):
        raise ValueError('invalid JSON integer')
    return int(token)


def _reject_number(_token):
    raise ValueError('canonical KP4 JSON has no floating point numbers')


def decode_canonical_json(payload):
    """Decode only bytes which are already in the one canonical spelling."""
    if not isinstance(payload, (bytes, bytearray)):
        raise ValueError('canonical JSON payload must be bytes')
    payload = bytes(payload)
    if not payload or len(payload) > MAX_RESPONSE_BYTES:
        raise ValueError('invalid canonical JSON payload length')
    try:
        value = json.loads(payload.decode('utf-8'), object_pairs_hook=_pairs,
                           parse_int=_integer, parse_float=_reject_number,
                           parse_constant=_reject_number)
    except (UnicodeError, json.JSONDecodeError, ValueError, TypeError, RecursionError) as error:
        raise ValueError('malformed canonical JSON') from error
    if canonical_json_bytes(value) != payload:
        raise ValueError('noncanonical JSON')
    return value


def encode_frame(payload, prefix=PROTOCOL):
    if prefix not in (PROTOCOL, 'KR4'):
        raise ValueError('unsupported frame protocol')
    body = canonical_json_bytes(payload)
    maximum = MAX_BYTES if prefix == PROTOCOL else MAX_RESPONSE_BYTES
    if len(body) > maximum:
        raise ValueError('frame exceeds transport bounds')
    return prefix.encode('ascii') + b' ' + str(len(body)).encode('ascii') + b'\n' + body + b'\n'


def decode_frame(frame, prefix=PROTOCOL):
    if prefix not in (PROTOCOL, 'KR4') or not isinstance(frame, (bytes, bytearray)):
        raise ValueError('invalid frame')
    frame = bytes(frame)
    marker = prefix.encode('ascii') + b' '
    if not frame.startswith(marker):
        raise ValueError('unsupported frame protocol')
    header_end = frame.find(b'\n', len(marker))
    if header_end < 0:
        raise ValueError('truncated frame header')
    length = frame[len(marker):header_end]
    if not length or not re.fullmatch(rb'0|[1-9][0-9]*', length):
        raise ValueError('invalid frame length')
    size = int(length)
    maximum = MAX_BYTES if prefix == PROTOCOL else MAX_RESPONSE_BYTES
    if size > maximum:
        raise ValueError('frame exceeds transport bounds')
    payload_start = header_end + 1
    payload_end = payload_start + size
    if payload_end + 1 != len(frame) or frame[payload_end:payload_end + 1] != b'\n':
        raise ValueError('frame length or terminator mismatch')
    return decode_canonical_json(frame[payload_start:payload_end])


def _lower_row(item, fields, active, depth):
    if depth > RESOURCE_MAXIMA['depth'] or not isinstance(item, dict) or id(item) in active:
        raise ValueError('row expression exceeds bounds or contains a cycle')
    active.add(id(item))
    try:
        keys = set(item)
        if keys == {'column'}:
            column = _identifier(item['column'], 'column')
            if column not in fields:
                raise ValueError('column is not granted by the scope: ' + column)
            return {'column': column}
        if keys == {'num'}:
            number = item['num']
            if not isinstance(number, str) or len(number) > 8200 or not NUMBER.fullmatch(number):
                raise ValueError('invalid exact number lexeme')
            return {'num': number}
        if keys == {'bool'} and type(item['bool']) is bool:
            return {'bool': item['bool']}
        if keys == {'text'} and isinstance(item['text'], str):
            _unicode_scalar(item['text'])
            return {'text': item['text']}
        if keys == {'null'} and item['null'] is True:
            return {'null': True}
        if keys == {'op', 'args'}:
            name, args = item['op'], item['args']
            if name not in ARITHMETIC | BOOLEAN or not isinstance(args, list) \
                    or len(args) != (1 if name == 'not' else 2):
                raise ValueError('invalid row operation or arity')
            return {'op': name, 'args': [_lower_row(child, fields, active, depth + 1)
                                         for child in args]}
        if keys == {'if', 'then', 'else'}:
            return {key: _lower_row(item[key], fields, active, depth + 1)
                    for key in ('if', 'then', 'else')}
        if keys == {'list'} and isinstance(item['list'], list) \
                and len(item['list']) <= RESOURCE_MAXIMA['value_nodes']:
            return {'list': [_lower_row(child, fields, active, depth + 1) for child in item['list']]}
        if keys == {'record'} and isinstance(item['record'], dict) \
                and len(item['record']) <= RESOURCE_MAXIMA['value_nodes'] \
                and all(isinstance(key, str) and len(key) <= 500 for key in item['record']):
            for key in item['record']:
                _unicode_scalar(key)
            return {'record': {key: _lower_row(item['record'][key], fields, active, depth + 1)
                               for key in sorted(item['record'])}}
        if keys == {'field', 'key'} and isinstance(item['key'], str) and len(item['key']) <= 500:
            _unicode_scalar(item['key'])
            return {'field': _lower_row(item['field'], fields, active, depth + 1),
                    'key': item['key']}
        raise ValueError('invalid row expression fields')
    finally:
        active.remove(id(item))


def lower_row(expression, fields):
    if not isinstance(fields, (list, tuple)) or list(fields) != sorted(set(fields)) \
            or any(not isinstance(field, str) or not field for field in fields):
        raise ValueError('scope fields must be sorted unique text')
    return _lower_row(expression, frozenset(fields), set(), 0)


def lower(authored_operation, fields):
    """Return normalized ``{query: ...}`` data; never evaluate any row."""
    if not isinstance(authored_operation, dict) or set(authored_operation) != {'query'} \
            or not isinstance(authored_operation['query'], dict):
        raise ValueError('query operation needs exactly one query key')
    query = authored_operation['query']
    op = query.get('op')
    if op not in OP_KEYS or type(query.get('version')) is not int or query['version'] != 1:
        raise ValueError('unknown query version or operation')
    scope = _identifier(query.get('scope'), 'scope ID')
    expected = OP_KEYS[op]
    if op == 'sum':
        if set(query) not in ({'version', 'scope', 'op', 'value'},
                              {'version', 'scope', 'op', 'where', 'value'}):
            raise ValueError('invalid keys for sum query')
    elif set(query) != expected:
        raise ValueError('invalid keys for ' + op + ' query')
    normalized = {'version': 1, 'scope': scope, 'op': op}
    for key in ('where', 'value'):
        if key in query:
            normalized[key] = lower_row(query[key], fields)
    # Fixed key order is useful to humans, while canonical JSON remains the
    # authority for wire order.
    return {'query': normalized}


def _children(expression):
    if 'args' in expression:
        return expression['args']
    if 'if' in expression:
        return [expression[key] for key in ('if', 'then', 'else')]
    if 'list' in expression:
        return expression['list']
    if 'record' in expression:
        return [expression['record'][key] for key in sorted(expression['record'])]
    if 'field' in expression:
        return [expression['field']]
    return []


def required_columns(normalized_operation):
    query = normalized_operation['query']
    pending = [query[key] for key in ('where', 'value') if key in query]
    result = set()
    while pending:
        item = pending.pop()
        if 'column' in item:
            result.add(item['column'])
        pending.extend(_children(item))
    return sorted(result)


def required_modules(normalized_operation):
    found = {MODULE}
    query = normalized_operation['query']
    pending = [query[key] for key in ('where', 'value') if key in query]
    while pending:
        item = pending.pop()
        if item.get('op') in ARITHMETIC:
            found.add('arithmetic/v1')
        if item.get('op') in BOOLEAN or any(key in item for key in ('if', 'list', 'record', 'field')):
            found.add('composition/v1')
        pending.extend(_children(item))
    return sorted(found)


def resources_v4(limits=None):
    limits = {} if limits is None else limits
    if not isinstance(limits, dict):
        raise ValueError('query resources must be a mapping')
    supplied = dict(limits)
    version = supplied.pop('version', RESOURCE_VERSION)
    if version != RESOURCE_VERSION or set(supplied) - set(RESOURCE_MAXIMA):
        raise ValueError('unknown query resource profile or field')
    result = dict(RESOURCE_MAXIMA)
    for key, value in supplied.items():
        if type(value) is not int or not 0 < value <= RESOURCE_MAXIMA[key]:
            raise ValueError('invalid query resource limit: ' + key)
        result[key] = value
    return {'version': RESOURCE_VERSION, **result}


def _host_value(value, depth=0, active=None):
    if depth > RESOURCE_MAXIMA['value_depth']:
        raise ValueError('authored value exceeds query depth')
    if value is None:
        return {'type': 'null'}
    if type(value) is bool:
        return {'type': 'boolean', 'value': value}
    if type(value) is int:
        return {'type': 'number', 'numerator': str(value), 'denominator': '1'}
    if type(value) is float:
        if not math.isfinite(value):
            raise ValueError('nonfinite authored number')
        value = Fraction(str(value))
        return {'type': 'number', 'numerator': str(value.numerator),
                'denominator': str(value.denominator)}
    if isinstance(value, str):
        _unicode_scalar(value)
        return {'type': 'text', 'value': value}
    if isinstance(value, (datetime.date, datetime.datetime)):
        raise TypeError('dates are unsupported query values')
    if not isinstance(value, (list, dict)):
        raise TypeError('unsupported authored query value')
    active = set() if active is None else active
    if id(value) in active:
        raise ValueError('cyclic authored query value')
    active.add(id(value))
    try:
        if isinstance(value, list):
            typed = {'type': 'list', 'items': [_host_value(item, depth + 1, active) for item in value]}
        else:
            if any(not isinstance(key, str) for key in value):
                raise ValueError('authored records require text keys')
            for key in value:
                _unicode_scalar(key)
            typed = {'type': 'record', 'fields': {
                key: _host_value(value[key], depth + 1, active) for key in sorted(value)}}
    finally:
        active.remove(id(value))
    _typed_value(typed)
    return typed


def _witness(value, kind=None):
    if not isinstance(value, dict):
        raise ValueError('invalid dependency witness')
    actual = value.get('kind')
    if kind is not None and actual != kind:
        raise ValueError('unexpected dependency witness kind')
    if actual == 'node' and set(value) == {'kind', 'id', 'fingerprint'}:
        _identifier(value['id'], 'node ID')
        if not isinstance(value['fingerprint'], str) or not HEX64.fullmatch(value['fingerprint']):
            raise ValueError('invalid node fingerprint')
    elif actual == 'scope' and set(value) == {
            'kind', 'scope_id', 'definition_digest', 'membership_digest',
            'projected_inputs_digest'}:
        _identifier(value['scope_id'], 'scope ID')
        for key in ('definition_digest', 'membership_digest', 'projected_inputs_digest'):
            if not isinstance(value[key], str) or not HEX64.fullmatch(value[key]):
                raise ValueError('invalid scope witness digest')
    else:
        raise ValueError('invalid dependency witness shape')
    return copy.deepcopy(value)


def _observation(cell, column):
    if not isinstance(cell, dict):
        raise ValueError('scope field observation must be a mapping')
    allowed = {'status', 'value', 'reason', 'alternatives', 'computed_basis', 'fingerprint'}
    if set(cell) - allowed or cell.get('status') not in ('known', 'missing', 'contested', 'unavailable'):
        raise ValueError('invalid scope field observation')
    status = cell['status']
    if status == 'unavailable':
        if set(cell) - {'status', 'reason', 'fingerprint'} or cell.get('reason') not in UNAVAILABLE_REASONS:
            raise ValueError('invalid unavailable field observation')
        return {'status': 'unavailable', 'reason': cell['reason']}
    if status == 'contested':
        return {'status': 'contested'}
    if status == 'missing':
        if column == 'rule' or 'computed_basis' in cell:
            return {'status': 'unavailable', 'reason': 'formula_value'}
        return {'status': 'missing'}
    if 'value' not in cell:
        raise ValueError('known scope field has no value')
    if column == 'rule':
        return {'status': 'unavailable', 'reason': 'formula_value'}
    try:
        return {'status': 'known', 'value': _host_value(cell['value'])}
    except TypeError:
        return {'status': 'unavailable', 'reason': 'unsupported_type'}
    except (ValueError, OverflowError, RecursionError):
        return {'status': 'unavailable', 'reason': 'invalid_value'}


def scope_rows(capture):
    """Adapt one detached ScopeCapture data shape to canonical KP4 scope rows."""
    captured_rows = capture.query_rows() if callable(getattr(capture, 'query_rows', None)) else None
    if hasattr(capture, 'to_data'):
        capture = capture.to_data()
    if not isinstance(capture, dict) or set(capture) != {
            'scope_id', 'definition', 'value', 'witness', 'basis', 'candidates'}:
        raise ValueError('invalid scope capture shape')
    scope_id = _identifier(capture['scope_id'], 'scope ID')
    definition = capture['definition']
    if not isinstance(definition, dict) or set(definition) != {'collection', 'fields'} \
            or not isinstance(definition['collection'], str) or not definition['collection']:
        raise ValueError('invalid scope definition')
    fields = definition.get('fields')
    if not isinstance(fields, list) or fields != sorted(set(fields)) \
            or any(not isinstance(field, str) or not field for field in fields):
        raise ValueError('scope fields must be sorted unique text')
    witness = _witness(capture['witness'], 'scope')
    if witness['scope_id'] != scope_id:
        raise ValueError('scope witness ID mismatch')
    candidates = capture['candidates']
    if not isinstance(candidates, dict) or any(not isinstance(member, str) or not member for member in candidates):
        raise ValueError('invalid scope candidates')
    members = []
    supplied = None
    if captured_rows is not None:
        if not isinstance(captured_rows, list):
            raise ValueError('query scope rows must be a list')
        supplied = {row.get('id'): row for row in captured_rows if isinstance(row, dict)}
        if len(supplied) != len(captured_rows) or list(supplied) != sorted(candidates):
            raise ValueError('query scope rows disagree with captured membership')
    for member in sorted(candidates):
        _identifier(member, 'member ID')
        observations = candidates[member]
        if not isinstance(observations, dict) or set(observations) != set(fields):
            raise ValueError('scope candidate fields disagree with grant')
        if supplied is None:
            row_fields = {field: _observation(observations[field], field) for field in fields}
        else:
            row = supplied[member]
            if set(row) != {'id', 'fields'} or row['id'] != member \
                    or not isinstance(row['fields'], dict) or set(row['fields']) != set(fields):
                raise ValueError('invalid canonical query row')
            row_fields = {}
            for field in fields:
                cell = row['fields'][field]
                if not isinstance(cell, dict) or cell.get('status') not in (
                        'known', 'missing', 'contested', 'unavailable'):
                    raise ValueError('invalid canonical query field')
                if cell['status'] == 'known':
                    if set(cell) != {'status', 'value'}:
                        raise ValueError('invalid known canonical query field')
                    _typed_value(cell['value'])
                elif cell['status'] == 'unavailable':
                    if set(cell) != {'status', 'reason'} or cell['reason'] not in UNAVAILABLE_REASONS:
                        raise ValueError('invalid unavailable canonical query field')
                elif set(cell) != {'status'}:
                    raise ValueError('invalid missing or contested canonical query field')
                row_fields[field] = copy.deepcopy(cell)
        members.append({'id': member, 'fields': row_fields})
    basis = capture['basis']
    if not isinstance(basis, dict) or basis.get('witness') != witness \
            or basis.get('members') != [item['id'] for item in members] \
            or basis.get('fields') != fields or not isinstance(basis.get('digest'), str) \
            or digest({key: value for key, value in basis.items() if key != 'digest'}) != basis['digest']:
        raise ValueError('scope basis does not bind the supplied rows')
    return {'id': scope_id, 'witness': witness, 'fields': list(fields), 'members': members}


def _expression_stats(expression):
    nodes, edges, maximum, numeric_digits = 0, 0, 0, 0
    pending = [(expression, 1)]
    while pending:
        item, depth = pending.pop()
        nodes += 1
        maximum = max(maximum, depth)
        if 'num' in item:
            numeric_digits = max(numeric_digits, _number_digits(item['num']))
        children = _children(item)
        edges += len(children)
        pending.extend((child, depth + 1) for child in children)
    return {'nodes': nodes, 'edges': edges, 'depth': maximum, 'digits': numeric_digits}


def _number_digits(value):
    """Exact digit charge when bounded; safely saturate hostile exponents."""
    match = NUMBER_PARTS.fullmatch(value)
    if match is None:
        raise ValueError('invalid exact number lexeme')
    coefficient = (match.group('integer') + (match.group('fraction') or '')).lstrip('0')
    if not coefficient:
        return 1
    exponent_text = match.group('exponent') or '0'
    # Any exponent with this many digits is far outside the maximum 256-digit
    # profile.  Do not materialize 10**exponent merely to prove that refusal.
    if len(exponent_text.lstrip('+-').lstrip('0')) > 6:
        return RESOURCE_MAXIMA['digits'] + 1
    exponent = int(exponent_text)
    scale = len(match.group('fraction') or '') - exponent
    while scale > 0 and coefficient.endswith('0'):
        coefficient = coefficient[:-1]
        scale -= 1
    rough = max(len(coefficient) + max(-scale, 0), max(scale, 0) + 1)
    if rough > 512:
        return RESOURCE_MAXIMA['digits'] + 1
    try:
        number = Fraction(int(coefficient), 10 ** scale) if scale >= 0 \
            else Fraction(int(coefficient) * (10 ** -scale), 1)
    except (ValueError, OverflowError, ZeroDivisionError) as error:
        raise ValueError('invalid exact number lexeme') from error
    return max(len(str(abs(number.numerator))), len(str(number.denominator)))


def _typed_stats(value, depth=1):
    _typed_value(value)
    if value['type'] == 'list':
        child = [_typed_stats(item, depth + 1) for item in value['items']]
    elif value['type'] == 'record':
        child = [_typed_stats(value['fields'][key], depth + 1) for key in sorted(value['fields'])]
    else:
        child = []
    digits = 0
    byte_size = 0
    if value['type'] == 'number':
        digits = max(len(value['numerator'].lstrip('-')), len(value['denominator']))
        byte_size = 3 + len(value['numerator'].encode('utf-8')) + len(value['denominator'])
    elif value['type'] == 'boolean':
        byte_size = 3
    elif value['type'] == 'text':
        byte_size = 2 + 2 * len(value['value'].encode('utf-8'))
    elif value['type'] == 'null':
        byte_size = 1
    elif value['type'] == 'list':
        byte_size = 2 + sum(item['bytes'] + 1 for item in child)
    elif value['type'] == 'record':
        byte_size = 2 + sum(item['bytes'] + 2 + 2 * len(key.encode('utf-8'))
                            for key, item in zip(sorted(value['fields']), child))
    return {'nodes': 1 + sum(item['nodes'] for item in child),
            'depth': max([depth] + [item['depth'] for item in child]),
            'digits': max([digits] + [item['digits'] for item in child]),
            'bytes': byte_size}


def _size(nodes=0, depth=0, bytes_=0, digits=0):
    return {'nodes': nodes, 'depth': depth, 'bytes': bytes_, 'digits': digits}


def _max_size(*values):
    return {key: max([0] + [value[key] for value in values])
            for key in ('nodes', 'depth', 'bytes', 'digits')}


def _container_size(kind, children, keys=()):
    if kind == 'list':
        return _size(1 + sum(item['nodes'] for item in children),
                     max([0] + [item['depth'] + 1 for item in children]),
                     2 + sum(item['bytes'] + 1 for item in children),
                     max([0] + [item['digits'] for item in children]))
    return _size(1 + sum(item['nodes'] for item in children),
                 max([0] + [item['depth'] + 1 for item in children]),
                 2 + sum(item['bytes'] + 2 + 2 * len(key.encode('utf-8'))
                         for key, item in zip(keys, children)),
                 max([0] + [item['digits'] for item in children]))


def _expr_resource_upper(expression, fields):
    """Conservative output/peak value bounds for one concrete captured row."""
    if 'column' in expression:
        cell = fields[expression['column']]
        output = (_typed_stats(cell['value'], depth=0) if cell['status'] == 'known'
                  else _size(nodes=1))
        return output, output
    if 'num' in expression:
        digits = _number_digits(expression['num'])
        output = _size(1, 0, 4 + 2 * digits, digits)
        return output, output
    if 'bool' in expression:
        output = _size(1, 0, 3, 0)
        return output, output
    if 'text' in expression:
        output = _size(1, 0, 2 + 2 * len(expression['text'].encode('utf-8')), 0)
        return output, output
    if 'null' in expression:
        output = _size(1, 0, 1, 0)
        return output, output
    if 'op' in expression:
        children = [_expr_resource_upper(child, fields) for child in expression['args']]
        outputs, peaks = [item[0] for item in children], [item[1] for item in children]
        if expression['op'] in ('add', 'sub'):
            digits = sum(item['digits'] for item in outputs) + 1
            output = _size(1, 0, 4 + 2 * digits, digits)
        elif expression['op'] in ('mul', 'div'):
            digits = sum(item['digits'] for item in outputs)
            output = _size(1, 0, 4 + 2 * digits, digits)
        else:
            output = _size(1, 0, 3, 0)
        return output, _max_size(output, *peaks)
    if 'if' in expression:
        children = [_expr_resource_upper(expression[key], fields)
                    for key in ('if', 'then', 'else')]
        output = _max_size(children[1][0], children[2][0])
        return output, _max_size(output, *(item[1] for item in children))
    if 'list' in expression:
        children = [_expr_resource_upper(child, fields) for child in expression['list']]
        output = _container_size('list', [item[0] for item in children])
        return output, _max_size(output, *(item[1] for item in children))
    if 'record' in expression:
        keys = sorted(expression['record'])
        children = [_expr_resource_upper(expression['record'][key], fields) for key in keys]
        output = _container_size('record', [item[0] for item in children], keys)
        return output, _max_size(output, *(item[1] for item in children))
    if 'field' in expression:
        output, peak = _expr_resource_upper(expression['field'], fields)
        return output, peak  # The containing record is a safe upper bound for its selected field.
    raise ValueError('invalid row expression')


def _result_resource_upper(normalized_operation, scope):
    query = normalized_operation['query']
    op, members = query['op'], scope['members']
    expression_peaks = []
    value_outputs = []
    analysis_members = members or [{'id': '', 'fields': {
        field: {'status': 'missing'} for field in scope['fields']}}]
    for index, member in enumerate(analysis_members):
        fields = member['fields']
        for key in ('where', 'value'):
            if key in query:
                output, peak = _expr_resource_upper(query[key], fields)
                expression_peaks.append(peak)
                if key == 'value' and index < len(members):
                    value_outputs.append(output)
    count_digits = len(str(len(members)))
    counter = _size(1, 0, 4 + 2 * count_digits, count_digits)
    if op == 'filter':
        outputs = [_size(1, 0, 2 + 2 * len(member['id'].encode('utf-8')), 0)
                   for member in members]
        raw = _container_size('list', outputs)
    elif op in ('project', 'select'):
        raw = _container_size('list', value_outputs)
    elif op == 'sum':
        aggregate_digits = max(1, sum(item['digits'] + 1 for item in value_outputs))
        raw = _size(1, 0, 4 + 2 * aggregate_digits, aggregate_digits)
    elif op == 'count':
        raw = counter
    else:
        raw = _size(1, 0, 3, 0)
    keys = ['definite_match_count', 'error_count', 'input_count', 'result',
            'unknown_membership_count', 'unknown_value_count']
    result = _container_size('record', [counter, counter, counter, raw, counter, counter], keys)
    return result, _max_size(result, *expression_peaks)


def _expr_execution_upper(expression, fields):
    if any(key in expression for key in ('column', 'num', 'bool', 'text', 'null')):
        return 1
    if 'op' in expression:
        children = expression['args']
        result = 1 + sum(_expr_execution_upper(child, fields) for child in children)
        if expression['op'] in ('eq', 'ne'):
            result += sum(_expr_resource_upper(child, fields)[0]['nodes'] for child in children)
        return result
    if 'if' in expression:
        return (1 + _expr_execution_upper(expression['if'], fields)
                + max(_expr_execution_upper(expression['then'], fields),
                      _expr_execution_upper(expression['else'], fields)))
    if 'list' in expression:
        return 1 + sum(_expr_execution_upper(child, fields) for child in expression['list'])
    if 'record' in expression:
        return 1 + sum(_expr_execution_upper(child, fields)
                       for child in expression['record'].values())
    if 'field' in expression:
        return (1 + _expr_execution_upper(expression['field'], fields)
                + _expr_resource_upper(expression['field'], fields)[0]['nodes'])
    raise ValueError('invalid normalized row expression')


def _operation_execution_upper(normalized_operation, scope):
    query = normalized_operation['query']
    total = 0
    for member in scope['members']:
        fields = member['fields']
        total += 1
        if query['op'] in ('filter', 'count', 'all', 'any'):
            total += _expr_execution_upper(query['where'], fields)
        elif query['op'] == 'project':
            total += _expr_execution_upper(query['value'], fields)
        elif query['op'] in ('select', 'sum'):
            if 'where' in query:
                total += _expr_execution_upper(query['where'], fields)
            total += _expr_execution_upper(query['value'], fields)
    return total


def preflight_counts(normalized_operation, scope, resources):
    """Return deterministic conservative costs, refusing over-limit inputs."""
    resources = resources_v4(resources)
    members = scope['members']
    fields = scope['fields']
    candidate_count = len(members)
    field_reads = candidate_count * len(fields)
    query = normalized_operation['query']
    stats = [_expression_stats(query[key]) for key in ('where', 'value') if key in query]
    ir_nodes = sum(item['nodes'] for item in stats)
    ir_edges = sum(item['edges'] for item in stats)
    maximum_depth = max([0] + [item['depth'] for item in stats])
    maximum_digits = max([0] + [item['digits'] for item in stats])
    _, peak = _result_resource_upper(normalized_operation, scope)
    value_nodes = peak['nodes']
    value_depth = peak['depth']
    value_bytes = peak['bytes']
    maximum_digits = max(maximum_digits, peak['digits'])
    for member in members:
        for cell in member['fields'].values():
            if cell['status'] == 'known':
                item = _typed_stats(cell['value'], depth=0)
                value_nodes = max(value_nodes, item['nodes'])
                value_depth = max(value_depth, item['depth'])
                value_bytes = max(value_bytes, item['bytes'])
                maximum_digits = max(maximum_digits, item['digits'])
    preflight = ir_nodes + ir_edges + field_reads
    # Every row may take every authored branch; equality/field traversal and
    # the aggregate transition are included exactly as the native preflight does.
    step_upper_bound = _operation_execution_upper(normalized_operation, scope)
    checks = (
        ('candidate_limit', candidate_count, resources['candidates']),
        ('field_read_limit', field_reads, resources['field_reads']),
        ('depth_limit', maximum_depth, resources['depth']),
        ('value_limit', value_nodes, resources['value_nodes']),
        ('value_limit', value_depth, resources['value_depth']),
        ('value_limit', value_bytes, resources['value_bytes']),
        ('digit_limit', maximum_digits, resources['digits']),
        ('step_limit', max(preflight, step_upper_bound), resources['steps']),
    )
    for code, actual, bound in checks:
        if actual > bound:
            raise ValueError(code)
    return {'candidates': candidate_count, 'field_reads': field_reads,
            'preflight_steps': preflight, 'step_upper_bound': step_upper_bound}


def prepare(capture, authored_operation, *, request_id, declared_capabilities,
            root_witness=None, limits=None):
    """Construct the detached public prepared-request envelope from captured data."""
    if not isinstance(request_id, str) or not HEX64.fullmatch(request_id):
        raise ValueError('request_id must be 64 lowercase hexadecimal characters')
    capture_data = capture.to_data() if hasattr(capture, 'to_data') else capture
    fields = capture_data.get('definition', {}).get('fields') if isinstance(capture_data, dict) else None
    normalized = lower(authored_operation, fields)
    scope = scope_rows(capture)
    if normalized['query']['scope'] != scope['id']:
        raise ValueError('query scope does not match captured scope')
    modules = required_modules(normalized)
    if not isinstance(declared_capabilities, list) \
            or declared_capabilities != sorted(set(declared_capabilities)) \
            or any(not isinstance(item, str) for item in declared_capabilities):
        raise ValueError('declared capabilities must be sorted unique text')
    missing = set(modules) - set(declared_capabilities)
    if missing:
        raise ValueError('undeclared modules: ' + ', '.join(sorted(missing)))
    root = None if root_witness is None else _witness(root_witness, 'node')
    dependencies = ([] if root is None else [root]) + [copy.deepcopy(scope['witness'])]
    potential_ids = sorted(([] if root is None else [root['id']]) + [scope['id']])
    resources = resources_v4(limits)
    costs = preflight_counts(normalized, scope, resources)
    request = {'version': 4, 'request_id': request_id, 'resources': resources,
               'required_modules': modules, 'operation': normalized,
               'root_witness': root, 'scope': scope}
    basis = {'version': 1, 'recipe': 'query-inputs/v1', 'profile': PROFILE,
             'modules': modules, 'as_of': capture_data['basis'].get('as_of'),
             'operation': copy.deepcopy(normalized), 'scope': copy.deepcopy(capture_data['basis']),
             'dependencies': copy.deepcopy(dependencies), 'resources': copy.deepcopy(resources),
             'preflight_cost': costs}
    basis['digest'] = digest(basis)
    if len(canonical_json_bytes(request)) > MAX_BYTES:
        raise ValueError('transport_limit')
    return {'version': 1, 'module': MODULE, 'protocol': PROTOCOL,
            'resources': RESOURCE_VERSION, 'normalized_operation': normalized,
            'required_modules': modules, 'potential_dependencies': dependencies,
            'potential_ids': potential_ids, 'basis_template': basis, 'request': request}


def validate_request(request):
    """Validate a detached KP4 request without executing its operation."""
    if not isinstance(request, dict) or set(request) != {
            'version', 'request_id', 'resources', 'required_modules', 'operation',
            'root_witness', 'scope'} or request.get('version') != 4:
        raise ValueError('invalid KP4 request shape')
    if not isinstance(request['request_id'], str) or not HEX64.fullmatch(request['request_id']):
        raise ValueError('invalid KP4 request ID')
    resources = resources_v4(request['resources'])
    scope = request['scope']
    if not isinstance(scope, dict) or set(scope) != {'id', 'witness', 'fields', 'members'}:
        raise ValueError('invalid KP4 scope shape')
    scope_id = _identifier(scope['id'], 'scope ID')
    witness = _witness(scope['witness'], 'scope')
    if witness['scope_id'] != scope_id or not isinstance(scope['members'], list) \
            or not isinstance(scope['fields'], list) \
            or scope['fields'] != sorted(set(scope['fields'])) \
            or any(not isinstance(field, str) or not field for field in scope['fields']):
        raise ValueError('invalid KP4 scope binding')
    for field in scope['fields']:
        _identifier(field, 'column')
    ids = [item.get('id') for item in scope['members'] if isinstance(item, dict)]
    if len(ids) != len(scope['members']) or ids != sorted(set(ids)):
        raise ValueError('scope members must be sorted and unique')
    field_set = scope['fields']
    for member in scope['members']:
        if set(member) != {'id', 'fields'} or not isinstance(member['fields'], dict):
            raise ValueError('invalid scope member')
        _identifier(member['id'], 'member ID')
        fields = sorted(member['fields'])
        if fields != field_set:
            raise ValueError('scope member grants disagree')
        for column, cell in member['fields'].items():
            _identifier(column, 'column')
            if not isinstance(cell, dict) or cell.get('status') not in (
                    'known', 'missing', 'contested', 'unavailable'):
                raise ValueError('invalid wire field state')
            if cell['status'] == 'known':
                if set(cell) != {'status', 'value'}:
                    raise ValueError('invalid known wire field')
                _typed_value(cell['value'])
            elif cell['status'] == 'unavailable':
                if set(cell) != {'status', 'reason'} or cell['reason'] not in UNAVAILABLE_REASONS:
                    raise ValueError('invalid unavailable wire field')
            elif set(cell) != {'status'}:
                raise ValueError('invalid missing/contested wire field')
    normalized = lower(request['operation'], field_set)
    if normalized != request['operation'] or normalized['query']['scope'] != scope_id:
        raise ValueError('noncanonical KP4 operation')
    modules = required_modules(normalized)
    if request['required_modules'] != modules:
        raise ValueError('noncanonical closure-local module list')
    if request['root_witness'] is not None:
        _witness(request['root_witness'], 'node')
    preflight_counts(normalized, scope, resources)
    return copy.deepcopy(request)


def encode_request(request):
    return encode_frame(validate_request(request), PROTOCOL)


def decode_request(frame):
    return validate_request(decode_frame(frame, PROTOCOL))


def validate_prepared(prepared):
    """Verify every cross-field binding in the public prepared envelope."""
    if not isinstance(prepared, dict) or set(prepared) != {
            'version', 'module', 'protocol', 'resources', 'normalized_operation',
            'required_modules', 'potential_dependencies', 'potential_ids',
            'basis_template', 'request'} \
            or prepared.get('version') != 1 or prepared.get('module') != MODULE \
            or prepared.get('protocol') != PROTOCOL or prepared.get('resources') != RESOURCE_VERSION:
        raise ValueError('invalid prepared query envelope')
    request = validate_request(prepared['request'])
    root = request['root_witness']
    dependencies = ([] if root is None else [root]) + [request['scope']['witness']]
    ids = sorted(([] if root is None else [root['id']]) + [request['scope']['id']])
    if prepared['normalized_operation'] != request['operation'] \
            or prepared['required_modules'] != request['required_modules'] \
            or prepared['potential_dependencies'] != dependencies \
            or prepared['potential_ids'] != ids:
        raise ValueError('prepared request fields disagree')
    basis = prepared['basis_template']
    if not isinstance(basis, dict) or set(basis) != {
            'version', 'recipe', 'profile', 'modules', 'as_of', 'operation',
            'scope', 'dependencies', 'resources', 'preflight_cost', 'digest'} \
            or basis.get('version') != 1 or basis.get('recipe') != 'query-inputs/v1' \
            or basis.get('profile') != PROFILE or basis.get('modules') != request['required_modules'] \
            or basis.get('operation') != request['operation'] \
            or basis.get('dependencies') != dependencies \
            or basis.get('resources') != request['resources'] \
            or not isinstance(basis.get('digest'), str) \
            or digest({key: value for key, value in basis.items() if key != 'digest'}) != basis['digest']:
        raise ValueError('invalid query basis template')
    scope_basis = basis['scope']
    if not isinstance(scope_basis, dict) or scope_basis.get('witness') != request['scope']['witness'] \
            or scope_basis.get('members') != [row['id'] for row in request['scope']['members']] \
            or scope_basis.get('fields') != request['scope']['fields'] \
            or not isinstance(scope_basis.get('digest'), str) \
            or digest({key: value for key, value in scope_basis.items() if key != 'digest'}) \
            != scope_basis['digest']:
        raise ValueError('query scope basis disagrees with request')
    if basis.get('as_of') != scope_basis.get('as_of'):
        raise ValueError('query basis time disagrees with scope basis')
    expected_cost = preflight_counts(request['operation'], request['scope'], request['resources'])
    if basis['preflight_cost'] != expected_cost:
        raise ValueError('query preflight cost disagrees with request')
    return copy.deepcopy(prepared)


def _diagnostic(value, scope_id=None, member_ids=None):
    if not isinstance(value, dict) or set(value) != {'code', 'related_ids', 'locations'} \
            or value['code'] not in DIAGNOSTIC_CODES:
        raise ValueError('invalid query diagnostic')
    related = value['related_ids']
    locations = value['locations']
    if not isinstance(related, list) or related != sorted(set(related)) \
            or any(not isinstance(item, str) for item in related):
        raise ValueError('diagnostic related IDs must be sorted unique text')
    if not isinstance(locations, list):
        raise ValueError('diagnostic locations must be a list')
    location_keys = []
    for location in locations:
        if not isinstance(location, dict) or set(location) != {'candidate', 'column', 'phase'} \
                or location['phase'] not in ('where', 'value', 'preflight', 'aggregate'):
            raise ValueError('invalid diagnostic location')
        _identifier(location['candidate'], 'diagnostic candidate')
        _identifier(location['column'], 'diagnostic column')
        if member_ids is not None and location['candidate'] not in member_ids:
            raise ValueError('diagnostic candidate is outside captured scope')
        if scope_id is not None and (scope_id not in related or location['candidate'] not in related):
            raise ValueError('diagnostic location is not bound by related IDs')
        location_keys.append((location['candidate'], location['column'], location['phase']))
    if location_keys != sorted(set(location_keys)):
        raise ValueError('diagnostic locations must be sorted and unique')
    return (value['code'], tuple(related), tuple(location_keys))


def _query_counts(value):
    if not isinstance(value, dict) or set(value) != set(QUERY_COUNT_KEYS):
        raise ValueError('invalid query counts')
    if any(type(value[key]) is not int or value[key] < 0 for key in QUERY_COUNT_KEYS):
        raise ValueError('query counts must be nonnegative JSON integers')
    if any(value[key] > value['input_count'] for key in QUERY_COUNT_KEYS
           if key != 'input_count'):
        raise ValueError('query row counters exceed input count')
    if value['unknown_value_count'] > value['definite_match_count']:
        raise ValueError('unknown value count exceeds definite membership')
    return value


def _counter(value, key):
    field = value['fields'][key]
    _rational(field)
    if field['denominator'] != '1' or int(field['numerator']) < 0:
        raise ValueError('result query counter must be a natural number')
    return int(field['numerator'])


def validate_response(response, prepared):
    """Validate KR4 structure/bindings only; never reproduce query semantics."""
    prepared = validate_prepared(prepared)
    if not isinstance(response, dict) or set(response) != {
            'version', 'request_id', 'status', 'value', 'diagnostics',
            'query_counts', 'executed_reads', 'cost'} or response.get('version') != 4:
        raise ValueError('invalid KR4 response shape')
    if response['request_id'] != prepared['request']['request_id']:
        raise ValueError('KR4 request binding mismatch')
    status = response['status']
    if status not in ('ok', 'unknown', 'error', 'limit', 'unsupported_capability'):
        raise ValueError('invalid KR4 status')
    diagnostics = response['diagnostics']
    if not isinstance(diagnostics, list):
        raise ValueError('invalid KR4 diagnostics')
    scope_id = prepared['request']['scope']['id']
    member_ids = frozenset(row['id'] for row in prepared['request']['scope']['members'])
    diagnostic_keys = [_diagnostic(item, scope_id, member_ids) for item in diagnostics]
    if diagnostic_keys != sorted(set(diagnostic_keys)):
        raise ValueError('diagnostics must be sorted and unique')
    if response['executed_reads'] != prepared['potential_dependencies']:
        raise ValueError('KR4 executed reads disagree with prepared witnesses')
    for witness in response['executed_reads']:
        _witness(witness)
    counts = response['query_counts']
    if counts is not None:
        counts = _query_counts(counts)
    if status == 'ok':
        if counts is None or not isinstance(response['value'], dict):
            raise ValueError('ok query response needs value and counts')
        value = response['value']
        _typed_value(value)
        if value.get('type') != 'record' or set(value.get('fields', {})) != {
                *QUERY_COUNT_KEYS, 'result'}:
            raise ValueError('ok query result has the wrong record shape')
        for key in QUERY_COUNT_KEYS:
            if _counter(value, key) != counts[key]:
                raise ValueError('typed result counter disagrees with query_counts')
    elif response['value'] is not None:
        raise ValueError('non-ok query response must have null value')
    if status == 'unknown' and counts is None:
        raise ValueError('unknown row-scan response needs query counts')
    if status in ('limit', 'unsupported_capability') and counts is not None:
        raise ValueError('preflight/capability refusal cannot claim query counts')
    cost = response['cost']
    if not isinstance(cost, dict) or set(cost) != set(COST_KEYS):
        raise ValueError('invalid KR4 cost shape')
    for key in set(COST_KEYS) - {'node_evaluations'}:
        if type(cost[key]) is not int or cost[key] < 0:
            raise ValueError('KR4 costs must be nonnegative integers')
    expected_counts = prepared['basis_template']['preflight_cost']
    resource_bounds = prepared['request']['resources']
    if cost['candidates'] != expected_counts['candidates'] \
            or cost['field_reads'] != expected_counts['field_reads'] \
            or cost['preflight_steps'] != expected_counts['preflight_steps'] \
            or cost['evaluated_field_reads'] > cost['steps'] \
            or cost['steps'] > resource_bounds['steps'] \
            or cost['preflight_steps'] > resource_bounds['steps'] \
            or cost['candidates'] > resource_bounds['candidates'] \
            or cost['field_reads'] > resource_bounds['field_reads']:
        raise ValueError('KR4 cost disagrees with prepared request')
    node_counts = cost['node_evaluations']
    root = prepared['request']['root_witness']
    expected_nodes = {} if root is None else {root['id']: 1}
    if node_counts != expected_nodes:
        raise ValueError('KR4 node evaluation counts disagree with root witness')
    if counts is not None and counts['input_count'] != cost['candidates']:
        raise ValueError('query input count disagrees with captured candidates')
    return copy.deepcopy(response)


def decode_response(frame, prepared):
    return validate_response(decode_frame(frame, 'KR4'), prepared)


def finalize_basis(prepared, response):
    """Add validated row-scan counts to a fresh basis and bind its digest."""
    response = validate_response(response, prepared)
    if response['query_counts'] is None:
        return None
    basis = copy.deepcopy(prepared['basis_template'])
    basis.pop('digest')
    basis['query_counts'] = copy.deepcopy(response['query_counts'])
    basis['digest'] = digest(basis)
    return basis
