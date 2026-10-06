"""Offline contracts for skills, contributor documentation and GitHub issue forms.

This module runs in the mandatory CI job, including documentation-only changes.
"""
from html import unescape
from html.parser import HTMLParser
import hashlib
import json
import pathlib
import re
import shlex
import subprocess
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

    def test_living_knowledge_model_guide_links_resolve(self):
        path = ROOT / "docs" / "living-knowledge-models.md"
        self.assertEqual(local_link_errors(path.read_text(encoding="utf-8"), path, ROOT), [])

    def test_living_travel_rights_example_matches_its_manifest(self):
        directory = ROOT / "examples" / "living-travel-rights"
        manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
        excerpt_path = directory / "model-excerpt.yaml"
        results_path = directory / "results.json"
        case_path = directory / "case.json"
        self.assertEqual(hashlib.sha256(excerpt_path.read_bytes()).hexdigest(),
                         manifest["files"]["model-excerpt.yaml"])
        self.assertEqual(hashlib.sha256(results_path.read_bytes()).hexdigest(),
                         manifest["files"]["results.json"])
        self.assertEqual(hashlib.sha256(case_path.read_bytes()).hexdigest(),
                         manifest["files"]["case.json"])
        self.assertEqual(manifest["files"]["case.json"],
                         manifest["source"]["files"]["examples/domestic-below.json"])
        self.assertEqual(manifest["source"]["commit"], "8cc12d243bef52bb2442abe5915238c2ba97adb5")
        self.assertEqual(manifest["source"]["model_path"],
                         "knowledge/us-flight-refunds/GROUNDING.yaml")
        self.assertEqual(manifest["source"]["files"][manifest["source"]["model_path"]],
                         manifest["source"]["model_sha256"])
        self.assertNotIn("GROUNDING.yaml", manifest["source"]["files"])
        self.assertFalse(any(name.startswith((".kpopper/", "evidence/"))
                             for name in manifest["source"]["files"]))
        self.assertEqual(manifest["source"]["model_sha256"],
                         "55a95eae5c79de98e4c02e26a8332f21de4838a3dfa197cb548dedbc6bb4e500")
        excerpt = yaml.safe_load(excerpt_path.read_text(encoding="utf-8"))
        self.assertEqual(excerpt["kind"], "read-only-model-excerpt/v1")
        self.assertEqual(excerpt["source_commit"], manifest["source"]["commit"])
        self.assertEqual(excerpt["source_model_sha256"], manifest["source"]["model_sha256"])
        results = json.loads(results_path.read_text(encoding="utf-8"))
        self.assertEqual(excerpt["parameters"]["policy.domestic_arrival_minutes"]["v"],
                         results["hypothetical"]["policy_overrides"][0]["baseline"]["v"])
        case = json.loads(case_path.read_text(encoding="utf-8"))
        self.assertEqual(case, {
            "jurisdiction": "US",
            "covered_scheduled_service": True,
            "direct_airline_purchase": True,
            "airline_merchant_of_record": True,
            "entirely_unused": True,
            "nonrefundable_ticket": True,
            "flight_cancelled": False,
            "renumbering_only": False,
            "itinerary_kind": "domestic",
            "scheduled_arrival_shift_minutes": 179,
            "choice": "declined_all",
            "carrier_changed_itinerary": True,
        })
        self.assertEqual(manifest["case_sha256"], results["baseline"]["identity"]["input_digest"])
        self.assertEqual(results["baseline"]["identity"]["input_digest"],
                         results["hypothetical"]["identity"]["input_digest"])
        for result in results.values():
            self.assertIs(result["other_rights_not_ruled_out"], True)
            self.assertTrue(result["coverage_notice"])
            self.assertEqual(result["identity"]["model_commit"], manifest["source"]["commit"])
            self.assertEqual(result["identity"]["model_digest"], manifest["source"]["model_sha256"])
            self.assertEqual(result["identity"]["runtime_version"], "kpop 0.15.1")
            self.assertEqual(result["identity"]["adapter_digest"],
                             manifest["source"]["files"]["scripts/assess_case.py"])
            self.assertEqual(result["identity"]["input_digest"], manifest["case_sha256"])
            self.assertEqual(result["components"]["cancellation"]["evaluation"], "fail")
            self.assertIn("not a denial of the right",
                          result["components"]["cancellation"]["scope_reason"])
        hypo = results["hypothetical"]["policy_overrides"]
        self.assertEqual(hypo[0]["id"], "policy.domestic_arrival_minutes")
        self.assertEqual(hypo[0]["hypothetical_value"],
                         manifest["hypothetical_overrides"]["policy.domestic_arrival_minutes"])
        self.assertEqual(hypo[0]["baseline_value"],
                         excerpt["parameters"]["policy.domestic_arrival_minutes"]["v"])
        self.assertEqual(hypo[0]["baseline"]["v"], hypo[0]["baseline_value"])
        self.assertEqual(hypo[0]["baseline_source_id"],
                         excerpt["parameters"]["policy.domestic_arrival_minutes"]["from"])
        self.assertEqual(hypo[0]["baseline_locator"],
                         excerpt["parameters"]["policy.domestic_arrival_minutes"]["at"])
        self.assertEqual({item["id"]: item["hypothetical_value"] for item in hypo},
                         manifest["hypothetical_overrides"])
        self.assertEqual(results["baseline"]["components"]["arrival_change"]["evaluation"], "fail")
        self.assertEqual(results["hypothetical"]["components"]["arrival_change"]["evaluation"], "pass")
        self.assertEqual(results["baseline"]["components"]["cancellation"]["evaluation"], "fail")
        self.assertEqual(results["hypothetical"]["components"]["cancellation"]["evaluation"], "fail")
        readme = directory / "README.md"
        readme_raw = readme.read_text(encoding="utf-8")
        readme_text = " ".join(readme_raw.split())
        self.assertEqual(local_link_errors(readme_raw, readme, ROOT), [])
        referenced_sources = {source for result in results.values()
                              for component in result["components"].values()
                              for source in component["sources"]}
        for source in referenced_sources:
            with self.subTest(source=source):
                self.assertIn(f"`{source}`", readme_text)
        self.assertIn("flight_cancelled: false", readme_text)
        self.assertIn("renumbering_only: false", readme_text)
        self.assertIn("not an active exclusion in this case", readme_text)
        self.assertIn("Both baseline components fail", readme_text)
        self.assertIn("arrival-change passes", readme_text)
        for label, result in results.items():
            self.assertIn(result["context"]["source_as_of"], readme_text)

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


# Every `kpop ...` (and `kpopper ...`) invocation cited in code, checked against the command
# surface native/tests/cli_surface.rs renders from `kpop --help`.
CLI_SURFACE = ROOT / "native" / "tests" / "fixtures" / "cli-surface.txt"
REFERENCE_DOCS = ("README.md", "CONTRIBUTING.md", "native/README.md")
REFERENCE_TREES = ("skills", "docs", "adapters")
REFERENCE_SUFFIXES = (".md", ".mdc", ".snippet")
INVOCATION = re.compile(r"(?:^|(?<=[\s(`'\"$|&;/]))(kpop|kpopper)(?=\s|$)")
# Compatibility aliases kpop accepts but does not list in its help.
HIDDEN_ALIASES = {("experimental", "page"): ("experimental", "hub"),
                  ("experimental", "document"): ("experimental", "annotated-doc")}


def cli_surface(text):
    """Map each command path (a tuple of words) to its long options; `=` marks a value."""
    surface = {}
    for line in text.splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        words = line.split()[1:]
        path = tuple(w for w in words if not w.startswith("--"))
        surface[path] = {w.rstrip("="): w.endswith("=") for w in words if w.startswith("--")}
    return surface


def code_segments(text):
    """(line number, code) for every fenced command and every inline code span.

    A fenced line ending in a backslash continues on the next; a span may wrap within its
    paragraph. Each is numbered by the line it starts on.
    """
    fence, command, paragraph = None, None, []
    for number, line in enumerate(text.splitlines() + [""], 1):
        match = re.match(r"^\s*(`{3,}|~{3,})(.*)$", line)
        if fence and match and match[1][0] == fence[0] and len(match[1]) >= len(fence) and not match[2].strip():
            fence = None
            if command:
                yield command
            command = None
        elif fence:
            command = (command[0], command[1] + " " + line) if command else (number, line)
            if command[1].rstrip().endswith("\\"):
                command = (command[0], command[1].rstrip()[:-1])
            else:
                yield command
                command = None
        elif match or not line.strip():
            joined = "\n".join(text for _, text in paragraph)
            for span in re.finditer(r"(`+)(.+?)\1(?!`)", joined, re.S):
                yield paragraph[0][0] + joined.count("\n", 0, span.start(2)), span[2].replace("\n", " ")
            paragraph = []
            if match:
                fence = match[1]
        else:
            paragraph.append((number, line))
    if command:
        yield command


def without_comment(code):
    """Code up to a shell comment: a # at the start or after a space, outside quotes."""
    quote = None
    for index, char in enumerate(code):
        if quote:
            quote = None if char == quote else quote
        elif char in "'\"":
            quote = char
        elif char == "#" and (index == 0 or code[index - 1].isspace()):
            return code[:index]
    return code


def invocation_words(code):
    """The words after each kpop or kpopper in a piece of code, one list per invocation."""
    code = without_comment(code)
    for match in INVOCATION.finditer(code):
        rest = code[match.end():]
        rest = re.split(r"\s(?:\||&&|\|\||;|>|2>)\s|[;`]", rest)[0]
        try:
            words = shlex.split(rest)
        except ValueError:
            words = rest.split()
        cleaned = []
        for word in words:
            while word != (stripped := word.strip("[]{}()").rstrip(".,;:!?")):
                word = stripped
            if word.startswith("--"):
                word = word.split("=", 1)[0] + ("=" if "=" in word else "")
            if word:
                cleaned.append(word)
        yield match[1], cleaned


def placeholder(word):
    return (word.startswith(("<", "\"", "'", "$", "…", "...")) or word.endswith(("…", "..."))
            or (word.isupper() and len(word) > 1) or "|" in word)


def invocation_errors(program, words, surface):
    """Unknown commands and options in one invocation; an empty list when it is not a command."""
    path, allowed, positional, errors = (), dict(surface[()]), False, []
    index = 0
    while index < len(words):
        word = words[index]
        index += 1
        if word.startswith("--"):
            name, attached = word.rstrip("="), word.endswith("=")
            if name not in allowed:
                errors.append("%s has no option %s" % (" ".join(("kpop",) + path), name))
            elif allowed[name] and not attached and index < len(words) and not words[index].startswith("-"):
                index += 1
        elif word.startswith("-") or positional:
            continue
        elif not path and word == "help" and program == "kpop":
            continue
        elif path + (word,) in surface or path + (word,) in HIDDEN_ALIASES:
            path = HIDDEN_ALIASES.get(path + (word,), path + (word,))
            allowed.pop("--version", None)    # the one root option subcommands do not inherit
            allowed.update(surface[path])
        elif placeholder(word):
            if not path:
                return errors    # `kpop <command> ...` names no command to check against
            positional = True
        elif not path:
            return [] if program == "kpopper" else errors + ["kpop has no command %s" % word]
        elif any(len(other) == len(path) + 1 and other[:len(path)] == path for other in surface):
            return errors + ["%s has no subcommand %s" % (" ".join(("kpop",) + path), word)]
        else:
            positional = True
    if program == "kpopper" and not path:
        return []
    return errors


def command_reference_errors(paths, surface, root=ROOT):
    errors = []
    for path in paths:
        text = path.read_text(encoding="utf-8")
        for number, code in code_segments(text):
            for program, words in invocation_words(code):
                for error in invocation_errors(program, words, surface):
                    errors.append("%s:%d: %s" % (path.relative_to(root), number, error))
    return errors


def reference_files(root=ROOT):
    files = [root / name for name in REFERENCE_DOCS if (root / name).is_file()]
    for tree in REFERENCE_TREES:
        files += sorted(p for p in (root / tree).rglob("*") if p.is_file() and p.suffix in REFERENCE_SUFFIXES)
    return files


class CommandReferences(unittest.TestCase):
    def surface(self):
        return cli_surface(CLI_SURFACE.read_text(encoding="utf-8"))

    def test_cited_commands_and_options_exist(self):
        files = reference_files()
        self.assertGreater(len(files), 30)
        self.assertEqual(command_reference_errors(files, self.surface()), [])

    def test_the_check_reads_invocations_like_a_shell(self):
        surface = self.surface()
        good = ["kpop --workspace DIR pull <id> --from origin/main",
                "$ kpop session view --view-format json  # a comment with --not-a-flag",
                "kpop pull [--from <ref>].", "kpop set key value --why=\"a reason\"",
                "kpop experimental hub --verify", "kpop <command> --help", "kpopper keeps a record",
                "kpopper _agent guide", "kpop check && git diff --exit-code",
                "kpop --workspace=DIR pull --from x", "kpop --version", "kpop pull [--history].",
                "kpop pull x # kpop nonexistent", "# kpop nonexistent", "kpop help pull",
                "kpop experimental page --verify", "<plugin>/scripts/bin/kpop open", "kpop session <OPERATION>"]
        for code in good:
            with self.subTest(code=code):
                self.assertEqual([e for p, w in invocation_words(code) for e in invocation_errors(p, w, surface)], [])
        bad = {"kpop pull x --bogus": "kpop pull has no option --bogus", "kpop pul x": "kpop has no command pul",
               "kpop --workspace DIR pull --bogus": "kpop pull has no option --bogus",
               "kpopper pull --bogus": "kpop pull has no option --bogus",
               "kpop --workspace=DIR bogus": "kpop has no command bogus",
               "kpop pull --version": "kpop pull has no option --version",
               "kpop session bogus": "kpop session has no subcommand bogus",
               "kpop experimental bogus": "kpop experimental has no subcommand bogus",
               "kpop followups dialy status": "kpop followups has no subcommand dialy",
               "kpop pull x '# quoted' --bogus": "kpop pull has no option --bogus",
               "<plugin>/scripts/bin/kpop open --bogus": "kpop open has no option --bogus"}
        for code, error in bad.items():
            with self.subTest(code=code):
                self.assertEqual([e for p, w in invocation_words(code) for e in invocation_errors(p, w, surface)], [error])

    def test_drift_in_a_skill_names_the_file_and_line(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            skill = root / "skills" / "probe" / "SKILL.md"
            skill.parent.mkdir(parents=True)
            skill.write_text("# Probe\n\nRun `kpop pull x`.\n\n```sh\nkpop pull x --bogus\n```\n"
                             "\n```sh\nkpop session --no-settings \\\n  --bogus-flag open\n```\n"
                             "\nA wrapped `kpop pull\nx --wrapped` span.\n", encoding="utf-8")
            self.assertEqual(command_reference_errors(reference_files(root), self.surface(), root),
                             ["skills/probe/SKILL.md:6: kpop pull has no option --bogus",
                              "skills/probe/SKILL.md:10: kpop session has no option --bogus-flag",
                              "skills/probe/SKILL.md:14: kpop pull has no option --wrapped"])

    def test_adapter_method_summaries_match_their_source(self):
        result = subprocess.run(["sh", str(ROOT / "adapters" / "_shared" / "check-drift.sh")],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
