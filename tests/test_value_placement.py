"""Where `add` files a new value whose prefix no section holds yet: where the values are. A
record that writes its values bare keeps them together, but a bare scalar counts as a value
only when no section holds one written out, and never in the questions or the sources. Runs on
throwaway records with no browser and no network:

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

CLAIMS = ('claims:\n  why_acme: {rests_on: [acme.seats], verdict: "prefer Acme", '
          'wrong_if: "acme.seats < 50", seen: {acme.seats: 120}}\n')


def run(*args, cwd):
    p = subprocess.run([sys.executable, str(SCRIPTS / "provenance.py"), *map(str, args)],
                       cwd=cwd, capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


class ValuePlacement(unittest.TestCase):
    def home_of(self, record, *add):
        with tempfile.TemporaryDirectory() as d:
            path = pathlib.Path(d) / "GROUNDING.yaml"
            path.write_text(record, encoding="utf-8")
            code, out, err = run("add", *add, "--as-of", "2026-09-22", path, cwd=d)
            self.assertEqual(code, 0, out + err)
            saved = yaml.safe_load(path.read_text(encoding="utf-8"))
            return [section for section, members in saved.items()
                    if isinstance(members, dict) and add[0] in members]

    def test_a_value_joins_the_section_of_bare_values(self):
        record = "facts:\n  acme.seats: 120\n" + CLAIMS
        self.assertEqual(self.home_of(record, "beta.price", "v=3"), ["facts"])

    def test_bare_scalars_do_not_count_where_a_section_holds_written_values(self):
        record = ('notes:\n  n.call: "call Dana"\n  n.lease: "check the lease"\n'
                  '  n.term: "ask about the term"\nfacts:\n  acme.seats: {v: 120}\n' + CLAIMS)
        self.assertEqual(self.home_of(record, "beta.price", "v=3"), ["facts"])

    def test_questions_and_sources_are_never_a_home_for_values(self):
        for section, nid, text in (("open", "q.budget", "is the budget fixed?"),
                                   ("sources", "pricing", "https://example.test/pricing")):
            record = (f'{section}:\n  {nid}: "{text}"\nclaims:\n  why_acme: {{rests_on: [{nid}], '
                      f'verdict: "prefer Acme", seen: {{{nid}: "{text}"}}}}\n')
            with self.subTest(section=section):
                self.assertEqual(self.home_of(record, "beta.price", "v=3"), ["known"])

    def test_a_bare_value_with_a_new_prefix_is_still_a_question(self):
        record = "facts:\n  acme.seats: 120\n" + CLAIMS
        self.assertEqual(self.home_of(record, "beta.price", "3"), ["open"])

    def test_a_tie_between_sections_of_bare_values_goes_to_the_first_by_name(self):
        record = ('zeta:\n  z.a: 1\nalpha:\n  a.b: 2\nclaims:\n  why: {rests_on: [z.a], '
                  'verdict: keep, wrong_if: "z.a < 0", seen: {z.a: 1}}\n')
        self.assertEqual(self.home_of(record, "beta.price", "v=3"), ["alpha"])


if __name__ == "__main__":
    unittest.main()
