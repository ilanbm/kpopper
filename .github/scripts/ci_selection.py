"""Choose CI families conservatively; unknown paths or unavailable history run all."""
import argparse
import json
import os
from pathlib import Path
import subprocess

LANES = ("python", "documents", "session", "installed", "native")
TEST_SUITES = ("core", "documents", "reasoning", "session", "other")
JOB_LANES = {"check": ("python", "documents"), "document-ui": ("documents",),
             "session": ("session",), "reasoning-runtime": ("installed",)}
RUNTIME = set(LANES) - {"native"}


def families(path):
    # Community metadata does not change runtime or build inputs. Keep this list
    # narrow: scripts, workflows and unknown files still take their checks below.
    if (path in {"SECURITY.md", "CODE_OF_CONDUCT.md", ".gitignore",
                 ".github/pull_request_template.md"}
            or (Path(path).parent.as_posix() == ".github/ISSUE_TEMPLATE"
                and Path(path).suffix in {".md", ".yml", ".yaml"})):
        return set()
    # The mandatory record job executes selector regressions and workflow-contract
    # tests on every PR, including changes to this selector and its test module.
    if path in {".github/scripts/ci_selection.py", "tests/test_ci_selection.py"}:
        return set()
    # These workflows never compile the reasoning runtime. Changes still exercise
    # all consumers; the native workflow itself takes the full audit.
    if path in {".github/workflows/check.yml", ".github/workflows/session.yml",
                ".github/scripts/ci_execution.py", ".github/requirements-test.txt",
                ".github/test-durations.json",
                "tests/test_ci_execution.py",
                "skills/watch/agents/openai.yaml"}:
        return RUNTIME
    # These inputs can change compilation, its audit, or modified GMP loading.
    if (path.startswith(("scripts/reasoning/lean/", "scripts/reasoning/native/",
                         "scripts/reasoning/third_party/", ".github/"))
            or path in {"scripts/reasoning/build_runtime.py", "scripts/reasoning/runtime.py",
                        "tests/test_reasoning_runtime.py", "tests/test_reasoning_distribution.py",
                        "tests/test_reasoning_composition_kernel.py",
                        "tests/test_reasoning_composition_review.py",
                        "tests/test_reasoning_composition_acceptance.py",
                        "tests/test_core_composition.py"}):
        return set(LANES)
    # The record and skill contracts run on every PR, independent of these flags.
    if (path in {"GROUNDING.yaml", "README.md", "CONTRIBUTING.md", "CHANGELOG.md", "tests/test_skills.py"}
            or (path.startswith(("skills/", "docs/", "examples/")) and path.endswith(".md"))
            or (path.startswith(".kpopper/") and path.endswith((".yaml", ".json")))
            or (path.startswith("assets/") and path.endswith((".png", ".jpg", ".svg", ".webp")))):
        return set()
    if (path.startswith(("scripts/document/", "tests/document-support/"))
            or path in {"scripts/documents.py", "scripts/document_cli.py", "scripts/document_html.py",
                        "scripts/document-guide.md", "tests/document_ui_fixture.py", "tests/test_document_ui.cjs"}
            or (path.startswith("tests/test_document") and path.endswith(".py"))):
        return {"documents"}
    # Shared Python code and fixtures can reach every consumer. Keep this broad;
    # this selector is not an inferred Python dependency graph.
    if (path.startswith(("scripts/", "tests/", "adapters/", "hooks/", ".claude-plugin/", ".codex-plugin/"))
            or path in {"pyproject.toml", "package.json", "LICENSE"}):
        return RUNTIME
    return set(LANES)


def select(paths, full=False, push=False):
    chosen = set(LANES) if full or not paths else set().union(*(families(p) for p in paths))
    if push:
        # Main checks all consumers; compiling unchanged native source adds no coverage.
        chosen |= RUNTIME
    return {lane: lane in chosen for lane in LANES}


def test_suites(selected):
    # Shared readers can reach every consumer. Keep their full coverage while
    # giving the runner concrete, disjoint suites to execute and report.
    if selected["python"]:
        return list(TEST_SUITES)
    return ["documents"] if selected["documents"] else []


def test_matrix(selected, pull_request=True):
    if not (selected["python"] or selected["documents"]):
        return {"include": []}
    rows = []
    for python in (("3.9", "3.13") if pull_request else ("3.13",)):
        # The slower interpreter gets more machines and CPU headroom for the
        # subprocess-heavy history tests. Document-only changes need one group.
        splits = (8 if python == "3.9" else 4) if selected["python"] else 1
        workers = 2 if python == "3.9" else 4
        rows.extend({"python": python, "group": group, "splits": splits, "workers": workers}
                    for group in range(1, splits + 1))
    return {"include": rows}


def changed_files(base, head, cwd=None, merge_base=True):
    try:
        if merge_base:
            base = subprocess.check_output(["git", "merge-base", base, head], cwd=cwd,
                                           stderr=subprocess.PIPE).decode().strip()
        raw = subprocess.check_output(["git", "diff", "--name-only", "--no-renames", "-z", base, head, "--"],
                                      cwd=cwd, stderr=subprocess.PIPE)
        # --no-renames includes the old AND new name; -z preserves unusual filenames.
        return [p.decode("utf-8", errors="surrogateescape") for p in raw.split(b"\0") if p]
    except (OSError, subprocess.CalledProcessError):
        print("Change history unavailable; selecting every CI family.")
        return []


def required_failures(needs, pull_request=True):
    failures = []
    for job in ("changes", "record"):
        if needs.get(job, {}).get("result") != "success":
            failures.append(job + " must succeed")
    outputs = needs.get("changes", {}).get("outputs", {})
    for lane in LANES:
        if outputs.get(lane) not in ("true", "false"):
            failures.append("missing or invalid selection: " + lane)
    try:
        suites = json.loads(outputs.get("test_suites", "null"))
        expected = test_suites({lane: outputs.get(lane) == "true" for lane in LANES})
        if suites != expected:
            failures.append("test plan does not cover the selected CI families")
    except (TypeError, ValueError):
        failures.append("missing or invalid test plan")
    try:
        matrix = json.loads(outputs.get("test_matrix", "null"))
        selected = {lane: outputs.get(lane) == "true" for lane in LANES}
        if matrix != test_matrix(selected, pull_request):
            failures.append("test matrix omits a required Python version or shard")
    except (TypeError, ValueError):
        failures.append("missing or invalid test matrix")
    if outputs.get("native") == "true" and outputs.get("installed") != "true":
        failures.append("native audit requires installed checks")
    for job, lanes in JOB_LANES.items():
        expected = "success" if any(outputs.get(lane) == "true" for lane in lanes) else "skipped"
        actual = needs.get(job, {}).get("result")
        if actual != expected:
            failures.append("%s: expected %s, got %s" % (job, expected, actual))
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--push", action="store_true")
    parser.add_argument("--full", action="store_true")
    parser.add_argument("--required", action="store_true")
    args = parser.parse_args()
    if args.required:
        failures = required_failures(json.loads(os.environ["CI_NEEDS"]),
                                     os.environ.get("GITHUB_EVENT_NAME") == "pull_request")
        for failure in failures:
            print(failure)
        return 1 if failures else 0
    paths = [] if args.full else changed_files(args.base, args.head, merge_base=not args.push)
    selected = select(paths, full=args.full, push=args.push)
    suites = test_suites(selected)
    matrix = test_matrix(selected, pull_request=not (args.push or args.full))
    print(json.dumps({"changed_files": paths, "selected": selected,
                      "test_suites": suites, "test_matrix": matrix}, indent=2))
    if os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as stream:
            for lane, enabled in selected.items():
                stream.write("%s=%s\n" % (lane, str(enabled).lower()))
            stream.write("test_suites=" + json.dumps(suites) + "\n")
            stream.write("test_matrix=" + json.dumps(matrix) + "\n")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as stream:
            stream.write("| CI family | Selected |\n|---|---|\n")
            for lane, enabled in selected.items():
                stream.write("| %s | %s |\n" % (lane, "yes" if enabled else "no"))
            stream.write("\nRecord, skill/release contracts and CI selection tests always run.\n")
            stream.write("\nPython test suites: " + (", ".join(suites) or "none") + ".\n")
            stream.write("Python test jobs: " + str(len(matrix["include"])) + ".\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
