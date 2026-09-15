"""Data contracts shared by capture, evaluation and generic consumers."""
import datetime
import json
import math
import re
from types import MappingProxyType

PROFILE = 'core/v1'
LEGACY_PROFILES = ('ordinary-reader/v1', 'checked-reader/v1')
SCHEMA_VERSION = 1
MODULES = MappingProxyType({
    'arithmetic/v1': MappingProxyType({
        'operators': ('add', 'sub', 'mul', 'div', 'eq', 'ne', 'lt', 'le', 'gt', 'ge'),
        'arity': 2,
        'inputs': ('number', 'boolean', 'text', 'null'),
        'discovery': 'transitive-node-closure/v1',
        'executor': 'KP2',
    }),
})
DEFAULT_LIMITS = MappingProxyType({'steps': 1000000, 'depth': 128, 'digits': 256})
MAX_LIMITS = MappingProxyType({'steps': 10000000, 'depth': 4096, 'digits': 4096})
MAX_NODES = 20000
MAX_EDGES = 100000
MAX_COLLECTION = 10000
MAX_REQUEST_BYTES = 16 * 1024 * 1024
OPERATIONAL_LIMITS = MappingProxyType({
    'timeout_seconds': 30, 'batch_requests': 1000,
    'input_bytes': MAX_REQUEST_BYTES, 'output_bytes': 64 * 1024 * 1024,
})


class OperationalLimit(ValueError):
    """Refuse the complete batch/report; never return clipped witnesses."""
    def __init__(self, code):
        self.code = code
        super().__init__(code + ': operational budget exceeded; select fewer results or smaller inputs')


def operational_bounds(limits=None):
    result = dict(OPERATIONAL_LIMITS)
    if limits is not None:
        if not isinstance(limits, dict) or set(limits) - set(result):
            raise ValueError('unknown operational limits')
        for key, value in limits.items():
            if type(value) is not int or not 0 < value <= result[key]:
                raise ValueError('invalid operational limit: ' + key)
            result[key] = value
    return result


class OutputBudget:
    """Count every serialized occurrence, including shared evidence, incrementally.

    Compact ASCII JSON is the accounting format. Dates use their explicit ISO
    representation. No complete serialized report is allocated just to size it.
    """
    def __init__(self, maximum, code='output_limit'):
        self.remaining = maximum
        self.code = code

    def charge(self, count):
        self.remaining -= count
        if self.remaining < 0:
            raise OperationalLimit(self.code)

    def add(self, value):
        def mapping_items(item):
            for key, child in item.items():
                yield key
                yield child
        pending = [iter((value,))]
        while pending:
            try:
                item = next(pending[-1])
            except StopIteration:
                pending.pop()
                continue
            if type(item) in (datetime.date, datetime.datetime):
                item = item.isoformat()
            if isinstance(item, str):
                self.charge(2)
                for offset in range(0, len(item), 4096):
                    self.charge(len(json.dumps(item[offset:offset + 4096], ensure_ascii=True)) - 2)
            elif isinstance(item, dict):
                self.charge(2 + max(0, len(item) - 1) + len(item))
                pending.append(iter(mapping_items(item)))
            elif isinstance(item, (list, tuple)):
                self.charge(2 + max(0, len(item) - 1))
                pending.append(iter(item))
            else:
                self.charge(len(json.dumps(item, allow_nan=False, separators=(',', ':'))))


class CapabilityError(ValueError):
    def __init__(self, code, detail):
        self.code = code
        super().__init__(detail)


def _check_finite(value, active=None, depth=0):
    """Refuse lossy/cyclic input before calling the existing typed encoder."""
    if depth > 128:
        raise ValueError('snapshot data exceeds 128 levels')
    kind = type(value)
    if value is None or kind in (str, bool, int, datetime.date, datetime.datetime):
        return
    if kind is float:
        if not math.isfinite(value):
            raise ValueError('snapshot data contains a nonfinite value')
        return
    if not isinstance(value, dict) and kind is not list:
        raise ValueError('unsupported snapshot value type: ' + kind.__name__)
    active = set() if active is None else active
    if id(value) in active:
        raise ValueError('snapshot data contains a cycle')
    active.add(id(value))
    try:
        if isinstance(value, dict):
            if any(type(key) is not str for key in value):
                raise ValueError('snapshot mappings require string keys')
            children = value.values()
        else:
            children = value
        for child in children:
            _check_finite(child, active, depth + 1)
    finally:
        active.remove(id(value))


def digest(value):
    """Reuse contribution identity: exact types and map ordering are significant."""
    _check_finite(value)
    from ..pending_grounding import identity
    return identity(value)


def normalize_as_of(value):
    if value is None:
        return None
    if not isinstance(value, str):
        raise ValueError('as_of must be an ISO date or timezone-aware datetime')
    if re.fullmatch(r'\d{4}-\d{2}-\d{2}', value):
        return datetime.date.fromisoformat(value).isoformat()
    if not re.match(r'^\d{4}-\d{2}-\d{2}T', value):
        raise ValueError('as_of must be an ISO date or timezone-aware datetime')
    # Python 3.9 does not accept the Z suffix in fromisoformat.
    parsed = datetime.datetime.fromisoformat(value[:-1] + '+00:00' if value.endswith('Z') else value)
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise ValueError('as_of datetime requires an explicit timezone')
    return parsed.astimezone(datetime.timezone.utc).isoformat().replace('+00:00', 'Z')


def capabilities(document, *, profile=None):
    """Interpret declared versions without guessing from installed executors."""
    meta = document.get('meta', {})
    if not isinstance(meta, dict):
        # Legacy records can have an empty/prose head. Only an actual reasoning
        # declaration changes interpretation; unrelated metadata is not migrated.
        meta = {}
    if 'reasoning' not in meta:
        result = {'version': 1, 'profile': 'ordinary-reader/v1', 'requires': []}
    else:
        value = meta['reasoning']
        if not isinstance(value, dict) or set(value) != {'version', 'profile', 'requires'} \
                or type(value['version']) is not int or not isinstance(value['profile'], str) \
                or not isinstance(value['requires'], list) \
                or any(not isinstance(item, str) for item in value['requires']) \
                or value['requires'] != sorted(set(value['requires'])):
            raise CapabilityError('invalid_capability', 'invalid meta.reasoning capability declaration')
        if value['version'] != 1 or value['profile'] != PROFILE:
            raise CapabilityError('unsupported_capability', 'unsupported reasoning profile or metadata version')
        unknown = set(value['requires']) - set(MODULES)
        if unknown:
            raise CapabilityError('unsupported_capability', 'unsupported modules: ' + ', '.join(sorted(unknown)))
        if 'arithmetic/v1' not in value['requires']:
            raise CapabilityError('invalid_capability', 'core/v1 requires arithmetic/v1')
        result = {**value, 'requires': list(value['requires'])}
    if profile is not None:
        if profile not in (*LEGACY_PROFILES, PROFILE):
            raise CapabilityError('unsupported_capability', 'unsupported requested profile')
        if result['profile'] == PROFILE and profile != PROFILE:
            raise CapabilityError('unsupported_capability', 'a declared core profile cannot be read as legacy')
        if profile != result['profile']:
            result = {**result, 'declared_profile': result['profile'], 'profile': profile,
                      'requires': ['arithmetic/v1'] if profile == PROFILE else [],
                      'experimental_override': profile == PROFILE}
    return result


def resource_limits(limits=None):
    if limits is None:
        return dict(DEFAULT_LIMITS)
    if not isinstance(limits, dict) or set(limits) - set(DEFAULT_LIMITS):
        raise ValueError('unknown resource limits')
    result = dict(DEFAULT_LIMITS)
    for key, value in limits.items():
        if type(value) is not int or value <= 0 or value > MAX_LIMITS[key]:
            raise ValueError('invalid resource limit: ' + key)
        result[key] = value
    return result


def validate_value(value, depth=0):
    if depth > 128 or not isinstance(value, dict):
        raise ValueError('invalid typed value')
    kind = value.get('type')
    if kind == 'number' and set(value) == {'type', 'numerator', 'denominator'}:
        numerator, denominator = value['numerator'], value['denominator']
        if not isinstance(numerator, str) or not isinstance(denominator, str) \
                or len(numerator) > 4097 or len(denominator) > 4096 \
                or not re.fullmatch(r'0|-?[1-9][0-9]*', numerator) \
                or not re.fullmatch(r'[1-9][0-9]*', denominator):
            raise ValueError('invalid exact numeric value')
    elif kind in ('boolean', 'text') and set(value) == {'type', 'value'}:
        if type(value['value']) is not (bool if kind == 'boolean' else str):
            raise ValueError('invalid scalar type')
    elif kind == 'null' and set(value) == {'type'}:
        pass
    elif kind == 'list' and set(value) == {'type', 'items'} and isinstance(value['items'], list):
        if len(value['items']) > MAX_COLLECTION:
            raise ValueError('typed collection exceeds limit')
        for item in value['items']:
            validate_value(item, depth + 1)
    elif kind == 'record' and set(value) == {'type', 'fields'} and isinstance(value['fields'], dict):
        if len(value['fields']) > MAX_COLLECTION or any(type(key) is not str for key in value['fields']):
            raise ValueError('invalid typed record fields')
        for item in value['fields'].values():
            validate_value(item, depth + 1)
    else:
        raise ValueError('invalid typed value shape')
    return value


def render_value(value):
    """Generic rendering depends on result types, never on operation names."""
    validate_value(value)
    kind = value['type']
    if kind == 'number':
        return value['numerator'] if value['denominator'] == '1' else \
            value['numerator'] + '/' + value['denominator']
    if kind == 'boolean':
        return 'true' if value['value'] else 'false'
    if kind == 'text':
        return json.dumps(value['value'], ensure_ascii=False)
    if kind == 'null':
        return 'null'
    if kind == 'list':
        return '[' + ', '.join(render_value(item) for item in value['items']) + ']'
    return '{' + ', '.join(json.dumps(key, ensure_ascii=False) + ': ' + render_value(item)
                           for key, item in sorted(value['fields'].items())) + '}'


def node_basis(snapshot_data, node_id):
    """Generate one complete basis; repeated reads should share InputBasis."""
    from .basis import InputBasis
    return InputBasis(snapshot_data).basis(node_id)
