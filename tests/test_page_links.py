"""Source links stay attached to the record when its generated page moves."""
import html.parser
import pathlib
import subprocess
import sys
import tempfile
import unittest
import unittest.mock
import urllib.parse

import yaml


ROOT = pathlib.Path(__file__).resolve().parent.parent
CLI = ROOT / "scripts" / "cli.py"
sys.path.insert(0, str(ROOT / "scripts"))
import cli as C  # noqa: E402
import render_page as R  # noqa: E402


class Anchors(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.items = []
        self.current = None

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            self.current = {**dict(attrs), "text": ""}

    def handle_data(self, data):
        if self.current is not None:
            self.current["text"] += data

    def handle_endtag(self, tag):
        if tag == "a" and self.current is not None:
            self.items.append(self.current)
            self.current = None


def resolved_file(page, href):
    """The filesystem destination a browser reaches from one generated page."""
    # The CLI opens the absolute spelling it wrote. URL resolution is lexical; filesystem
    # resolution (including macOS's /var -> /private/var link) happens at the destination.
    location = urllib.parse.urlsplit(urllib.parse.urljoin(page.absolute().as_uri(), href))
    return pathlib.Path(urllib.parse.unquote(location.path)).resolve()


class PageSourceLinks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name) / "record # שלום"
        self.root.mkdir()
        (self.root / ".kpopper").mkdir()
        self.local = self.root / "notes with spaces" / "source 100% # שלום.txt"
        self.local.parent.mkdir()
        self.local.write_text("record-relative evidence", encoding="utf-8")
        self.absolute = pathlib.Path(self.temp.name) / "absolute # שלום.txt"
        self.absolute.write_text("absolute evidence", encoding="utf-8")

        relative_url = urllib.parse.quote(str(self.local.relative_to(self.root)), safe="/") + "?view=1#part"
        record = {
            "sources": {
                "doc.local": {"name": "Local evidence", "file": str(self.local.relative_to(self.root))},
                "doc.local_url": {"name": "Relative URL evidence", "url": relative_url},
                "doc.absolute": {"name": "Absolute evidence", "file": str(self.absolute)},
                "doc.file_url": {"name": "File URL evidence", "url": self.absolute.as_uri()},
                "doc.external": {"name": "External evidence", "url": "https://example.test/a?q=one#part"},
                "doc.fragment": {"name": "Tree navigation", "url": "#tree"},
            }
        }
        (self.root / "GROUNDING.yaml").write_text(
            yaml.safe_dump(record, allow_unicode=True, sort_keys=False), encoding="utf-8")
        brief = {
            "title": "Sources",
            "sections": [{"title": "Evidence", "why": "Open the evidence",
                          "pick": list(record["sources"]), "as": "links"}],
            "truth": "doc.local",
            "elsewhere": "doc.absolute",
        }
        (self.root / ".kpopper" / "view.yaml").write_text(
            yaml.safe_dump(brief, allow_unicode=True, sort_keys=False), encoding="utf-8")

    def render(self, *args, cwd=None):
        result = subprocess.run([sys.executable, str(CLI), "page", *map(str, args)],
                                cwd=cwd or self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def anchors(self, page):
        parser = Anchors()
        parser.feed(page.read_text(encoding="utf-8"))
        return parser.items

    def named(self, page, label, css_class=None):
        found = [a for a in self.anchors(page)
                 if label in a["text"] and (css_class is None or css_class in a.get("class", "").split())]
        self.assertTrue(found, f"no {label!r} link in {page}")
        return found

    def test_default_page_from_a_subdirectory_resolves_links_and_footer_to_the_record(self):
        nested = self.root / "work" / "below"
        nested.mkdir(parents=True)
        self.render(cwd=nested)
        page = self.root / ".kpopper" / "build" / "page.html"

        for anchor in self.named(page, "Local evidence"):
            self.assertEqual(resolved_file(page, anchor["href"]), self.local.resolve())
        for anchor in self.named(page, "Absolute evidence"):
            self.assertEqual(resolved_file(page, anchor["href"]), self.absolute.resolve())

    def test_page_written_outside_the_record_keeps_every_kind_of_destination(self):
        output = pathlib.Path(self.temp.name) / "published # שלום" / "report.html"
        output.parent.mkdir()
        self.render("--out", output, cwd=self.root / "notes with spaces")

        for label in ("Local evidence", "Relative URL evidence", "Absolute evidence", "File URL evidence"):
            for anchor in self.named(output, label, "lk"):
                expected = self.local if label in ("Local evidence", "Relative URL evidence") else self.absolute
                self.assertEqual(resolved_file(output, anchor["href"]), expected.resolve())
        self.assertTrue(self.named(output, "Relative URL evidence", "lk")[0]["href"].endswith("?view=1#part"))
        self.assertEqual(self.named(output, "External evidence", "lk")[0]["href"],
                         "https://example.test/a?q=one#part")
        self.assertEqual(self.named(output, "Tree navigation", "lk")[0]["href"], "#tree")
        hrefs = {a.get("href") for a in self.anchors(output)}
        self.assertIn("#tree", hrefs)
        self.assertTrue(any((href or "").startswith("#g-") for href in hrefs))

    def test_a_relative_pointer_keeps_sources_relative_to_the_primary_record(self):
        # A divided record is rooted at its entry file: the page/layout contract follows the
        # primary GROUNDING.yaml, while P.load follows this pointer relative to that file.
        entries = self.root / "parts" / "sources.yaml"
        entries.parent.mkdir()
        entries.write_text((self.root / "GROUNDING.yaml").read_text(encoding="utf-8"), encoding="utf-8")
        (self.root / "GROUNDING.yaml").write_text("record: parts/sources.yaml\n", encoding="utf-8")

        self.render(cwd=self.root / "parts")
        page = self.root / ".kpopper" / "build" / "page.html"
        for anchor in self.named(page, "Local evidence"):
            self.assertEqual(resolved_file(page, anchor["href"]), self.local.resolve())

    def test_opening_the_page_hands_the_browser_a_url_its_own_name_cannot_cut_short(self):
        """The record directory is named with a '#' and a space, as a person may name one."""
        page = self.root / ".kpopper" / "build" / "page.html"
        record = str(self.root / "GROUNDING.yaml")

        with unittest.mock.patch.object(C.webbrowser, "open") as opened:
            with self.assertRaises(SystemExit) as stopped:
                C.do_page([record, "--open", "--tree"])
        self.assertEqual(stopped.exception.code, 0)

        location = urllib.parse.urlsplit(opened.call_args[0][0])
        self.assertEqual(location.scheme, "file")
        self.assertEqual(location.fragment, "tree")
        self.assertNotIn("#", location.path)
        self.assertEqual(pathlib.Path(urllib.parse.unquote(location.path)).resolve(), page.resolve())

        # The fragment is the flag's, not the name's: without --tree the address carries none.
        with unittest.mock.patch.object(C.webbrowser, "open") as plain:
            with self.assertRaises(SystemExit):
                C.do_page([record, "--open"])
        self.assertEqual(urllib.parse.urlsplit(plain.call_args[0][0]).fragment, "")

    def test_a_cross_volume_source_falls_back_to_an_escaped_file_uri(self):
        page = self.root / ".kpopper" / "build" / "page.html"
        relative_url = urllib.parse.quote(str(self.local.relative_to(self.root)), safe="/") + "?view=1#part"
        with unittest.mock.patch.object(R.os.path, "relpath", side_effect=ValueError("different drives")):
            file_href = R.link_target({"file": str(self.local.relative_to(self.root))}, self.root, page)
            url_href = R.link_target({"url": relative_url}, self.root, page)

        self.assertTrue(file_href.startswith("file:"))
        self.assertEqual(resolved_file(page, file_href), self.local.resolve())
        location = urllib.parse.urlsplit(url_href)
        self.assertEqual(location.scheme, "file")
        self.assertEqual(pathlib.Path(urllib.parse.unquote(location.path)).resolve(), self.local.resolve())
        self.assertEqual((location.query, location.fragment), ("view=1", "part"))

    def test_a_same_volume_windows_path_uses_url_separators(self):
        page = self.root / ".kpopper" / "build" / "page.html"
        with unittest.mock.patch.object(R.os.path, "relpath", return_value=r"..\record\notes\source.txt"), \
                unittest.mock.patch.object(R.os, "sep", "\\"):
            href = R.link_target({"file": "notes/source.txt"}, self.root, page)
        self.assertEqual(href, "../record/notes/source.txt")
        self.assertNotIn("%5C", href.upper())


if __name__ == "__main__":
    unittest.main()
