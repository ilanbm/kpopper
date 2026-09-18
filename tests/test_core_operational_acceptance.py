"""Public CLI consolidation acceptance, reusable against installed distributions."""
from pathlib import Path
import subprocess
import sys
import tempfile
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
        self.assertIn("nothing to consolidate", result.stdout.lower())
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




if __name__ == "__main__":
    unittest.main()
