"""Parked T4c acceptance boundary for new defaults and legacy preservation.

These cases intentionally describe the post-parity default.  They are expected
to fail until the default route is integrated; the legacy controls must remain
green throughout.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class NewRecordHistoryDefault(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.workspace = self.root / "fresh workspace"
        self.workspace.mkdir()
        self.env = dict(os.environ)
        self.env.update(XDG_STATE_HOME=str(self.root / "state"),
                        XDG_CONFIG_HOME=str(self.root / "config"),
                        KPOPPER_SESSION_DISABLE="1")

    def cli(self, *args, cwd=None):
        return subprocess.run([sys.executable, str(ROOT / "scripts/cli.py"), *args],
                              cwd=cwd or self.workspace, env=self.env,
                              capture_output=True, text=True)

    def add_first(self):
        result = self.cli("add", "p.value", "v=1", "--as-of", "2026-09-19")
        self.assertEqual(result.returncode, 0, result.stderr)
        return self.workspace / "GROUNDING.yaml"

    def test_fresh_first_write_defaults_to_core_history_and_reopens(self):
        record = self.add_first()
        self.assertTrue(record.is_file())
        import yaml
        document = yaml.safe_load(record.read_text())
        self.assertEqual(document["meta"]["reasoning"]["profile"], "core/v1")
        self.assertEqual(document["meta"]["history"]["authority"], "history")
        self.assertTrue((self.workspace / ".kpopper" / "history.yaml").is_file())

        reopened = self.cli("open", "--json")
        self.assertEqual(reopened.returncode, 0, reopened.stderr)
        payload = json.loads(reopened.stdout)
        self.assertEqual(payload["status"], "found")
        self.assertEqual(Path(payload["record"]).resolve(), record.resolve())

    def test_existing_undeclared_grounding_stays_legacy(self):
        record = self.workspace / "GROUNDING.yaml"
        record.write_text("known:\n  p.value: {v: 1}\n", encoding="utf-8")
        reopened = self.cli("open", "--json")
        self.assertEqual(reopened.returncode, 0, reopened.stderr)
        self.assertEqual(Path(json.loads(reopened.stdout)["record"]).resolve(), record.resolve())
        import yaml
        document = yaml.safe_load(record.read_text())
        self.assertNotIn("reasoning", document.get("meta", {}))
        self.assertFalse((self.workspace / ".kpopper" / "history.yaml").exists())

    def test_existing_undeclared_legacy_name_stays_legacy(self):
        record = self.workspace / "PROVENANCE.yaml"
        record.write_text("known:\n  p.value: {v: 1}\n", encoding="utf-8")
        reopened = self.cli("open", "--json")
        self.assertEqual(reopened.returncode, 0, reopened.stderr)
        self.assertEqual(Path(json.loads(reopened.stdout)["record"]).resolve(), record.resolve())
        import yaml
        document = yaml.safe_load(record.read_text())
        self.assertNotIn("reasoning", document.get("meta", {}))
        self.assertFalse((self.workspace / "PROVENANCE.history.yaml").exists())


if __name__ == "__main__":
    unittest.main()
