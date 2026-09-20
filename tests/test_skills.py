"""Offline contracts for skills, contributor documentation and GitHub issue forms.

This module runs in the mandatory CI job, including documentation-only changes.
"""
from html import unescape
from html.parser import HTMLParser
import pathlib
import re
import tempfile
import unicodedata
import unittest
from urllib.parse import unquote, urlsplit

import yaml

ROOT = pathlib.Path(__file__).resolve().parent.parent
SKILLS = ROOT / "skills"
# Canonical occasions and thin compatibility aliases, with a body-size ceiling for each.
OCCASIONS = {"kpopper": 140, "ground": 162, "record": 262, "map": 110, "annotated-doc": 70, "hub": 60, "document": 12, "page": 12,
             "consolidate": 160, "watch": 110}
DESCRIPTION_CHARS = 1536     # the host truncates the listing's description past this
COMMUNITY_DOCS = ("README.md", "CONTRIBUTING.md", "SECURITY.md", "CODE_OF_CONDUCT.md")


class HtmlReferences(HTMLParser):
    def __init__(self, text):
        super().__init__()
        self.links, self.anchors = [], set()
        self.feed(text)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        self.links.extend(attrs[key] for key in ("href", "src") if attrs.get(key))
        for key in (("id", "name") if tag == "a" else ("id",)):
            if attrs.get(key):
                self.anchors.add(attrs[key])


def without_fences(text):
    lines, fence = [], None
    for line in text.splitlines():
        match = re.match(r"^\s{0,3}(`{3,}|~{3,})(.*)$", line)
        if fence:
            if match and match[1][0] == fence[0] and len(match[1]) >= len(fence) and not match[2].strip():
                fence = None
        elif match:
            fence = match[1]
        else:
            lines.append(line)
    return "\n".join(lines)


def markdown_anchors(text):
    text = without_fences(text)
    anchors = HtmlReferences(text).anchors
    slugs = set()
    for heading in re.findall(r"^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$", text, re.M):
        heading = re.sub(r"\[([^]]+)\]\([^)]*\)", r"\1", heading)
        heading = unescape(re.sub(r"<[^>]+>", "", heading)).lower()
        base = "".join(c for c in heading if c in " -_" or unicodedata.category(c)[0] in "LNM").replace(" ", "-")
        slug, suffix = base, 0
        while slug in slugs:
            suffix += 1
            slug = "%s-%s" % (base, suffix)
        slugs.add(slug)
    return anchors | slugs


def local_link_errors(text, source, root):
    """Check local and this repo's main-branch links without network access."""
    text = re.sub(r"<!--.*?-->", "", without_fences(text), flags=re.S)
    links = HtmlReferences(text).links
    links += [m[0] or m[1] for m in re.findall(r"\]\((?:<([^>]+)>|([^\s)]+))(?:\s+[^)]*)?\)", text)]
    errors = []
    for link in links:
        url = urlsplit(unescape(link))
        target_path = unquote(url.path)
        if url.netloc == "github.com" and target_path.startswith(("/ilanbm/kpopper/blob/main/", "/ilanbm/kpopper/tree/main/")):
            target = root / target_path.split("/main/", 1)[1]
        elif url.scheme or url.netloc:
            continue
        else:
            target = source.parent / target_path if target_path else source
        if not target.exists():
            errors.append("missing file: " + link)
        elif url.fragment and target.suffix == ".md":
            if unquote(url.fragment) not in markdown_anchors(target.read_text(encoding="utf-8")):
                errors.append("missing anchor: " + link)
    return errors


def issue_form_errors(form):
    """Check the structural contract used by this project's text-based forms."""
    if not isinstance(form, dict):
        return ["form must be a mapping"]
    errors = []
    for key in ("name", "description"):
        if not isinstance(form.get(key), str) or not form[key].strip():
            errors.append("missing " + key)
    if not isinstance(form.get("body"), list) or not form["body"]:
        return errors + ["body must be a nonempty list"]
    ids, labels, inputs = set(), set(), 0
    for field in form["body"]:
        if not isinstance(field, dict) or field.get("type") not in {"markdown", "input", "textarea"}:
            errors.append("unsupported field: extend validation before adopting another input type")
            continue
        attrs = field.get("attributes")
        if not isinstance(attrs, dict):
            errors.append("attributes must be a mapping")
            continue
        if field["type"] == "markdown":
            if not isinstance(attrs.get("value"), str) or not attrs["value"].strip():
                errors.append("markdown must have text")
            continue
        inputs += 1
        if "id" in field:
            field_id = field["id"]
            if not isinstance(field_id, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", field_id):
                errors.append("invalid field id")
            elif field_id in ids:
                errors.append("duplicate id: " + field_id)
            else:
                ids.add(field_id)
        label = attrs.get("label")
        if not isinstance(label, str) or not label.strip():
            errors.append("interactive field needs a label")
        elif label in labels:
            errors.append("duplicate label: " + label)
        else:
            labels.add(label)
        validation = field.get("validations", {})
        if not isinstance(validation, dict) or type(validation.get("required", False)) is not bool:
            errors.append("required must be a boolean")
    if not inputs:
        errors.append("form needs an interactive field")
    return errors


def frontmatter(text):
    m = re.match(r"---\n(.*?)\n---\n", text, re.S)
    fields = {}
    for line in (m.group(1) if m else "").splitlines():
        k, _, v = line.partition(":")
        fields[k.strip()] = v.strip().strip('"')
    return fields


class Skills(unittest.TestCase):
    def test_canonical_skills_and_aliases_match_their_directories(self):
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


class Community(unittest.TestCase):
    def test_root_document_links_and_heading_anchors_resolve(self):
        for name in COMMUNITY_DOCS:
            path = ROOT / name
            with self.subTest(file=name):
                self.assertEqual(local_link_errors(path.read_text(encoding="utf-8"), path, ROOT), [])

    def test_issue_forms_and_their_documentation_links(self):
        directory = ROOT / ".github/ISSUE_TEMPLATE"
        forms = [p for p in directory.iterdir() if p.suffix in {".yml", ".yaml"} and p.stem != "config"]
        self.assertTrue(forms, "the issue chooser needs forms")
        names = set()
        for path in forms:
            with self.subTest(form=path.name):
                text = path.read_text(encoding="utf-8")
                form = yaml.safe_load(text)
                self.assertEqual(issue_form_errors(form), [])
                self.assertNotIn(form["name"], names, "chooser names must be distinct")
                names.add(form["name"])
                self.assertEqual(local_link_errors(text, path, ROOT), [])

    def test_issue_chooser_contacts_are_complete_and_resolve(self):
        config = yaml.safe_load((ROOT / ".github/ISSUE_TEMPLATE/config.yml").read_text(encoding="utf-8"))
        self.assertIsInstance(config.get("blank_issues_enabled"), bool)
        self.assertIsInstance(config.get("contact_links"), list)
        for link in config["contact_links"]:
            for field in ("name", "url", "about"):
                self.assertIsInstance(link.get(field), str)
                self.assertTrue(link[field].strip())
            self.assertEqual(urlsplit(link["url"]).scheme, "https")
            self.assertTrue(urlsplit(link["url"]).netloc)
            self.assertEqual(local_link_errors("[contact](%s)" % link["url"], ROOT / "README.md", ROOT), [])

    def test_guard_catches_broken_links_and_renamed_headings(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            target = root / "Guide name.md"
            target.write_text('# Install `package`\n\n## Install package\n\n<a id="compat"></a>\n', encoding="utf-8")
            source = root / "README.md"
            good = '[guide](Guide%20name.md#install-package) [again](Guide%20name.md#install-package-1) <a href="Guide%20name.md#compat">old link</a>'
            self.assertEqual(local_link_errors(good, source, root), [])
            self.assertEqual(len(local_link_errors('[guide](Guide%20name.md#install)', source, root)), 1)
            self.assertEqual(len(local_link_errors('<img src="missing.png">', source, root)), 1)
            target.write_text('# Setup\n', encoding="utf-8")
            self.assertEqual(len(local_link_errors(good, source, root)), 3)
            self.assertEqual(local_link_errors('```md\n[example](missing.md)\n```', source, root), [])

    def test_guard_rejects_forms_github_cannot_use(self):
        valid = {'name': 'Report', 'description': 'A problem', 'body': [
            {'type': 'textarea', 'id': 'details', 'attributes': {'label': 'Details'}}]}
        self.assertEqual(issue_form_errors(valid), [])
        invalid = [dict(valid, body=[]), dict(valid, body=valid['body'] * 2),
                   dict(valid, body=[dict(valid['body'][0], id='invalid id')]),
                   dict(valid, body=[dict(valid['body'][0], validations={'required': 'true'})]),
                   dict(valid, body=[{'type': 'markdown', 'attributes': {'value': 'No inputs'}}])]
        for form in invalid:
            with self.subTest(form=form):
                self.assertTrue(issue_form_errors(form))


if __name__ == "__main__":
    unittest.main()
