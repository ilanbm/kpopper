"""Source-preserving readings and finishing with a legitimately falsified judgment."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import yaml

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"


class Ingestion(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.rec = self.root / "PROVENANCE.yaml"
        self.mark = self.root / "mark.json"
        for name in ("old.txt", "new.txt"):
            (self.root / name).write_text("Synthetic source.\n")
        self.doc = {
            "meta": {"name": "Reading test", "updated": "2026-09-06"},
            "sources": {
                "s.old": {"name": "Old report", "file": "old.txt", "read": "2026-09-06"},
                "s.new": {"name": "User correction", "file": "new.txt", "read": "2026-09-07"},
            },
            "known": {
                "pay.deposited": {"name": "Cheque deposited", "v": True, "of": "2026-09-06",
                                  "from": "s.old", "at": "line 1"},
                "pay.other": {"name": "Other cheque", "v": True, "from": "s.old"},
            },
            "judgments": {
                "c.complete": {"rests_on": ["pay.deposited"], "verdict": "Complete",
                               "because": "This fixture requires the cheque to be deposited.",
                               "wrong_if": "pay.deposited == false", "seen": {"pay.deposited": True}},
                "c.other": {"rests_on": ["pay.other"], "verdict": "Other complete",
                            "wrong_if": "pay.other == false", "seen": {"pay.other": True}},
            },
        }
        self.save(self.doc)

    def save(self, doc):
        self.rec.write_text(yaml.safe_dump(doc, sort_keys=False))

    def read(self):
        return yaml.safe_load(self.rec.read_text())

    def cli(self, *args):
        p = subprocess.run([sys.executable, str(SCRIPTS / "provenance.py"), *map(str, args)],
                           cwd=self.root, text=True, capture_output=True)
        return p.returncode, p.stdout + p.stderr

    def update(self, *extra):
        return self.cli("set", "pay.deposited", "false", "--as-of", "2026-09-07", *extra)

    def test_set_updates_the_value_and_its_citation(self):
        judgments = copy.deepcopy(self.doc["judgments"])
        code, out = self.update("--source", "s.new", "--at", "line 2", "--why", "user correction")
        self.assertEqual(code, 0, out)
        b = self.read()["known"]["pay.deposited"]
        self.assertEqual((b["v"], b["from"], b["at"], b["of"]),
                         (False, "s.new", "line 2", "2026-09-07"))
        self.assertEqual(self.read()["judgments"], judgments)
        self.assertIn("FIRED", out)
        self.assertIn("s.new", self.cli("pull", "pay.deposited")[1])

    def test_citation_only_update_is_not_a_value_noop(self):
        code, out = self.cli("set", "pay.deposited", "true", "--source", "s.new", "--at", "line 2")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.read()["known"]["pay.deposited"]["from"], "s.new")

    def test_source_and_location_are_required_together(self):
        before = self.rec.read_bytes()
        for args in (("--source", "s.new"), ("--at", "line 2"),
                     ("--source", "", "--at", "line 2"),
                     ("--source", "s.new", "--at", " ")):
            code, out = self.update(*args)
            self.assertNotEqual(code, 0, out)
            self.assertEqual(self.rec.read_bytes(), before)

    def test_invalid_source_never_changes_the_value(self):
        before = self.rec.read_bytes()
        for source in ("s.missing", "pay.deposited", "pay.other", "c.complete", "graph.flagged"):
            code, out = self.update("--source", source, "--at", "line 2")
            self.assertNotEqual(code, 0, (source, out))
            self.assertEqual(self.rec.read_bytes(), before)

    def test_flow_mapping_keeps_other_fields_and_quotes_literal_locations(self):
        body = self.doc["known"]["pay.deposited"]
        body["name"] = 'A label with from: s.fake and at: elsewhere'
        body["metadata"] = {"from": "leave this nested value alone"}
        self.save(self.doc)
        text = self.rec.read_text()
        start = text.index("  pay.deposited:")
        end = text.index("  pay.other:", start)
        flow = yaml.safe_dump(body, default_flow_style=True, sort_keys=False, width=1000).strip()
        self.rec.write_text(text[:start] + "  pay.deposited: " + flow + " # keep\n" + text[end:])
        code, out = self.update("--source", "s.new", "--at", 'line 2: "false", [3]')
        self.assertEqual(code, 0, out)
        b = self.read()["known"]["pay.deposited"]
        self.assertEqual(b["from"], "s.new")
        self.assertEqual(b["at"], 'line 2: "false", [3]')
        self.assertEqual(b["metadata"], body["metadata"])
        self.assertEqual(b["name"], body["name"])
        self.assertIn("# keep", self.rec.read_text())

    def test_source_update_follows_pointer_without_editing_source_file(self):
        pointer = self.root / "entry.yaml"
        pointer.write_text("record: PROVENANCE.yaml\n")
        before = (self.root / "new.txt").read_bytes()
        code, out = self.cli("set", "pay.deposited", "false", "--source", "s.new", "--at", "line 2",
                             "--as-of", "2026-09-07", pointer)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.read()["known"]["pay.deposited"]["from"], "s.new")
        self.assertEqual(pointer.read_text(), "record: PROVENANCE.yaml\n")
        self.assertEqual((self.root / "new.txt").read_bytes(), before)

    def test_missing_citation_fields_are_inserted(self):
        for flow in (False, True):
            with self.subTest(flow=flow):
                body = copy.deepcopy(self.doc)
                del body["known"]["pay.deposited"]["from"]
                del body["known"]["pay.deposited"]["at"]
                self.rec.write_text(yaml.safe_dump(body, sort_keys=False, default_flow_style=flow))
                # Keep the record's collections in block style; only the entry may be flow.
                if flow:
                    self.save(body)
                    text = self.rec.read_text()
                    start, end = text.index("  pay.deposited:"), text.index("  pay.other:")
                    entry = yaml.safe_dump(body["known"]["pay.deposited"], default_flow_style=True, width=1000).strip()
                    self.rec.write_text(text[:start] + "  pay.deposited: " + entry + "\n" + text[end:])
                code, out = self.update("--source", "s.new", "--at", "line 2")
                self.assertEqual(code, 0, out)
                self.assertEqual(self.read()["known"]["pay.deposited"]["from"], "s.new")
                self.assertEqual(self.read()["known"]["pay.deposited"]["at"], "line 2")

    def test_citation_alias_and_multiline_location_are_replaced_locally(self):
        text = self.rec.read_text().replace("name: Old report", "name: Old report\n    alias: &source s.old")
        text = text.replace("from: s.old", "from: *source", 1)
        self.rec.write_text(text)
        code, out = self.update("--source", "s.new", "--at", "line 2\nsecond clause")
        self.assertEqual(code, 0, out)
        b = self.read()["known"]["pay.deposited"]
        self.assertEqual(b["from"], "s.new")
        self.assertEqual(b["at"], "line 2\nsecond clause")
        self.assertEqual(self.read()["sources"]["s.old"]["alias"], "s.old")

    def test_hypothesis_keeps_the_new_source_and_leaves_base_untouched(self):
        before = self.rec.read_bytes()
        code, out = self.cli("set", "pay.deposited", "false", "--as-of", "2026-09-06",
                             "--source", "s.new", "--at", "line 2")
        self.assertNotEqual(code, 0)
        self.assertIn("--source s.new", out)
        self.assertIn("--at", out)
        code, out = self.cli("set", "pay.deposited", "false", "--as-of", "2026-09-06",
                             "--source", "s.new", "--at", "line 2", "--hypothesis", "correction")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.rec.read_bytes(), before)
        hyp = yaml.safe_load((self.root / "PROVENANCE.d" / "correction.yaml").read_text())
        self.assertEqual(hyp["known"]["pay.deposited"]["from"], "s.new")
        code, out = self.cli("set", "pay.deposited", "false", "--source", "s.old", "--at", "line 3",
                             "--hypothesis", "correction")
        self.assertEqual(code, 0, out)
        hyp = yaml.safe_load((self.root / "PROVENANCE.d" / "correction.yaml").read_text())
        self.assertEqual(hyp["known"]["pay.deposited"]["at"], "line 3")

    def test_citation_options_are_not_ignored_on_add_or_review(self):
        before = self.rec.read_bytes()
        for args in (("review", "c.complete"), ("add", "pay.extra", "v=true")):
            code, out = self.cli(*args, "--source", "s.new", "--at", "line 2")
            self.assertNotEqual(code, 0, out)
            self.assertEqual(self.rec.read_bytes(), before)

    def test_gate_allows_unchanged_judgment_fired_by_new_evidence(self):
        self.assertEqual(self.cli("mark", self.mark)[0], 0)
        self.assertEqual(self.update()[0], 0)
        code, out = self.cli("gate", self.mark)
        self.assertEqual(code, 0, out)
        self.assertIn("c.complete", out)
        self.assertIn("review", out)
        self.assertEqual(self.cli("check")[0], 1)
        self.assertIn("broken:", self.cli("pull", "c.complete")[1])
        self.assertEqual(self.read()["judgments"], self.doc["judgments"])

    def test_gate_does_not_excuse_new_or_edited_falsified_judgments(self):
        for variant in ("edited", "new", "reviewed"):
            with self.subTest(variant=variant):
                self.save(self.doc)
                self.cli("mark", self.mark)
                self.update()
                now = self.read()
                if variant == "edited":
                    now["judgments"]["c.complete"]["verdict"] = "New unsupported verdict"
                elif variant == "new":
                    now["judgments"]["c.fresh"] = copy.deepcopy(now["judgments"]["c.complete"])
                else:
                    now["judgments"]["c.complete"]["seen"]["pay.deposited"] = False
                self.save(now)
                self.assertEqual(self.cli("gate", self.mark)[0], 2)

    def test_mark_without_recorded_inputs_cannot_excuse_falsification(self):
        self.cli('mark', self.mark)
        marked = json.loads(self.mark.read_text())
        del marked['judgments']['c.complete']['inputs']
        self.mark.write_text(json.dumps(marked))
        self.update()
        self.assertEqual(self.cli('gate', self.mark)[0], 2)

    def test_gate_never_excuses_a_structural_error_alongside_a_fired_judgment(self):
        self.cli("mark", self.mark)
        self.update()
        now = self.read()
        now["judgments"]["c.other"]["seen"] = {}
        self.save(now)
        code, out = self.cli("gate", self.mark)
        self.assertEqual(code, 2, out)
        self.assertIn("no snapshot", out)

    def test_a_changed_predicate_literal_is_not_an_unchanged_judgment(self):
        self.doc["known"]["pay.deposited"]["v"] = "pending"
        j = self.doc["judgments"]["c.complete"]
        j["seen"]["pay.deposited"] = "pending"
        j["wrong_if"] = "pay.deposited == 'not  paid'"
        self.save(self.doc)
        self.cli("mark", self.mark)
        now = self.read()
        now["known"]["pay.deposited"]["v"] = "not paid"
        now["judgments"]["c.complete"]["wrong_if"] = "pay.deposited == 'not paid'"
        self.save(now)
        self.assertEqual(self.cli("gate", self.mark)[0], 2)

    def test_equal_failure_counts_do_not_hide_a_new_failure(self):
        self.doc["judgments"]["c.other"]["seen"] = {}
        self.save(self.doc)
        self.cli("mark", self.mark)
        now = self.read()
        now["judgments"]["c.other"]["seen"] = {"pay.other": True}
        now["judgments"]["c.complete"]["seen"] = {}
        self.save(now)
        code, out = self.cli("gate", self.mark)
        self.assertEqual(code, 2, out)
        self.assertIn("c.complete: no snapshot", out)

    def test_legacy_marks_still_use_the_conservative_gate(self):
        self.mark.write_text("0\n")
        self.update()
        self.assertEqual(self.cli("gate", self.mark)[0], 2)

    def test_stop_hook_accepts_a_retained_falsified_judgment(self):
        env = dict(os.environ, TMPDIR=str(self.root))
        def hook(name):
            return subprocess.run(["sh", str(SCRIPTS / name)], cwd=self.root,
                                  input=json.dumps({"session_id": "ingestion"}),
                                  capture_output=True, text=True, env=env)
        self.assertEqual(hook("session_open.sh").returncode, 0)
        self.update()
        p = hook("session_gate.sh")
        self.assertEqual(p.returncode, 0, p.stderr)


if __name__ == "__main__":
    unittest.main()
