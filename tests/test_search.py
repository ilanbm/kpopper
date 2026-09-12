"""Find source evidence across languages and retain its current epistemic scope."""
import json
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("search_ingestion", ROOT / "scripts/ingestion.py")
I = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(I)


class Search(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.rec = self.root / "PROVENANCE.yaml"
        self.source = self.root / "contract.md"
        self.source.write_text("הסכם ספק\nמחיר ההובלה הוא 80 שקלים.\nתשלום בהעברה.\n", encoding="utf-8")
        self.doc = {
            "schema": {"deps": "rests_on", "snapshot": "seen", "predicate": "wrong_if"},
            "sources": {"s.contract": {"name": "הסכם ספק", "file": "contract.md"}},
            "known": {"shipping.cost": {"v": 80, "name": "עלות המשלוח", "from": "s.contract", "at": "line 2"}},
            "judgments": {"c.shipping": {"rests_on": ["shipping.cost"], "verdict": "Shipping remains affordable",
                           "wrong_if": "shipping.cost > 100", "seen": {"shipping.cost": 50}}},
        }
        self.rec.write_text(yaml.safe_dump(self.doc, allow_unicode=True, sort_keys=False))
        self.env = {**os.environ, "XDG_STATE_HOME": str(self.root / "state")}

    def cli(self, *args):
        p = subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), "search", *args,
                            "--record", str(self.rec), "--json"],
                           cwd=self.root, env=self.env, text=True, capture_output=True)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        return json.loads(p.stdout)

    def test_hebrew_source_match_and_exact_read_preserve_records_and_ingestion_state(self):
        before = self.rec.read_bytes()
        found = self.cli("ההובלה")
        hit = next(x for x in found["results"] if x["kind"] == "source")
        self.assertIn("מחיר ההובלה הוא 80 שקלים.", hit["excerpt"])
        self.assertEqual(hit["line"], 2)
        full = self.cli("--read", hit["ref"], "--revision", found["revision"])
        self.assertEqual(full["content"], self.source.read_text())
        self.assertEqual(self.rec.read_bytes(), before)
        self.assertFalse((self.root / "state" / "kpopper" / "ingestion").exists())

    def test_no_cache_source_search_performs_no_state_writes(self):
        self.env["KPOPPER_NO_CACHE"] = "1"
        found = self.cli("ההובלה")
        self.assertTrue(found["results"])
        self.assertFalse((self.root / "state").exists())

    def test_claim_match_carries_sources_dependencies_and_live_state(self):
        self.doc["known"]["shipping.cost"]["v"] = 120
        self.rec.write_text(yaml.safe_dump(self.doc, allow_unicode=True, sort_keys=False))
        found = self.cli("affordable")
        hit = next(x for x in found["results"] if x["id"] == "c.shipping")
        self.assertEqual(hit["status"], "FIRED")
        self.assertEqual(hit["dependencies"], ["shipping.cost"])
        self.assertEqual(hit["sources"], ["s.contract"])

    def test_inline_rule_retains_its_source_chain(self):
        self.doc["known"]["shipping.double"] = {"v": "shipping.cost * 2", "name": "Double cost"}
        self.doc["known"]["shipping.quote"] = {"quoted": "shipping.cost * 2", "name": "Unlinked quotation"}
        self.rec.write_text(yaml.safe_dump(self.doc, allow_unicode=True, sort_keys=False))
        found = self.cli("Double")
        hit = next(x for x in found["results"] if x["id"] == "shipping.double")
        self.assertEqual(hit["sources"], ["s.contract"])
        self.assertEqual(hit["rule_dependencies"], ["shipping.cost"])
        quote = self.cli("Unlinked")["results"][0]
        self.assertEqual(quote["sources"], [])
        self.assertEqual(quote["rule_dependencies"], [])

    def test_source_change_rejects_old_revision(self):
        found = self.cli("ההובלה")
        hit = next(x for x in found["results"] if x["kind"] == "source")
        self.source.write_text("המחיר עודכן\n")
        p = subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), "search", "--read", hit["ref"],
                            "--revision", found["revision"], "--record", str(self.rec), "--json"],
                           cwd=self.root, env=self.env, text=True, capture_output=True)
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("changed", p.stdout + p.stderr)

    def test_budget_reports_omitted_results(self):
        found = self.cli("shipping", "--limit", "1", "--chars", "2200")
        self.assertEqual(len(found["results"]), 1)
        self.assertGreater(found["omitted"], 0)
        self.assertLessEqual(len(json.dumps(found, ensure_ascii=False)), 2200)

    def test_query_is_literal_and_needs_no_english_rewriter(self):
        self.assertTrue(self.cli("עלות המשלוח")["results"])
        self.assertEqual(self.cli('unfindable " OR * - :')["results"], [])

    def test_unresolved_report_is_searchable_but_not_a_recorded_fact(self):
        state = self.root / "capture-state"
        report = {"event_id": "unresolved", "source_quote": "המשלוח של אורן בוטל.",
                  "question": "לאיזה משלוח הכוונה?"}
        event = I.capture(report, self.rec, state, start=False)
        I.process(self.rec, state)
        result = self.cli("אורן", "--state-dir", str(state))
        hit = result["results"][0]
        self.assertEqual(hit["scope"], "captured_report")
        self.assertEqual(hit["status"], "needs_primary")
        self.assertEqual(hit["question"], report["question"])
        self.assertEqual(hit["id"], event["event_id"])
        full = self.cli("--read", hit["ref"], "--revision", result["revision"], "--state-dir", str(state))
        self.assertEqual(full["content"], report["source_quote"])

    def test_tampered_capture_is_not_indexed_as_evidence(self):
        state = self.root / "capture-state"
        event = I.capture({"source_quote": "Original source", "question": "Which entry?"}, self.rec, state, start=False)
        Path(I.status(event["event_id"], self.rec, state)["source_file"]).write_text("forged-evidence")
        result = self.cli("forged", "--state-dir", str(state))
        self.assertEqual(result["results"], [])
        self.assertGreater(result["unindexed_count"], 0)

    def test_hypothesis_does_not_become_a_current_fact(self):
        hypotheses = self.root / "PROVENANCE.d"
        hypotheses.mkdir()
        (hypotheses / "alternative.yaml").write_text(yaml.safe_dump({
            "known": {"shipping.cost": {"v": 30, "name": "Alternative freight", "from": "s.contract"}}}))
        result = self.cli("alternative")
        hit = next(x for x in result["results"] if x["id"] == "shipping.cost")
        self.assertEqual(hit["scope"], "hypothesis:alternative")
        self.assertEqual(hit["status"], "HYPOTHESIS")
        self.assertEqual(yaml.safe_load(self.rec.read_text())["known"]["shipping.cost"]["v"], 80)

    def test_pointer_resolves_source_relative_to_owning_file(self):
        sub = self.root / "records"
        sub.mkdir()
        self.rec.rename(sub / "PROVENANCE.yaml")
        self.source.rename(sub / "contract.md")
        self.rec.write_text("record: records/PROVENANCE.yaml\n")
        result = self.cli("ההובלה")
        hit = next(x for x in result["results"] if x["kind"] == "source")
        self.assertEqual(Path(hit["file"]), (sub / "contract.md").resolve())

    def test_source_origin_follows_pointer_merge_order(self):
        for name in ("z", "a"):
            folder = self.root / name
            folder.mkdir()
            (folder / "text.md").write_text(name + " evidence")
            (folder / "PROVENANCE.yaml").write_text(yaml.safe_dump({
                "sources": {"s.source": {"name": name + " source", "file": "text.md"}}}))
        self.rec.write_text("also: [z/PROVENANCE.yaml, a/PROVENANCE.yaml]\n")
        result = self.cli("evidence")
        hit = next(x for x in result["results"] if x["kind"] == "source")
        self.assertEqual(hit["name"], "a source")
        self.assertEqual(hit["excerpt"], "a evidence")
        self.assertEqual(Path(hit["file"]), (self.root / "a/text.md").resolve())

    def test_hypothesis_source_override_does_not_borrow_a_pointer_origin(self):
        folder = self.root / "base"
        folder.mkdir()
        self.rec.rename(folder / self.rec.name)
        self.source.rename(folder / self.source.name)
        self.rec.write_text("record: base/PROVENANCE.yaml\n")
        hypotheses = self.root / "PROVENANCE.d"
        hypotheses.mkdir()
        (self.root / "alternative.md").write_text("Alternative source evidence")
        (hypotheses / "other.yaml").write_text(yaml.safe_dump({
            "sources": {"s.contract": {"name": "Other source", "file": "alternative.md"}}}))
        result = self.cli("Alternative")
        hit = next(x for x in result["results"] if x["kind"] == "source")
        self.assertEqual(hit["scope"], "hypothesis:other")
        self.assertEqual(Path(hit["file"]), (self.root / "alternative.md").resolve())

    def test_missing_source_and_partial_reads_are_explicit(self):
        self.source.unlink()
        missing = self.cli("ההובלה")
        self.assertEqual(missing["results"], [])
        self.assertEqual(missing["unindexed_count"], 1)
        text = "תחילת המסמך\n" + "הרבה מידע\n" * 700 + "מחטבסוף\n"
        self.source.write_text(text)
        found = self.cli("מחטבסוף")
        hit = found["results"][0]
        self.assertIn("מחטבסוף", hit["excerpt"])
        self.assertTrue(hit["excerpt_truncated"])
        read = self.cli("--read", hit["ref"], "--revision", found["revision"], "--length", "100")
        self.assertFalse(read["complete"])
        self.assertEqual(read["content"], text[:100])
        self.assertEqual(read["next_offset"], 100)

    def test_end_to_end_capture_then_find_claim_and_its_source(self):
        state = self.root / "capture-state"
        report = {"date": "2026-09-10", "source_quote": "Shipment Cedar has five packages.", "updates": [
            {"kind": "add", "id": "cedar.packages", "body": {"name": "משלוח Cedar", "v": 5}},
            {"kind": "add", "id": "c.cedar", "body": {"rests_on": ["cedar.packages"],
                "verdict": "Cedar fits one van", "wrong_if": "cedar.packages > 10"}},
        ]}
        report["record_sha256"] = self.cli("shipping")["record_sha256"]
        self.assertEqual(I.capture(report, self.rec, state, start=False)["state"], "captured")
        receipt = I.process(self.rec, state)[0]
        self.assertEqual(receipt["state"], "applied", receipt)
        found = self.cli("Cedar", "--state-dir", str(state), "--limit", "10")
        claim = next(x for x in found["results"] if x["id"] == "c.cedar")
        self.assertEqual(claim["sources"], [receipt["source"]])
        sources = [x for x in found["results"] if x.get("file") == receipt["source_file"]]
        self.assertEqual(len(sources), 1)
        self.assertEqual(sources[0]["kind"], "source")
        self.assertIn(report["source_quote"], sources[0]["excerpt"])


if __name__ == "__main__":
    unittest.main()
