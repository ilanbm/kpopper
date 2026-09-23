"""A record before its first judgment reads its sections by what their entries are, as it does
once a judgment exists: a section of values under a name of the record's own reads as `known:`
does, and a misspelled dependency declaration is still refused by its shape, whatever the
section is called. Runs on throwaway records with no browser and no network:

    python3 -m unittest discover -s tests
"""
import pathlib
import subprocess
import sys
import tempfile
import unittest

import yaml

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"

sys.path.insert(0, str(SCRIPTS))
import provenance as P  # noqa: E402

SOURCES = {"pricing": {"name": "Acme's pricing page", "url": "https://example.test/pricing",
                       "read": "2026-09-20"}}
FACTS = {"acme.seat_price": {"v": 42, "from": "pricing"},
         "acme.seats": {"v": 120, "from": "pricing"}}
FIRST_JUDGMENT = ["why_acme", "rests_on=[acme.seats]", "verdict=prefer Acme",
                  "because=cheaper above 100 seats", "wrong_if=acme.seats < 100"]
NO_GRAPH = "no dependency field found"


def young(**sections):
    return {"meta": {"updated": "2026-09-22"}, "sources": dict(SOURCES), **sections}


def run(*args, cwd):
    p = subprocess.run([sys.executable, str(SCRIPTS / "provenance.py"), *map(str, args)],
                       cwd=cwd, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


class YoungRecordSections(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = pathlib.Path(tmp.name)
        self.record = self.root / "GROUNDING.yaml"

    def write(self, doc):
        self.record.write_text(yaml.safe_dump(doc, sort_keys=False), encoding="utf-8")

    def test_a_section_of_values_reads_under_any_name(self):
        known = P.infer(young(known=dict(FACTS)))
        for name in ("facts", "parameters", "readings"):
            with self.subTest(section=name):
                self.assertEqual(P.infer(young(**{name: dict(FACTS)})), known)
                self.write(young(**{name: dict(FACTS)}))
                code, out, err = run("--frozen", "check", cwd=self.root)
                self.assertEqual(code, 0, err)
                self.assertIn("0 judgments, 3 entries, 0 problems", out)

    def test_every_shape_a_value_or_a_source_takes_reads(self):
        for section in ({"acme.annual": {"rule": "acme.seat_price * acme.seats * 12"},
                         "contract.exit": {"quoted": "Either party may end it on 90 days' notice.",
                                           "from": "pricing"}},
                        {"acme.discount": 0.1},
                        {"memo": {"name": "the planning memo", "file": "memo.md", "read": "2026-09-19"},
                         "acme.term": {"v": 12, "from": "memo"}}):
            with self.subTest(section=section):
                ids, judgments, fields = P.infer(young(facts=dict(FACTS, **section)))
                self.assertEqual(judgments, {})
                self.assertLessEqual(set(section), ids)
                self.assertEqual(fields["deps"], "rests_on")

    def test_the_first_judgment_lands_through_add(self):
        self.write(young(facts=dict(FACTS)))
        code, out, err = run("add", *FIRST_JUDGMENT, "--as-of", "2026-09-22", self.record,
                             cwd=self.root)
        self.assertEqual(code, 0, out + err)
        saved = yaml.safe_load(self.record.read_text(encoding="utf-8"))
        self.assertEqual(saved["facts"], FACTS)
        judgment = saved["judgments"]["why_acme"]
        self.assertEqual(judgment["rests_on"], ["acme.seats"])
        self.assertEqual(judgment["seen"], {"acme.seats": 120})
        code, out, err = run("--frozen", "check", cwd=self.root)
        self.assertEqual(code, 0, err)
        self.assertIn("1 judgments, 4 entries, 0 problems", out)
        # once a judgment exists, a new value still goes where the values are
        code, out, err = run("add", "acme.discount", "v=0.1", "from=pricing", "--as-of",
                             "2026-09-22", self.record, cwd=self.root)
        self.assertEqual(code, 0, out + err)
        saved = yaml.safe_load(self.record.read_text(encoding="utf-8"))
        self.assertEqual(saved["facts"]["acme.discount"]["v"], 0.1)

    def test_a_role_the_schema_names_is_the_one_the_first_judgment_takes(self):
        self.write(young(schema={"snapshot": "reviewed"}, facts=dict(FACTS)))
        code, out, err = run("add", *FIRST_JUDGMENT, "--as-of", "2026-09-22", self.record,
                             cwd=self.root)
        self.assertEqual(code, 0, out + err)
        judgment = yaml.safe_load(self.record.read_text(encoding="utf-8"))["judgments"]["why_acme"]
        self.assertEqual(judgment["reviewed"], {"acme.seats": 120})
        self.assertNotIn("seen", judgment)
        code, out, err = run("--frozen", "check", cwd=self.root)
        self.assertEqual(code, 0, out + err)
        self.assertIn("1 judgments, 4 entries, 0 problems", out)

    def test_a_value_added_by_position_keeps_the_record_readable(self):
        # `add <id> <value>` writes the bare value into the section its prefix already holds
        self.write(young(facts=dict(FACTS)))
        code, out, err = run("add", "acme.term", "12", "--as-of", "2026-09-22", self.record,
                             cwd=self.root)
        self.assertEqual(code, 0, out + err)
        saved = yaml.safe_load(self.record.read_text(encoding="utf-8"))
        self.assertEqual(saved["facts"]["acme.term"], "12")
        code, out, err = run("--frozen", "check", cwd=self.root)
        self.assertEqual(code, 0, err)
        self.assertIn("0 judgments, 4 entries, 0 problems", out)

    def test_a_misspelled_dependency_declaration_is_still_refused(self):
        for claim in (
            # the dependency field misspelled, the judgment's other fields still its own
            {"restson": "acme.seats", "verdict": "prefer Acme", "wrong_if": "acme.seats < 100"},
            # a dependency list naming what is not an entry, under either name
            {"rests_on": ["acme.seat"], "verdict": "prefer Acme"},
            {"depends_on": ["acme.seat"], "conclusion": "prefer Acme"},
            # a judgment in names of the record's own, holding no value
            {"depends": "acme.seats", "conclusion": "prefer Acme",
             "falsified_when": "acme.seats < 100"},
        ):
            with self.subTest(claim=claim):
                doc = young(facts=dict(FACTS), claims={"why_acme": claim})
                with self.assertRaisesRegex(SystemExit, NO_GRAPH):
                    P.infer(doc)
                self.write(doc)
                before = self.record.read_bytes()
                code, _, err = run("add", "acme.discount", "v=0.1", "--as-of", "2026-09-22",
                                   self.record, cwd=self.root)
                self.assertEqual(code, 1)
                self.assertIn(NO_GRAPH, err)
                self.assertEqual(self.record.read_bytes(), before)
        with self.assertRaisesRegex(SystemExit, "schema names 'restson'"):
            P.infer(young(schema={"deps": "restson"}, facts=dict(FACTS)))


if __name__ == "__main__":
    unittest.main()
