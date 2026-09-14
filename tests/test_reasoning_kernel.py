"""Public scalar arithmetic/v1 and strict native framing regression corpus.

Native tests require KPOPPER_REASONING_TEST_BINARY; no compiler/network setup is
performed. Set KPOPPER_REQUIRE_REASONING_TESTS=1 to make missing runtime fatal.
"""
from fractions import Fraction
from pathlib import Path
import json
import os
import random
import subprocess
import unittest

from scripts.reasoning.transport import decode_response, encode_request

BINARY = os.environ.get("KPOPPER_REASONING_TEST_BINARY")
if os.environ.get("KPOPPER_REQUIRE_REASONING_TESTS") == "1" and not BINARY:
    raise RuntimeError("KPOPPER_REASONING_TEST_BINARY is required")


def num(value):
    return {"num": str(value)}


def op(name, left, right):
    return {"op": name, "args": [left, right]}


def request(expression, nodes=None, declared=None, limits=None):
    return {"expression": expression, "nodes": nodes or {}, "declared": declared or [], "limits": limits}


class TransportTests(unittest.TestCase):
    def test_canonical_requests_preserve_unicode_and_empty_text(self):
        a = request({"text": "\t\nעברית🙂\0"}, {"é": num(1), "a": num(2)}, ["é", "a"])
        b = request(a["expression"], {"a": num(2), "é": num(1)}, ["a", "é"])
        self.assertEqual(encode_request(a), encode_request(b))
        self.assertTrue(encode_request(request({"text": ""})).endswith("\ts\t"))
        self.assertTrue(encode_request(a).isascii())

    def test_request_validation_is_strict_and_cyclic_input_is_safe(self):
        invalid = [request({"bool": 0}), request({"num": True}), request({"num": "NaN"}),
                   request({"num": "01"}), request({"null": 1}), request({"text": "\ud800"}),
                   request({"op": "add\tn", "args": [num(1), num(2)]}),
                   request(num(1), declared=["a", "a"]), request(num(1), limits={"steps": True}),
                   request(num(1), limits={"depth": 0}), request(num(1), limits={"digits": 4097}),
                   request(num(1), limits={"unrecognized": 1}), request({"unavailable": "fabricated"}),
                   dict(request(num(1)), seen={"a": 3})]
        cycle = op("add", num(1), num(2))
        cycle["args"][1] = cycle
        invalid.append(request(cycle))
        for item in invalid:
            with self.subTest(item=str(item)[:80]), self.assertRaises(ValueError):
                encode_request(item)

    def test_response_framing_types_counts_and_canonicalization(self):
        good = "KR1\tok\tn\t3\t10\t0\t0\t0\t3\t0"
        self.assertEqual(decode_response(good + "\n")["value"]["numerator"], "3")
        invalid = ["", good + "\tx", good + "\n\n", good.replace("KR1", "KR2"),
                   good.replace("\tn\t3\t10", "\tn\t6\t20"),
                   good.replace("\tn\t3\t10", "\tn\t-0\t1"),
                   good.replace("\tn\t3\t10", "\tn\t1\t0"),
                   good.replace("\tn\t3\t10", "\tb\ttrue"),
                   good.replace("\tn\t3\t10", "\ts\tff"),
                   good.replace("\tok\t", "\tunknown\t"),
                   "KR1\tok\tu\t0\t0\t0\t0\t0",
                   "KR1\terror\tu\t1\tinvented\t0\t0\t0\t0",
                   "KR1\terror\tu\t2\ttype_error\ttype_error\t0\t0\t0\t0",
                   "KR1\tok\tz\t0\t2\t62\t61\t0\t1\t0",
                   "KR1\tok\tz\t0\t1\t61\t1\t62\t1\t0",
                   "KR1\tok\tz\t0\t1\t61\t1\t61\t1\t1\t61\t2",
                   "KR1\tok\tz\t0\t00\t0\t1\t0"]
        for item in invalid:
            with self.subTest(item=item), self.assertRaises(ValueError):
                decode_response(item)


@unittest.skipUnless(BINARY, "set KPOPPER_REASONING_TEST_BINARY for native conformance")
class NativeKernelTests(unittest.TestCase):
    def batch(self, requests):
        run = subprocess.run([BINARY], input="\n".join(encode_request(r) for r in requests) + "\n",
                             text=True, capture_output=True, timeout=15, check=True)
        self.assertEqual(run.stderr, "")
        lines = run.stdout.splitlines()
        self.assertEqual(len(lines), len(requests))
        return [decode_response(line) for line in lines]

    def evaluate(self, expression, **kwargs):
        return self.batch([request(expression, **kwargs)])[0]

    def assertNumber(self, result, expected):
        self.assertEqual(result["status"], "ok", result)
        self.assertEqual(result["value"], {"type": "number", "numerator": str(expected.numerator),
                                           "denominator": str(expected.denominator)})

    def test_exact_decimals_and_comparisons(self):
        self.assertNumber(self.evaluate(op("add", num("0.1"), num("0.2"))), Fraction(3, 10))
        for literal in ["0", "-0", "123", "-18.75", "1e-40", "0.00E+04", "12.5e+5"]:
            with self.subTest(literal=literal):
                self.assertNumber(self.evaluate(num(literal)), Fraction(literal))
        for name, expected in [("eq", True), ("ne", False), ("le", True), ("lt", False), ("ge", True), ("gt", False)]:
            result = self.evaluate(op(name, op("add", num("0.1"), num("0.2")), num("0.3")))
            self.assertEqual(result["value"], {"type": "boolean", "value": expected})

    def test_types_null_unavailability_and_no_coercion(self):
        self.assertEqual(self.evaluate({"null": True})["value"], {"type": "null"})
        self.assertEqual(self.evaluate(op("eq", {"null": True}, {"null": True}))["value"]["value"], True)
        for left, right in [(num(0), {"bool": False}), (num(1), {"text": "1"}), ({"null": True}, num(0))]:
            result = self.evaluate(op("eq", left, right))
            self.assertEqual((result["status"], result["steps"]), ("error", 0))
            self.assertEqual(result["diagnostics"], ["type_error"])
        for code in ["missing_reference", "missing_input", "contested", "unavailable_input"]:
            result = self.evaluate(op("add", num(2), {"unavailable": code}))
            self.assertEqual((result["status"], result["value"], result["diagnostics"]), ("unknown", None, [code]))
        # Known type errors are found before a sibling arithmetic error can run.
        result = self.evaluate(op("add", op("div", num(1), num(0)), {"text": "bad"}))
        self.assertEqual((result["diagnostics"], result["steps"]), (["type_error"], 0))
        result = self.evaluate(op("add", {"unavailable": "contested"}, {"text": "1"}))
        self.assertEqual((result["status"], result["steps"]), ("error", 0))

    def test_transitive_missing_unicode_and_declaration_preflight(self):
        nodes = {"חישוב🙂": op("add", {"ref": "élève"}, {"ref": "missing"}), "élève": num(2)}
        result = self.evaluate({"ref": "חישוב🙂"}, nodes=nodes, declared=["חישוב🙂", "élève", "missing"])
        self.assertEqual(result["status"], "unknown")
        self.assertEqual(result["potential_reads"], sorted(["חישוב🙂", "élève", "missing"]))
        self.assertEqual(result["executed_reads"], result["potential_reads"])
        refused = self.evaluate({"ref": "חישוב🙂"}, nodes=nodes, declared=["חישוב🙂", "élève"])
        self.assertEqual((refused["status"], refused["steps"]), ("error", 0))
        self.assertEqual(refused["diagnostics"], ["undeclared_dependency"])
        self.assertEqual(refused["potential_reads"], result["potential_reads"])
        self.assertEqual(self.evaluate({"text": "missing"})["executed_reads"], [])

    def test_cycles_errors_and_memoized_shared_nodes(self):
        result = self.evaluate({"ref": "a"}, nodes={"a": {"ref": "b"}, "b": {"ref": "a"}}, declared=["a", "b"])
        self.assertEqual((result["status"], result["diagnostics"]), ("error", ["cyclic_reference"]))
        self.assertEqual(result["node_evaluations"], {"a": 1, "b": 1})
        shared = {"a": num(2), "b": op("add", {"ref": "a"}, {"ref": "a"})}
        result = self.evaluate(op("mul", {"ref": "b"}, {"ref": "b"}), nodes=shared, declared=["a", "b"])
        self.assertNumber(result, Fraction(16))
        self.assertEqual(result["node_evaluations"], {"a": 1, "b": 1})
        self.assertEqual(result["steps"], 7)
        results = self.batch([request(op("div", num(1), num(0))), request(num(42))])
        self.assertEqual(results[0]["diagnostics"], ["division_by_zero"])
        self.assertNumber(results[1], Fraction(42))

    def test_limits_preserve_potential_and_never_approximate(self):
        for expr, limits, code in [(op("add", num(1), num(2)), {"steps": 2}, "step_limit"),
                                   (op("mul", num(99), num(99)), {"digits": 2}, "number_limit"),
                                   (num("1e4097"), {}, "number_limit")]:
            result = self.evaluate(expr, limits=limits)
            self.assertEqual((result["status"], result["value"], result["diagnostics"]), ("limit", None, [code]))
        result = self.evaluate({"ref": "a"}, nodes={"a": {"ref": "b"}, "b": num(1)}, declared=["a", "b"], limits={"depth": 1})
        self.assertEqual(result["diagnostics"], ["depth_limit"])
        self.assertEqual(result["potential_reads"], ["a", "b"])

    def test_unknown_operations_and_malformed_native_requests(self):
        unsupported = self.evaluate(op("and", {"bool": True}, {"bool": False}))
        self.assertEqual(unsupported["status"], "unsupported_capability")
        base = "KP1\t1000\t128\t256\t0\t0\t"
        invalid = ["", "KP2\t1", base + "b\t2", base + "n\t01", base + "n\t+1", base + "n\t1.",
                   base + "n\t1e", base + "z\tz", base + "s\tf", base + "s\tFF", base + "s\tff",
                   base + "s\tc080", base + "s\teda080", base + "u\tforged", base + "o\tadd\t1\tn\t1",
                   base + "o\tadd\t3\tn\t1\tn\t2\tn\t3", base + "r", base.replace("1000", "00") + "z",
                   base.replace("1000", "10000001") + "z", base.replace("128", "0") + "z",
                   "KP1\t1000\t128\t256\t2\t61\t61\t0\tz",
                   "KP1\t1000\t128\t256\t0\t2\t61\tn\t1\t61\tn\t2\tz"]
        run = subprocess.run([BINARY], input="\n".join(invalid) + "\n", text=True, capture_output=True, check=True, timeout=15)
        outputs = [decode_response(line) for line in run.stdout.splitlines()]
        self.assertEqual(len(outputs), len(invalid))
        for result in outputs:
            self.assertEqual((result["status"], result["value"], result["steps"]), ("error", None, 0), result)
        for raw, code in [("KP1\t1000\t128\t256\t0\t20001", "node_limit"),
                          ("KP1\t1000\t128\t256\t100001", "edge_limit"),
                          (base + "o\tadd\t2\tn\t1\t" * 129 + "n\t1", "parser_depth_limit")]:
            run = subprocess.run([BINARY], input=raw + "\n", text=True, capture_output=True, check=True, timeout=15)
            result = decode_response(run.stdout)
            self.assertEqual((result["status"], result["diagnostics"]), ("limit", [code]))
        # Invalid raw bytes cannot be accepted as an ID or text token.
        run = subprocess.run([BINARY], input=base.encode() + b"s\t\xff\n", capture_output=True, check=True, timeout=15)
        self.assertEqual(decode_response(run.stdout.decode())["status"], "error")

    def test_text_roundtrip(self):
        for text in ["", "\0", "\t\n", "é", "עברית🙂", "𝕒", "é\u0301"]:
            self.assertEqual(self.evaluate({"text": text})["value"], {"type": "text", "value": text})

    def test_seeded_rational_oracle(self):
        rng = random.Random(7315)
        def generate(depth):
            if not depth or rng.random() < .3:
                value = f"{rng.randrange(-100, 100)}.{rng.randrange(100):02d}"
                return num(value), Fraction(value)
            left, a = generate(depth - 1)
            right, b = generate(depth - 1)
            name = rng.choice(["add", "sub", "mul", "div"])
            if name == "div" and not b:
                name = "add"
            expected = {"add": lambda: a + b, "sub": lambda: a - b,
                        "mul": lambda: a * b, "div": lambda: a / b}[name]()
            return op(name, left, right), expected
        corpus = [generate(5) for _ in range(150)]
        results = self.batch([request(expr) for expr, _ in corpus])
        for result, (_, expected) in zip(results, corpus):
            self.assertNumber(result, expected)

    def test_public_fixture_slice(self):
        for filename in ["scalar-v1.json", "t0-scalar-slice.json"]:
            corpus = json.loads((Path(__file__).parent / "reasoning" / filename).read_text())
            results = self.batch([case["request"] for case in corpus["cases"]])
            for case, result in zip(corpus["cases"], results):
                with self.subTest(corpus=filename, case=case["id"]):
                    for key, expected in case["expected"].items():
                        self.assertEqual(result[key], expected)
