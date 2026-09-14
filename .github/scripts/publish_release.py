#!/usr/bin/env python3
"""Publish the released version on GitHub.

Runs on every push to main. When the version the version files carry is not the one the
commit before carried, this commit is the release: it is tagged v<version>, and a release
is opened carrying that version's changelog section and the files built here.

Everything the release holds comes from this one commit - the tag, the text and the files -
because the checkout is that commit. Publishing an older version from a later checkout would
attach files built from a tree the tag does not point at, which is the one thing a release
must never do. So a push that moved no version publishes nothing, and a release that failed
is made by rerunning that run, which replays the same commit. A version already published is
left alone, so a rerun that succeeded changes nothing.

    python3 .github/scripts/publish_release.py --dry-run   # print the plan, publish nothing
    python3 .github/scripts/publish_release.py             # tag it and open the release
"""
import json
import pathlib
import re
import shutil
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import release  # noqa: E402 - the version files are read the one way

ROOT = release.ROOT
DIST = ROOT / "dist"
# Only a version heading closes a section, so a line beginning with ## in the text a section
# carries - a fenced example, a quoted heading - stays part of that section.
VERSION_HEADING = re.compile(r"^## (\d+\.\d+\.\d+) ")
NO_RELEASE = re.compile(r"release not found", re.I)


# ── pure parts ───────────────────────────────────────────────────────────────

def tag_for(version):
    return f"v{version}"


def section_for(changelog_text, version):
    """The changelog section for one version - what it lists, without its own heading and
    without the next version's. None when the changelog carries no section for it."""
    lines = changelog_text.splitlines()
    heads = [i for i, line in enumerate(lines) if VERSION_HEADING.match(line)]
    start = next((i for i in heads if VERSION_HEADING.match(lines[i]).group(1) == version), None)
    if start is None:
        return None
    end = next((i for i in heads if i > start), len(lines))
    return "\n".join(lines[start + 1:end]).strip()


def dists_in(paths, version):
    """-> (the wheel and the sdist for `version`, anything else that was lying there). Named
    by what a registry accepts rather than by prefix: a file that merely starts with the
    version is not a distribution of it."""
    mine = sorted(p for p in paths
                  if (p.name.startswith(f"kpopper-{version}-") and p.name.endswith(".whl"))
                  or p.name == f"kpopper-{version}.tar.gz")
    return mine, sorted(p.name for p in paths if p not in mine)


# ── the repository ───────────────────────────────────────────────────────────

def previous_version():
    """The version the commit before this one carried; None where there is no commit before
    it, or where it carried no version."""
    p = subprocess.run(["git", "show", "HEAD^:pyproject.toml"],
                       cwd=ROOT, capture_output=True, text=True)
    if p.returncode:
        return None
    m = release.VERSION_FILES["pyproject.toml"][0].search(p.stdout)
    return m.group(1) if m else None


def published(tag):
    """Whether GitHub already holds a release for this tag. A lookup that fails for any
    other reason stops here: reading it as 'no release' would publish over an answer nobody
    ever got."""
    p = subprocess.run(["gh", "release", "view", tag, "--json", "tagName"],
                       cwd=ROOT, capture_output=True, text=True)
    if p.returncode == 0:
        return True
    if NO_RELEASE.search(p.stderr) or NO_RELEASE.search(p.stdout):
        return False
    raise SystemExit(f"gh release view {tag}\n{(p.stderr or p.stdout).strip()}")


def tag_elsewhere(tag):
    """-> the commit a tag of this name already names, when that is not this one. The release
    would then be made for that commit's tree while its files come from this one."""
    here = release.sh("git", "rev-parse", "HEAD").strip()
    for line in release.sh("git", "ls-remote", "--tags", "origin", f"refs/tags/{tag}").splitlines():
        sha = line.split()[0]
        if sha != here:
            return sha
    return None


def build(version):
    """-> the wheel and the sdist for `version`, built from this commit into an empty
    directory."""
    if DIST.is_symlink():
        raise SystemExit(f"{DIST} is a link; refusing to build through it")
    shutil.rmtree(DIST, ignore_errors=True)
    if DIST.exists():
        raise SystemExit(f"{DIST} could not be emptied")
    release.sh(sys.executable, "-m", "build", "--outdir", str(DIST))
    mine, stray = dists_in(sorted(DIST.iterdir()), version)
    if stray:
        raise SystemExit(f"the build left files this release cannot name: {stray}")
    kinds = {p.name.split(f"kpopper-{version}")[1] for p in mine}
    if not any(k.endswith(".whl") for k in kinds) or ".tar.gz" not in kinds:
        raise SystemExit(f"the build produced {[p.name for p in mine]}; a release carries "
                         f"both a wheel and an sdist")
    return mine


def main(argv):
    dry = "--dry-run" in argv
    found = release.versions_in(release.read_texts())
    if len(set(found.values())) != 1:
        raise SystemExit("the version files disagree: " + json.dumps(found))
    version = found["pyproject.toml"]
    before = previous_version()
    if before == version:
        print(f"this commit did not move the version ({version}); nothing to publish")
        return 0
    tag = tag_for(version)
    if published(tag):
        print(f"{tag} is already published; nothing to do")
        return 0
    elsewhere = tag_elsewhere(tag)
    if elsewhere:
        raise SystemExit(f"{tag} already names {elsewhere[:9]}, and this is a different "
                         f"commit; a release made now would carry files from neither")
    notes = section_for((ROOT / "CHANGELOG.md").read_text(encoding="utf-8"), version)
    if notes is None:
        raise SystemExit(f"CHANGELOG.md carries no section for {version}")
    print(f"publishing {tag} ({before or 'nothing'} -> {version})\n\n{notes}\n")
    if dry:
        return 0
    files = build(version)
    print("attaching " + ", ".join(p.name for p in files))
    release.sh("gh", "release", "create", tag, *[str(p) for p in files],
               "--title", f"Release {version}", "--notes", notes)
    print(f"published {tag}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
