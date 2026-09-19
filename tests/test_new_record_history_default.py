"""Acceptance boundary for new history defaults and legacy preservation."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from scripts import history_contract as C, history_store as H

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
        result = self.cli("add", "p.value", "v=1", "--as-of", "2026-09-01")
        self.assertEqual(result.returncode, 0, result.stderr)
        return self.workspace / "GROUNDING.yaml"

    def test_fresh_first_write_defaults_to_core_history_and_reopens(self):
        record = self.add_first()
        self.assertTrue(record.is_file())
        import yaml
        document = yaml.safe_load(record.read_text())
        self.assertEqual(document["meta"]["reasoning"]["profile"], "core/v1")
        marker_path = self.workspace / ".kpopper" / "history.yaml"
        marker = C.decode_document(marker_path.read_bytes())
        self.assertEqual(marker["authority"], "history")
        self.assertEqual(marker["record_id"], document["meta"]["history"]["record_id"])
        self.assertEqual(marker["generation"], document["meta"]["history"]["authority_generation"])
        captured = H.Store(record).capture()
        self.assertEqual(captured.state["subjects"]["p.value"]["body"]["v"], 1)

        reopened = self.cli("--json", "history", "status")
        self.assertEqual(reopened.returncode, 0, reopened.stderr)
        payload = json.loads(reopened.stdout)
        self.assertEqual(payload["state"], "captured")
        self.assertEqual(payload["authority"]["authority"], "history")
        self.assertEqual(Path(payload["record"]).resolve(), record.resolve())
        checked = self.cli("check", "--profile", "core/v1")
        self.assertEqual(checked.returncode, 0, checked.stderr)

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

    def test_concurrent_first_writers_produce_one_history_generation(self):
        commands = [[sys.executable, str(ROOT / "scripts/cli.py"), "add", subject,
                     "v=" + value, "--as-of", "2026-09-01"]
                    for subject, value in (("p.first", "1"), ("p.second", "2"))]
        processes = [subprocess.Popen(command, cwd=self.workspace, env=self.env,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                     for command in commands]
        results = [process.communicate(timeout=20) + (process.returncode,) for process in processes]
        self.assertTrue(all(code == 0 for stdout, stderr, code in results), results)
        captured = H.Store(self.workspace / "GROUNDING.yaml").capture()
        self.assertEqual(set(captured.state["subjects"]), {"p.first", "p.second"})
        self.assertEqual(captured.marker["generation"], 1)
        self.assertEqual(len(captured.commits), 2)


if __name__ == "__main__":
    unittest.main()
