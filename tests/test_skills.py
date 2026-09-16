"""The skills a session is offered: one per occasion, each small enough to load whole, each
described within the host's listing limit, every link between them resolving."""
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SKILLS = ROOT / "skills"
# the occasions, and the ceiling each body keeps so an invocation costs the occasion and not the method
OCCASIONS = {"kpopper": 140, "ground": 162, "record": 262, "map": 110, "document": 70, "page": 60,
             "consolidate": 160, "watch": 110}
DESCRIPTION_CHARS = 1536     # the host truncates the listing's description past this


def frontmatter(text):
    m = re.match(r"---\n(.*?)\n---\n", text, re.S)
    fields = {}
    for line in (m.group(1) if m else "").splitlines():
        k, _, v = line.partition(":")
        fields[k.strip()] = v.strip().strip('"')
    return fields


class Skills(unittest.TestCase):
    def test_one_skill_per_occasion_each_named_for_its_directory(self):
        found = {p.parent.name for p in SKILLS.glob("*/SKILL.md")}
        self.assertEqual(found, set(OCCASIONS))
        for name in OCCASIONS:
            fields = frontmatter((SKILLS / name / "SKILL.md").read_text(encoding="utf-8"))
            self.assertEqual(fields.get("name"), name)

    def test_every_description_says_when_and_fits_the_listing(self):
        for name in OCCASIONS:
            desc = frontmatter((SKILLS / name / "SKILL.md").read_text(encoding="utf-8")).get("description", "")
            with self.subTest(skill=name):
                self.assertLessEqual(len(desc), DESCRIPTION_CHARS)
                self.assertRegex(desc, r"\bUse (when|whenever|before|the moment)\b",
                                 "a description names the occasion, not only the subject")

    def test_each_body_stays_within_its_ceiling(self):
        for name, ceiling in OCCASIONS.items():
            text = (SKILLS / name / "SKILL.md").read_text(encoding="utf-8")
            with self.subTest(skill=name):
                self.assertLessEqual(text.count("\n"), ceiling)

    def test_every_relative_link_between_skill_files_resolves(self):
        for path in list(SKILLS.rglob("*.md")) + [ROOT / "docs" / "reference.md"]:
            text = path.read_text(encoding="utf-8")
            for target in re.findall(r"\]\(([^)#\s]+)(?:#[^)]*)?\)", text):
                if re.match(r"[a-z]+://", target):
                    continue
                with self.subTest(file=str(path.relative_to(ROOT)), link=target):
                    self.assertTrue((path.parent / target).exists(), target)

    def test_the_umbrella_names_every_other_skill(self):
        text = (SKILLS / "kpopper" / "SKILL.md").read_text(encoding="utf-8")
        for name in OCCASIONS:
            if name != "kpopper":
                self.assertIn("../%s/SKILL.md" % name, text)
        for ref in ("references/method.md", "references/shape.md", "references/falsifiers.md"):
            self.assertIn(ref, text)


if __name__ == "__main__":
    unittest.main()
