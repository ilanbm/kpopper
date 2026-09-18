"""Disposable ordinary-core remeasure acceptance cases."""
import datetime
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
CLI = ROOT / "scripts" / "cli.py"


class CoreRemeasure(unittest.TestCase):
    def fixture(self, value="10", recipe="10", *, judgment=False):
        root = Path(tempfile.mkdtemp())
        self.addCleanup(lambda: None)
        root.joinpath(".kpopper").mkdir()
        text = ("meta:\n  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}\n"
                "known:\n  p.input: {v: " + value + ", measure: base_value}\n")
        if judgment:
            text += ("judgments:\n  d.limit:\n    rests_on: [p.input]\n    seen: {p.input: " + value + "}\n"
                     "    verdict: okay\n    wrong_if: {op: gt, args: [{ref: p.input}, {num: '20'}]}\n")
        root.joinpath("GROUNDING.yaml").write_text(text, encoding="utf-8")
        root.joinpath(".kpopper/measure.yaml").write_text(
            "base_value: [" + sys.executable + ", -I, -c, \"print(" + recipe + ")\"]\n", encoding="utf-8")
        return root

    def cli(self, root, *args):
        return subprocess.run([sys.executable, str(CLI), "remeasure", *args,
                               str(root / "GROUNDING.yaml")], cwd=root,
                              capture_output=True, text=True, check=False)

    def test_plan_is_read_only_and_keeps_exact_recipe(self):
        root = self.fixture()
        before = (root / "GROUNDING.yaml").read_bytes()
        result = self.cli(root)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("base_value", result.stdout)
        self.assertIn(sys.executable + " -I -c", result.stdout)
        self.assertIn("nothing ran - add --run", result.stdout)
        self.assertEqual((root / "GROUNDING.yaml").read_bytes(), before)

    def test_changed_reading_and_falsifier_are_red_without_writing(self):
        root = self.fixture(recipe="30", judgment=True)
        before = (root / "GROUNDING.yaml").read_bytes()
        result = self.cli(root, "--run")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("FALSIFIED d.limit", result.stdout)
        self.assertIn("p.input: 10 recorded", result.stdout)
        self.assertIn("refresh: kpop set p.input 30", result.stdout)
        self.assertEqual((root / "GROUNDING.yaml").read_bytes(), before)

    def test_unknown_or_error_assessment_is_a_hole(self):
        root = self.fixture(judgment=True)
        record = root / "GROUNDING.yaml"
        record.write_text(record.read_text(encoding="utf-8").replace("{ref: p.input}", "{ref: missing.x}"), encoding="utf-8")
        result = self.cli(root, "--run")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("not clean: a hole", result.stdout)

    def test_explicit_utc_day_is_used_by_adapter(self):
        root = self.fixture()
        from scripts import remeasure
        lines, code = remeasure.measure([str(root / "GROUNDING.yaml")], run=True,
                                        today=datetime.date(2026, 9, 17))
        output = "\n".join(lines)
        self.assertEqual(code, 0, output)
        self.assertIn("measured on 2026-09-17 (UTC)", output)


if __name__ == "__main__":
    unittest.main()
