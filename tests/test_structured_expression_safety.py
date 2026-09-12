"""Regressions for unavailable calculations, conservative migration and evidence reads."""
import copy
import datetime as dt
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import yaml

from scripts import expressions as E, followups as F, followup_triggers as T
from scripts import ingestion as I, render_page as R
from scripts.expression_cli import migrate
from scripts.session.core import Core

P = I.P


def ref(key):
    return {"ref": key}


def num(value):
    return {"num": str(value)}


def op(name, left, right):
    return {"op": name, "args": [left, right]}


class RecordCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.path = self.root / "GROUNDING.yaml"
        self.rule = op("mul", ref("order.price"), num(5))
        self.doc = {"sources": {"s.report": {"file": "report.md"}},
                    "known": {"order.price": {"v": 20, "from": "s.report"},
                              "order.total": {"rule": self.rule}},
                    "judgments": {"c.budget": {"rests_on": ["order.total"], "verdict": "Affordable",
                        "wrong_if": op("gt", ref("order.total"), num(150)),
                        "seen": {"order.total": {"computed": {"value": 100, "rule": copy.deepcopy(self.rule)}}}}}}
        (self.root / "report.md").write_text("Price 20.")
        self.save()

    def save(self):
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))

    def world(self):
        doc = P.load([str(self.path)])
        ids, jud, fields = P.infer(doc)
        return ids, jud, fields, P.with_builtins(doc, ids, jud, fields)

    def trigger(self, trigger, values, baseline=None):
        return T.evaluate(trigger, values, baseline or {}, set(), {}, dt.datetime.now(dt.timezone.utc))["value"]


class WithoutCore(RecordCase):
    def setUp(self):
        super().setUp()
        # A fresh process on an installation without the compiled core, regardless
        # of other tests having cached an evaluator from a different module import.
        for module in {E, P.E, F.P.E, R.P.E}:
            guard = patch.object(module, "_core_type", side_effect=ValueError("checked session core is not ready; run kpopper session setup"))
            guard.start()
            self.addCleanup(guard.stop)
            previous = module._CORE
            module._CORE = None
            self.addCleanup(setattr, module, "_CORE", previous)

    def test_unknown_is_visible_on_reader_page_and_ingestion(self):
        ids, jud, fields, raw = self.world()
        self.assertEqual(P._state("c.budget", jud["c.budget"], raw, ids, fields)[0], "UNKNOWN")
        self.assertIn("unknown", P.flags(ids, jud, fields, raw)["c.budget"])
        self.assertEqual(P.flags(ids, jud, fields, raw), R.flags_of(ids, jud, fields, raw))
        self.assertIn("UNKNOWN", I.ACTIONABLE)
        for language in ("en", "he", "ar"):
            self.doc["meta"] = {"lang": language}
            self.save()
            page = R.build([str(self.path)], None)
            self.assertIn("unknown", page[4]["flags"]["c.budget"])

    def test_followups_keep_independent_values_and_unknown_is_not_a_change(self):
        values, flags, error = F.graph(str(self.path))
        self.assertIsNone(error)
        self.assertEqual(values["order.price"], 20)
        self.assertTrue(self.trigger({"condition": {"id": "order.price", "op": ">", "value": 10}}, values))
        for trigger in ({"condition": {"id": "order.total", "op": "!=", "value": 100}},
                        {"changed": "order.total"}):
            self.assertIsNone(self.trigger(trigger, values, {"order.total": {"computed": {"value": 100, "rule": self.rule}}}))

    def test_a_write_that_needs_a_computed_snapshot_remains_refused(self):
        ids, jud, fields, raw = self.world()
        with self.assertRaises(P.Refused):
            P.snapshot_value("order.total", raw, ids, jud, {})
        self.assertEqual(P.value_of(raw, ids, "order.price"), 20)

    def test_moved_bare_scalar_still_has_a_state(self):
        self.doc["known"]["order.price"] = 21
        self.doc["judgments"]["c.price"] = {"rests_on": ["order.price"], "seen": {"order.price": 20}, "verdict": "Price"}
        self.save()
        ids, jud, fields, raw = self.world()
        self.assertEqual(P._state("c.price", jud["c.price"], raw, ids, fields)[0], "MOVED")

    def test_independent_write_and_scheduled_followup_work_without_core(self):
        import contextlib
        import io
        with contextlib.redirect_stdout(io.StringIO()):
            P.apply([str(self.path)], {"kind": "add", "id": "order.other", "body": {"v": 7, "from": "s.report"}})
        self.assertEqual(yaml.safe_load(self.path.read_text())["known"]["order.other"]["v"], 7)
        with patch.dict(os.environ, {"XDG_STATE_HOME": str(self.root / "state")}):
            store = F.Store(self.root)
            store.setup(timezone="UTC")
            for key, related in (("independent", "order.price"), ("dependent", "order.total")):
                store.add({"id": key, "title": "Review", "why": "Check a reading", "how": "Read it",
                           "scope": "Read only", "related": [related], "when": {"at": "2026-01-01"}})
            states = {row["id"]: row["state"] for row in store.scan()["items"]}
            self.assertEqual(states, {"independent": "ready", "dependent": "unknown"})


class WithCore(RecordCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.core = Core()
        except ValueError:
            if os.environ.get("KPOPPER_REQUIRE_CORE_TESTS") == "1":
                raise
            raise unittest.SkipTest("build the packaged core to exercise expressions")

    def test_builtin_followup_and_unrelated_failed_rule_are_isolated(self):
        self.doc["known"]["record.size"] = {"rule": op("add", ref("graph.entries"), num(1))}
        self.doc["known"]["order.invalid"] = {"rule": op("div", num(1), num(0))}
        self.save()
        ids, jud, fields, raw = self.world()
        values, _, error = F.graph(str(self.path))
        self.assertIsNone(error)
        self.assertEqual(values["record.size"]["computed"]["value"], P.value_of(raw, ids, "graph.entries") + 1)
        self.assertTrue(self.trigger({"condition": {"id": "order.total", "op": "==", "value": 100}}, values))
        self.assertIsNone(self.trigger({"changed": "order.invalid"}, values, {"order.invalid": 2}))

    def test_nonfinite_reading_only_invalidates_calculations_that_read_it(self):
        for value in (float("nan"), float("inf"), float("-inf")):
            with self.subTest(value=value):
                self.doc["known"]["order.invalid"] = {"v": value, "quoted": 10}
                self.doc["known"]["order.dependent"] = {"rule": op("add", ref("order.invalid"), num(1))}
                self.save()
                ids, jud, fields, raw = self.world()
                self.assertEqual(P.value_of(raw, ids, "order.total"), 100)
                self.assertIsNone(P.value_of(raw, ids, "order.dependent"))
                values, _, error = F.graph(str(self.path))
                self.assertIsNone(error)
                self.assertIsNone(self.trigger({"changed": "order.invalid"}, values, {"order.invalid": 2}))

    def test_migration_refuses_a_known_condition_becoming_unknown(self):
        for reading in ("120", "80", "1,250"):
            with self.subTest(reading=reading):
                self.doc["known"] = {"order.price": {"quoted": reading, "from": "s.report"}}
                self.doc["judgments"]["c.budget"].update(rests_on=["order.price"], seen={"order.price": reading}, wrong_if="order.price > 100")
                self.save()
                before = self.path.read_bytes()
                result = migrate(self.path, apply=True)
                self.assertFalse(result["applied"])
                self.assertTrue(any("condition" in problem for problem in result["problems"]), result)
                self.assertEqual(self.path.read_bytes(), before)

    def test_structured_numeric_text_is_unknown_without_coercing_text_literals(self):
        raw = {"input.text": {"quoted": "120"}}
        ids = set(raw)
        self.assertIsNone(P.evaluate(op("gt", ref("input.text"), num(100)), raw, ids))
        self.assertTrue(P.evaluate(op("eq", ref("input.text"), {"text": "120"}), raw, ids))

    def test_migration_preserves_formula_only_history_and_requests_first_numeric_review(self):
        self.doc["known"]["order.total"]["rule"] = "order.price * 5"
        self.doc["judgments"]["c.budget"].update(wrong_if="order.total > 150", seen={"order.total": "order.price * 5"})
        self.save()
        self.assertTrue(migrate(self.path, apply=True)["applied"])
        ids, jud, fields, raw = self.world()
        self.assertEqual(jud["c.budget"]["snap"], {"order.total": "order.price * 5"})
        self.assertEqual(P.moved_deps(jud["c.budget"], raw, ids), [])
        tag, reason = P._state("c.budget", jud["c.budget"], raw, ids, fields)
        self.assertEqual(tag, "UNCHECKED")
        self.assertIn("formula", reason)

    def test_plain_decimal_comparison_is_numeric(self):
        raw = {"input.one": {"v": 1.0}, "input.other": {"v": 1}}
        for operator, expected in (("eq", True), ("gt", False), ("lt", False)):
            self.assertIs(P.evaluate(op(operator, ref("input.one"), ref("input.other")), raw, set(raw)), expected)


class StaticExpressions(unittest.TestCase):
    def test_arrangement_bounds_and_types_apply_to_both_formats(self):
        for source in ("graph.flagged < 0", "graph.entries == True", "page.drift > 5"):
            with self.subTest(source=source):
                self.assertTrue(P.one_comparison(source))
                self.assertEqual(P.one_comparison(E.convert(source, predicate=True)).lower(), P.one_comparison(source).lower())

    def test_deep_legacy_bridge_returns_unknown_instead_of_raising(self):
        raw = {"order.total": {"rule": num(1)}}
        with patch.object(P.E, "convert", side_effect=RecursionError("too deep")):
            self.assertIsNone(P.evaluate("order.total > 0", raw, set(raw)))
