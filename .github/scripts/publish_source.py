#!/usr/bin/env python3
"""Bind a publication dispatch to a successful main-push check and its artifacts.

The workflow runs this reader from its own main revision, before checking out the
release commit. Dispatch inputs are claims: GitHub must independently confirm the
run, workflow, repositories, result and full commit before publication can start.
"""
import argparse
import json
import os
import re
import subprocess


CHECK_WORKFLOW = ".github/workflows/check.yml"


def validate_run(run, workflow, repository, run_id, commit):
    """Common provenance boundary; callers additionally enforce their release policy."""
    expected = {
        "id": int(run_id),
        "status": "completed",
        "head_sha": commit,
    }
    for key, value in expected.items():
        if run.get(key) != value:
            raise SystemExit(f"publication source {key} must be {value!r}, got {run.get(key)!r}")
    # Run paths may include a ref suffix; the workflow endpoint supplies the bare
    # path, and its numeric identity must still match the run independently.
    if (str(run.get("path", "")).split("@", 1)[0] != CHECK_WORKFLOW
            or workflow.get("path") != CHECK_WORKFLOW or type(workflow.get("id")) is not int
            or run.get("workflow_id") != workflow["id"]):
        raise SystemExit("publication source is not the repository's check workflow")
    for key in ("repository", "head_repository"):
        if (run.get(key) or {}).get("full_name") != repository:
            raise SystemExit(f"publication source {key} is not {repository}")
    return {"commit": commit, "run_id": str(run["id"])}


def validate(run, workflow, repository, run_id, commit):
    values = validate_run(run, workflow, repository, run_id, commit)
    for key, value in {"event": "push", "conclusion": "success", "head_branch": "main"}.items():
        if run.get(key) != value:
            raise SystemExit(f"publication source {key} must be {value!r}, got {run.get(key)!r}")
    return values


def api(path):
    return json.loads(subprocess.check_output(["gh", "api", path], text=True))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args(argv)
    if not re.fullmatch(r"[1-9][0-9]*", args.run_id):
        parser.error("run-id must be a positive integer")
    if not re.fullmatch(r"[0-9a-f]{40}", args.commit):
        parser.error("commit must be a full lowercase commit SHA")
    repository = os.environ["GITHUB_REPOSITORY"]
    workflow = api(f"repos/{repository}/actions/workflows/check.yml")
    run = api(f"repos/{repository}/actions/runs/{args.run_id}")
    values = validate(run, workflow, repository, args.run_id, args.commit)
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            for key, value in values.items():
                output.write(f"{key}={value}\n")
    print(json.dumps(values))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
