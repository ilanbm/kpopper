"""Structured expression data and a bridge to the single Lean evaluator.

Python validates shape, derives references and renders text. Arithmetic is exclusively
computed by the packaged Lean program. Legacy text is converted only on explicit request.
"""
import ast
import copy
from functools import lru_cache
import hashlib
import importlib
import importlib.util
import json
from pathlib import Path
import re
import sys
import subprocess
from decimal import Decimal
from fractions import Fraction

ARITHMETIC = {"add": "+", "sub": "-", "mul": "*", "div": "/"}
COMPARISONS = {"eq": "==", "ne": "!=", "lt": "<", "le": "<=", "gt": ">", "ge": ">="}
NUMERIC = re.compile(r"-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?\Z")


def _refs_valid(tree):
    if "ref" in tree:
        return [tree["ref"]]
    return [key for child in tree.get("args", []) for key in _refs_valid(child)]


def validate(tree, predicate=False, depth=0):
    if depth >= 64 or not isinstance(tree, dict):
        raise ValueError("expression must be a mapping within 64 levels")
    keys = set(tree)
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
        validate(tree, predicate=tree.get("op") in COMPARISONS)
    except (ValueError, TypeError):
        return []
    return sorted(set(_refs_valid(tree)))


def text(tree):
    if not isinstance(tree, dict):
        return str(tree or "")
    try:
        validate(tree, predicate=tree.get("op") in COMPARISONS)
    except (ValueError, TypeError) as error:
        return "<invalid expression: " + str(error) + ">"
    def render(node):
        if "ref" in node: return node["ref"]
        if "num" in node: return node["num"]
        if "text" in node: return json.dumps(node["text"], ensure_ascii=False)
        if "bool" in node: return "true" if node["bool"] else "false"
        symbol = {**ARITHMETIC, **COMPARISONS}[node["op"]]
        return "(" + render(node["args"][0]) + " " + symbol + " " + render(node["args"][1]) + ")"
    return render(tree)


def number(value):
    """Decode exact numeric output for display/comparison, never evaluate a formula."""
    if type(value) in (int, float):
        return Fraction(str(value))
    if isinstance(value, dict) and set(value) == {"rational"}:
        pair = value["rational"]
        if isinstance(pair, list) and len(pair) == 2:
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
        record = {"nodes": {key: {"body": body if isinstance(body, dict) else {"v": body}}
                            for key, body in raw.items() if key in ids}}
        result = _compute(json.dumps(record, sort_keys=True, ensure_ascii=False, default=str, allow_nan=False),
                          json.dumps(predicate, sort_keys=True, ensure_ascii=False),
                          json.dumps(sorted(dependencies if dependencies is not None else refs(predicate))),
                          _CORE.build["source_sha256"] + _CORE.build["binary_sha256"])
        return copy.deepcopy(result)
    except (ValueError, TypeError, OSError, ImportError, RecursionError, subprocess.SubprocessError) as error:
        _CORE = None
        return {"values": {}, "predicate": {"holds_on_current_values": None, "reason": str(error)},
                "error": str(error)}


def current(raw, ids, key):
    result = compute(raw, ids)
    return result["values"].get(key, {"value": None, "reason": result.get("error", "unavailable")})


def convert(source, predicate=False):
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
            if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*", name or ""):
                raise ValueError("unsupported reference")
            if name in {"true", "false", "True", "False"}:
                return {"bool": name.lower() == "true"}
            return {"ref": name}
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
    return validate(visit(tree.body), predicate=predicate)
