"""Execute the real Lean evaluator on structured expressions and graph changes."""
import copy
import json
import os
import tempfile
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from scripts.session.core import Core


def ref(name):
    return {"ref": name}


def num(value):
    return {"num": str(value)}


def op(name, left, right):
    return {"op": name, "args": [left, right]}


class StructuredKernel(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.core = Core()
        except ValueError:
            if os.environ.get("KPOPPER_REQUIRE_CORE_TESTS") == "1":
                raise
            raise unittest.SkipTest("build the packaged Lean core to exercise expressions")

    def record(self):
        return {"nodes": {
            "order.price": {"kind": "known", "body": {"v": 20}},
            "order.quantity": {"kind": "known", "body": {"v": 5}},
            "order.total": {"kind": "known", "body": {"rule": op("mul", ref("order.price"), ref("order.quantity"))}},
            "c.budget": {"kind": "judgment", "body": {"verdict": "The order fits the budget",
                "rests_on": ["order.total"], "seen": {"order.total": 100},
                "wrong_if": op("gt", ref("order.total"), num(150))}},
        }}

    def test_formula_and_predicate_react_to_current_inputs(self):
        record = self.record()
        first = self.core.assess(record, "c.budget")["bundle"]
        self.assertEqual(first["premises"][0]["current_recorded_value"], 100)
        self.assertFalse(first["falsifier"]["holds_on_current_values"])
        record["nodes"]["order.quantity"]["body"]["v"] = 10
        after = self.core.assess(record, "c.budget")["bundle"]
        self.assertEqual(after["premises"][0]["current_recorded_value"], 200)
        self.assertEqual(after["premises"][0]["value_at_review"], 100)
        self.assertTrue(after["premise_changed"])
        self.assertTrue(after["falsifier"]["holds_on_current_values"])
        self.assertEqual(record["nodes"]["c.budget"]["body"]["seen"], {"order.total": 100})

    def calculate(self, expression):
        record = {"nodes": {"value.result": {"body": {"rule": expression}}}}
        return self.core.request({"operation": "compute", "record": record})["values"]["value.result"]

    def test_exact_decimals_and_nonterminating_fractions(self):
        result = self.calculate(op("add", num("0.1"), num("0.2")))
        self.assertEqual(result["value"], {"rational": ["3", "10"]})
        result = self.calculate(op("div", num(1), num(3)))
        self.assertEqual(result["value"], {"rational": ["1", "3"]})
        self.assertEqual(self.calculate(op("sub", num(2), num(5)))["value"], -3)
        self.assertEqual(self.calculate(num("1e+2"))["value"], 100)
        self.assertEqual(self.calculate(num("0.123456789123456789"))["value"],
                         {"rational": ["123456789123456789", "1000000000000000000"]})

    def test_unknown_reasons_never_become_false_or_zero(self):
        for expression, reason in [
            (op("div", num(1), num(0)), "division_by_zero"),
            (op("add", ref("missing.value"), num(1)), "missing_reference"),
            (op("mul", {"text": "20"}, num(5)), "numeric_operands_required"),
            ({"op": "execute", "args": [num(1), num(2)]}, "unsupported_operator"),
            ({"ref": "value.result"}, "cyclic_reference"),
        ]:
            with self.subTest(reason=reason):
                result = self.calculate(expression)
                self.assertIsNone(result["value"])
                self.assertIn(reason, result["reason"])

    def test_hidden_keys_and_invalid_arity_are_rejected(self):
        for expression in [{"num": "1", "extra": "ignored?"}, {"ref": "x", "op": "add"},
                           {"op": "add", "args": [num(1)]}, {"num": "nan"}]:
            self.assertIsNone(self.calculate(expression)["value"])

    def test_predicate_refs_must_be_declared_even_inside_arithmetic(self):
        record = self.record()
        record["nodes"]["c.budget"]["body"]["wrong_if"] = op("gt", op("mul", ref("order.price"), num(8)), num(150))
        result = self.core.assess(record, "c.budget")["bundle"]["falsifier"]
        self.assertIsNone(result["holds_on_current_values"])
        self.assertIn("undeclared", result["reason"])

    def test_text_that_looks_like_an_id_is_a_literal(self):
        record = self.record()
        record["nodes"]["input.label"] = {"kind": "known", "body": {"rule": {"text": "order.price"}}}
        body = record["nodes"]["c.budget"]["body"]
        body["rests_on"] = ["input.label"]
        body["wrong_if"] = op("eq", ref("input.label"), {"text": "order.price"})
        result = self.core.assess(record, "c.budget")["bundle"]["falsifier"]
        self.assertTrue(result["holds_on_current_values"])
        self.assertEqual(result["reads"], ["input.label"])

    def test_legacy_text_rule_stays_unknown_until_explicit_conversion(self):
        record = self.record()
        record["nodes"]["order.total"]["body"]["rule"] = "order.price * order.quantity"
        result = self.core.assess(record, "c.budget")["bundle"]
        self.assertIsNone(result["premises"][0]["current_recorded_value"])
        self.assertIsNone(result["falsifier"]["holds_on_current_values"])

    def test_exponent_is_bounded_before_number_expansion(self):
        for token in ("1e100000000", "1e-100000000", " 1", "1 "):
            self.assertIsNone(self.calculate(num(token))["value"])

    def test_computed_snapshot_detects_a_formula_change_even_with_same_value(self):
        record = self.record()
        old_rule = record["nodes"]["order.total"]["body"]["rule"]
        record["nodes"]["c.budget"]["body"]["seen"]["order.total"] = {"computed": {"value": 100, "rule": old_rule}}
        record["nodes"]["order.total"]["body"]["rule"] = op("add", num(50), num(50))
        result = self.core.assess(record, "c.budget")["bundle"]
        self.assertTrue(result["premise_changed"])
        self.assertTrue(result["premises"][0]["rule_changed"])


class StructuredRecord(unittest.TestCase):
    setUpClass = classmethod(StructuredKernel.setUpClass.__func__)
    record = StructuredKernel.record
    def setUp(self):
        import yaml
        from scripts import ingestion
        self.I = ingestion
        self.P = ingestion.P
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / "PROVENANCE.yaml"
        record = self.record()["nodes"]
        self.doc = {"sources": {"s.report": {"file": "report.md", "read": "2026-09-10"}},
                    "known": {key: value["body"] for key, value in record.items() if key != "c.budget"},
                    "judgments": {"c.budget": record["c.budget"]["body"]}}
        for key in ("order.price", "order.quantity"):
            self.doc["known"][key]["from"] = "s.report"
            self.doc["known"][key]["of"] = "2026-09-10"
        (self.path.parent / "report.md").write_text("Price 20, quantity 5.")
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))

    def world(self):
        doc = self.P.load([str(self.path)])
        ids, jud, fields = self.P.infer(doc)
        return doc, ids, jud, fields, self.P.with_builtins(doc, ids, jud, fields)

    def test_native_reader_check_reach_and_search_use_the_same_tree(self):
        from scripts.search import search
        doc, ids, jud, fields, raw = self.world()
        self.assertEqual(self.P.value_of(raw, ids, "order.total"), 100)
        self.assertFalse(self.P.evaluate(jud["c.budget"]["pred"], raw, ids))
        self.assertEqual(self.P.check_lines([str(self.path)])[0], [])
        self.assertIn("c.budget", self.P.reach_of(ids, jud, raw, ["order.price"])[0])
        result = search("total", self.path)
        hit = next(row for row in result["results"] if row["id"] == "order.total")
        self.assertEqual(hit["sources"], ["s.report"])
        self.assertEqual(hit["calculation"]["value"], 100)

    def test_writer_stores_result_and_formula_in_review_snapshot(self):
        import contextlib, io, yaml
        action = {"kind": "add", "id": "c.extra", "body": {"rests_on": ["order.total"],
                  "verdict": "The order fits", "wrong_if": op("gt", ref("order.total"), num(150))}}
        with contextlib.redirect_stdout(io.StringIO()):
            self.P.apply([str(self.path)], action)
        snapshot = yaml.safe_load(self.path.read_text())["judgments"]["c.extra"]["seen"]["order.total"]
        self.assertEqual(snapshot["computed"]["value"], 100)
        self.assertEqual(snapshot["computed"]["rule"], self.doc["known"]["order.total"]["rule"])

    def test_final_batch_arithmetic_not_intermediate_state(self):
        report = {"source_quote": "Price 40, quantity 2", "date": "2026-09-11", "updates": [
            {"kind": "set", "id": "order.price", "value": 40},
            {"kind": "set", "id": "order.quantity", "value": 2}]}
        state = self.path.parent / "state"
        self.I.capture(report, self.path, state, start=False)
        receipt = self.I.process(self.path, state)[0]
        self.assertEqual(receipt["state"], "applied", receipt)
        self.assertEqual(receipt["newly_fired_judgments"], [])
        doc, ids, _, _, raw = self.world()
        self.assertEqual(self.P.value_of(raw, ids, "order.total"), 80)

    def test_new_batch_preserves_structures_and_computed_snapshots(self):
        report = {"record_sha256": self.I._sha(self.path.read_bytes()),
                  "source_quote": "Price 20 and quantity 5", "date": "2026-09-11", "updates": [
            {"kind": "add", "id": "order.double", "body": {"rule": op("mul", ref("order.total"), num(2))}},
            {"kind": "add", "id": "c.double", "body": {"rests_on": ["order.double"],
             "verdict": "Double order remains within 250", "wrong_if": op("gt", ref("order.double"), num(250))}}]}
        state = self.path.parent / "state"
        self.I.capture(report, self.path, state, start=False)
        receipt = self.I.process(self.path, state)[0]
        self.assertEqual(receipt["state"], "applied", receipt)
        _, _, jud, _, _ = self.world()
        self.assertEqual(jud["c.double"]["snap"]["order.double"]["computed"]["value"], 200)
        self.assertIsInstance(jud["c.double"]["pred"], dict)

    def test_checked_session_renders_structured_predicate(self):
        try:
            from scripts.session.view import GroundingService
        except ImportError:
            self.skipTest("checked transport needs session extras")
        service = GroundingService("test", self.path, self.path.parent / "session", ROOT / "scripts/provenance.py")
        opened = service.opening(3000)
        _, revision = service.graph()
        text = service.reading("node:c.budget", revision, 3000)
        self.assertIn("order.total", text)
        self.assertIn("150", text)

    def test_followup_conditions_consume_computed_results_and_track_formula_changes(self):
        from scripts import followups as F, followup_triggers as T
        import datetime, yaml
        values, _, error = F.graph(str(self.path))
        self.assertIsNone(error)
        def evaluate(trigger, current):
            return T.evaluate(trigger, values=current, baseline=values, completed=set(), observations={},
                              now=datetime.datetime(2026, 9, 12, tzinfo=datetime.timezone.utc))["value"]
        self.assertTrue(evaluate({"condition": {"id": "order.total", "op": ">", "value": 50}}, values))
        self.doc["known"]["order.total"]["rule"] = op("add", num(50), num(50))
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        updated, _, error = F.graph(str(self.path))
        self.assertIsNone(error)
        self.assertTrue(evaluate({"changed": "order.total"}, updated))

    def test_watch_reports_unknown_structured_condition(self):
        from scripts import watch as W
        import yaml
        self.doc["known"]["order.quantity"]["v"] = True
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        findings = W.uncertain(self.P.load([str(self.path)]))
        self.assertIn("c.budget", findings)
        self.assertIn("cannot currently be evaluated", " ".join(findings["c.budget"]))

    def test_remeasurement_can_compare_structured_conditions(self):
        import subprocess, yaml
        self.doc["known"]["order.price"]["measure"] = "price"
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        recipe = Path(self.P.layout(self.path)["measure"])
        recipe.write_text(yaml.safe_dump({"price": [sys.executable, "-I", "-c", "print(40)"]}))
        result = subprocess.run([sys.executable, str(ROOT / "scripts/remeasure.py"), "--run", str(self.path)],
                                text=True, capture_output=True)
        self.assertNotIn("Traceback", result.stdout + result.stderr)
        self.assertIn("c.budget", result.stdout)
        self.assertNotEqual(result.returncode, 0)  # The new price actually breaks the budget.

    def test_migration_keeps_ambiguous_literals_and_comments(self):
        from scripts.expression_cli import migrate
        import yaml
        self.doc["known"]["order.total"]["rule"] = "order.price * order.quantity"
        self.doc["judgments"]["c.budget"]["wrong_if"] = "order.total > limit"
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False).replace("    rule:", "    # arithmetic source note\n    rule:"))
        preview = migrate(self.path)
        self.assertTrue(any("ambiguous" in row["reason"] for row in preview["skipped"]))
        migrate(self.path, apply=True)
        self.assertIn("# arithmetic source note", self.path.read_text())

    def test_page_renders_formula_and_result(self):
        import subprocess
        result = subprocess.run([sys.executable, str(ROOT / "scripts/render_page.py"), str(self.path)], text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("order.total", result.stdout)
        self.assertNotIn("&#x27;op&#x27;", result.stdout)

    def test_formula_removal_and_failure_preserve_review_signal(self):
        _, ids, jud, _, raw = self.world()
        old_rule = copy.deepcopy(raw["order.total"]["rule"])
        jud["c.budget"]["snap"]["order.total"] = {"computed": {"value": 100, "rule": old_rule}}
        for replacement in ({"v": 100}, {"rule": op("div", num(1), num(0))}):
            raw["order.total"] = replacement
            moved = self.P.moved_deps(jud["c.budget"], raw, ids)
            self.assertTrue(moved)
            self.assertEqual(moved[0][3], "moved")

    def test_constant_only_predicate_is_refused_by_writer_and_kernel(self):
        from scripts import expressions
        tree = op("eq", {"text": "fixed"}, {"text": "other"})
        with self.assertRaises(ValueError):
            expressions.validate(tree, predicate=True)
        _, ids, _, _, raw = self.world()
        self.assertIsNone(self.P.evaluate(tree, raw, ids))
        result = StructuredKernel.core.request({"operation": "compute", "record": self.record(), "predicate": tree})
        self.assertIsNone(result["predicate"]["holds_on_current_values"])

    def test_builtin_refs_inside_rules_are_materialized(self):
        import yaml
        self.doc["known"]["record.size"] = {"rule": op("add", ref("graph.entries"), num(1))}
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        _, ids, _, _, raw = self.world()
        self.assertIn("graph.entries", ids)
        self.assertEqual(self.P.value_of(raw, ids, "record.size"), self.P.value_of(raw, ids, "graph.entries") + 1)

    def test_identity_migration_changes_refs_but_preserves_typed_literals(self):
        from scripts import sameness
        import yaml
        data = {"known": {"note.explanation": {"text": "Use {{old.id}} for the invoice"},
                          "result.rule": {"rule": op("add", ref("old.id"), num(1))}},
                "judgments": {"c.label": {"wrong_if": op("eq", ref("old.id"), {"text": "old.id"})}}}
        rewritten, _ = sameness._rewrite_text(yaml.safe_dump(data), "old.id", "new.id")
        result = yaml.safe_load(rewritten)
        self.assertIn("{{new.id}}", result["known"]["note.explanation"]["text"])
        self.assertEqual(result["known"]["result.rule"]["rule"]["args"][0], ref("new.id"))
        self.assertEqual(result["judgments"]["c.label"]["wrong_if"]["args"][1], {"text": "old.id"})

    def test_full_identity_merge_preserves_historical_computed_text(self):
        from scripts import sameness
        import contextlib, io, yaml
        self.doc["known"].update({"old.id": {"v": 10, "from": "s.report"},
                                  "new.id": {"v": 10, "from": "s.report"},
                                  "result.label": {"rule": {"text": "old.id"}}})
        snapshot = {"computed": {"value": "old.id", "rule": {"text": "old.id"}}}
        self.doc["judgments"]["c.label"] = {"rests_on": ["result.label"], "verdict": "Label is unchanged",
            "wrong_if": op("ne", ref("result.label"), {"text": "old.id"}), "seen": {"result.label": snapshot}}
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        with contextlib.redirect_stdout(io.StringIO()):
            code = sameness.command("same", ["new.id", "old.id", "--keep", "new.id", str(self.path)])
        self.assertEqual(code, 0)
        after = yaml.safe_load(self.path.read_text())
        self.assertEqual(after["judgments"]["c.label"]["seen"]["result.label"], snapshot)
        self.assertEqual(after["known"]["result.label"]["rule"], {"text": "old.id"})
        self.assertEqual(self.P.check_lines([str(self.path)])[0], [])

    def test_migration_is_explicit_preserves_snapshots_and_refuses_new_failures(self):
        from scripts.expression_cli import migrate
        import yaml
        self.doc["known"]["order.total"]["rule"] = "order.price * order.quantity"
        self.doc["judgments"]["c.budget"]["wrong_if"] = "order.total > 150"
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        before = self.path.read_bytes()
        preview = migrate(self.path)
        self.assertEqual(len(preview["changes"]), 2)
        self.assertEqual(self.path.read_bytes(), before)
        self.assertTrue(migrate(self.path, apply=True)["applied"])
        after = yaml.safe_load(self.path.read_text())
        self.assertEqual(after["judgments"]["c.budget"]["seen"], self.doc["judgments"]["c.budget"]["seen"])
        self.doc["known"]["order.quantity"]["v"] = 10
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        before = self.path.read_bytes()
        refused = migrate(self.path, apply=True)
        self.assertFalse(refused["applied"])
        self.assertTrue(refused["problems"])
        self.assertEqual(self.path.read_bytes(), before)

    def test_grounding_migration_preserves_the_hidden_brief(self):
        from scripts.expression_cli import migrate
        import yaml
        modern = self.path.with_name("GROUNDING.yaml")
        self.path.rename(modern)
        self.path = modern
        self.doc["known"]["order.total"]["rule"] = "order.price * order.quantity"
        self.doc["judgments"]["c.budget"]["wrong_if"] = "order.total > 150"
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        brief = self.path.parent / ".kpopper/view.yaml"
        brief.parent.mkdir()
        brief.write_text(yaml.safe_dump({"title": "Order", "sections": [{"title": "Total", "pick": ["order.total"]}]}))
        before = brief.read_bytes()
        result = migrate(self.path, apply=True)
        self.assertTrue(result["applied"], result)
        self.assertEqual(brief.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
