"""Structured core writes preserve entry bodies and refuse invalid predicates atomically."""
import contextlib
import copy
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import yaml

from scripts import provenance as P


@unittest.skipUnless(os.name == "posix", "record writers require POSIX locks")
class CoreCompositionWrites(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.record = self.root / "GROUNDING.yaml"

    @staticmethod
    def document(body):
        return {
            "meta": {"updated": "2026-09-16", "reasoning": {
                "version": 2, "profile": "core/v1",
                "requires": ["arithmetic/v1", "composition/v1"],
            }},
            "sources": {
                "s.old": {"file": "old.json", "read": "2026-09-16"},
                "s.new": {"file": "new.json", "read": "2026-09-17"},
            },
            "known": {"p.value": body},
        }

    def save(self, body, *, flow=False):
        document = self.document(body)
        if not flow:
            self.record.write_text(yaml.safe_dump(document, sort_keys=False), encoding="utf-8")
            return
        text = yaml.safe_dump(document, sort_keys=False)
        start = text.index("  p.value:")
        entry = yaml.safe_dump(body, default_flow_style=True, sort_keys=False,
                               width=1000000).strip()
        self.record.write_text(text[:start] + "  p.value: " + entry + " # keep\n",
                               encoding="utf-8")

    def apply(self, action):
        with contextlib.redirect_stdout(io.StringIO()):
            return P.apply([str(self.record)], action)

    def value_body(self):
        return yaml.safe_load(self.record.read_text(encoding="utf-8"))["known"]["p.value"]

    def test_block_list_replacement_keeps_complete_body_and_citation(self):
        body = {
            "name": "Original label",
            "v": [1, {"nested": [True, None]}],
            "of": "2026-09-16",
            "from": "s.old",
            "at": "row 1",
            "metadata": {"unit": "items", "flags": ["keep", {"deep": False}]},
        }
        self.save(body)
        replacement = [{"nested": [False, 2]}, "done"]
        self.apply({"kind": "set", "id": "p.value", "value": replacement,
                    "as_of": "2026-09-17", "source": "s.new", "at": "row 9",
                    "why": "new structured reading"})

        actual = self.value_body()
        self.assertEqual(actual["v"], replacement)
        self.assertEqual(actual["name"], body["name"])
        self.assertEqual(actual["metadata"], body["metadata"])
        self.assertEqual((actual["of"], actual["from"], actual["at"]),
                         ("2026-09-17", "s.new", "row 9"))
        self.assertIn("# set 2026-09-17: new structured reading",
                      self.record.read_text(encoding="utf-8"))

    def test_flow_list_and_record_round_trip_without_splicing_yaml(self):
        cases = [
            ([1, "two"], [False, {"deep": [None, 3]}]),
            ({"left": [1], "right": {"ok": True}},
             {"left": [], "right": {"ok": False, "extra": "yes"}}),
        ]
        for original, replacement in cases:
            with self.subTest(replacement=replacement):
                body = {"v": original, "of": "2026-09-16", "metadata": {"keep": [1, 2]}}
                self.save(body, flow=True)
                self.apply({"kind": "set", "id": "p.value", "value": replacement,
                            "as_of": "2026-09-17"})
                actual = self.value_body()
                self.assertEqual(actual["v"], replacement)
                self.assertEqual(actual["metadata"], body["metadata"])
                self.assertEqual(actual["of"], "2026-09-17")

    def test_scalar_container_transitions_keep_scalar_path_and_body(self):
        cases = [
            ("'001'", "001", {"items": [1, True]}),
            ("[1, {nested: true}]", [1, {"nested": True}], "001"),
        ]
        for written, original, replacement in cases:
            with self.subTest(original=original, replacement=replacement):
                body = {"v": original, "of": "2026-09-16", "metadata": {"keep": True}}
                self.save(body)
                text = self.record.read_text(encoding="utf-8")
                dumped = yaml.safe_dump(original, default_flow_style=True).strip()
                self.record.write_text(text.replace("v: " + dumped, "v: " + written, 1),
                                       encoding="utf-8")
                self.apply({"kind": "set", "id": "p.value", "value": replacement,
                            "as_of": "2026-09-17"})
                actual = self.value_body()
                self.assertEqual(actual["v"], replacement)
                self.assertEqual(actual["metadata"], body["metadata"])
                self.assertEqual(actual["of"], "2026-09-17")
                if replacement == "001":
                    self.assertRegex(self.record.read_text(encoding="utf-8"),
                                     r'''v: ["']001["']''')

    def test_unchanged_and_refused_container_sets_leave_original_bytes(self):
        body = {"v": [1, {"keep": True}], "of": "2026-09-17",
                "metadata": {"untouched": ["yes"]}}
        self.save(body, flow=True)
        before = self.record.read_bytes()
        self.assertEqual(self.apply({"kind": "set", "id": "p.value",
                                     "value": copy.deepcopy(body["v"])}), 0)
        self.assertEqual(self.record.read_bytes(), before)

        with self.assertRaises(P.Refused):
            self.apply({"kind": "set", "id": "p.value", "value": [2],
                        "as_of": "2026-09-17"})
        self.assertEqual(self.record.read_bytes(), before)

    def test_hypothesis_container_set_keeps_base_and_complete_carried_body(self):
        body = {"v": [1], "of": "2026-09-17", "from": "s.old", "at": "row 1",
                "metadata": {"keep": {"nested": True}}}
        self.save(body, flow=True)
        before = self.record.read_bytes()
        replacement = {"items": [2, False]}
        self.apply({"kind": "set", "id": "p.value", "value": replacement,
                    "as_of": "2026-09-17", "source": "s.new", "at": "row 8",
                    "hypothesis": "proposal"})

        self.assertEqual(self.record.read_bytes(), before)
        hypothesis = Path(P.hypothesis_path([str(self.record)], "proposal"))
        actual = yaml.safe_load(hypothesis.read_text(encoding="utf-8"))["known"]["p.value"]
        self.assertEqual(actual["v"], replacement)
        self.assertEqual(actual["metadata"], body["metadata"])
        self.assertEqual((actual["of"], actual["from"], actual["at"]),
                         ("2026-09-17", "s.new", "row 8"))

    def test_invalid_structured_predicate_refuses_without_traceback_or_mutation(self):
        self.save({"v": 1, "of": "2026-09-16"})
        before = self.record.read_bytes()
        action = {"kind": "add", "id": "c.invalid", "body": {
            "rests_on": ["p.value"], "verdict": "invalid",
            "wrong_if": {"list": {}},
        }}
        with self.assertRaisesRegex(P.Refused, "wrong_if: invalid composition expression fields"):
            self.apply(action)
        self.assertEqual(self.record.read_bytes(), before)

        run = subprocess.run([
            sys.executable, str(Path(P.__file__)), "add", "c.invalid",
            "rests_on=[p.value]", "verdict=invalid", "wrong_if={list: {}}",
            str(self.record),
        ], cwd=self.root, text=True, capture_output=True)
        self.assertNotEqual(run.returncode, 0)
        self.assertIn("invalid composition expression fields", run.stderr + run.stdout)
        self.assertNotIn("Traceback", run.stderr + run.stdout)
        self.assertEqual(self.record.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
