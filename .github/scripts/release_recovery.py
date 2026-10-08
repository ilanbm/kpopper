#!/usr/bin/env python3
"""Reuse complete platform tests after a Windows-only test-fixture correction.

This deliberately narrow admission path never supplies release artifacts. The current
candidate must still build and acceptance-test every distribution and its own crate.
"""
import argparse
import copy
from dataclasses import dataclass
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tomllib
import xml.etree.ElementTree as ET
import zipfile


class RecoveryError(ValueError):
    pass


TARGETS = {
    "linux-x86_64": "ubuntu-24.04, linux-x86_64, kpop",
    "linux-aarch64": "ubuntu-24.04-arm, linux-aarch64, kpop",
    "darwin-arm64": "macos-14, darwin-arm64, kpop",
    "darwin-x86_64": "macos-15-intel, darwin-x86_64, kpop",
    "windows-x86_64": "windows-2022, windows-x86_64, kpop.exe",
}
WINDOWS = "windows-x86_64"
CONTROL_FILES = {
    ".github/workflows/check.yml", ".github/scripts/release_recovery.py",
    ".github/scripts/ci_selection.py", "tests/test_release_recovery.py",
    "tests/test_ci_release_flow.py", "tests/test_ci_selection.py",
}
MAX_ZIP = 64 * 1024 * 1024
MEMBERS = {"commit.txt": 64, "validation-scope.txt": 16, "junit.xml": 8 * 1024 * 1024}


def require(condition, message):
    if not condition:
        raise RecoveryError(message)


def sha(value):
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}", value)


@dataclass
class Evidence:
    commit: str
    scope: str
    tests: dict[tuple[str, str], str]


def parse_evidence_zip(raw, expected_commit):
    require(sha(expected_commit) and len(raw) <= MAX_ZIP, "invalid evidence identity or ZIP size")
    try:
        with zipfile.ZipFile(io.BytesIO(raw)) as archive:
            entries = archive.infolist()
            names = [entry.filename for entry in entries]
            require(len(entries) <= 64 and len(names) == len(set(names)), "ambiguous archive members")
            for name in names:
                require(not name.startswith("/") and "\\" not in name and ":" not in name
                        and ".." not in PurePosixPath(name).parts, "unsafe archive member")
            content = {}
            for name, limit in MEMBERS.items():
                require(name in names, "missing evidence member: " + name)
                entry = archive.getinfo(name)
                require(0 < entry.file_size <= limit and not entry.flag_bits & 1,
                        "invalid evidence member size or encryption: " + name)
                with archive.open(entry) as stream:
                    content[name] = stream.read(limit + 1)
                require(len(content[name]) == entry.file_size, "evidence member size mismatch")
            # Other diagnostics can be very large. Never decompress or execute them.
        commit = content["commit.txt"].decode("ascii").strip()
        scope = content["validation-scope.txt"].decode("ascii").strip()
        require(commit == expected_commit and scope == "full", "wrong evidence source or validation scope")
        xml = content["junit.xml"].decode("utf-8")
        require(not re.search(r"<!\s*(DOCTYPE|ENTITY)\b", xml, re.I), "XML declarations are forbidden")
        root = ET.fromstring(xml)
        require(root.tag in ("testsuites", "testsuite"), "unexpected JUnit root")
        for suite in root.iter():
            if suite.tag not in ("testsuites", "testsuite"):
                continue
            cases = list(suite.iter("testcase"))
            counts = {"tests": len(cases), "failures": len(list(suite.iter("failure"))),
                      "errors": len(list(suite.iter("error"))), "skipped": len(list(suite.iter("skipped")))}
            for key, expected in counts.items():
                if key in suite.attrib:
                    require(suite.get(key) == str(expected), "JUnit count mismatch: " + key)
        tests = {}
        for case in root.iter("testcase"):
            identity = (case.get("classname", ""), case.get("name", ""))
            require(all(identity) and identity not in tests, "missing or duplicate testcase identity")
            outcomes = [child.tag for child in case if child.tag in ("failure", "error", "skipped")]
            require(len(outcomes) <= 1 and "error" not in outcomes, "ambiguous or errored testcase")
            tests[identity] = {"failure": "fail", "skipped": "skipped"}.get(
                outcomes[0] if outcomes else "", "pass")
        require(tests, "empty JUnit evidence")
        require(len(list(root.iter("failure"))) == sum(v == "fail" for v in tests.values())
                and not list(root.iter("error")), "unattributed JUnit failure")
        return Evidence(commit, scope, tests)
    except (zipfile.BadZipFile, RuntimeError, UnicodeError, ET.ParseError, OSError) as error:
        raise RecoveryError("unreadable evidence archive: " + str(error)) from error


def validate_evidence(baseline, windows):
    require(set(baseline) == set(TARGETS), "baseline must contain all five platforms")
    require(len({item.commit for item in baseline.values()}) == 1, "mixed baseline sources")
    for target, evidence in baseline.items():
        require(sha(evidence.commit) and evidence.scope == "full" and evidence.tests,
                "incomplete platform evidence: " + target)
        allowed = {"pass", "fail"} if target == WINDOWS else {"pass"}
        require(set(evidence.tests.values()) <= allowed, "platform has skipped or failed tests: " + target)
    require(sha(windows.commit) and windows.scope == "full" and windows.tests
            and set(windows.tests.values()) == {"pass"}, "Windows supplement is not fully passing")
    old = baseline[WINDOWS].tests
    require(set(old) <= set(windows.tests), "Windows supplement omitted baseline testcases")
    failures = {identity for identity, outcome in old.items() if outcome == "fail"}
    require(failures, "baseline has no Windows testcase failure to recover")
    return failures


def validate_jobs(jobs, required):
    names = [job.get("name") for job in jobs]
    require(all(names) and len(names) == len(set(names)), "missing or duplicate job names")
    by_name = {job["name"]: job for job in jobs}
    require(set(required) <= set(by_name), "missing required platform jobs")
    for name, job in by_name.items():
        require(job.get("status") == "completed", "incomplete job: " + name)
        allowed = {required[name]} if name in required else {"success", "skipped"}
        require(job.get("conclusion") in allowed, "unexpected job outcome: " + name)


def validate_baseline_jobs(jobs):
    required = {"changes": "success", "record": "success", "native-cli / verdict": "failure",
                "candidate-checked": "failure", "ci-required": "failure"}
    for target, label in TARGETS.items():
        required[f"native-cli / tests ({label})"] = "failure" if target == WINDOWS else "success"
        required[f"native-cli / release ({label})"] = "success"
    validate_jobs(jobs, required)


def validate_windows_jobs(jobs):
    required = {f"tests ({TARGETS[WINDOWS]})": "success",
                f"release ({TARGETS[WINDOWS]})": "success", "verdict": "success"}
    validate_jobs(jobs, required)
    require(not any(job["name"].startswith(("tests (", "release (")) and job["name"] not in required
                    for job in jobs), "supplement must be Windows-only")


def validate_run(run, workflow, repo, expected_path, *, success):
    require(workflow.get("path") == expected_path and workflow.get("id") is not None
            and run.get("workflow_id") == workflow["id"]
            and run.get("path", "").split("@", 1)[0] == expected_path,
            "wrong evidence workflow")
    for field in ("repository", "head_repository"):
        require((run.get(field) or {}).get("full_name") == repo, "foreign evidence repository")
    require(run.get("event") == "workflow_dispatch" and run.get("status") == "completed"
            and run.get("conclusion") == ("success" if success else "failure"),
            "evidence run has wrong event or outcome")
    require(sha(run.get("head_sha")) and re.fullmatch(r"release/[0-9]+\.[0-9]+\.[0-9]+",
                                                     run.get("head_branch", "")),
            "evidence run is not a release source")


def rust_structure(text):
    """Mask literals/comments, preserving offsets; this is not a general Rust parser.

    Only plain, top-level, Windows-gated integration test functions are admitted.
    Unknown/unterminated lexical forms fail closed or cannot match that shape.
    """
    result = list(text)
    i = 0
    while i < len(text):
        start = i
        if text.startswith("//", i):
            i = text.find("\n", i)
            if i < 0:
                i = len(text)
        elif text.startswith("/*", i):
            i += 2
            depth = 1
            while depth and i < len(text):
                if text.startswith("/*", i):
                    depth += 1
                    i += 2
                elif text.startswith("*/", i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
            require(depth == 0, "unterminated Rust comment")
        elif (raw := re.match(r'(?:b|c)?r(\#*)"', text[i:])):
            end = '"' + raw[1]
            closing = text.find(end, i + len(raw[0]))
            require(closing >= 0, "unterminated raw Rust string")
            i = closing + len(end)
        elif text[i] == '"':
            i += 1
            while i < len(text) and text[i] != '"':
                i += 2 if text[i] == "\\" else 1
            require(i < len(text), "unterminated Rust string")
            i += 1
        elif text[i] == "'" and (char := re.match(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\])'", text[i:])):
            i += len(char[0])
        else:
            i += 1
            continue
        result[start:i] = ["\n" if c == "\n" else " " for c in text[start:i]]
    return "".join(result)


def without_windows_bodies(raw, names):
    text = raw.decode("utf-8")
    structure = rust_structure(text)
    depths = []
    stack = []
    for char in structure:
        depths.append(len(stack))
        if char in "({[":
            stack.append(char)
        elif char in ")}]":
            require(stack and stack.pop() == {")": "(", "}": "{", "]": "["}[char],
                    "unbalanced Rust delimiters")
    require(not stack, "unbalanced Rust delimiters")
    spans = []
    for name in sorted(names):
        pattern = (r"(?m)^\s*#\[cfg\(windows\)\]\s*#\[test\]\s*fn\s+"
                   + re.escape(name) + r"\s*\(\s*\)\s*\{")
        matches = list(re.finditer(pattern, structure))
        require(len(matches) == 1, "unsupported Windows test shape: " + name)
        opening = matches[0].end() - 1
        require(depths[opening] == 0, "test is not top-level: " + name)
        closing = next((i for i in range(opening + 1, len(structure))
                        if structure[i] == "}" and depths[i] == 1), None)
        require(closing is not None, "missing test body end")
        spans.append((opening + 1, closing))
    for start, end in sorted(spans, reverse=True):
        text = text[:start] + "/* recovered Windows test body */" + text[end:]
    return text


def validate_nextest(before, after, failures):
    try:
        old = tomllib.loads(before.decode("utf-8"))
        new = tomllib.loads(after.decode("utf-8"))
    except (UnicodeError, tomllib.TOMLDecodeError) as error:
        raise RecoveryError("invalid nextest configuration") from error
    allowed = [{"platform": "cfg(windows)", "threads-required": "num-test-threads",
                "filter": f"binary(={binary}) & test(={name})"} for binary, name in failures]
    new = copy.deepcopy(new)
    old_overrides = old.get("profile", {}).get("ci", {}).get("overrides", [])
    new_overrides = new.get("profile", {}).get("ci", {}).get("overrides", [])
    remaining = list(old_overrides)
    kept = []
    for override in new_overrides:
        if remaining and override == remaining[0]:
            kept.append(remaining.pop(0))
        else:
            require(override in allowed, "nextest recovery may only reserve the failed Windows fixture")
            allowed.remove(override)
    require(not remaining, "pre-existing nextest policies changed")
    if new_overrides != kept:
        if old_overrides:
            new["profile"]["ci"]["overrides"] = kept
        else:
            del new["profile"]["ci"]["overrides"]
    require(new == old, "other nextest settings changed")


def validate_sources(repository, baseline, windows, source, failures):
    require(all(sha(ref) for ref in (baseline, windows, source)), "invalid source commit")
    fixtures = {}
    for classname, name in failures:
        match = re.fullmatch(r"kpopper::([A-Za-z_][A-Za-z_0-9]*)", classname)
        require(match and re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]*", name),
                "only top-level native integration-test failures are recoverable")
        fixtures.setdefault(match[1], set()).add(name)
    require(fixtures, "no recoverable Windows fixture")
    old, supplemental, current = (repository.tree(ref) for ref in (baseline, windows, source))
    controls = CONTROL_FILES | {"CHANGELOG.md"}
    for tree in (old, supplemental):
        for path in set(tree) | set(current):
            if tree.get(path) == current.get(path):
                continue
            require(path in controls or (path in tree and path in current), "added/deleted input: " + path)
            require(all(entry is None or entry[0] == "100644" for entry in (tree.get(path), current.get(path))),
                    "changed file mode: " + path)
            if path in controls:
                continue
            require(tree is old, "native inputs changed since successful Windows check: " + path)
            before, after = repository.read(baseline, path), repository.read(source, path)
            if path == "native/.config/nextest.toml":
                validate_nextest(before, after, {(binary, name) for binary, names in fixtures.items() for name in names})
            else:
                binary = path.removeprefix("native/tests/").removesuffix(".rs")
                require(path == f"native/tests/{binary}.rs" and binary in fixtures,
                        "unvalidated input changed: " + path)
                require(without_windows_bodies(before, fixtures[binary]) == without_windows_bodies(after, fixtures[binary]),
                        "changes outside failed Windows test bodies: " + path)
    # All fixed inputs, including dependencies, resources, modes and workflow that ran
    # the tests, participate in the receipt. Nothing is inferred from the working tree.
    return hashlib.sha256(json.dumps(sorted(current.items()), separators=(",", ":")).encode()).hexdigest()


class GitRepository:
    def tree(self, ref):
        raw = subprocess.check_output(["git", "ls-tree", "-rz", "--full-tree", ref])
        result = {}
        for line in raw.split(b"\0"):
            if line:
                metadata, path = line.split(b"\t", 1)
                mode, kind, blob = metadata.decode().split()
                require(kind == "blob", "unsupported non-file source input")
                result[path.decode()] = (mode, blob)
        return result

    def read(self, ref, path):
        return subprocess.check_output(["git", "show", f"{ref}:{path}"])


def api(path, *, paginated=False):
    command = ["gh", "api"] + (["--paginate", "--slurp"] if paginated else []) + [path]
    return json.loads(subprocess.check_output(command, timeout=120))


def download(path):
    with subprocess.Popen(["gh", "api", path], stdout=subprocess.PIPE) as process:
        raw = process.stdout.read(MAX_ZIP + 1)
        if len(raw) > MAX_ZIP:
            process.kill()
        require(process.wait(timeout=60) == 0 and len(raw) <= MAX_ZIP, "artifact download failed or too large")
    return raw


def evidence_from_run(repo, run, targets):
    pages = api(f"repos/{repo}/actions/runs/{run['id']}/artifacts?per_page=100", paginated=True)
    artifacts = [item for page in pages for item in page["artifacts"]]
    evidence, receipts = {}, []
    for target in targets:
        name = "native-test-evidence-" + target
        matches = [item for item in artifacts if item["name"] == name]
        require(len(matches) == 1, "missing or ambiguous artifact: " + name)
        item = matches[0]
        require(not item.get("expired") and 0 < item.get("size_in_bytes", 0) <= MAX_ZIP
                and item.get("workflow_run", {}).get("head_sha") == run["head_sha"]
                and item["workflow_run"].get("id") == run["id"], "stale or foreign artifact")
        raw = download(f"repos/{repo}/actions/artifacts/{int(item['id'])}/zip")
        digest = "sha256:" + hashlib.sha256(raw).hexdigest()
        require(digest == item.get("digest") and len(raw) == item["size_in_bytes"], "artifact digest or size mismatch")
        evidence[target] = parse_evidence_zip(raw, run["head_sha"])
        receipts.append({"target": target, "artifact_id": item["id"], "digest": digest,
                         "testcases": len(evidence[target].tests)})
    return evidence, receipts


def recover(repo, baseline_id, windows_id, source):
    runs = []
    for run_id, filename, success in ((baseline_id, "check.yml", False), (windows_id, "native-rust.yml", True)):
        run = api(f"repos/{repo}/actions/runs/{run_id}")
        require(str(run.get("id")) == run_id and isinstance(run.get("run_attempt"), int), "wrong run identity")
        workflow = api(f"repos/{repo}/actions/workflows/{filename}")
        validate_run(run, workflow, repo, ".github/workflows/" + filename, success=success)
        pages = api(f"repos/{repo}/actions/runs/{run_id}/jobs?filter=latest&per_page=100", paginated=True)
        jobs = [job for page in pages for job in page["jobs"]]
        (validate_windows_jobs if success else validate_baseline_jobs)(jobs)
        runs.append(run)
    baseline, windows = runs
    subprocess.run(["git", "fetch", "--no-tags", "origin", baseline["head_sha"], windows["head_sha"], source], check=True)
    repository = GitRepository()
    version = repository.read(source, "VERSION").decode().strip()
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version)
            and all(run["head_branch"] == "release/" + version for run in runs), "release version mismatch")
    old, baseline_artifacts = evidence_from_run(repo, baseline, TARGETS)
    fresh, windows_artifacts = evidence_from_run(repo, windows, [WINDOWS])
    failures = validate_evidence(old, fresh[WINDOWS])
    fingerprint = validate_sources(repository, baseline["head_sha"], windows["head_sha"], source, failures)
    # A rerun requested while evidence was being collected invalidates this admission.
    for run in runs:
        latest = api(f"repos/{repo}/actions/runs/{run['id']}")
        require(all(latest.get(key) == run.get(key) for key in ("run_attempt", "head_sha", "status", "conclusion", "updated_at")),
                "evidence run changed during admission")
    def receipt(run, artifacts):
        return {"run_id": run["id"], "attempt": run["run_attempt"], "source": run["head_sha"], "artifacts": artifacts}
    return {"schema": 1, "source": source, "version": version, "repository": repo,
            "input_tree_sha256": fingerprint, "baseline": receipt(baseline, baseline_artifacts),
            "windows": receipt(windows, windows_artifacts), "recovered_failures": sorted(failures),
            "inherited_targets": sorted(set(TARGETS) - {WINDOWS}),
            "current_validation": "fresh distributions, acceptance and crate; full tests inherited as recorded"}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-run", required=True)
    parser.add_argument("--windows-run", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--pr", required=True)
    parser.add_argument("--receipt", required=True)
    args = parser.parse_args(argv)
    try:
        require(all(re.fullmatch(r"[1-9][0-9]*", value) for value in (args.baseline_run, args.windows_run, args.pr))
                and args.baseline_run != args.windows_run and sha(args.source), "invalid recovery inputs")
        require(os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch"
                and os.environ.get("GITHUB_SHA") == args.source, "recovery requires an exact release dispatch")
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        import release_candidate
        pr = release_candidate.fetch_pr(args.pr)
        require(pr["head"]["sha"] == args.source, "release candidate moved")
        result = recover(os.environ["GITHUB_REPOSITORY"], args.baseline_run, args.windows_run, args.source)
        # Recheck current-main and release-PR identity after the network reads.
        require(release_candidate.fetch_pr(args.pr)["head"]["sha"] == args.source, "release candidate moved")
        receipt = json.dumps(result, indent=2) + "\n"
        Path(args.receipt).write_text(receipt)
        print(json.dumps(result, indent=2))
        if os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a") as output:
                output.write("validation=distribution\n")
                output.write("evidence_sha256=" + hashlib.sha256(receipt.encode()).hexdigest() + "\n")
        if os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
                summary.write("\nVerified test reuse: four baseline platforms and the full Windows supplement.\n"
                              f"Baseline run {args.baseline_run}; Windows run {args.windows_run}.\n"
                              "Full tests were inherited, not rerun on this commit. Every distribution and the crate are built fresh.\n")
        return 0
    except RecoveryError as error:
        print("release recovery refused: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
