"""Run the research example against the installed package and check its published output."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[2]
EXAMPLE = ROOT / "examples/dark-matter/advanced"


def projection(report):
    result = subprocess.run(
        ["jq", "-e", "-f", str(EXAMPLE / "assessment-summary.jq")],
        input=json.dumps(report), text=True, stdout=subprocess.PIPE,
        check=True, timeout=30,
    )
    return json.loads(result.stdout)


def main():
    with tempfile.TemporaryDirectory(prefix="kpopper-ci-research-") as directory:
        output = Path(directory) / "example"
        result = subprocess.run(
            [sys.executable, str(EXAMPLE / "exercise.py"), "--output", str(output)],
            cwd=ROOT, text=True, stdout=subprocess.PIPE, check=True, timeout=600,
        )
        actual = json.loads(result.stdout)
        expected = json.loads((EXAMPLE / "captured-output.json").read_text(encoding="utf-8"))
        if actual != expected:
            raise AssertionError("Research output differs from captured-output.json")

        report = json.loads((output / "captures/after.assess.stdout").read_text(encoding="utf-8"))
        shown = projection(report)
        if shown != {
            "studies_scanned": 6,
            "astronomy_studies_selected": 5,
            "particle_identity_status": "unknown",
            "review_basis": "changed",
        }:
            raise AssertionError("Unexpected CLI projection: " + json.dumps(shown))

        # Four definite matches cannot be shown as a complete count when the
        # evaluator reports an unknown or failed result.
        for status in ("unknown", "error", "unsupported_capability", None):
            incomplete = copy.deepcopy(report)
            computation = incomplete["nodes"]["m.astronomy_count"]["computation"]
            computation["query_counts"]["definite_match_count"] = 4
            computation["value"] = None
            if status is None:
                computation.pop("status")
            else:
                computation["status"] = status
            if projection(incomplete)["astronomy_studies_selected"] != (status or "unavailable"):
                raise AssertionError("CLI projection concealed an incomplete count")

        print(json.dumps({
            "captured_summary_matches": True,
            "cli_projection": shown,
            "incomplete_count_statuses_preserved": True,
        }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
