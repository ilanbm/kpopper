"""Bounded, pure, three-valued evaluation of followup trigger expressions.

The caller supplies graph snapshots, observations, completed task identities and
an aware clock. This module never reads files, polls services or performs work.
"""
import datetime as dt
import math
import re
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError
try:
    from .expressions import number as _exact_number
except ImportError:
    from expressions import number as _exact_number


UTC = dt.timezone.utc
MAX_DEPTH = 8
MAX_NODES = 100
_OPS = {'==', '!=', '<', '<=', '>', '>='}
_DATE = re.compile(r'^\d{4}-\d{2}-\d{2}$')
_STAMP = re.compile(r'^\d{4}-\d{2}-\d{2}[Tt ]\d{2}:\d{2}(?::\d{2}(?:\.\d{1,6})?)?(?:[Zz]|[+-]\d{2}:\d{2})$')
_MISSING = object()


def _iso(value):
    return value.astimezone(UTC).isoformat().replace('+00:00', 'Z')


def parse_time(value, zone='UTC'):
    """Parse an offset timestamp or a date at midnight in an IANA timezone."""
    try:
        tz = UTC if zone == 'UTC' else ZoneInfo(zone)
        if isinstance(value, dt.datetime):
            result = value
        elif isinstance(value, dt.date):
            result = dt.datetime.combine(value, dt.time.min, tzinfo=tz)
        elif isinstance(value, str) and _DATE.fullmatch(value):
            result = dt.datetime.combine(dt.date.fromisoformat(value), dt.time.min, tzinfo=tz)
        elif isinstance(value, str) and _STAMP.fullmatch(value):
            normalized = value.replace('t', 'T').replace('z', '+00:00').replace('Z', '+00:00')
            # Python 3.9/3.10 accept only 3 or 6 fractional-second digits.
            normalized = re.sub(r'\.(\d{1,6})(?=[+-]\d{2}:\d{2}$)',
                                lambda match: '.' + match.group(1).ljust(6, '0'), normalized)
            result = dt.datetime.fromisoformat(normalized)
        else:
            raise ValueError('expected an ISO date or timestamp with an explicit offset')
        if result.tzinfo is None or result.utcoffset() is None:
            raise ValueError('timestamps require an explicit offset')
        return result.astimezone(UTC)
    except (ValueError, TypeError, OverflowError, ZoneInfoNotFoundError) as exc:
        raise ValueError('invalid time or timezone: {}'.format(exc)) from exc


def normalize(value):
    """Return stable JSON-compatible data, rejecting cycles and nonfinite values."""
    active = set()
    count = [0]

    def visit(item, depth):
        count[0] += 1
        if depth > 100 or count[0] > 100000:
            raise ValueError('value exceeds normalization bounds')
        if isinstance(item, dt.datetime):
            return _iso(parse_time(item))
        if isinstance(item, dt.date):
            return item.isoformat()
        if item is None or type(item) in (str, bool, int):
            return item
        if type(item) is float:
            if not math.isfinite(item):
                raise ValueError('nonfinite numbers are not supported')
            return item
        if type(item) not in (dict, list):
            raise ValueError('expected JSON-compatible data')
        identity = id(item)
        if identity in active:
            raise ValueError('cyclic data is not supported')
        active.add(identity)
        try:
            if isinstance(item, list):
                return [visit(child, depth + 1) for child in item]
            if any(type(key) is not str for key in item):
                raise ValueError('mapping keys must be strings')
            return {key: visit(item[key], depth + 1) for key in sorted(item)}
        finally:
            active.remove(identity)

    return visit(value, 0)


def _text(value, label):
    if not isinstance(value, str) or not value.strip():
        raise ValueError('{} must be a nonempty string'.format(label))


def _number(value):
    return type(value) is int or (type(value) is float and math.isfinite(value))


def _walk(trigger):
    """Walk bounded syntax, validating node shape before yielding a leaf."""
    count = 0
    stack = [(trigger, 1, '0')]
    while stack:
        node, depth, path = stack.pop()
        count += 1
        if depth > MAX_DEPTH or count > MAX_NODES:
            raise ValueError('trigger exceeds depth 8 or size 100')
        if type(node) is not dict or len(node) != 1:
            raise ValueError('each trigger must be a mapping with exactly one key')
        kind, value = next(iter(node.items()))
        if kind in ('all', 'any'):
            if type(value) is not list or not value:
                raise ValueError('{} requires a nonempty child list'.format(kind))
            # Fail before allocating a stack from an unbounded user list.
            if len(value) > MAX_NODES - count:
                raise ValueError('trigger exceeds size 100')
            stack.extend((child, depth + 1, '{}.{}'.format(path, index))
                         for index, child in reversed(list(enumerate(value))))
        elif kind not in ('at', 'changed', 'condition', 'completed', 'external', 'manual'):
            raise ValueError('unknown trigger kind: {}'.format(kind))
        yield kind, value, path


def _validate(trigger, related):
    for kind, value, _ in _walk(trigger):
        if kind == 'at':
            parse_time(value)
        elif kind in ('changed', 'completed', 'manual'):
            _text(value, kind)
            if kind == 'changed' and related is not None and value not in related:
                raise ValueError('graph reference must be declared in related: {}'.format(value))
        elif kind == 'condition':
            if type(value) is not dict or set(value) != {'id', 'op', 'value'}:
                raise ValueError('condition requires exactly id, op and value')
            _text(value['id'], 'condition id')
            if related is not None and value['id'] not in related:
                raise ValueError('graph reference must be declared in related: {}'.format(value['id']))
            if type(value['op']) is not str or value['op'] not in _OPS:
                raise ValueError('unknown condition operator')
            scalar = normalize(value['value'])
            if isinstance(scalar, (dict, list)) and _exact_number(scalar) is None:
                raise ValueError('condition comparison value must be scalar')
            if value['op'] in ('<', '<=', '>', '>=') and _exact_number(scalar) is None:
                raise ValueError('ordering requires a finite numeric comparison value')
        elif kind == 'external':
            if (type(value) is not dict or not {'ref', 'equals'} <= set(value)
                    or set(value) - {'ref', 'equals', 'max_age_hours'}):
                raise ValueError('external requires ref, equals and optional max_age_hours')
            _text(value['ref'], 'external ref')
            normalize(value['equals'])
            age = value.get('max_age_hours', 24)
            if not _number(age) or age <= 0:
                raise ValueError('max_age_hours must be a positive finite number')
            try:
                dt.timedelta(hours=age)
            except (OverflowError, TypeError) as exc:
                raise ValueError('max_age_hours is too large') from exc


def validate(trigger, related):
    """Reject malformed expressions and undeclared graph references."""
    if not isinstance(related, (list, tuple, set, frozenset)):
        raise ValueError('related must be a collection of graph ids')
    for identity in related:
        _text(identity, 'related id')
    _validate(trigger, set(related))


def _references(trigger, target):
    _validate(trigger, None)
    result = set()
    for kind, value, _ in _walk(trigger):
        if target == 'graph' and kind in ('changed', 'condition'):
            result.add(value if kind == 'changed' else value['id'])
        elif target == 'tasks' and kind == 'completed':
            result.add(value)
        elif target == 'external' and kind == 'external':
            result.add(value['ref'])
    return result


def referenced_tasks(trigger):
    return _references(trigger, 'tasks')


def referenced_graph(trigger):
    return _references(trigger, 'graph')


def referenced_external(trigger):
    return _references(trigger, 'external')


def _equal(left, right):
    left_number, right_number = _exact_number(left), _exact_number(right)
    if left_number is not None and right_number is not None:
        return left_number == right_number
    if type(left) is not type(right):
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(_equal(left[key], right[key]) for key in left)
    if isinstance(left, list):
        return len(left) == len(right) and all(_equal(a, b) for a, b in zip(left, right))
    return left == right


def _condition_scalar(value):
    """Conditions read the result; changed triggers retain the full historical basis."""
    if isinstance(value, dict) and set(value) == {'computed'}:
        calculated = value['computed']
        if not isinstance(calculated, dict) or not {'value', 'rule'} <= set(calculated):
            return _MISSING
        value = calculated['value']
        if value is None:
            return _MISSING
    if isinstance(value, dict) and set(value) == {'rational'} and _exact_number(value) is None:
        return _MISSING
    return value


def unavailable(value):
    """A failed read is distinct from a recorded null, for conditions and changes."""
    if not isinstance(value, dict):
        return False
    if set(value) == {'unavailable'} and isinstance(value['unavailable'], str):
        return True
    if set(value) == {'computed'}:
        computed = value['computed']
        return not isinstance(computed, dict) or computed.get('value') is None
    return False


def evaluate(trigger, values, baseline, completed, observations, now, zone='UTC'):
    """Evaluate to true, false or unknown, with stable relevant inputs and wake time."""
    _validate(trigger, None)
    if not isinstance(now, dt.datetime):
        raise ValueError('now must be an aware datetime')
    now = parse_time(now, zone)
    inputs = {}
    reasons = []
    wakes = []

    def snapshot(group, mapping, identity):
        raw = mapping.get(identity, _MISSING)
        if raw is _MISSING or unavailable(raw):
            state = {'available': False}
            if raw is not _MISSING:
                state['detail'] = raw
            result = _MISSING
        else:
            try:
                result = normalize(raw)
                state = {'available': True, 'value': result}
            except ValueError:
                result = _MISSING
                state = {'available': False, 'invalid': True}
        inputs.setdefault(group, {})[identity] = state
        return result

    def visit(node, path):
        kind, spec = next(iter(node.items()))
        if kind in ('all', 'any'):
            results = [visit(child, '{}.{}'.format(path, index)) for index, child in enumerate(spec)]
            if kind == 'all':
                return False if False in results else (None if None in results else True)
            return True if True in results else (None if None in results else False)
        if kind == 'at':
            target = parse_time(spec, zone)
            result = now >= target
            inputs.setdefault('at', {})[path] = {'time': _iso(target), 'reached': result}
            if not result:
                wakes.append(target)
            reasons.append('{} time {}'.format('Reached' if result else 'Awaiting', _iso(target)))
            return result
        if kind == 'manual':
            inputs.setdefault('manual', {})[path] = spec
            reasons.append('Manual confirmation required: {}'.format(spec))
            return None
        if kind == 'completed':
            result = spec in completed
            inputs.setdefault('completed', {})[spec] = result
            reasons.append('{} is {}'.format(spec, 'completed' if result else 'not completed'))
            return result
        if kind == 'changed':
            current = snapshot('graph', values, spec)
            previous = snapshot('baseline', baseline, spec)
            result = None if current is _MISSING or previous is _MISSING else not _equal(current, previous)
            reasons.append('{} {}'.format(spec, 'has unknown current/baseline value' if result is None else
                                           ('changed' if result else 'matches baseline')))
            return result
        if kind == 'condition':
            current = _condition_scalar(snapshot('graph', values, spec['id']))
            expected = normalize(spec['value'])
            op = spec['op']
            if current is _MISSING:
                result = None
            elif op in ('==', '!='):
                result = _equal(current, expected)
                if op == '!=':
                    result = not result
            elif _exact_number(current) is None:
                result = None
            else:
                left, right = _exact_number(current), _exact_number(expected)
                result = {'<': lambda: left < right, '<=': lambda: left <= right,
                          '>': lambda: left > right, '>=': lambda: left >= right}[op]()
            inputs.setdefault('condition', {})[path] = {'id': spec['id'], 'op': op, 'value': expected, 'state': result}
            reasons.append('Condition on {} is {}'.format(spec['id'], 'unknown' if result is None else str(result).lower()))
            return result
        ref = spec['ref']
        obs = observations.get(ref)
        state = {'status': 'missing'}
        result = None
        if obs is not None:
            try:
                if type(obs) is not dict or not {'value', 'observed_at', 'evidence'} <= set(obs):
                    raise ValueError('incomplete observation')
                _text(obs['evidence'], 'observation evidence')
                # Observations require offset timestamps, never dates.
                if not isinstance(obs['observed_at'], dt.datetime) and not (
                        isinstance(obs['observed_at'], str) and _STAMP.fullmatch(obs['observed_at'])):
                    raise ValueError('observation requires an offset timestamp')
                observed = parse_time(obs['observed_at'])
                observed_value = normalize(obs['value'])
                expiry = observed + dt.timedelta(hours=spec.get('max_age_hours', 24))
                status = 'future' if observed > now else ('expired' if now >= expiry else 'fresh')
                state = {'status': status, 'value': observed_value, 'observed_at': _iso(observed),
                         'evidence': obs['evidence']}
                if status == 'fresh':
                    result = _equal(observed_value, normalize(spec['equals']))
                    wakes.append(expiry)
            except (ValueError, OverflowError, TypeError):
                state = {'status': 'invalid'}
        # A ref may appear with different freshness windows, so retain per-leaf state.
        inputs.setdefault('external', {})[path] = {'ref': ref, 'equals': normalize(spec['equals']),
                                                 'max_age_hours': spec.get('max_age_hours', 24), **state}
        reasons.append('External observation {} is {}'.format(ref, state['status']))
        return result

    value = visit(trigger, '0')
    return {'value': value, 'reasons': reasons, 'inputs': inputs,
            'next_at': _iso(min(wakes)) if wakes else None}
