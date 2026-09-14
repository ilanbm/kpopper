"""Behavioral contract for authored documents, source snapshots and copy-only updates."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import yaml

from scripts import documents as D
from scripts import ingestion as I

ROOT = Path(__file__).resolve().parents[1]
NOW = "2026-09-10T00:00:00+00:00"
LATER = "2026-09-11T00:00:00+00:00"


def html(body, extra=""):
    return ('<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Report</title>'
            '<style>body{color:#243342}em{font-weight:700}</style>' + extra + '</head><body>' + body +
            '<script>document.documentElement.dataset.authored="yes";</script></body></html>')


def span(cid, text):
    return '<span data-kpopper-claim="' + cid + '">' + text + '</span>'


class Documents(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.counts = {"registered": 80, "finished": 60, "other": 240, "private_unselected": "DO NOT EMBED THIS"}
        self.save_counts()
        self.source = {"name": "Reading challenge counts", "format": "json", "path": "counts.json"}
        self.registered = {"id": "registered", "label": "Registered readers", "kind": "value",
                           "inputs": [{"source": "counts", "pointer": "/registered"}]}
        self.rate = {"id": "rate", "label": "Completion rate", "kind": "ratio",
                     "inputs": [{"source": "counts", "pointer": "/finished"}, {"source": "counts", "pointer": "/registered"}],
                     "format": {"scale": 100, "decimals": 0, "suffix": "%"}}
        self.inference = {"id": "interpretation", "label": "Interpretation", "kind": "inference",
                          "inputs": [{"source": "counts"}], "reason": "An interpretation; participation alone does not establish satisfaction."}
        self.draft = html('<h1>Reading — 📚</h1><p>Of ' + span('registered', '80') + ' registered readers, ' +
                          span('rate', '75%') + ' finished.</p><p data-kpopper-claim="interpretation">'
                          'The challenge may help <em>build a reading habit</em>.</p><footer>Author’s design.</footer>')
        self.manifest = {"version": 1, "title": "Reading challenge", "language": "en",
                         "sources": {"counts": self.source}, "claims": [self.registered, self.rate, self.inference]}

    def save_counts(self):
        (self.root / "counts.json").write_text(json.dumps(self.counts), encoding="utf-8")

    def build(self):
        return D.build(self.draft, self.manifest, self.root, NOW)

    def changed(self):
        initial = self.build()
        self.counts["registered"] = 100
        self.save_counts()
        return D.refresh(initial, {"counts": self.source}, self.root, LATER)

    def decide(self, data, decision, index=0):
        data = copy.deepcopy(data)
        data["groups"][index].update(decision=decision, decided_at=LATER)
        return data

    def test_build_preserves_author_html_and_only_embeds_selected_source_values(self):
        data = self.build()
        self.assertEqual(data["authored_html"], self.draft)
        self.assertEqual(data["checks"]["registered"]["status"], "match")
        self.assertEqual(data["checks"]["rate"]["status"], "match")
        self.assertEqual(data["checks"]["interpretation"]["status"], "unchecked")
        self.assertNotIn("DO NOT EMBED THIS", D.encoded(data))
        self.assertNotIn(str(self.root), D.encoded(data))
        self.assertEqual(data["sources"]["counts"]["sha256"], D.digest((self.root / "counts.json").read_bytes()))
        self.assertEqual(data["sources"]["counts"]["read_at"], NOW)
        self.assertEqual(data["groups"], [])

    def test_source_refresh_proposes_dependent_values_atomically_and_rechecks(self):
        data = self.changed()
        self.assertEqual(len(data["groups"]), 1)
        group = data["groups"][0]
        self.assertEqual(set(group["members"]), {"registered", "rate"})
        self.assertEqual([(e["before"], e["after"]) for e in group["edits"]], [("80", "100"), ("75%", "60%")])
        self.assertEqual(group["contexts"], [{"before": "Of 80 registered readers, 75% finished.", "after": "Of 100 registered readers, 60% finished."}])
        self.assertTrue(all(c["status"] == "match" for c in group["after_checks"].values()))
        self.assertEqual(D.selected_html(data), self.draft)
        self.assertEqual(data["checks"]["interpretation"]["status"], "unchecked")
        self.assertTrue(data["checks"]["interpretation"]["source_changed"])
        self.assertEqual(data["previous_sources"]["counts"]["read_at"], NOW)
        accepted = self.decide(data, "accepted")
        self.assertEqual(D.selected_html(accepted), self.draft.replace('>80</span>', '>100</span>').replace('>75%</span>', '>60%</span>'))
        self.assertEqual(D.summary(accepted)["checks"]["rate"]["status"], "match")

    def test_rejected_copy_retains_original_content_and_mismatch(self):
        data = self.decide(self.changed(), "rejected")
        D.validate_artifact(data)
        self.assertEqual(D.selected_html(data), self.draft)
        self.assertEqual(D.summary(data)["checks"]["rate"]["status"], "mismatch")

    def test_refresh_uses_accepted_copy_as_base_and_retains_decision_history(self):
        accepted = self.decide(self.changed(), "accepted")
        self.counts["finished"] = 75
        self.save_counts()
        data = D.refresh(accepted, {"counts": self.source}, self.root, "2026-09-12T00:00:00+00:00")
        self.assertIn('>60%</span>', data["authored_html"])
        self.assertEqual(data["groups"][0]["edits"][0]["after"], "75%")
        self.assertEqual(data["history"][-1]["decisions"][0]["decision"], "accepted")

    def test_pending_proposal_is_not_silently_discarded_by_refresh(self):
        with self.assertRaisesRegex(D.DocumentError, "Decide the pending"):
            D.refresh(self.changed(), {"counts": self.source}, self.root, LATER)

    def test_partial_refresh_retains_old_source_and_does_not_claim_it_was_reread(self):
        (self.root / "other.json").write_text('{"books":240}')
        self.manifest["sources"]["other"] = {"name": "Book count", "format": "json", "path": "other.json"}
        self.manifest["claims"].append({"id": "books", "label": "Books", "kind": "value",
                                        "inputs": [{"source": "other", "pointer": "/books"}]})
        self.draft = self.draft.replace('<footer>', '<p>' + span('books', '240') + '</p><footer>')
        data = self.changed()
        self.assertFalse(data["sources"]["other"]["reread"])
        self.assertEqual(data["sources"]["other"]["read_at"], NOW)
        self.assertEqual(data["checks"]["books"]["status"], "match")
        self.assertFalse(data["checks"]["books"]["source_changed"])

    def test_missing_field_blocks_the_entire_related_group(self):
        initial = self.build()
        self.counts = {"registered": 100}
        self.save_counts()
        data = D.refresh(initial, {"counts": self.source}, self.root, LATER)
        self.assertEqual(data["checks"]["rate"]["status"], "unavailable")
        self.assertIsNone(data["checks"]["rate"]["expected"])
        self.assertEqual(data["groups"][0]["status"], "blocked")
        self.assertEqual(data["groups"][0]["edits"], [])
        with self.assertRaisesRegex(D.DocumentError, "blocked proposal"):
            D.validate_artifact(self.decide(data, "accepted"))

    def test_explicit_missing_source_never_supplies_a_number(self):
        self.manifest["sources"]["counts"] = {"name": "Counts", "unavailable": "No source was supplied"}
        data = self.build()
        for cid in ("registered", "rate"):
            self.assertEqual(data["checks"][cid]["status"], "unavailable")
            self.assertIsNone(data["checks"][cid]["expected"])
        self.assertIsNone(data["sources"]["counts"]["read_at"])
        self.assertEqual(data["groups"][0]["edits"], [])

    def test_null_text_numeric_and_zero_divisor_are_not_checked_as_numbers(self):
        for value in (None, "80", 0):
            with self.subTest(value=value):
                self.counts["registered"] = value
                self.save_counts()
                data = self.build()
                self.assertEqual(data["checks"]["rate"]["status"], "unavailable")
                self.assertIsNone(data["checks"]["rate"]["expected"])

    def test_exact_quote_is_source_grounded_and_ambiguous_quote_is_unavailable(self):
        quote = "The room is reserved for 24 participants."
        (self.root / "notes.txt").write_text("Notes\n" + quote + "\n", encoding="utf-8")
        manifest = {"version": 1, "sources": {"notes": {"name": "Notes", "path": "notes.txt", "format": "text"}},
                    "claims": [{"id": "capacity", "label": "Capacity", "kind": "quote",
                                "inputs": [{"source": "notes", "quote": quote}]}]}
        data = D.build(html('<p>' + span('capacity', quote) + '</p>'), manifest, self.root, NOW)
        self.assertEqual(data["checks"]["capacity"]["status"], "match")
        selected = next(iter(data["sources"]["notes"]["selections"].values()))
        self.assertEqual(selected["location"], "lines 2–2")
        (self.root / "notes.txt").write_text(quote + "\n" + quote)
        repeated = D.refresh(data, manifest["sources"], self.root, LATER)
        self.assertEqual(repeated["checks"]["capacity"]["status"], "unavailable")

    def test_overlapping_source_quote_matches_are_ambiguous(self):
        (self.root / "notes.txt").write_text("aaaa")
        manifest = {"version": 1, "sources": {"notes": {"name": "Notes", "path": "notes.txt", "format": "text"}},
                    "claims": [{"id": "quote", "label": "Quote", "kind": "quote",
                                "inputs": [{"source": "notes", "quote": "aaa"}]}]}
        data = D.build(html('<p>' + span('quote', 'aaa') + '</p>'), manifest, self.root, NOW)
        self.assertEqual(data["checks"]["quote"]["status"], "unavailable")

    def test_decimal_math_preserves_source_precision_and_explicit_rounding(self):
        (self.root / "counts.json").write_text('{"registered":0.1,"finished":0.2}')
        self.manifest["claims"] = [{"id": "sum", "label": "Sum", "kind": "sum",
            "inputs": self.rate["inputs"], "format": {"decimals": 2}}]
        data = D.build(html('<p>' + span('sum', '0.30') + '</p>'), self.manifest, self.root, NOW)
        self.assertEqual(data["checks"]["sum"]["status"], "match")
        self.assertEqual(D._literal({"type": "number", "value": "1234.565"}, {"decimals": 2, "thousands": ".", "decimal": ",", "prefix": "€ "}), "€ 1.234,57")

    def test_ratio_requires_an_explicit_display_precision(self):
        self.manifest["claims"][1].pop("format")
        with self.assertRaisesRegex(D.DocumentError, "explicit.*precision"):
            self.build()

    def test_source_revision_change_is_separate_from_matching_value(self):
        initial = self.build()
        self.counts["private_unselected"] = "A different unrelated field"
        self.save_counts()
        data = D.refresh(initial, {"counts": self.source}, self.root, LATER)
        self.assertEqual(data["checks"]["registered"]["status"], "match")
        self.assertTrue(data["checks"]["registered"]["source_changed"])
        self.assertEqual(data["groups"], [])

    def test_unmarked_values_and_prose_remain_visible_in_coverage(self):
        self.draft = self.draft.replace('<footer>', '<p>There are 500 additional books; this is unmarked.</p><footer>')
        coverage = self.build()["coverage"]
        self.assertEqual(coverage["anchored_claims"], 3)
        self.assertGreaterEqual(coverage["unmarked_values"], 1)
        self.assertGreater(coverage["unmarked_blocks"], 0)
        self.assertTrue(any('500' in text for text in coverage["excerpts"]))

    def test_unsupported_check_and_unknown_source_are_rejected(self):
        for mutate in (lambda m: m["claims"][0].update(kind="eval"),
                       lambda m: m["claims"][0]["inputs"][0].update(source="invented"),
                       lambda m: m["claims"][0].update(expression="run_this()")):
            with self.subTest(mutate=mutate):
                manifest = copy.deepcopy(self.manifest)
                mutate(manifest)
                with self.assertRaises(D.DocumentError):
                    D.build(self.draft, manifest, self.root, NOW)

    def test_duplicate_keys_nonfinite_numbers_and_bad_pointers_are_rejected(self):
        for raw in ('{"a":1,"a":2}', '{"a":NaN}', '{"a":1e999}'):
            with self.subTest(raw=raw), self.assertRaises(D.DocumentError):
                D.read_json(raw)
        self.manifest["claims"][0]["inputs"][0]["pointer"] = "/bad~2escape"
        with self.assertRaises(D.DocumentError):
            self.build()

    def test_source_path_escape_and_symlink_are_rejected(self):
        self.manifest["sources"]["counts"]["path"] = "../counts.json"
        with self.assertRaises(D.DocumentError):
            self.build()
        target = self.root / "linked.json"
        try:
            target.symlink_to(self.root / "counts.json")
        except OSError:
            self.skipTest("Symlinks are unavailable on this platform")
        self.manifest["sources"]["counts"]["path"] = "linked.json"
        with self.assertRaises(D.DocumentError):
            self.build()

    def test_source_read_race_is_refused_without_emitting_a_combined_snapshot(self):
        original = D.read_bytes
        reads = []
        def moving(path, limit=D.MAX_SOURCE):
            reads.append(path)
            value = original(path, limit)
            return value if len(reads) == 1 else value + b" "
        with patch.object(D, "read_bytes", side_effect=moving), self.assertRaisesRegex(D.DocumentError, "changed during capture"):
            self.build()

    def test_saved_check_proposal_or_author_tampering_is_rejected(self):
        data = self.changed()
        for mutate in (lambda d: d["checks"]["rate"].update(status="match"),
                       lambda d: d["groups"][0]["edits"][0].update(after_raw="999"),
                       lambda d: d.update(authored_html=d["authored_html"] + "x")):
            with self.subTest(mutate=mutate):
                changed = copy.deepcopy(data)
                mutate(changed)
                with self.assertRaises(D.DocumentError):
                    D.validate_artifact(changed)

    def test_whole_payload_serialization_escapes_author_and_source_script_terminators(self):
        data = self.build()
        data["title"] = '</script><script>alert("source")</script>'
        serialized = D.safe_json(data)
        self.assertNotIn('<', serialized)
        self.assertEqual(json.loads(serialized), data)

    def test_two_independent_paragraphs_have_separate_atomic_proposals(self):
        self.counts["other"] = 241
        self.save_counts()
        self.manifest["claims"].append({"id": "books", "label": "Books", "kind": "value",
                                       "inputs": [{"source": "counts", "pointer": "/other"}]})
        self.draft = self.draft.replace('<footer>', '<p>' + span('books', '240') + '</p><footer>')
        data = self.build()
        self.assertEqual(len(data["groups"]), 1)
        data = self.decide(data, "accepted")
        self.counts.update(registered=100, other=250)
        self.save_counts()
        refreshed = D.refresh(data, {"counts": self.source}, self.root, LATER)
        self.assertEqual(len(refreshed["groups"]), 2)
        self.assertEqual([set(g["members"]) for g in refreshed["groups"]], [{'registered', 'rate'}, {'books'}])


class RecordSources(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.path = self.root / "PROVENANCE.yaml"
        self.state = self.root / "state"
        self.doc = {"sources": {"s.report": {"name": "Count report", "file": "report.txt", "read": "2026-09-10"}},
                    "known": {"reading.capacity": {"name": "Capacity", "v": 80, "from": "s.report", "at": "capacity line"}}}
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False), encoding="utf-8")
        self.source = {"name": "Recorded capacity", "path": "PROVENANCE.yaml", "format": "record"}
        self.manifest = {"version": 1, "sources": {"record": self.source}, "claims": [
            {"id": "capacity", "label": "Capacity", "kind": "value", "inputs": [{"source": "record", "pointer": "/reading.capacity/v"}]}]}
        self.draft = html('<p>' + span('capacity', '80') + '</p>')

    def apply_batch(self, updates):
        envelope = {"source_quote": "Capacity 100; reserve 20.", "date": "2026-09-11",
                    "record_sha256": I._sha(self.path.read_bytes()), "updates": updates}
        event = I.capture(envelope, self.path, self.state, start=False)
        result = I.process(self.path, self.state, event_id=event["event_id"])[0]
        self.assertEqual(result["state"], "applied", result)
        return {**self.source, "event_id": event["event_id"], "state_dir": "state"}

    def test_applied_set_batch_can_refresh_document(self):
        data = D.build(self.draft, self.manifest, self.root, NOW)
        spec = self.apply_batch([{"kind": "set", "id": "reading.capacity", "value": 100}])
        after = D.refresh(data, {"record": spec}, self.root, LATER)
        self.assertEqual(after["groups"][0]["edits"][0]["after"], "100")
        self.assertEqual(after["sources"]["record"]["event"]["targets"], ["reading.capacity"])

    def test_batch_checks_every_selected_reading_and_its_citation(self):
        self.doc["known"]["reading.reserve"] = {"v": 10, "from": "s.report"}
        self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False))
        self.manifest["claims"].append({"id": "reserve", "label": "Reserve", "kind": "value",
            "inputs": [{"source": "record", "pointer": "/reading.reserve/v"}]})
        self.draft = html('<p>' + span('capacity', '80') + ' ' + span('reserve', '10') + '</p>')
        data = D.build(self.draft, self.manifest, self.root, NOW)
        spec = self.apply_batch([{"kind": "set", "id": "reading.capacity", "value": 100},
                                 {"kind": "set", "id": "reading.reserve", "value": 20}])
        after = D.refresh(data, {"record": spec}, self.root, LATER)
        self.assertEqual(after["sources"]["record"]["event"]["targets"], ["reading.capacity", "reading.reserve"])
        changed = yaml.safe_load(self.path.read_text())
        changed["known"]["reading.reserve"]["from"] = "s.report"
        self.path.write_text(yaml.safe_dump(changed, sort_keys=False))
        with self.assertRaisesRegex(D.DocumentError, "does not match"):
            D.refresh(data, {"record": spec}, self.root, LATER)

    def test_batch_new_reading_can_bind_a_document_source(self):
        spec = self.apply_batch([{"kind": "add", "id": "reading.reserve", "body": {"v": 20}}])
        self.manifest["sources"]["record"] = spec
        self.manifest["claims"][0]["inputs"][0]["pointer"] = "/reading.reserve/v"
        data = D.build(self.draft, self.manifest, self.root, NOW)
        self.assertEqual(data["sources"]["record"]["event"]["targets"], ["reading.reserve"])

    def test_existing_source_events_bind_set_add_and_single_readings(self):
        for kind in ('set', 'add', 'single'):
            with self.subTest(kind=kind):
                self.path.write_text(yaml.safe_dump(self.doc, sort_keys=False), encoding='utf-8')
                envelope = {'event_id': kind, 'source': 's.report', 'at': 'shared line',
                            'date': '2026-09-11', 'source_quote': 'Capacity 100; reserve 20.',
                            'record_sha256': I._sha(self.path.read_bytes())}
                if kind == 'single':
                    envelope.update(target='reading.capacity', value=100)
                else:
                    envelope['updates'] = [{'kind': kind, 'id': 'reading.reserve' if kind == 'add' else 'reading.capacity',
                                            'at': 'reserve line' if kind == 'add' else 'capacity line',
                                            **({'body': {'v': 20}} if kind == 'add' else {'value': 100})}]
                receipt = I.update(envelope, self.path, self.state)
                self.assertEqual(receipt['state'], 'applied', receipt)
                manifest = copy.deepcopy(self.manifest)
                manifest['sources']['record'] = {**self.source, 'event_id': receipt['event_id'], 'state_dir': 'state'}
                if kind == 'add':
                    manifest['claims'][0]['inputs'][0]['pointer'] = '/reading.reserve/v'
                data = D.build(self.draft, manifest, self.root, NOW)
                selection = next(iter(data['sources']['record']['selections'].values()))
                self.assertEqual(selection['citation']['source'], 's.report')
                self.assertEqual(data['sources']['record']['event']['state'], 'applied')

    def test_old_event_cannot_bind_same_value_with_a_new_location_or_date(self):
        data = D.build(self.draft, self.manifest, self.root, NOW)
        envelope = {'source': 's.report', 'at': 'capacity line', 'date': '2026-09-11',
                    'source_quote': 'Capacity is 100.', 'record_sha256': I._sha(self.path.read_bytes()),
                    'updates': [{'kind': 'set', 'id': 'reading.capacity', 'value': 100}]}
        receipt = I.update(envelope, self.path, self.state)
        self.assertEqual(receipt['state'], 'applied', receipt)
        spec = {**self.source, 'event_id': receipt['event_id'], 'state_dir': 'state'}
        updated = self.path.read_bytes()
        self.assertEqual(D.refresh(data, {'record': spec}, self.root, LATER)['sources']['record']['event']['state'], 'applied')
        for field, value in [('at', 'different line'), ('of', '2026-09-12')]:
            with self.subTest(field=field):
                changed = yaml.safe_load(updated)
                changed['known']['reading.capacity'][field] = value
                self.path.write_text(yaml.safe_dump(changed, sort_keys=False), encoding='utf-8')
                with self.assertRaisesRegex(D.DocumentError, 'does not match'):
                    D.refresh(data, {'record': spec}, self.root, LATER)

    def test_record_pointer_and_hypotheses_are_rejected_before_canonical_file_reads(self):
        for context in ("pointer", "hypothesis"):
            with self.subTest(context=context):
                self.path.write_text(yaml.safe_dump({"record": "../outside.yaml"} if context == "pointer" else self.doc))
                if context == "hypothesis":
                    folder = self.root / "PROVENANCE.d"
                    folder.mkdir()
                    (folder / "candidate.yaml").write_text("known: {}")
                with patch.object(I, "_target", side_effect=AssertionError("must not follow additional files")) as target:
                    data = D.build(self.draft, self.manifest, self.root, NOW)
                    self.assertEqual(data["checks"]["capacity"]["status"], "unavailable")
                    target.assert_not_called()

    def test_invalid_record_is_explicitly_unavailable(self):
        self.path.write_text("known: [invalid YAML")
        data = D.build(self.draft, self.manifest, self.root, NOW)
        self.assertEqual(data["checks"]["capacity"]["status"], "unavailable")

    def test_record_filename_glob_characters_cannot_select_a_different_file(self):
        actual = self.root / "record[1].yaml"
        actual.write_bytes(self.path.read_bytes())
        other = copy.deepcopy(self.doc)
        other["known"]["reading.capacity"]["v"] = 999
        (self.root / "record1.yaml").write_text(yaml.safe_dump(other))
        self.manifest["sources"]["record"]["path"] = actual.name
        data = D.build(self.draft, self.manifest, self.root, NOW)
        self.assertEqual(data["checks"]["capacity"]["status"], "match")
        self.assertEqual(data["checks"]["capacity"]["expected"], "80")

    def test_noncanonical_event_id_is_rejected_before_state_lookup(self):
        self.manifest["sources"]["record"] = {**self.source, "event_id": "../../outside", "state_dir": "state"}
        with patch.object(I, "status", side_effect=AssertionError("event lookup escaped identity validation")) as status:
            with self.assertRaises(D.DocumentError):
                D.build(self.draft, self.manifest, self.root, NOW)
            status.assert_not_called()

    def test_canonical_read_retains_citation_and_does_not_write_the_record(self):
        before = self.path.read_bytes()
        data = D.build(self.draft, self.manifest, self.root, NOW)
        self.assertEqual(self.path.read_bytes(), before)
        selected = next(iter(data["sources"]["record"]["selections"].values()))
        self.assertEqual(selected["citation"]["source"], "s.report")
        self.assertEqual(selected["citation"]["at"], "capacity line")
        self.assertEqual(data["checks"]["capacity"]["status"], "match")

    def test_capture_is_not_applied_and_repeated_empty_pass_recovers_by_event_id(self):
        data = D.build(self.draft, self.manifest, self.root, NOW)
        envelope = {"event_id": "document-test", "target": "reading.capacity", "value": 100,
                    "source_quote": "Capacity is now 100.", "date": "2026-09-11"}
        captured = I.capture(envelope, self.path, self.state, start=False)
        spec = {**self.source, "event_id": captured["event_id"], "state_dir": "state"}
        with self.assertRaisesRegex(D.DocumentError, "not durably applied"):
            D.refresh(data, {"record": spec}, self.root, LATER)
        processed = I.process(self.path, self.state, event_id=captured["event_id"])
        self.assertEqual(processed[0]["state"], "applied", processed)
        self.assertEqual(I.process(self.path, self.state, event_id=captured["event_id"]), [])
        refreshed = D.refresh(data, {"record": spec}, self.root, LATER)
        self.assertEqual(refreshed["sources"]["record"]["event"]["state"], "applied")
        self.assertEqual(refreshed["groups"][0]["edits"][0]["after"], "100")
        duplicate = I.capture(envelope, self.path, self.state, start=False)
        self.assertEqual(duplicate["event_id"], captured["event_id"])
        with self.assertRaises(ValueError):
            I.capture({**envelope, "date": "2026-09-12"}, self.path, self.state, start=False)

    def test_corrupt_event_state_has_a_clean_cli_error_and_no_output(self):
        captured = I.capture({"target": "reading.capacity", "value": 100, "source_quote": "100 places"}, self.path, self.state, start=False)
        self.manifest["sources"]["record"] = {**self.source, "event_id": captured["event_id"], "state_dir": "state"}
        (self.state / "events" / (captured["event_id"] + ".json")).write_text('{"state":')
        (self.root / "manifest.json").write_text(json.dumps(self.manifest))
        (self.root / "draft.html").write_text(self.draft)
        result = subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), "document", "build",
                                 "--html", "draft.html", "--manifest", "manifest.json", "--out", "out.html"],
                                cwd=self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("ingestion outcome", result.stderr)
        self.assertNotIn("Traceback", result.stderr)
        self.assertFalse((self.root / "out.html").exists())

    def test_stale_event_citation_cannot_be_used_to_bind_a_newer_reading(self):
        first = I.capture({"target": "reading.capacity", "value": 100, "source_quote": "100 places", "date": "2026-09-11"}, self.path, self.state, start=False)
        I.process(self.path, self.state, first["event_id"])
        second = I.capture({"target": "reading.capacity", "value": 110, "source_quote": "110 places", "date": "2026-09-12"}, self.path, self.state, start=False)
        I.process(self.path, self.state, second["event_id"])
        self.manifest["sources"]["record"] = {**self.source, "event_id": first["event_id"], "state_dir": "state"}
        with self.assertRaisesRegex(D.DocumentError, "does not match"):
            D.build(self.draft, self.manifest, self.root, NOW)


if __name__ == "__main__":
    unittest.main()
