#!/usr/bin/env python3
"""Publish the source-bound native release on GitHub.

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
import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import release  # noqa: E402 - the version files are read the one way

ROOT = release.ROOT
DIST = ROOT / "dist"
ARTIFACTS = pathlib.Path(os.environ.get("KPOPPER_NATIVE_ARTIFACTS", ROOT / "native-dist"))
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


# ── the repository ───────────────────────────────────────────────────────────

def previous_version():
    """The version the commit before this one carried; None where there is no commit before
    it, or where it carried no version."""
    p = subprocess.run(["git", "show", "HEAD^:VERSION"],
                       cwd=ROOT, capture_output=True, text=True)
    if p.returncode:
        return None
    m = release.VERSION_FILES["VERSION"][0].search(p.stdout)
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
    """Validate all five native artifacts before staging publishable output."""
    import native_assets
    commit = release.sh("git", "rev-parse", "HEAD").strip()
    return native_assets.prepare(version, commit, ARTIFACTS, DIST, ROOT)


def release_info(tag):
    repository = os.environ.get("GITHUB_REPOSITORY") or release.sh(
        "gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner").strip()
    # The REST tag endpoint only finds published releases. The CLI also resolves
    # drafts, whose database ID works before and after publication.
    identity = json.loads(release.sh("gh", "release", "view", tag, "--repo", repository,
                                     "--json", "databaseId"))
    release_id = identity.get("databaseId")
    if type(release_id) is not int or release_id <= 0:
        raise SystemExit("GitHub did not identify the release database ID")
    value = json.loads(release.sh("gh", "api", f"repos/{repository}/releases/{release_id}"))
    if value.get("tag_name") != tag:
        raise SystemExit("GitHub release identity changed during lookup")
    return value


def verify_published(tag, files):
    """GitHub's asset digests must match the exact bytes supplied by this run."""
    value = release_info(tag)
    assets = {item["name"]: item for item in value.get("assets", [])}
    expected_names = {path.name for path in files}
    if set(assets) != expected_names:
        raise SystemExit("published release asset names differ from the verified bundle")
    for path in files:
        digest = hashlib.sha256()
        with path.open("rb") as source:
            for block in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(block)
        asset = assets[path.name]
        if asset.get("size") != path.stat().st_size or asset.get("digest") != "sha256:" + digest.hexdigest():
            raise SystemExit("published release asset digest is missing or mismatched: " + path.name)


def main(argv):
    global ARTIFACTS
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--plan", action="store_true", help="emit source-only JSON planning data")
    parser.add_argument("--verify", action="store_true", help="validate and stage assets without publishing")
    parser.add_argument("--artifacts", type=pathlib.Path)
    options = parser.parse_args(argv)
    if options.artifacts is not None:
        ARTIFACTS = options.artifacts
    found = release.versions_in(release.read_texts())
    if len(set(found.values())) != 1:
        raise SystemExit("the version files disagree: " + json.dumps(found))
    version = found["VERSION"]
    before = previous_version()
    target_commit = release.sh("git", "rev-parse", "HEAD").strip()
    if options.plan:
        print(json.dumps({"publish": before != version, "version": version, "commit": target_commit}))
        return 0
    if before == version:
        print(f"this commit did not move the version ({version}); nothing to publish")
        return 0
    release.sh("git", "diff", "--quiet", "HEAD", "--")
    tag = tag_for(version)
    exists = published(tag)
    elsewhere = tag_elsewhere(tag)
    if elsewhere:
        raise SystemExit(f"{tag} already names {elsewhere[:9]}, and this is a different "
                         f"commit; a release made now would carry files from neither")
    notes = section_for((ROOT / "CHANGELOG.md").read_text(encoding="utf-8"), version)
    if notes is None:
        raise SystemExit(f"CHANGELOG.md carries no section for {version}")
    print(f"publishing {tag} ({before or 'nothing'} -> {version})\n\n{notes}\n")
    if options.dry_run:
        return 0
    files = build(version)
    if release.sh("git", "rev-parse", "HEAD").strip() != target_commit:
        raise SystemExit("the checkout changed while building the release; nothing was published")
    release.sh("git", "diff", "--quiet", "HEAD", "--")
    print("attaching " + ", ".join(p.name for p in files))
    if options.verify:
        print("native release assets verified; nothing published")
        return 0
    if exists:
        info = release_info(tag)
        if not info.get("draft"):
            verify_published(tag, files)
            print(f"{tag} already contains the exact verified assets; nothing changed")
            return 0
        if info.get("target_commitish") != target_commit:
            raise SystemExit("existing release draft belongs to a different source commit")
        if {asset["name"] for asset in info.get("assets", [])} - {path.name for path in files}:
            raise SystemExit("existing release draft contains unexpected assets")
        release.sh("gh", "release", "upload", tag, *[str(p) for p in files], "--clobber")
    else:
        notes_file = DIST / "release-notes.md"
        notes_file.write_text(notes, encoding="utf-8")
        release.sh("gh", "release", "create", tag, *[str(p) for p in files],
                   "--target", target_commit, "--title", f"kpopper {version}",
                   "--notes-file", str(notes_file), "--draft")
    verify_published(tag, files)
    release.sh("gh", "release", "edit", tag, "--draft=false", "--latest")
    verify_published(tag, files)
    print(f"published {tag}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
