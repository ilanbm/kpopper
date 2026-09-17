"""Strict, data-only KP2/KR2 transport for the packaged scalar kernel.

This module validates framing and tags; it does not evaluate expressions, locate
or build executables, download artifacts, or supply a semantic fallback.
"""
import math
import re

MAX_BYTES = 16 * 1024 * 1024
# Missing IDs may occur once in the request but twice in response witnesses.
MAX_RESPONSE_BYTES = 64 * 1024 * 1024
DEFAULT_LIMITS = {"steps": 1_000_000, "depth": 128, "digits": 256}
HARD_LIMITS = {"steps": 10_000_000, "depth": 4096, "digits": 4096}
OPS = frozenset(("add", "sub", "mul", "div", "eq", "ne", "lt", "le", "gt", "ge"))
UNAVAILABLE = frozenset(("missing_reference", "missing_input", "contested", "unavailable_input"))
DIAGNOSTICS = UNAVAILABLE | frozenset(("invalid_expression", "division_by_zero", "cyclic_reference", "type_error", "undeclared_dependency", "invalid_transport", "invalid_limits", "unsupported_capability", "step_limit", "depth_limit", "parser_depth_limit", "number_limit", "edge_limit", "expression_limit", "transport_limit", "node_limit"))
NUMBER = re.compile(r"-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?\Z", re.ASCII)
NATURAL = re.compile(r"(?:0|[1-9][0-9]*)\Z", re.ASCII)
INTEGER = re.compile(r"(?:0|-?[1-9][0-9]*)\Z", re.ASCII)
HEX = re.compile(r"(?:[0-9a-f]{2})*\Z", re.ASCII)


def _text(value):
    if not isinstance(value, str):
        raise ValueError("transport text must be a string")
    try:
        return value.encode("utf-8").hex()
    except UnicodeError as exc:
        raise ValueError("invalid UTF-8 text") from exc


def _encode_request_v2(request):
    """Encode nodes/expression/declared/limits without computing their meaning.

    ``declared`` is the adapter's complete allowed transitive closure. The public
    evaluator separately checks the user's direct declarations before deriving it.
    Expressions use {num: decimal-string}, {bool: bool}, {text: str}, {null: True},
    {ref: str}, {unavailable: code}, or {op: name, args: [left, right]}.
    """
    if not isinstance(request, dict) or not {"nodes", "expression", "declared"} <= request.keys() or request.keys() - {"nodes", "expression", "declared", "limits"}:
        raise ValueError("invalid request fields")
    limits = request.get("limits")
    if limits is None:
        limits = {}
    if not isinstance(limits, dict) or limits.keys() - HARD_LIMITS.keys():
        raise ValueError("invalid limits")
    bounds = dict(DEFAULT_LIMITS, **limits)
    if any(type(n) is not int or n < 1 or n > HARD_LIMITS[k] for k, n in bounds.items()):
        raise ValueError("invalid limits")
    declared = request["declared"]
    nodes = request["nodes"]
    if not isinstance(declared, (list, tuple)) or len(declared) > 100_000 or not all(isinstance(x, str) for x in declared) or len(set(declared)) != len(declared):
        raise ValueError("invalid declared IDs")
    if not isinstance(nodes, dict) or len(nodes) > 20_000 or not all(isinstance(x, str) for x in nodes):
        raise ValueError("invalid nodes")
    tokens = ["KP2", str(bounds["steps"]), str(bounds["depth"]), str(bounds["digits"]), str(len(declared))]
    tokens.extend(_text(x) for x in sorted(declared))
    tokens.append(str(len(nodes)))
    expression_count = 0

    def expression(expr):
        # An explicit stack also rejects cyclic Python values without RecursionError.
        nonlocal expression_count
        pending = [(expr, 0, frozenset())]
        while pending:
            current, depth, ancestors = pending.pop()
            expression_count += 1
            if expression_count > 1_000_000 or depth > 128:
                raise ValueError("expression limit")
            if not isinstance(current, dict) or id(current) in ancestors:
                raise ValueError("invalid or cyclic expression")
            keys = set(current)
            if keys == {"num"}:
                value = current["num"]
                if not isinstance(value, str) or len(value) > 8200 or not NUMBER.fullmatch(value):
                    raise ValueError("invalid number lexeme")
                tokens.extend(("n", value))
            elif keys == {"bool"} and type(current["bool"]) is bool:
                tokens.extend(("b", "1" if current["bool"] else "0"))
            elif keys == {"text"}:
                tokens.extend(("s", _text(current["text"])))
            elif keys == {"null"} and current["null"] is True:
                tokens.append("z")
            elif keys == {"ref"}:
                tokens.extend(("r", _text(current["ref"])))
            elif keys == {"unavailable"} and isinstance(current["unavailable"], str) and current["unavailable"] in UNAVAILABLE:
                tokens.extend(("u", current["unavailable"]))
            elif keys == {"op", "args"}:
                op, args = current["op"], current["args"]
                # Unknown operation names are sent as safe tokens so the compiled
                # registry returns the public unsupported_capability status.
                if not isinstance(op, str) or not re.fullmatch(r"[a-z][a-z0-9_./-]*", op, re.ASCII) or not isinstance(args, (list, tuple)) or len(args) != 2:
                    raise ValueError("invalid operation")
                tokens.extend(("o", op, "2"))
                seen = ancestors | {id(current)}
                pending.append((args[1], depth + 1, seen))
                pending.append((args[0], depth + 1, seen))
            else:
                raise ValueError("invalid expression")
    for node in sorted(nodes):
        tokens.append(_text(node))
        expression(nodes[node])
    expression(request["expression"])
    line = "\t".join(tokens)
    if len(line) > MAX_BYTES:
        raise ValueError("transport limit")
    return line


def _decode_response_v2(line):
    """Decode one canonical KR2 response, refusing ambiguous or forged framing."""
    if not isinstance(line, str) or len(line) > MAX_RESPONSE_BYTES:
        raise ValueError("invalid response")
    if line.endswith("\n"):
        line = line[:-1]
    if "\n" in line or "\r" in line or not line.isascii():
        raise ValueError("invalid response framing")
    tokens = iter(line.split("\t"))

    def take():
        try:
            return next(tokens)
        except StopIteration as exc:
            raise ValueError("truncated response") from exc

    def natural(bound):
        value = take()
        if len(value) > 8 or not NATURAL.fullmatch(value):
            raise ValueError("invalid count")
        number = int(value)
        if number > bound:
            raise ValueError("count limit")
        return number

    def text():
        value = take()
        if not HEX.fullmatch(value):
            raise ValueError("invalid hex")
        try:
            return bytes.fromhex(value).decode("utf-8")
        except UnicodeError as exc:
            raise ValueError("invalid UTF-8") from exc

    def ordered(items):
        if items != sorted(set(items)):
            raise ValueError("noncanonical or duplicate response fields")
        return items

    if take() != "KR2":
        raise ValueError("unsupported response protocol")
    status = take()
    if status not in ("ok", "unknown", "error", "limit", "unsupported_capability"):
        raise ValueError("invalid status")
    tag = take()
    if tag == "n":
        numerator, denominator = take(), take()
        if len(numerator.lstrip("-")) > 4096 or len(denominator) > 4096 or not INTEGER.fullmatch(numerator) or not NATURAL.fullmatch(denominator) or denominator == "0":
            raise ValueError("invalid rational value")
        if math.gcd(int(numerator), int(denominator)) != 1:
            raise ValueError("noncanonical rational value")
        value = {"type": "number", "numerator": numerator, "denominator": denominator}
    elif tag == "b":
        boolean = take()
        if boolean not in ("0", "1"):
            raise ValueError("invalid boolean")
        value = {"type": "boolean", "value": boolean == "1"}
    elif tag == "s":
        value = {"type": "text", "value": text()}
    elif tag == "z":
        value = {"type": "null"}
    elif tag == "u":
        value = None
    else:
        raise ValueError("invalid value tag")
    if (status == "ok") != (value is not None):
        raise ValueError("status/value mismatch")
    diagnostics = ordered([take() for _ in range(natural(len(DIAGNOSTICS)))])
    if any(code not in DIAGNOSTICS for code in diagnostics):
        raise ValueError("invalid diagnostic")
    potential = ordered([text() for _ in range(natural(100_000))])
    executed = ordered([text() for _ in range(natural(100_000))])
    if not set(executed) <= set(potential):
        raise ValueError("executed reads outside potential closure")
    steps = natural(10_000_000)
    preflight_steps = natural(10_000_000)
    counts = [(text(), natural(1)) for _ in range(natural(20_000))]
    ordered([node for node, _ in counts])
    executed_set = set(executed)
    if any(count != 1 or node not in executed_set for node, count in counts):
        raise ValueError("invalid node evaluation count")
    if next(tokens, None) is not None:
        raise ValueError("trailing response data")
    return {"status": status, "value": value, "diagnostics": diagnostics,
            "potential_reads": potential, "executed_reads": executed,
            "steps": steps, "preflight_steps": preflight_steps,
            "node_evaluations": dict(counts)}


VALUE_LIMITS = {'value_nodes': 10000, 'value_depth': 128, 'value_bytes': 16777216}
DIAGNOSTICS_V3 = DIAGNOSTICS | frozenset(('missing_field', 'collection_limit'))


def encode_request(request):
    """Select an explicit grammar; KP2 never silently accepts composed nodes."""
    if isinstance(request, dict) and request.get('protocol') == 'KP3':
        return _encode_request_v3(request)
    return _encode_request_v2(request)


def _encode_request_v3(request):
    allowed = {'protocol', 'nodes', 'expression', 'declared', 'limits'}
    if request.keys() - allowed or not {'protocol', 'nodes', 'expression', 'declared'} <= request.keys():
        raise ValueError('invalid request fields')
    limits = request.get('limits')
    limits = {} if limits is None else limits
    hard = dict(HARD_LIMITS, **VALUE_LIMITS)
    if not isinstance(limits, dict) or limits.keys() - hard.keys():
        raise ValueError('invalid limits')
    bounds = dict(DEFAULT_LIMITS, **VALUE_LIMITS)
    bounds.update(limits)
    if any(type(value) is not int or not 0 < value <= hard[key] for key, value in bounds.items()):
        raise ValueError('invalid limits')
    declared, nodes = request['declared'], request['nodes']

    def identifier(value):
        return isinstance(value, str) and 0 < len(value) <= 500

    if not isinstance(declared, (list, tuple)) or len(declared) > 100000 \
            or not all(identifier(value) for value in declared) or len(set(declared)) != len(declared):
        raise ValueError('invalid declared IDs')
    if not isinstance(nodes, dict) or len(nodes) > 20000 or not all(identifier(key) for key in nodes):
        raise ValueError('invalid nodes')
    tokens, used, count, active = [], 0, 0, set()

    def put(*values):
        nonlocal used
        for value in values:
            size = len(value) + bool(tokens)
            if used + size > MAX_BYTES:
                raise ValueError('transport limit')
            used += size
            tokens.append(value)

    def text(value):
        if not isinstance(value, str) or len(value) > MAX_BYTES // 2:
            raise ValueError('invalid or oversized text')
        try:
            data = value.encode('utf-8')
        except UnicodeError as error:
            raise ValueError('invalid UTF-8 text') from error
        if used + 2 * len(data) + bool(tokens) > MAX_BYTES:
            raise ValueError('transport limit')
        return data.hex()

    def expression(item, depth=0):
        nonlocal count
        count += 1
        if count > 1000000 or depth > 128:
            raise ValueError('expression limit')
        if not isinstance(item, dict) or id(item) in active:
            raise ValueError('invalid or cyclic expression')
        active.add(id(item))
        try:
            keys = set(item)
            if keys == {'num'}:
                value = item['num']
                if not isinstance(value, str) or len(value) > 8200 or not NUMBER.fullmatch(value):
                    raise ValueError('invalid number lexeme')
                put('n', value)
            elif keys == {'bool'} and type(item['bool']) is bool:
                put('b', '1' if item['bool'] else '0')
            elif keys == {'text'}:
                put('s', text(item['text']))
            elif keys == {'null'} and item['null'] is True:
                put('z')
            elif keys == {'ref'} and identifier(item['ref']):
                put('r', text(item['ref']))
            elif keys == {'unavailable'} and isinstance(item['unavailable'], str) and item['unavailable'] in UNAVAILABLE:
                put('u', item['unavailable'])
            elif keys == {'op', 'args'}:
                name, args = item['op'], item['args']
                if not isinstance(name, str) or not re.fullmatch(r'[a-z][a-z0-9_./-]*', name, re.ASCII) \
                        or not isinstance(args, (list, tuple)) or len(args) != (1 if name == 'not' else 2):
                    raise ValueError('invalid operation')
                put('o', name, str(len(args)))
                for child in args:
                    expression(child, depth + 1)
            elif keys == {'if', 'then', 'else'}:
                put('i')
                for key in ('if', 'then', 'else'):
                    expression(item[key], depth + 1)
            elif keys == {'list'} and isinstance(item['list'], list) and len(item['list']) <= 10000:
                put('l', str(len(item['list'])))
                for child in item['list']:
                    expression(child, depth + 1)
            elif keys == {'record'} and isinstance(item['record'], dict) and len(item['record']) <= 10000:
                fields = item['record']
                if not all(isinstance(key, str) and len(key) <= 500 for key in fields):
                    raise ValueError('invalid record key')
                put('m', str(len(fields)))
                for key in sorted(fields):
                    put(text(key))
                    expression(fields[key], depth + 1)
            elif keys == {'field', 'key'} and isinstance(item['key'], str) and len(item['key']) <= 500:
                put('f', text(item['key']))
                expression(item['field'], depth + 1)
            else:
                raise ValueError('invalid expression')
        finally:
            active.remove(id(item))

    put('KP3', *(str(bounds[key]) for key in ('steps', 'depth', 'digits', 'value_nodes', 'value_depth', 'value_bytes')))
    put(str(len(declared)))
    for name in sorted(declared):
        put(text(name))
    put(str(len(nodes)))
    for name in sorted(nodes):
        put(text(name))
        expression(nodes[name])
    expression(request['expression'])
    return '\t'.join(tokens)


def decode_response(line):
    if isinstance(line, str) and line.startswith('KR3\t'):
        return _decode_response_v3(line)
    return _decode_response_v2(line)


def _decode_response_v3(line):
    if len(line) > MAX_RESPONSE_BYTES:
        raise ValueError('invalid response')
    if line.endswith('\n'):
        line = line[:-1]
    if '\n' in line or '\r' in line or not line.isascii():
        raise ValueError('invalid response framing')
    position, value_start, value_nodes = 0, None, 0

    def take():
        nonlocal position
        if position > len(line):
            raise ValueError('truncated response')
        end = line.find('\t', position)
        if end < 0:
            end = len(line)
        if value_start is not None and end - value_start > VALUE_LIMITS['value_bytes']:
            raise ValueError('value byte limit')
        token = line[position:end]
        position = end + 1
        return token

    def natural(bound):
        token = take()
        if len(token) > 8 or not NATURAL.fullmatch(token) or int(token) > bound:
            raise ValueError('invalid count')
        return int(token)

    def text():
        token = take()
        if not HEX.fullmatch(token):
            raise ValueError('invalid hex')
        try:
            return bytes.fromhex(token).decode('utf-8')
        except UnicodeError as error:
            raise ValueError('invalid UTF-8') from error

    def value(depth=0):
        nonlocal value_nodes
        value_nodes += 1
        if value_nodes > VALUE_LIMITS['value_nodes'] or depth > VALUE_LIMITS['value_depth']:
            raise ValueError('value collection limit')
        tag = take()
        if tag == 'u' and depth == 0:
            return None
        if tag == 'n':
            numerator, denominator = take(), take()
            if len(numerator.lstrip('-')) > 4096 or len(denominator) > 4096 \
                    or not INTEGER.fullmatch(numerator) or not NATURAL.fullmatch(denominator) \
                    or denominator == '0' or math.gcd(int(numerator), int(denominator)) != 1:
                raise ValueError('invalid rational value')
            return {'type': 'number', 'numerator': numerator, 'denominator': denominator}
        if tag == 'b':
            token = take()
            if token not in ('0', '1'):
                raise ValueError('invalid boolean')
            return {'type': 'boolean', 'value': token == '1'}
        if tag == 's':
            return {'type': 'text', 'value': text()}
        if tag == 'z':
            return {'type': 'null'}
        if tag == 'l':
            return {'type': 'list', 'items': [value(depth + 1) for _ in range(natural(10000))]}
        if tag == 'm':
            fields, previous = {}, None
            for _ in range(natural(10000)):
                key = text()
                if len(key) > 500 or (previous is not None and key <= previous):
                    raise ValueError('noncanonical or duplicate record key')
                previous = key
                fields[key] = value(depth + 1)
            return {'type': 'record', 'fields': fields}
        raise ValueError('invalid value tag')

    def ordered(values):
        if values != sorted(set(values)):
            raise ValueError('noncanonical or duplicate response fields')
        return values

    if take() != 'KR3':
        raise ValueError('unsupported response protocol')
    status = take()
    if status not in ('ok', 'unknown', 'error', 'limit', 'unsupported_capability'):
        raise ValueError('invalid status')
    value_start = position
    result = value()
    value_start = None
    if (status == 'ok') != (result is not None):
        raise ValueError('status/value mismatch')
    diagnostics = ordered([take() for _ in range(natural(len(DIAGNOSTICS_V3)))])
    if any(code not in DIAGNOSTICS_V3 for code in diagnostics):
        raise ValueError('invalid diagnostic')
    potential = ordered([text() for _ in range(natural(100000))])
    executed = ordered([text() for _ in range(natural(100000))])
    if not set(executed) <= set(potential):
        raise ValueError('executed reads outside potential closure')
    steps, preflight = natural(10000000), natural(10000000)
    counts = [(text(), natural(1)) for _ in range(natural(20000))]
    ordered([name for name, _ in counts])
    executed_set = set(executed)
    if any(count != 1 or name not in executed_set for name, count in counts):
        raise ValueError('invalid node evaluation count')
    if position <= len(line):
        raise ValueError('trailing response data')
    return {'status': status, 'value': result, 'diagnostics': diagnostics,
            'potential_reads': potential, 'executed_reads': executed,
            'steps': steps, 'preflight_steps': preflight, 'node_evaluations': dict(counts)}
