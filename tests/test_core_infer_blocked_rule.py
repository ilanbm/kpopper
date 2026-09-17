"""Inference keeps admitted core value holes distinct from sparse judgments."""
import unittest

from scripts import provenance as P


def source_record():
    return {
        "meta": {"reasoning": {"version": 2, "profile": "core/v1",
                                "requires": ["arithmetic/v1", "composition/v1"]}},
        "known": {"p.input": {"v": {}}},
    }


class BlockedCoreRuleInference(unittest.TestCase):
    def test_mutated_source_closure_keeps_blocked_structured_rule_out_of_judgments(self):
        for expression in ('missing.input + 1', 'field(p.input, "missing")'):
            with self.subTest(expression=expression):
                doc = source_record()
                P.infer(doc)
                doc["known"]["p.result"] = {
                    "rule": {"expr": expression},
                    "blocked_on": "await the unavailable input",
                }
                ids, judgments, fields = P.infer(doc)
                self.assertEqual(ids, {"p.input", "p.result"})
                self.assertEqual(judgments, {})
                self.assertEqual(fields, {"deps": "rests_on", "snapshot": "seen",
                                          "predicate": "wrong_if"})

    def test_retained_schema_accepts_the_same_value_hole(self):
        doc = source_record()
        doc["schema"] = {"deps": "rests_on", "snapshot": "seen", "predicate": "wrong_if"}
        doc["known"]["p.result"] = {
            "rule": {"expr": 'field(p.input, "missing")'},
            "blocked_on": "await the unavailable field",
        }
        _, judgments, fields = P.infer(doc)
        self.assertEqual(judgments, {})
        self.assertEqual(fields, doc["schema"])

    def test_blocked_judgment_with_misspelled_dependency_still_refuses(self):
        doc = source_record()
        doc["schema"] = {"deps": "rests_on", "snapshot": "seen", "predicate": "wrong_if"}
        doc["known"]["c.bad"] = {
            "rest_on": ["p.input"],
            "verdict": "ready",
            "blocked_on": "await review",
        }
        with self.assertRaisesRegex(SystemExit, "schema names 'rests_on'"):
            P.infer(doc)


if __name__ == "__main__":
    unittest.main()
