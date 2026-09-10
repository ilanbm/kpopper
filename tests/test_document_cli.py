"""The public CLI delivers and reopens an independent artifact, not sidecars."""
import errno
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from scripts import documents as D

ROOT = Path(__file__).resolve().parents[1]


class DocumentCLI(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.env = {**os.environ, "KPOPPER_SESSION_DISABLE": "1", "XDG_STATE_HOME": str(self.root / "state")}
        self.source = {"name": "Room capacity", "path": "source.json", "format": "json"}
        self.manifest = {"version": 1, "title": "Room plan", "sources": {"room": self.source}, "claims": [
            {"id": "capacity", "label": "Capacity", "kind": "value", "inputs": [{"source": "room", "pointer": "/capacity"}]}]}
        self.draft = '<!doctype html><html><head><title>Room plan</title><style>body{color:navy}</style></head><body><p>Capacity: <span data-kpopper-claim="capacity">24</span>.</p></body></html>'
        (self.root / "draft.html").write_text(self.draft, encoding="utf-8")
        (self.root / "manifest.json").write_text(json.dumps(self.manifest), encoding="utf-8")
        (self.root / "source.json").write_text('{"capacity":24}', encoding="utf-8")

    def cli(self, *args):
        return subprocess.run([sys.executable, str(ROOT / "scripts" / "cli.py"), *args],
                              cwd=self.root, env=self.env, capture_output=True, text=True, encoding="utf-8")

    def build(self, *extra):
        return self.cli("document", "build", "--html", "draft.html", "--manifest", "manifest.json", "--out", "result.html", *extra)

    def test_help_and_packaged_guide_explain_the_author_flow(self):
        public = self.cli("--help")
        self.assertIn("kpopper document", public.stdout)
        help_result = self.cli("document", "--help")
        self.assertEqual(help_result.returncode, 0, help_result.stderr)
        self.assertIn("refresh", help_result.stdout)
        guide = self.cli("document", "guide")
        self.assertEqual(guide.returncode, 0, guide.stderr)
        self.assertIn("The\nuser supplies neither anchors nor mapping", guide.stdout)

    def test_one_artifact_retains_sources_after_inputs_are_removed(self):
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        output = json.loads(result.stdout)
        self.assertEqual(output["checks"]["match"], 1)
        self.assertFalse(output["source_files_changed"])
        for name in ("draft.html", "manifest.json", "source.json"):
            (self.root / name).unlink()
        self.assertEqual({p.name for p in self.root.iterdir()}, {"result.html"})
        inspect = self.cli("document", "inspect", "result.html")
        self.assertEqual(inspect.returncode, 0, inspect.stderr)
        data = json.loads(inspect.stdout)
        self.assertEqual(data["checks"]["capacity"]["status"], "match")
        self.assertEqual(data["sources"]["room"]["name"], "Room capacity")
        page = (self.root / "result.html").read_text(encoding="utf-8")
        self.assertIn('<noscript>', page)
        self.assertIn('sandbox="allow-scripts"', page)
        self.assertIn('Content-Security-Policy', page)
        self.assertEqual(D.load_artifact(page)["authored_html"], self.draft)
        self.assertNotIn(str(ROOT), page)

    def test_refresh_writes_a_copy_and_preserves_saved_decisions(self):
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        original = (self.root / "result.html").read_bytes()
        (self.root / "source.json").write_text('{"capacity":30}')
        (self.root / "sources.json").write_text(json.dumps({"room": self.source}))
        changed = self.cli("document", "refresh", "result.html", "--sources", "sources.json", "--out", "refreshed.html")
        self.assertEqual(changed.returncode, 0, changed.stderr)
        self.assertEqual((self.root / "result.html").read_bytes(), original)
        data = D.load_artifact((self.root / "refreshed.html").read_text(encoding="utf-8"))
        data["groups"][0].update(decision="accepted", decided_at="2026-09-12T00:00:00Z")
        D.write_output(data, self.root / "chosen.html")
        reopened = self.cli("document", "inspect", "chosen.html")
        self.assertEqual(reopened.returncode, 0, reopened.stderr)
        self.assertEqual(json.loads(reopened.stdout)["checks"]["capacity"]["actual"], "30")
        self.assertEqual(json.loads(reopened.stdout)["proposals"][0]["decision"], "accepted")

    def test_no_implicit_overwrite_or_source_overwrite(self):
        result = self.build()
        self.assertEqual(result.returncode, 0, result.stderr)
        existing = (self.root / "result.html").read_bytes()
        self.assertNotEqual(self.build().returncode, 0)
        self.assertEqual((self.root / "result.html").read_bytes(), existing)
        self.assertEqual(self.build("--overwrite").returncode, 0)
        for out in ("draft.html", "manifest.json", "source.json"):
            before = (self.root / out).read_bytes()
            result = self.cli("document", "build", "--html", "draft.html", "--manifest", "manifest.json", "--out", out, "--overwrite")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("separate copy", result.stderr)
            self.assertEqual((self.root / out).read_bytes(), before)

    def test_no_clobber_output_works_without_filesystem_hard_links(self):
        data = D.build(self.draft, self.manifest, self.root)
        out = self.root / "portable.html"
        with patch.object(D.os, "link", side_effect=OSError(errno.EOPNOTSUPP, "no hard links")):
            D.write_output(data, out)
            self.assertEqual(D.load_artifact(out.read_text())["authored_html"], self.draft)
            before = out.read_bytes()
            with self.assertRaises(FileExistsError):
                D.write_output(data, out)
            self.assertEqual(out.read_bytes(), before)

    def test_failed_exclusive_fallback_removes_its_partial_output(self):
        data = D.build(self.draft, self.manifest, self.root)
        out = self.root / "portable.html"
        with patch.object(D.os, "link", side_effect=OSError(errno.EOPNOTSUPP, "no hard links")), \
                patch.object(D.os, "fsync", side_effect=[None, OSError("destination write failed")]):
            with self.assertRaises(OSError):
                D.write_output(data, out)
        self.assertFalse(out.exists())
        self.assertFalse(list(self.root.glob(".kpopper-document-*")))

    def test_invalid_manifest_does_not_leave_an_output_file(self):
        self.manifest["claims"][0]["kind"] = "evaluate_python"
        (self.root / "manifest.json").write_text(json.dumps(self.manifest))
        result = self.build()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "result.html").exists())
        self.assertNotIn("Traceback", result.stderr)

    def test_json_mode_retains_public_exit_status(self):
        result = self.cli("--json", "document", "inspect", "missing.html")
        self.assertNotEqual(result.returncode, 0)
        data = json.loads(result.stdout)
        self.assertEqual(data["exit_code"], result.returncode)
        self.assertEqual(data["command"], "document")


if __name__ == "__main__":
    unittest.main()
