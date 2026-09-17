"""Lower the explicit finite language without evaluating it in Python."""
import ast
import copy
import math
from functools import lru_cache

from .contract import MODULES, CapabilityError
from .transport import NUMBER


def lower(expression):
    """The core profile has its own grammar bounds; legacy syntax stays unchanged."""
    if isinstance(expression, dict) and set(expression) == {'expr'}:
        source = expression['expr']
        if not isinstance(source, str) or not source.strip() or len(source) > 4000:
            raise ValueError('expr must be nonempty text within 4000 characters')
        expression = copy.deepcopy(_parse(source.strip()))
    active = set()
    def validate(item, depth=0):
        if depth > 128 or not isinstance(item, dict) or id(item) in active:
            raise ValueError('expression nesting exceeds core/v1 bounds or contains a cycle')
        keys = set(item)
        if keys == {'op', 'args'}:
            if item['op'] not in (*MODULES['arithmetic/v1']['operators'], *MODULES['composition/v1']['operators']):
                raise CapabilityError('unsupported_capability', 'operator is not registered')
            if not isinstance(item['args'], list) or len(item['args']) != (1 if item['op'] == 'not' else 2):
                raise ValueError('invalid operator arity')
            active.add(id(item))
            try:
                return {'op': item['op'], 'args': [validate(child, depth + 1) for child in item['args']]}
            finally:
                active.remove(id(item))
        if keys in ({'if', 'then', 'else'}, {'list'}, {'record'}, {'field', 'key'}):
            active.add(id(item))
            try:
                if keys == {'if', 'then', 'else'}:
                    return {key: validate(item[key], depth + 1) for key in ('if', 'then', 'else')}
                if keys == {'list'} and isinstance(item['list'], list):
                    return {'list': [validate(child, depth + 1) for child in item['list']]}
                if keys == {'record'} and isinstance(item['record'], dict) and all(
                        isinstance(key, str) for key in item['record']):
                    return {'record': {key: validate(item['record'][key], depth + 1)
                                       for key in sorted(item['record'])}}
                if keys == {'field', 'key'} and isinstance(item['key'], str):
                    return {'field': validate(item['field'], depth + 1), 'key': item['key']}
            finally:
                active.remove(id(item))
            raise ValueError('invalid composition expression fields')
        if keys == {'num'} and isinstance(item['num'], str) and len(item['num']) <= 8200 \
                and NUMBER.fullmatch(item['num']):
            return dict(item)
        if keys == {'bool'} and type(item['bool']) is bool:
            return dict(item)
        if keys == {'text'} and isinstance(item['text'], str):
            return dict(item)
        if keys == {'null'} and item['null'] is True:
            return dict(item)
        if keys == {'ref'} and isinstance(item['ref'], str) and 0 < len(item['ref']) <= 500:
            return dict(item)
        raise ValueError('invalid expression fields')
    return validate(expression)


@lru_cache(maxsize=512)
def _parse(source):
    syntax = ast.parse(source, mode='eval')
    operators = {ast.Add: 'add', ast.Sub: 'sub', ast.Mult: 'mul', ast.Div: 'div',
                 ast.Eq: 'eq', ast.NotEq: 'ne', ast.Lt: 'lt', ast.LtE: 'le', ast.Gt: 'gt', ast.GtE: 'ge'}
    def visit(node, depth=0):
        if depth > 128:
            raise ValueError('expression nesting exceeds 128')
        if isinstance(node, (ast.Name, ast.Attribute)):
            spelling = ast.get_source_segment(source, node)
            parts = [part.strip() for part in spelling.split('.')]
            if not all(part.isidentifier() for part in parts):
                raise ValueError('invalid reference')
            name = '.'.join(parts)
            if name in ('true', 'false', 'True', 'False'):
                return {'bool': name.lower() == 'true'}
            if name == 'null':
                return {'null': True}
            return {'ref': name}
        if isinstance(node, ast.Constant):
            if node.value is None:
                return {'null': True}
            if type(node.value) is bool:
                return {'bool': node.value}
            if isinstance(node.value, str):
                return {'text': node.value}
            if type(node.value) in (int, float):
                return {'num': ast.get_source_segment(source, node)}
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == 'ref' \
                and len(node.args) == 1 and not node.keywords and isinstance(node.args[0], ast.Constant) \
                and isinstance(node.args[0].value, str):
            return {'ref': node.args[0].value}
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == 'field' \
                and len(node.args) == 2 and not node.keywords and isinstance(node.args[1], ast.Constant) \
                and isinstance(node.args[1].value, str):
            return {'field': visit(node.args[0], depth + 1), 'key': node.args[1].value}
        if isinstance(node, ast.BoolOp) and isinstance(node.op, (ast.And, ast.Or)):
            values = [visit(child, depth + 1) for child in node.values]
            result = values[0]
            for child in values[1:]:
                result = {'op': 'and' if isinstance(node.op, ast.And) else 'or', 'args': [result, child]}
            return result
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
            return {'op': 'not', 'args': [visit(node.operand, depth + 1)]}
        if isinstance(node, ast.IfExp):
            return {'if': visit(node.test, depth + 1), 'then': visit(node.body, depth + 1),
                    'else': visit(node.orelse, depth + 1)}
        if isinstance(node, ast.List):
            return {'list': [visit(child, depth + 1) for child in node.elts]}
        if isinstance(node, ast.Dict):
            if any(not isinstance(key, ast.Constant) or not isinstance(key.value, str) for key in node.keys):
                raise ValueError('record keys must be literal strings')
            keys = [key.value for key in node.keys]
            if len(keys) != len(set(keys)):
                raise ValueError('duplicate record key')
            return {'record': {key: visit(child, depth + 1) for key, child in zip(keys, node.values)}}
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, (ast.USub, ast.UAdd)):
            child = visit(node.operand, depth + 1)
            if isinstance(node.op, ast.UAdd):
                return {'op': 'add', 'args': [{'num': '0'}, child]}
            if 'num' in child:
                return {'num': child['num'][1:] if child['num'].startswith('-') else '-' + child['num']}
            return {'op': 'sub', 'args': [{'num': '0'}, child]}
        if isinstance(node, ast.BinOp) and type(node.op) in operators:
            return {'op': operators[type(node.op)], 'args': [visit(node.left, depth + 1), visit(node.right, depth + 1)]}
        if isinstance(node, ast.Compare) and len(node.ops) == 1 and type(node.ops[0]) in operators:
            return {'op': operators[type(node.ops[0])], 'args': [visit(node.left, depth + 1), visit(node.comparators[0], depth + 1)]}
        raise ValueError('unsupported core/v1 expression syntax')
    return visit(syntax.body)


def children(expression):
    """Ordered complete potential children, independent of runtime branch choice."""
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


def required_modules(*expressions):
    """Requirements of validated syntax, including all potential branches."""
    found, pending = {'arithmetic/v1'}, list(expressions)
    while pending:
        item = pending.pop()
        if any(key in item for key in ('if', 'list', 'record', 'field')) or item.get('op') in ('and', 'or', 'not'):
            found.add('composition/v1')
        pending.extend(children(item))
    return sorted(found)


def references(expression):
    """All syntactic reads, including missing nodes; quoted names are literals."""
    pending, found = [expression], set()
    while pending:
        node = pending.pop()
        if 'ref' in node:
            found.add(node['ref'])
        pending.extend(children(node))
    return sorted(found)


def literal(value):
    active = set()
    def convert(item, depth=0):
        if depth > 128 or id(item) in active:
            raise ValueError('literal exceeds core/v1 bounds or contains a cycle')
        if item is None:
            return {'null': True}
        if type(item) is bool:
            return {'bool': item}
        if type(item) is int or type(item) is float and math.isfinite(item):
            return lower({'num': str(item)})
        if isinstance(item, str):
            return {'text': item}
        if isinstance(item, (list, dict)):
            active.add(id(item))
            try:
                if isinstance(item, list):
                    return {'list': [convert(child, depth + 1) for child in item]}
                if all(isinstance(key, str) for key in item):
                    return {'record': {key: convert(item[key], depth + 1) for key in sorted(item)}}
            finally:
                active.remove(id(item))
        raise ValueError('input is outside core/v1 finite value types')
    return convert(value)


def node_expression(node):
    body = node.get('body')
    if not isinstance(body, dict):
        return literal(body)
    if 'rule' in body and body['rule'] is not None:
        return lower(body['rule'])
    for key in ('v', 'quoted'):
        if key in body:
            return literal(body[key])
    return {'unavailable': 'missing_reference'}


def closure(snapshot_data, roots):
    """Compile one potential component, preserving unrelated component failures."""
    from .contract import MAX_NODES, MAX_EDGES
    nodes, expressions, errors = snapshot_data['nodes'], {}, {}
    pending, seen, edges = list(roots), set(), 0
    conflicts = snapshot_data.get('context', {}).get('conflicts', {})
    while pending:
        nid = pending.pop()
        if nid in seen:
            continue
        seen.add(nid)
        if len(seen) > MAX_NODES:
            raise ValueError('node_limit')
        if nid not in nodes:
            continue
        try:
            expression = node_expression(nodes[nid])
        except (ValueError, SyntaxError, TypeError, RecursionError) as error:
            errors[nid] = 'invalid_expression'
            expressions[nid] = {'unavailable': 'invalid_expression'}
            continue
        refs = references(expression)
        edges += len(refs)
        if edges > MAX_EDGES:
            raise ValueError('edge_limit')
        pending.extend(refs)
        expressions[nid] = {'unavailable': 'contested'} if nid in conflicts else expression
    return {'ids': sorted(seen), 'nodes': expressions, 'errors': errors}
