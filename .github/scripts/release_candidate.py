#!/usr/bin/env python3
"""Bind release checks, publication and the merge gate to one immutable candidate."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release
import publish_source

ROOT = release.ROOT
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")


def git(*args):
    return release.sh("git", *args, cwd=ROOT).strip()


def api(path, method="GET"):
    return json.loads(release.sh("gh", "api", "--method", method, path))


def repository():
    return os.environ["GITHUB_REPOSITORY"]


def version_at(ref):
    value = git("show", f"{ref}:VERSION")
    if not VERSION.fullmatch(value):
        raise SystemExit("invalid native version")
    return value


def validate_candidate(head, base, pr, repo):
    """Only a single version/changelog commit on the current main can be released."""
    version = version_at(head)
    if (pr.get("state") != "open" or pr.get("draft") or
            pr["base"]["ref"] != "main" or pr["base"]["repo"]["full_name"] != repo or
            pr["head"]["repo"]["full_name"] != repo or
            pr["head"]["ref"] != f"release/{version}" or pr["head"]["sha"] != head or
            pr["base"]["sha"] != base):
        raise SystemExit("not the current same-repository release pull request")
    if git("rev-list", "--parents", "-n", "1", head).split() != [head, base]:
        raise SystemExit("release candidate must be one commit on the current main; refresh its checks")
    if tuple(map(int, version.split("."))) <= tuple(map(int, version_at(base).split("."))):
        raise SystemExit("release version must increase")
    changed = set(git("diff", "--name-only", base, head).splitlines())
    if changed != set(release.VERSION_FILES) | {"CHANGELOG.md"}:
        raise SystemExit("release candidate must change only every version file and CHANGELOG.md")
    for name in release.VERSION_FILES:
        before, after = (git("show", f"{ref}:{name}") for ref in (base, head))
        if release.versions_in({name: after})[name] != version:
            raise SystemExit("release version files disagree")
        if release.with_version({name: before}, "0.0.0") != release.with_version({name: after}, "0.0.0"):
            raise SystemExit("release candidate changes more than the version: " + name)
    # A version-only diff must not smuggle executable files through mode changes.
    for line in git("diff", "--raw", base, head).splitlines():
        modes = line.split()[:2]
        if modes != [":100644", "100644"]:
            raise SystemExit("release candidate changes file modes")
    return version


def fetch_pr(number):
    if not str(number).isdigit() or int(number) < 1:
        raise SystemExit("invalid pull request number")
    pr = api(f"repos/{repository()}/pulls/{number}")
    head, base = pr["head"]["sha"], pr["base"]["sha"]
    git("fetch", "--no-tags", "origin", head, base)
    validate_candidate(head, base, pr, repository())
    return pr


def previous_published(version):
    """The prior public release, not the small version-bump PR diff or an unshipped tag."""
    releases = json.loads(release.sh("gh", "api", "--paginate", "--slurp",
                                    f"repos/{repository()}/releases?per_page=100"))
    candidates = []
    for page in releases:
        for item in page:
            name = item["tag_name"].removeprefix("v")
            if not item["draft"] and not item["prerelease"] and VERSION.fullmatch(name):
                if tuple(map(int, name.split("."))) < tuple(map(int, version.split("."))):
                    candidates.append((tuple(map(int, name.split("."))), item["tag_name"]))
    if not candidates:
        return ""  # The selector's missing-history path runs everything.
    tag = max(candidates)[1]
    git("fetch", "--no-tags", "origin", f"refs/tags/{tag}")
    return git("rev-parse", "FETCH_HEAD^{commit}")


def select(event, event_name):
    head = os.environ["GITHUB_SHA"]
    result = {"release": "false", "promotion": "false", "source": head, "baseline": ""}
    if event_name == "push":
        result["promotion"] = str(version_at(head) != version_at(head + "^")).lower()
        return result
    number = event.get("inputs", {}).get("release_pr")
    if event_name == "pull_request":
        pr = event["pull_request"]
        if version_at(pr["head"]["sha"]) == version_at(pr["base"]["sha"]):
            return result
        number = pr["number"]
    if number:
        pr = fetch_pr(number)
        head = pr["head"]["sha"]
        expected = event["pull_request"]["head"]["sha"] if event_name == "pull_request" else os.environ["GITHUB_SHA"]
        if head != expected:
            raise SystemExit("candidate moved after this check run was requested")
        result.update(release="true", source=head, baseline=previous_published(version_at(head)))
    return result


def checked_run(run, jobs, pr, workflow, run_id):
    """A pending publication gate may be red; all actual candidate checks must be green."""
    head = pr["head"]["sha"]
    publish_source.validate_run(run, workflow, repository(), run_id, head)
    if run["event"] not in ("pull_request", "workflow_dispatch"):
        raise SystemExit("not a completed candidate check run in this repository")
    # Dispatch runs use the release branch so the API head and all artifact identities agree.
    if run["head_sha"] != head or run["head_branch"] != pr["head"]["ref"]:
        raise SystemExit("candidate checks are stale or belong to another branch")
    by_name = {job["name"]: job for job in jobs}
    for name in ("changes", "record", "native-cli / verdict", "candidate-checked"):
        if by_name.get(name, {}).get("conclusion") != "success":
            raise SystemExit("candidate check did not succeed: " + name)
    for job in jobs:
        if job["name"] != "ci-required" and job["conclusion"] not in ("success", "skipped"):
            raise SystemExit("candidate contains failed or unfinished checks")
    if "ci-required" not in by_name:
        raise SystemExit("candidate merge gate is missing")
    return by_name["ci-required"]["id"]


def require_strict_gate():
    rules = api(f"repos/{repository()}/rules/branches/main")
    if not any(rule["type"] == "required_status_checks" and
               rule["parameters"].get("strict_required_status_checks_policy") and
               any(check["context"] == "ci-required" for check in
                   rule["parameters"]["required_status_checks"]) for rule in rules):
        raise SystemExit("main must require ci-required with up-to-date branches before publishing")


def publication_plan(number, run_id):
    pr = fetch_pr(number)
    require_strict_gate()
    workflow = api(f"repos/{repository()}/actions/workflows/check.yml")
    run = api(f"repos/{repository()}/actions/runs/{int(run_id)}")
    pages = json.loads(release.sh("gh", "api", "--paginate", "--slurp",
                                f"repos/{repository()}/actions/runs/{int(run_id)}/jobs?filter=latest&per_page=100"))
    gate = checked_run(run, [job for page in pages for job in page["jobs"]], pr, workflow, run_id)
    return {"version": version_at(pr["head"]["sha"]), "source": pr["head"]["sha"],
            "base": pr["base"]["sha"], "run_id": str(run["id"]), "gate_job": str(gate)}


def verify_available(source, version):
    info = api(f"repos/{repository()}/releases/tags/v{version}")
    if info["draft"] or info["prerelease"]:
        raise SystemExit("native release is not public")
    git("fetch", "--no-tags", "origin", f"refs/tags/v{version}")
    if git("rev-parse", "FETCH_HEAD^{commit}") != source:
        raise SystemExit("release tag does not name the tested candidate")
    import native_assets
    expected = {f"kpopper-{version}-{target}." + ("zip" if target.startswith("windows") else "tar.gz")
                for target in native_assets.TARGETS} | {"install.sh", "install.ps1", "SHA256SUMS"}
    assets = info["assets"]
    if {asset["name"] for asset in assets} != expected:
        raise SystemExit("native release is missing platform downloads")
    for asset in assets:
        if asset.get("size", 0) <= 0 or not re.fullmatch(r"sha256:[0-9a-f]{64}", asset.get("digest") or ""):
            raise SystemExit("native release has an unverified asset")
        # Anonymous public URLs must work, not merely authenticated release metadata.
        subprocess.run(["curl", "--fail", "--silent", "--show-error", "--location", "--head",
                        "--retry", "3", "--max-time", "60", asset["browser_download_url"]],
                       check=True, stdout=subprocess.DEVNULL)


def verify_pr(number):
    pr = fetch_pr(number)
    verify_available(pr["head"]["sha"], version_at(pr["head"]["sha"]))


def verify_main(head):
    version = version_at(head)
    git("fetch", "--no-tags", "origin", f"refs/tags/v{version}")
    source = git("rev-parse", "FETCH_HEAD^{commit}")
    if git("rev-parse", source + "^{tree}") != git("rev-parse", head + "^{tree}"):
        raise SystemExit("main differs from the already published candidate; refusing to reuse its checks")
    verify_available(source, version)


def emit(values):
    print(json.dumps(values))
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            for key, value in values.items():
                output.write(f"{key}={value}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--select", action="store_true")
    parser.add_argument("--plan", action="store_true")
    parser.add_argument("--verify-pr", action="store_true")
    parser.add_argument("--verify-main", action="store_true")
    parser.add_argument("--pr", type=int)
    parser.add_argument("--run-id", type=int)
    args = parser.parse_args()
    if args.select:
        emit(select(json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text()), os.environ["GITHUB_EVENT_NAME"]))
    elif args.plan:
        emit(publication_plan(args.pr, args.run_id))
    elif args.verify_pr:
        verify_pr(args.pr)
    elif args.verify_main:
        verify_main(os.environ["GITHUB_SHA"])
    else:
        parser.error("choose an operation")


if __name__ == "__main__":
    main()
