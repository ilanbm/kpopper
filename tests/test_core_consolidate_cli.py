"""Public CLI acceptance for refuting and importing declared core records.

The records and repositories are disposable.  Core capture/assessment validation runs before
each operation so an unsupported consumer remains distinguishable from a malformed fixture.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from scripts.reasoning import assessment as CoreAssessment
from scripts.reasoning.snapshot import Snapshot
from scripts import provenance as P


ROOT = Path(__file__).resolve().parents[1]
CLI = ROOT / "scripts" / "cli.py"

CORE = """\
meta:
  name: disposable core consolidation fixture
  reasoning:
    version: 2
    profile: core/v1
    requires: [arithmetic/v1, composition/v1]
sources:
  s.session:
    asked: Should the proposed value be retained?
    name: fixture session source
    read: 2026-09-17
known:
  v.base:
    v: 10
judgments:
  d.bound:
    rests_on: [v.base]
    seen: {v.base: 10}
    verdict: base is within the bound
    wrong_if:
      op: gt
      args: [{ref: v.base}, {num: '20'}]
"""


class CoreConsolidateCLI(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.record = self.root / "GROUNDING.yaml"
        self.record.write_text(CORE, encoding="utf-8")
        self.assert_core_fixture(self.record)

    @staticmethod
    def assert_core_fixture(path):
        snapshot = Snapshot.capture([str(path)], read_mode="frozen")
        report = CoreAssessment.assess(snapshot)
        if report["snapshot_id"] != snapshot.snapshot_id:
            raise AssertionError("assessment is not bound to captured snapshot")
        for node_id, node in report["nodes"].items():
            # Session sources are provenance records, not evaluable core values.  The core
            # assessment deliberately leaves their direct computation unknown.
            if node_id in snapshot.to_data()["document"].get("sources", {}):
                continue
            computation = node.get("computation") or {}
            if computation.get("status") == "error" or computation.get("diagnostics"):
                raise AssertionError(f"invalid computation fixture for {node_id}: {computation}")
            issues = node.get("state", {}).get("integrity", {}).get("issues", [])
            if issues:
                raise AssertionError(f"invalid integrity fixture for {node_id}: {issues}")
            falsifier = node.get("state", {}).get("falsifier", {})
            fcomp = falsifier.get("computation") or {}
            if fcomp.get("status") == "error" or fcomp.get("diagnostics"):
                raise AssertionError(f"invalid falsifier fixture for {node_id}: {fcomp}")

    def cli(self, *args):
        return subprocess.run([sys.executable, str(CLI), *args], cwd=self.root,
                              text=True, capture_output=True, check=False)

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.root, text=True,
                              capture_output=True, check=True).stdout.strip()

    def test_refute_declared_core_hypothesis_preserves_base_and_records_source_evidence(self):
        hypothesis_dir = self.root / ".kpopper" / "hypotheses"
        hypothesis_dir.mkdir(parents=True)
        hypothesis = hypothesis_dir / "candidate.yaml"
        hypothesis.write_text(
            "hypothesis: {claim: retain the candidate value, folds: never}\n"
            "known:\n  v.candidate: {v: 25, from: s.session}\n", encoding="utf-8")
        before = self.record.read_bytes()
        result = self.cli("consolidate", "--refute", "candidate",
                          "the source check rejected the candidate", "--as", "s.session",
                          str(self.record))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        document = P.yaml.safe_load(self.record.read_text(encoding="utf-8"))
        self.assertEqual(document["known"]["v.base"]["v"], 10)
        refuted = [body for body in document.get("known", {}).values()
                   if isinstance(body, dict) and body.get("v") == "refuted"]
        self.assertEqual(len(refuted), 1)
        self.assertEqual(refuted[0]["from"], "s.session")
        self.assertEqual(refuted[0]["at"], "the source check rejected the candidate")
        self.assertFalse(hypothesis.exists())
        self.assertNotEqual(self.record.read_bytes(), before)

    def test_from_committed_core_branch_folds_addition_without_mutating_source(self):
        self.git("init", "-q", "-b", "main")
        self.git("add", "GROUNDING.yaml")
        self.git("-c", "user.name=fixture", "-c", "user.email=fixture@example.test",
                 "commit", "-qm", "core base")
        self.git("checkout", "-qb", "incoming")
        source_document = self.record.read_text(encoding="utf-8").replace(
            "judgments:\n", "  v.extra:\n    v: 7\n    from: s.session\njudgments:\n")
        self.record.write_text(source_document, encoding="utf-8")
        self.assert_core_fixture(self.record)
        self.git("add", "GROUNDING.yaml")
        self.git("-c", "user.name=fixture", "-c", "user.email=fixture@example.test",
                 "commit", "-qm", "core incoming addition")
        source_bytes = self.git("show", "incoming:GROUNDING.yaml")
        self.git("checkout", "main")
        self.assert_core_fixture(self.record)
        result = self.cli("consolidate", "--from", "incoming", str(self.record))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        target = self.record.read_text(encoding="utf-8")
        self.assertIn("v.extra:", target)
        self.assertIn("v: 7", target)
        self.assertEqual(self.git("show", "incoming:GROUNDING.yaml"), source_bytes)
        self.assertEqual(self.git("status", "--short"), "M GROUNDING.yaml")


if __name__ == "__main__":
    unittest.main()
