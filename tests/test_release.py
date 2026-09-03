"""The release pull request's arithmetic, and the invariant the three version files keep.

    python3 -m unittest discover -s tests
"""
import importlib.util
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("release", ROOT / ".github" / "scripts" / "release.py")
R = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(R)


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
    def test_the_three_files_agree(self):
        found = R.versions_in(R.read_texts())
        self.assertEqual(len(set(found.values())), 1, found)

    def test_moving_the_version_touches_only_the_version(self):
        texts = R.read_texts()
        moved = R.with_version(texts, "9.9.9")
        for name, text in moved.items():
            self.assertIn("9.9.9", text)
            before, after = texts[name].splitlines(), text.splitlines()
            self.assertEqual(len(before), len(after))
            self.assertEqual(sum(1 for a, b in zip(before, after) if a != b), 1, name)


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


if __name__ == "__main__":
    unittest.main()
