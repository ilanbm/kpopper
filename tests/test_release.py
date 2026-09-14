"""The release pull request's arithmetic, and the invariants a release carries: what the
version files keep, and what the packages ship.

    python3 -m unittest discover -s tests
"""
import fnmatch
import importlib.util
import json
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / ".github" / "scripts"


def _load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


R = _load("release")
P = _load("publish_release")


class TheBump(unittest.TestCase):
    def test_a_declared_bump_is_read_anywhere_in_the_body(self):
        self.assertEqual(R.bump_of("## What changed\n\nthings\n\nBump: minor\n\n---"), "minor")
        self.assertEqual(R.bump_of("bump : MAJOR"), "major")
        self.assertIsNone(R.bump_of("no line here"))
        self.assertIsNone(R.bump_of("Bump: none — this is the release"))

    def test_the_largest_bump_wins_and_none_counts_as_patch(self):
        self.assertEqual(R.largest(["patch", "minor", "patch"]), "minor")
        self.assertEqual(R.largest(["minor", "major"]), "major")
        self.assertEqual(R.largest([None, None]), "patch")
        self.assertEqual(R.largest([]), "patch")

    def test_the_next_version(self):
        self.assertEqual(R.next_version("0.20.0", "patch"), "0.20.1")
        self.assertEqual(R.next_version("0.20.3", "minor"), "0.21.0")
        self.assertEqual(R.next_version("0.20.3", "major"), "1.0.0")


class TheVersionFiles(unittest.TestCase):
    def test_every_channel_carries_the_same_version(self):
        found = R.versions_in(R.read_texts())
        self.assertEqual(len(set(found.values())), 1, found)

    def test_the_npm_package_is_one_of_them(self):
        # The npm package ships the browser checks, so it moves with everything else. Read
        # by the manifest rather than by the pattern, so a dependency version appearing
        # above the package's own cannot pass for it.
        self.assertIn("package.json", R.VERSION_FILES)
        found = R.versions_in(R.read_texts())
        manifest = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
        self.assertEqual(found["package.json"], manifest["version"])

    def test_moving_the_version_touches_only_the_version(self):
        texts = R.read_texts()
        moved = R.with_version(texts, "9.9.9")
        for name, text in moved.items():
            self.assertIn("9.9.9", text)
            before, after = texts[name].splitlines(), text.splitlines()
            self.assertEqual(len(before), len(after))
            self.assertEqual(sum(1 for a, b in zip(before, after) if a != b), 1, name)


class WhatShips(unittest.TestCase):
    def test_the_browser_checks_travel_with_the_command_line(self):
        # `kpopper page --checks` runs the file that came with the reader, so a wheel that
        # declares everything except that file turns the flag into an error message on every
        # installed copy - and nothing else in the suite opens a wheel to notice.
        toml = (ROOT / "pyproject.toml").read_text(encoding="utf-8")
        block = re.search(r"\[tool\.setuptools\.package-data\](.*?)(?:\n\[|\Z)", toml, re.S)
        self.assertIsNotNone(block, "no package-data section")
        globs = re.findall(r'"([^"]+)"', block.group(1))
        self.assertTrue(any(fnmatch.fnmatch("verify_page.js", g) for g in globs), globs)

    def test_standalone_document_assets_and_guide_ship_with_the_python_runtime(self):
        toml = (ROOT / "pyproject.toml").read_text(encoding="utf-8")
        block = re.search(r"\[tool\.setuptools\.package-data\](.*?)(?:\n\[|\Z)", toml, re.S)
        self.assertIsNotNone(block)
        globs = re.findall(r'"([^"]+)"', block.group(1))
        for name in ("document/layer.js", "document/frame.js", "document/layer.css", "document-guide.md"):
            self.assertTrue((ROOT / "scripts" / name).is_file(), name)
            self.assertTrue(any(fnmatch.fnmatch(name, pattern) for pattern in globs), name)


class TheChangelog(unittest.TestCase):
    def test_an_entry_lists_what_merged_and_what_the_record_gained(self):
        entry = R.changelog_entry("0.21.0", "2026-09-04",
                                  [{"subject": "Accept the page contract", "pr": 2, "bump": "minor"},
                                   {"subject": "Fix a thing", "pr": None, "bump": None}],
                                  ["d.page_is_graph"])
        self.assertIn("## 0.21.0 — 2026-09-04", entry)
        self.assertIn("- Accept the page contract (#2) — minor", entry)
        self.assertIn("- Fix a thing — no bump declared", entry)
        self.assertIn("Decisions recorded: d.page_is_graph", entry)

    def test_a_new_entry_goes_on_top(self):
        first = R.prepend("", "## 0.21.0 — x\n\n- a\n")
        self.assertTrue(first.startswith("# Changelog\n\n## 0.21.0"))
        second = R.prepend(first, "## 0.22.0 — y\n\n- b\n")
        self.assertLess(second.index("0.22.0"), second.index("0.21.0"))

    def test_the_release_body_declares_no_bump_of_its_own(self):
        body = R.pr_body("0.21.0", "0.20.0", "minor", [{"subject": "s", "pr": 2, "bump": "minor"}], [])
        self.assertIn("from 0.20.0 to **0.21.0**", body)
        self.assertIsNone(R.bump_of(body))


class WhatIsPublished(unittest.TestCase):
    def test_the_tag_carries_the_version(self):
        self.assertEqual(P.tag_for("1.5.2"), "v1.5.2")

    def test_a_release_says_what_its_own_version_changed(self):
        log = ("# Changelog\n\n"
               "## 0.22.0 — 2026-09-05\n\n- later (#9) — minor\n\n"
               "## 0.21.0 — 2026-09-04\n\n- earlier (#2) — minor\n\n"
               "Decisions recorded: d.page_is_graph\n")
        self.assertEqual(P.section_for(log, "0.22.0"), "- later (#9) — minor")
        older = P.section_for(log, "0.21.0")
        self.assertIn("- earlier (#2) — minor", older)
        self.assertIn("Decisions recorded: d.page_is_graph", older)
        self.assertNotIn("later", older)

    def test_only_a_version_heading_ends_a_section(self):
        # The section becomes the release's text. A heading quoted inside it - an example, a
        # fenced block - is text, and cutting there would publish half an entry.
        log = ("# Changelog\n\n"
               "## 0.22.0 — 2026-09-05\n\n"
               "- Accept a heading in a document\n\n"
               "```\n## example heading\n```\n\n"
               "- And the rest of it\n\n"
               "## 0.21.0 — 2026-09-04\n\n- earlier\n")
        section = P.section_for(log, "0.22.0")
        self.assertIn("## example heading", section)
        self.assertIn("- And the rest of it", section)
        self.assertNotIn("earlier", section)

    def test_a_version_the_changelog_never_mentions_has_no_section(self):
        log = "# Changelog\n\n## 0.22.0 — 2026-09-05\n\n- a\n"
        self.assertIsNone(P.section_for(log, "0.21.0"))
        self.assertIsNone(P.section_for(log, "0.2"))

    def test_the_released_version_has_something_to_say_for_itself(self):
        # The release is opened with this section as its text, so a version the changelog
        # skipped stops the release - after the merge, where nothing can be added to it.
        # Here it is a red pull request instead.
        version = R.versions_in(R.read_texts())["pyproject.toml"]
        section = P.section_for((ROOT / "CHANGELOG.md").read_text(encoding="utf-8"), version)
        self.assertIsNotNone(section, f"CHANGELOG.md carries no section for {version}")
        self.assertTrue(section.strip(), version)

    def test_a_release_carries_distributions_and_nothing_else(self):
        paths = [pathlib.Path(n) for n in ("kpopper-1.5.2.tar.gz",
                                           "kpopper-1.5.2-py3-none-any.whl",
                                           "kpopper-1.5.2.txt",
                                           "kpopper-1.5.20.tar.gz",
                                           "kpopper-1.5.1-py3-none-any.whl")]
        mine, stray = P.dists_in(paths, "1.5.2")
        self.assertEqual([p.name for p in mine],
                         ["kpopper-1.5.2-py3-none-any.whl", "kpopper-1.5.2.tar.gz"])
        # A file that merely carries the version is not a distribution of it, and 1.5.20
        # starts with 1.5.2 without being it.
        self.assertEqual(stray, ["kpopper-1.5.1-py3-none-any.whl",
                                 "kpopper-1.5.2.txt", "kpopper-1.5.20.tar.gz"])


if __name__ == "__main__":
    unittest.main()
