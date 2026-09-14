"""Structured expression data and a bridge to the single Lean evaluator.

Python validates shape, derives references and renders text. Arithmetic is exclusively
computed by the packaged Lean program. New supported formulas normalize at the writer;
existing textual expressions require explicit migration.
"""
import ast
import copy
from functools import lru_cache
import hashlib
import importlib
import importlib.util
import json
import math
from pathlib import Path
import re
import sys
import subprocess
from decimal import Decimal
from fractions import Fraction

LOADED_SOURCE_HASH = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()

ARITHMETIC = {"add": "+", "sub": "-", "mul": "*", "div": "/"}
COMPARISONS = {"eq": "==", "ne": "!=", "lt": "<", "le": "<=", "gt": ">", "ge": ">="}
NUMERIC = re.compile(r"-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?\Z")
GRAMMAR_VERSION = 1


def _refs_valid(tree):
    if "ref" in tree:
        return [tree["ref"]]
    return [key for child in tree.get("args", []) for key in _refs_valid(child)]


def validate(tree, predicate=False, depth=0):
    if depth >= 64 or not isinstance(tree, dict):
        raise ValueError("expression must be a mapping within 64 levels")
    keys = set(tree)
    if depth == 0 and keys == {'expr'}:
        _readable_tree(tree['expr'], predicate)
        return tree
    if keys == {"op", "args"}:
        operators = COMPARISONS if predicate else ARITHMETIC
        if not isinstance(tree["op"], str) or tree["op"] not in operators:
            raise ValueError("unsupported comparison" if predicate else "unsupported arithmetic operator")
        if not isinstance(tree["args"], list) or len(tree["args"]) != 2:
            raise ValueError("an operator requires exactly two arguments")
        for child in tree["args"]:
            validate(child, depth=depth + 1)
        if predicate and not _refs_valid(tree):
            raise ValueError("predicate_needs_a_reference")
    elif predicate:
        raise ValueError("a predicate must be a structured comparison")
    elif keys == {"ref"}:
        if not isinstance(tree["ref"], str) or not tree["ref"] or len(tree["ref"]) > 500:
            raise ValueError("ref must be an entry ID")
    elif keys == {"num"}:
        text = tree["num"]
        if not isinstance(text, str) or len(text) > 512 or not NUMERIC.fullmatch(text):
            raise ValueError("num must be a bounded decimal string")
        exponent = re.split("[eE]", text)
        if len(exponent) > 1 and abs(int(exponent[1])) > 256:
            raise ValueError("numeric exponent exceeds 256")
        number = Decimal(text)
        if number.as_tuple().exponent < -256:
            raise ValueError("numeric exponent exceeds 256")
    elif keys == {"text"}:
        if not isinstance(tree["text"], str):
            raise ValueError("text must be a string")
    elif keys == {"bool"}:
        if type(tree["bool"]) is not bool:
            raise ValueError("bool must be a boolean")
    else:
        raise ValueError("invalid expression fields")
    return tree


def refs(tree):
    """Only tagged references; ID-looking literal text is never a dependency."""
    if not isinstance(tree, dict):
        return []
    try:
        tree = lower(tree, predicate=predicate_shape(tree))
    except (ValueError, TypeError):
        return []
    return sorted(set(_refs_valid(tree)))


def text(tree):
    if not isinstance(tree, dict):
        return str(tree or "")
    try:
        validate(tree, predicate=predicate_shape(tree))
    except (ValueError, TypeError) as error:
        return "<invalid expression: " + str(error) + ">"
    if set(tree) == {'expr'}:
        return tree['expr']
    def render(node):
        if "ref" in node: return node["ref"]
        if "num" in node: return node["num"]
        if "text" in node: return json.dumps(node["text"], ensure_ascii=False)
        if "bool" in node: return "true" if node["bool"] else "false"
        symbol = {**ARITHMETIC, **COMPARISONS}[node["op"]]
        return "(" + render(node["args"][0]) + " " + symbol + " " + render(node["args"][1]) + ")"
    return render(tree)


def _readable_tree(source, predicate):
    if not isinstance(source, str) or not source.strip() or len(source) > 4000:
        raise ValueError('expr must be nonempty text within 4000 characters')
    tree = _parse_readable(source, GRAMMAR_VERSION)
    if predicate is not None:
        validate(tree, predicate=predicate)
    return tree


@lru_cache(maxsize=512)
def _parse_readable(source, version):
    """Cache syntax only, never values, and never expose the mutable cached tree."""
    try:
        return convert(source.strip(), predicate=None, explicit_refs=True)
    except (SyntaxError, RecursionError) as error:
        raise ValueError('unsupported expression syntax') from error


def predicate_shape(value):
    """Choose only the top-level grammar; malformed formulas are still rejected."""
    if not isinstance(value, dict):
        return False
    if 'expr' in value:
        try:
            return _readable_tree(value['expr'], None).get('op') in COMPARISONS
        except ValueError:
            return False
    return isinstance(value.get('op'), str) and value['op'] in COMPARISONS


def lower(value, predicate=False):
    """Compile an explicit stored expression to a detached Lean protocol tree."""
    validate(value, predicate=predicate)
    return copy.deepcopy(_readable_tree(value['expr'], predicate) if set(value) == {'expr'} else value)


def same(left, right):
    """Representation and whitespace do not change the formula's structure."""
    try:
        if left == right:
            return True
        if not isinstance(left, dict) or not isinstance(right, dict):
            return False
        return lower(left, predicate_shape(left)) == lower(right, predicate_shape(right))
    except (ValueError, TypeError, RecursionError):
        return False


def readable(tree, predicate=False):
    """Render a round-trippable formula, including opaque or reserved graph IDs."""
    tree = lower(tree, predicate)
    def render(node):
        if 'ref' in node:
            key = node['ref']
            try:
                bare = convert(key) == {'ref': key}
            except (ValueError, SyntaxError, RecursionError):
                bare = False
            return key if bare else 'ref(' + json.dumps(key, ensure_ascii=False) + ')'
        if 'num' in node: return node['num']
        if 'text' in node: return json.dumps(node['text'], ensure_ascii=False)
        if 'bool' in node: return 'true' if node['bool'] else 'false'
        return '(' + render(node['args'][0]) + ' ' + {**ARITHMETIC, **COMPARISONS}[node['op']] + ' ' + render(node['args'][1]) + ')'
    source = render(tree)
    if 'op' in tree:
        source = source[1:-1]
    result = {'expr': source}
    if lower(result, predicate) != tree:
        raise ValueError('expression cannot be represented without changing its meaning')
    return result


def rename(value, old, new, predicate=False):
    tree = lower(value, predicate)
    def visit(node):
        if node.get('ref') == old:
            node['ref'] = new
        for child in node.get('args', []):
            visit(child)
    visit(tree)
    return readable(tree, predicate) if set(value) == {'expr'} else tree


def wire_payload(payload):
    """Lower only executable fields on a copy, retaining input evidence verbatim."""
    payload = copy.deepcopy(payload)
    if not isinstance(payload, dict):
        return payload
    def expression(value, predicate=False):
        if isinstance(value, dict) and 'expr' in value:
            try:
                return lower(value, predicate)
            except ValueError:
                pass  # Lean rejects this node locally; other calculations still work.
        return value
    def body(value, predicates, snapshots):
        if not isinstance(value, dict): return
        if 'rule' in value:
            value['rule'] = expression(value['rule'])
        for field in predicates:
            if field in value:
                value[field] = expression(value[field], True)
        for field in snapshots:
            seen = value.get(field)
            if isinstance(seen, dict):
                for snapshot in seen.values():
                    computed = snapshot.get('computed') if isinstance(snapshot, dict) else None
                    if isinstance(computed, dict) and 'rule' in computed:
                        computed['rule'] = expression(computed['rule'])
    record = payload.get('record')
    if isinstance(record, dict) and isinstance(record.get('nodes'), dict):
        for node in record['nodes'].values():
            if not isinstance(node, dict): continue
            fields = node.get('assessment_fields')
            fields = fields if isinstance(fields, dict) else {}
            predicate = fields.get('predicate') if isinstance(fields.get('predicate'), str) else 'wrong_if'
            snapshot = fields.get('snapshot') if isinstance(fields.get('snapshot'), str) else 'seen'
            body(node.get('body'), {predicate}, {snapshot})
            body(node.get('assessment_body'), {predicate, 'wrong_if'}, {snapshot, 'seen'})
    if 'predicate' in payload:
        payload['predicate'] = expression(payload['predicate'], True)
    return payload


def number(value):
    """Decode exact numeric output for display/comparison, never evaluate a formula."""
    if type(value) in (int, float):
        if type(value) is float and not math.isfinite(value):
            return None
        return Fraction(str(value))
    if isinstance(value, dict) and set(value) == {"rational"}:
        pair = value["rational"]
        if isinstance(pair, list) and len(pair) == 2 and all(
                isinstance(item, str) and len(item) <= 1024 for item in pair) and \
                re.fullmatch(r"-?[0-9]+", pair[0]) and re.fullmatch(r"[0-9]+", pair[1]):
            try:
                return Fraction(int(pair[0]), int(pair[1]))
            except (ValueError, TypeError, ZeroDivisionError):
                pass
    return None


def display_value(value):
    exact = number(value)
    return str(exact) if exact is not None else str(value)


def _core_type():
    # Also works when the native reader is loaded by absolute path with no package
    # on sys.path. The namespace is tied to this installation, never another cache.
    root = Path(__file__).resolve().parent / "session"
    name = "_kpopper_expression_core_" + hashlib.sha256(str(root).encode()).hexdigest()[:12]
    if name not in sys.modules:
        spec = importlib.util.spec_from_file_location(name, root / "__init__.py", submodule_search_locations=[str(root)])
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        spec.loader.exec_module(module)
    return importlib.import_module(name + ".core").Core


_CORE = None


def _finite_payload(value, active=None, depth=0):
    """Unavailable JSON readings stay local; never turn NaN into a number or text."""
    if type(value) is float and not math.isfinite(value):
        return None
    if not isinstance(value, (dict, list)):
        return value
    active = set() if active is None else active
    if depth > 128 or id(value) in active:
        return None
    active.add(id(value))
    try:
        if isinstance(value, list):
            return [_finite_payload(v, active, depth + 1) for v in value]
        return {k: _finite_payload(v, active, depth + 1) for k, v in value.items()}
    finally:
        active.remove(id(value))


def _record_body(body):
    if not isinstance(body, dict):
        body = {"v": body}
    reading = body.get("v") if body.get("v") is not None else body.get("quoted")
    if body.get("rule") is None and type(reading) is float and not math.isfinite(reading):
        # A corrupt primary reading must not become a null that lets Lean fall
        # through to a secondary quote, verdict or source date.
        return {"v": None}
    return body


@lru_cache(maxsize=16)
def _compute(record_json, predicate_json, dependencies_json, source_hash):
    return _CORE.request({"operation": "compute", "record": json.loads(record_json),
                          "predicate": json.loads(predicate_json), "dependencies": json.loads(dependencies_json)})


def compute(raw, ids, predicate=None, dependencies=None):
    global _CORE
    try:
        if _CORE is None:
            _CORE = _core_type()()
        _CORE.ensure_program()
        if _CORE.expression_source_hash != LOADED_SOURCE_HASH:
            raise ValueError('expression parser changed; restart the reader')
        record = {"nodes": {key: {"body": _record_body(body)}
                            for key, body in raw.items() if key in ids}}
        result = _compute(json.dumps(_finite_payload(record), sort_keys=True, ensure_ascii=False, default=str, allow_nan=False),
                          json.dumps(predicate, sort_keys=True, ensure_ascii=False),
                          json.dumps(sorted(dependencies if dependencies is not None else refs(predicate))),
                          _CORE.build["source_sha256"] + _CORE.build["binary_sha256"] + _CORE.expression_source_hash)
        return copy.deepcopy(result)
    except (ValueError, TypeError, OSError, ImportError, RecursionError, subprocess.SubprocessError) as error:
        _CORE = None
        return {"values": {}, "predicate": {"holds_on_current_values": None, "reason": str(error)},
                "error": str(error)}


def current(raw, ids, key):
    result = compute(raw, ids)
    return result["values"].get(key, {"value": None, "reason": result.get("error", "unavailable")})


def convert(source, predicate=False, explicit_refs=False):
    """Translate an explicit, bounded legacy expression; never execute its code."""
    if not isinstance(source, str) or len(source) > 4000:
        raise ValueError("legacy expression must be text within 4000 characters")
    tree = ast.parse(source, mode="eval")
    operators = {ast.Add: "add", ast.Sub: "sub", ast.Mult: "mul", ast.Div: "div",
                 ast.Eq: "eq", ast.NotEq: "ne", ast.Lt: "lt", ast.LtE: "le", ast.Gt: "gt", ast.GtE: "ge"}
    def visit(node, depth=0):
        if depth >= 64: raise ValueError("expression depth exceeds 64")
        if isinstance(node, (ast.Name, ast.Attribute)):
            name = ast.get_source_segment(source, node)
            if not name or not all(part.isidentifier() for part in name.split('.')):
                raise ValueError("unsupported reference")
            if name in {"true", "false", "True", "False"}:
                return {"bool": name.lower() == "true"}
            return {"ref": name}
        if explicit_refs and isinstance(node, ast.Call) and isinstance(node.func, ast.Name) \
                and node.func.id == 'ref' and len(node.args) == 1 and not node.keywords \
                and isinstance(node.args[0], ast.Constant) and isinstance(node.args[0].value, str):
            return {'ref': node.args[0].value}
        if isinstance(node, ast.Constant):
            if type(node.value) is bool: return {"bool": node.value}
            if isinstance(node.value, str): return {"text": node.value}
            if type(node.value) in (int, float): return {"num": ast.get_source_segment(source, node)}
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
            child = visit(node.operand, depth + 1)
            if "num" in child: return {"num": "-" + child["num"]}
            return {"op": "sub", "args": [{"num": "0"}, child]}
        if isinstance(node, ast.BinOp) and type(node.op) in operators:
            return {"op": operators[type(node.op)], "args": [visit(node.left, depth + 1), visit(node.right, depth + 1)]}
        if isinstance(node, ast.Compare) and len(node.ops) == 1 and type(node.ops[0]) in operators:
            return {"op": operators[type(node.ops[0])], "args": [visit(node.left, depth + 1), visit(node.comparators[0], depth + 1)]}
        raise ValueError("unsupported legacy expression; it was not converted")
    expression = visit(tree.body)
    return validate(expression, predicate=expression.get('op') in COMPARISONS if predicate is None else predicate)


def convert_authored(source, predicate=False, legacy_rhs=None):
    """Conservative storage conversion, distinct from an explicit syntax translation."""
    if not isinstance(source, str) or len(source) > 4000:
        raise ValueError('expression must be text within 4000 characters')
    if not predicate and re.fullmatch(r'\s*\d{4}-\d{2}-\d{2}\s*', source):
        raise ValueError('date-like text is not implicitly arithmetic')
    if predicate:
        syntax = ast.parse(source, mode='eval').body
        if isinstance(syntax, ast.Compare) and len(syntax.comparators) == 1:
            rhs = syntax.comparators[0]
            if isinstance(rhs, ast.Name) and rhs.id.lower() not in {'true', 'false'}:
                raise ValueError('ambiguous bare right operand; choose a ref or text explicitly')
            if isinstance(rhs, ast.Constant) and isinstance(rhs.value, str):
                if legacy_rhs is not None and legacy_rhs.strip().strip("\"'") != rhs.value:
                    raise ValueError('literal quoting changes the legacy reading; choose typed text explicitly')
                try:
                    float(rhs.value.replace(',', ''))
                except ValueError:
                    pass
                else:
                    raise ValueError('numeric-looking text has legacy coercion; choose explicit typed operands')
        if any(isinstance(node, ast.Constant) and isinstance(node.value, str)
               and '\\' in (ast.get_source_segment(source, node) or '') for node in ast.walk(syntax)):
            raise ValueError('legacy escaped literal needs explicit review')
    return convert(source, predicate=predicate)
