"""Lower the explicit scalar language without evaluating it in Python."""
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
            if item['op'] not in MODULES['arithmetic/v1']['operators']:
                raise CapabilityError('unsupported_capability', 'operator is not registered in arithmetic/v1')
            if not isinstance(item['args'], list) or len(item['args']) != 2:
                raise ValueError('a scalar operator requires two arguments')
            active.add(id(item))
            try:
                return {'op': item['op'], 'args': [validate(child, depth + 1) for child in item['args']]}
            finally:
                active.remove(id(item))
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
        raise ValueError('invalid scalar expression fields')
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


def references(expression):
    """All syntactic reads, including missing nodes; quoted names are literals."""
    pending, found = [expression], set()
    while pending:
        node = pending.pop()
        if 'ref' in node:
            found.add(node['ref'])
        pending.extend(node.get('args', []))
    return sorted(found)


def literal(value):
    if value is None:
        return {'null': True}
    if type(value) is bool:
        return {'bool': value}
    if type(value) is int:
        return lower({'num': str(value)})
    if type(value) is float and math.isfinite(value):
        return lower({'num': str(value)})
    if isinstance(value, str):
        return {'text': value}
    raise ValueError('input is outside the scalar arithmetic/v1 value types')


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
