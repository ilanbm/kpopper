"""Public acceptance cases for the core/history operational consumers.

These fixtures are deliberately disposable.  They describe the settled consumer contract while
the implementation is being completed; a failure from an explicit unsupported-capability refusal
is therefore an expected product failure, whereas a malformed fixture or harness failure is not.
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from scripts.reasoning import assessment as CoreAssessment
from scripts.reasoning.snapshot import Snapshot


# Resolve the public dispatcher through the package alias used by the installed acceptance
# harness.  This keeps the same cases runnable against a wheel/plugin installation.
import importlib.util

_PROVENANCE = importlib.util.find_spec("scripts.provenance")
if _PROVENANCE is None or _PROVENANCE.origin is None:
    raise RuntimeError("scripts.provenance is unavailable")
CLI = Path(_PROVENANCE.origin).with_name("cli.py")

CORE = """\
meta:
  name: disposable core acceptance fixture
  reasoning:
    version: 2
    profile: core/v1
    requires: [arithmetic/v1, composition/v1]
known:
  v.base:
    v: 10
  v.flag:
    v: false
judgments:
  d.bound:
    rests_on: [v.base, v.flag]
    seen: {v.base: 10, v.flag: false}
    verdict: base is within the bound
    wrong_if:
      op: or
      args:
        - op: gt
          args: [{ref: v.base}, {num: '20'}]
        - op: eq
          args: [{ref: v.flag}, {bool: true}]
"""


class CoreOperationalAcceptance(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / "GROUNDING.yaml"
        self.record.write_text(CORE, encoding="utf-8")
        self.assert_core_fixture_is_assessable()

    def assert_core_fixture_is_assessable(self):
        """Keep operational failures distinct from malformed core input or capture errors."""
        snapshot = Snapshot.capture([str(self.record)], read_mode="frozen")
        report = CoreAssessment.assess(snapshot)
        self.assertEqual(report["snapshot_id"], snapshot.snapshot_id)
        for node_id, node in report["nodes"].items():
            computation = node.get("computation") or {}
            self.assertNotEqual(computation.get("status"), "error", node_id)
            self.assertEqual(computation.get("diagnostics", []), [], node_id)
            integrity = node.get("state", {}).get("integrity", {})
            self.assertEqual(integrity.get("issues", []), [], node_id)
            falsifier = node.get("state", {}).get("falsifier", {})
            falsifier_computation = falsifier.get("computation") or {}
            self.assertNotEqual(falsifier_computation.get("status"), "error", node_id)
            self.assertEqual(falsifier_computation.get("diagnostics", []), [], node_id)

    def cli(self, *args, cwd=None):
        return subprocess.run([sys.executable, str(CLI), *args], cwd=cwd or self.root,
                              text=True, capture_output=True, check=False)

    def test_core_consolidate_without_hypotheses_is_a_read_only_noop(self):
        before = self.record.read_bytes()
        result = self.cli("consolidate", "--dry-run", str(self.record))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("no hypothesis", result.stdout.lower())
        self.assertEqual(self.record.read_bytes(), before)
        self.assertEqual(sorted(p.relative_to(self.root).as_posix() for p in self.root.rglob("*")),
                         ["GROUNDING.yaml"])

    def test_core_consolidate_evaluates_formula_and_condition_in_a_hypothetical_union(self):
        hypothesis_dir = self.root / ".kpopper" / "hypotheses"
        hypothesis_dir.mkdir(parents=True)
        (hypothesis_dir / "flag-on.yaml").write_text(
            "hypothesis: {claim: turn the flag on, folds: never}\n"
            "known:\n  v.flag: {v: true}\n", encoding="utf-8")
        result = self.cli("consolidate", "--dry-run", str(self.record))
        self.assertNotEqual(result.returncode, 0)
        output = result.stdout + result.stderr
        self.assertIn("core/v1", output)
        self.assertIn("d.bound", output)
        self.assertRegex(output.lower(), r"falsif|wrong_if|condition")
        self.assertEqual(self.record.read_text(encoding="utf-8"), CORE)

    def test_core_remeasure_plan_is_read_only_and_keeps_the_exact_recipe_visible(self):
        self.record.write_text(CORE.replace("v: 10", "v: 10\n    measure: base_value"), encoding="utf-8")
        measure_dir = self.root / ".kpopper"
        measure_dir.mkdir()
        (measure_dir / "measure.yaml").write_text(
            "base_value: [python3, -I, -c, \"print(10)\"]\n", encoding="utf-8")
        result = self.cli("remeasure", str(self.record))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("base_value", result.stdout)
        self.assertIn("python3 -I -c", result.stdout)
        self.assertIn("nothing ran - add --run to measure this tree", result.stdout)
        self.assertEqual(self.record.read_text(encoding="utf-8"),
                         CORE.replace("v: 10", "v: 10\n    measure: base_value"))
        self.assertFalse((self.root / "ran").exists())

    def test_watch_scan_of_unchanged_core_snapshot_is_clear(self):
        subprocess.run(["git", "init", "-q", "-b", "main"], cwd=self.root, check=True)
        subprocess.run(["git", "add", "GROUNDING.yaml"], cwd=self.root, check=True)
        subprocess.run(["git", "-c", "user.name=fixture", "-c", "user.email=fixture@example.test",
                        "commit", "-qm", "core fixture"], cwd=self.root, check=True)
        setup = self.cli("watch", "setup", "--base-ref", "main")
        self.assertEqual(setup.returncode, 0, setup.stdout + setup.stderr)
        scan = self.cli("watch", "scan")
        self.assertEqual(scan.returncode, 0, scan.stdout + scan.stderr)
        status = None
        for _ in range(100):
            status_result = self.cli("watch", "status")
            self.assertEqual(status_result.returncode, 0, status_result.stdout + status_result.stderr)
            status = json.loads(status_result.stdout)
            if status.get("state") != "pending":
                break
            time.sleep(0.05)
        self.assertEqual(status.get("state"), "clear", status)
        self.assertEqual(status.get("findings"), [])


if __name__ == "__main__":
    unittest.main()
