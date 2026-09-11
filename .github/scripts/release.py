#!/usr/bin/env python3
"""Keep the release pull request current.

Runs on every push to main. Finds the last release - the earliest commit on main whose
pyproject.toml carries the current version - reads every pull request merged since it,
takes the largest bump they declared (a `Bump: patch | minor | major` line in the pull
request body), and keeps one branch, release/<next>, holding the version files and the
changelog entry, with the pull request "Release <next>" open for a person to merge.
Nothing merges by itself, and a feature pull request never touches the version.

    python3 .github/scripts/release.py --dry-run     # print the plan, write nothing
    python3 .github/scripts/release.py               # update the branch and the pull request
"""
import datetime
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
# One version number for every channel a user can install from: the Python package, the
# npm package, and the plugin as the marketplace lists it. A channel left out here is a
# channel that silently stops moving.
VERSION_FILES = {
    "pyproject.toml": (re.compile(r'^version = "(\d+\.\d+\.\d+)"', re.M), 'version = "{v}"'),
    "package.json": (re.compile(r'"version": "(\d+\.\d+\.\d+)"'), '"version": "{v}"'),
    ".claude-plugin/plugin.json": (re.compile(r'"version": "(\d+\.\d+\.\d+)"'), '"version": "{v}"'),
    ".codex-plugin/plugin.json": (re.compile(r'"version": "(\d+\.\d+\.\d+)"'), '"version": "{v}"'),
    ".claude-plugin/marketplace.json": (re.compile(r'"version": "(\d+\.\d+\.\d+)"'), '"version": "{v}"'),
}
BUMPS = ("patch", "minor", "major")
BUMP_LINE = re.compile(r"^\s*bump\s*:\s*(patch|minor|major)\b", re.I | re.M)
PR_IN_SUBJECT = re.compile(r"\(#(\d+)\)\s*$")
DECISION_ADDED = re.compile(r"^\+  (d\.[A-Za-z0-9_]+):", re.M)


def sh(*args, check=True, cwd=None):
    p = subprocess.run(list(args), cwd=cwd or ROOT, capture_output=True, text=True)
    if check and p.returncode:
        raise SystemExit(f"{' '.join(args)}\n{p.stderr.strip()}")
    return p.stdout


# ── pure parts ───────────────────────────────────────────────────────────────

def bump_of(text):
    """The bump a pull request declared, or None."""
    m = BUMP_LINE.search(text or "")
    return m.group(1).lower() if m else None


def largest(bumps):
    """The largest of the declared bumps; a merge that declared none counts as a patch."""
    ranked = [BUMPS.index(b) for b in bumps if b in BUMPS]
    return BUMPS[max(ranked)] if ranked else "patch"


def next_version(version, bump):
    major, minor, patch = (int(x) for x in version.split("."))
    if bump == "major":
        return f"{major + 1}.0.0"
    if bump == "minor":
        return f"{major}.{minor + 1}.0"
    return f"{major}.{minor}.{patch + 1}"


def versions_in(texts):
    """-> {file: version} read from each file's text; a file with no version is an error."""
    out = {}
    for name, text in texts.items():
        pat, _ = VERSION_FILES[name]
        m = pat.search(text)
        if not m:
            raise SystemExit(f"{name}: no version found")
        out[name] = m.group(1)
    return out


def with_version(texts, version):
    """-> the same texts with every version line moved to `version`."""
    out = {}
    for name, text in texts.items():
        pat, fmt = VERSION_FILES[name]
        out[name] = pat.sub(fmt.format(v=version), text, count=1)
    return out


def changelog_entry(version, date, merged, decisions):
    """One changelog section: the pull requests since the last release with the bump each
    declared, and the decisions the record gained."""
    lines = [f"## {version} — {date}", ""]
    for m in merged:
        ref = f" (#{m['pr']})" if m.get("pr") else ""
        bump = m.get("bump") or "no bump declared"
        lines.append(f"- {m['subject']}{ref} — {bump}")
    if decisions:
        lines += ["", "Decisions recorded: " + ", ".join(decisions)]
    return "\n".join(lines) + "\n"


def prepend(changelog_text, entry):
    head = "# Changelog\n\n"
    body = changelog_text[len(head):] if changelog_text.startswith(head) else changelog_text
    return head + entry + ("\n" + body if body.strip() else "")


def pr_body(version, previous, bump, merged, decisions):
    lines = ["## What changed", "",
             f"The version moves from {previous} to **{version}** ({bump}).", "",
             f"## Merged since {previous}", ""]
    for m in merged:
        ref = f"#{m['pr']} " if m.get("pr") else ""
        lines.append(f"- {ref}{m['subject']} — {m.get('bump') or 'no bump declared'}")
    if decisions:
        lines += ["", "## Decisions the record gained", ""] + [f"- `{d}`" for d in decisions]
    lines += ["", "---", "",
              "Merging this is the release. After it lands, an installed plugin picks it up with "
              "`claude plugin update kpopper@kpopper`. This pull request is refreshed on every "
              "push to main until it merges; it never merges by itself.", "",
              "Bump: none — this is the release"]
    return "\n".join(lines) + "\n"


# ── the repository ───────────────────────────────────────────────────────────

def read_texts():
    return {name: (ROOT / name).read_text(encoding="utf-8") for name in VERSION_FILES}


def release_commit(version):
    out = sh("git", "log", "--reverse", "--format=%H", f'-Sversion = "{version}"',
             "--", "pyproject.toml")
    return out.split()[0] if out.split() else None


def merged_since(rev):
    """Commits on main after the last release: subject, body, pull request number."""
    raw = sh("git", "log", f"{rev}..HEAD", "--format=%H%x00%s%x00%b%x1e")
    out = []
    for chunk in raw.split("\x1e"):
        if not chunk.strip():
            continue
        sha, subject, body = (chunk.strip("\n").split("\x00") + ["", ""])[:3]
        m = PR_IN_SUBJECT.search(subject)
        out.append({"sha": sha, "subject": PR_IN_SUBJECT.sub("", subject).strip(),
                    "body": body, "pr": int(m.group(1)) if m else None})
    return out


def declared_bump(m):
    """From the pull request body when the number is known, else from the commit body."""
    if m.get("pr"):
        p = subprocess.run(["gh", "pr", "view", str(m["pr"]), "--json", "body", "-q", ".body"],
                           cwd=ROOT, capture_output=True, text=True)
        if p.returncode == 0 and bump_of(p.stdout):
            return bump_of(p.stdout)
    return bump_of(m.get("body", ""))


def decisions_added(rev):
    # The record's entry file is GROUNDING.yaml; a release spanning its rename from
    # PROVENANCE.yaml reads the diff across both names, so the rename adds no decisions.
    diff = sh("git", "diff", "-M", f"{rev}..HEAD", "--", "GROUNDING.yaml", "PROVENANCE.yaml")
    seen, out = set(), []
    for d in DECISION_ADDED.findall(diff):
        if d not in seen:
            seen.add(d)
            out.append(d)
    return out


def main(argv):
    dry = "--dry-run" in argv
    texts = read_texts()
    found = versions_in(texts)
    if len(set(found.values())) != 1:
        raise SystemExit("the version files disagree: " + json.dumps(found))
    current = found["pyproject.toml"]
    rel = release_commit(current)
    if not rel:
        raise SystemExit(f"no commit carries version {current} in pyproject.toml")
    merged = merged_since(rel)
    if not merged:
        print(f"nothing merged since {current}; nothing to release")
        return 0
    for m in merged:
        m["bump"] = declared_bump(m)
    bump = largest(m["bump"] for m in merged)
    nxt = next_version(current, bump)
    decisions = decisions_added(rel)
    today = datetime.date.today().isoformat()
    entry = changelog_entry(nxt, today, merged, decisions)
    body = pr_body(nxt, current, bump, merged, decisions)
    print(f"{len(merged)} merged since {current}; largest bump {bump} -> {nxt}")
    print(entry)
    if dry:
        return 0

    branch = f"release/{nxt}"
    sh("git", "config", "user.name", "github-actions[bot]")
    sh("git", "config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com")
    sh("git", "switch", "-C", branch, "origin/main")
    for name, text in with_version(texts, nxt).items():
        (ROOT / name).write_text(text, encoding="utf-8")
    log = ROOT / "CHANGELOG.md"
    log.write_text(prepend(log.read_text(encoding="utf-8") if log.exists() else "", entry),
                   encoding="utf-8")
    # the release pull request is opened by a token whose pull requests run no checks,
    # so the record's own checks run here, on the tree the release would ship
    sh("kpopper", "check")
    sh("kpopper", "page", "--verify")
    sh("git", "add", "CHANGELOG.md", *VERSION_FILES)
    sh("git", "commit", "-q", "-m", f"Release {nxt}")
    sh("git", "push", "-f", "origin", branch)
    # one release branch at a time: an older one for a version that is no longer next goes
    for line in sh("git", "ls-remote", "--heads", "origin", "release/*").splitlines():
        ref = line.split()[-1]
        if ref != f"refs/heads/{branch}":
            sh("git", "push", "origin", "--delete", ref.replace("refs/heads/", ""), check=False)
    body_file = ROOT / ".release-body.md"
    body_file.write_text(body, encoding="utf-8")
    existing = subprocess.run(["gh", "pr", "list", "--head", branch, "--state", "open",
                               "--json", "number", "-q", ".[0].number"],
                              cwd=ROOT, capture_output=True, text=True).stdout.strip()
    if existing:
        sh("gh", "pr", "edit", existing, "--title", f"Release {nxt}", "--body-file", str(body_file))
        print(f"refreshed pull request #{existing}")
    else:
        p = subprocess.run(["gh", "pr", "create", "--head", branch, "--base", "main",
                            "--title", f"Release {nxt}", "--body-file", str(body_file)],
                           cwd=ROOT, capture_output=True, text=True)
        if p.returncode:
            print(f"the branch {branch} is pushed, but the pull request could not be opened:\n"
                  f"{p.stderr.strip()}\n\nEither allow GitHub Actions to create pull requests "
                  f"(repository Settings → Actions → General → Workflow permissions), or open "
                  f"it once by hand:\n  gh pr create --head {branch} --base main "
                  f"--title 'Release {nxt}' --body-file .release-body.md", file=sys.stderr)
            return 1
        print(p.stdout.strip())
    body_file.unlink(missing_ok=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
