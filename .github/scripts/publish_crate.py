#!/usr/bin/env python3
"""Publish a release's crate to crates.io, from the commit its GitHub release was made from.

Runs in the publish workflow once that release is out, and only for the version this commit
moved to. The build job has already built the crate from its packaged files and checked that it
is the program the release ships; its .crate is the verified bytes. crates.io never takes a
version back, so a version the registry already serves must be exactly those bytes, and a
version it lacks is published next. The first publication claims the name and needs an owner's
own token; until the crate exists this stops the run, naming the bytes that publication must
be, and publishes nothing. Later versions are published with a short-lived token the workflow's
identity is exchanged for, so the repository holds no registry credential.

    python3 .github/scripts/publish_crate.py --plan --crate FILE.crate    # what crates.io takes
    python3 .github/scripts/publish_crate.py --served FILE.crate          # it serves these bytes
"""
import argparse
import hashlib
import json
import os
import pathlib
import re
import sys
import time
import urllib.error
import urllib.request

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import release  # noqa: E402 - the version files are read the one way

ROOT = release.ROOT
CRATE = "kpopper"
# Who may own the crate. The published bytes can be rebuilt by anyone from the public tag, so a
# name someone else claimed can serve them too; it is still not this project's crate.
OWNERS = {"ilanbm"}
API = os.environ.get("KPOPPER_CRATES_API", "https://crates.io/api/v1")
# crates.io asks every client to say who it is.
AGENT = "kpopper release workflow (https://github.com/ilanbm/kpopper)"
PACKAGE_NAME = re.compile(r'^\[package\]\n(?:[^\[\n][^\n]*\n)*?name = "([^"]+)"', re.M)
CLAIM = (
    "The first version on crates.io claims the name, which needs an owner's token, so this "
    "run stops here until it exists. Check out the tag v{version}; from native/, with the "
    "toolchain native/rust-toolchain.toml pins, run `cargo package --locked --no-verify` and "
    "require `shasum -a 256 target/package/kpopper-{version}.crate` to print {sha256}; then run "
    "`cargo login` and `cargo publish --locked --no-verify`. Do it soon: until then anyone can "
    "publish these bytes under their own name. Rerun this job: it passes once crates.io serves "
    "exactly those bytes, owned by {owners}. Later releases publish from this workflow once "
    "crates.io trusts it (crate settings, Trusted Publishing: repository ilanbm/kpopper, "
    "workflow publish.yml, environment crates-io)."
)


def fetch(path):
    """-> the registry's JSON for a path, or None where it answers 404. Any other answer
    stops here: taking it for 'not published' would publish over an answer nobody got."""
    request = urllib.request.Request(API + path, headers={"User-Agent": AGENT, "Accept": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise SystemExit(f"crates.io answered {error.code} for {path}")
    except (urllib.error.URLError, TimeoutError) as error:
        raise SystemExit(f"crates.io could not be reached for {path}: {error}")


def digest(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def served_as(found, expected, version):
    """Require a registry version record to hold the verified bytes, not yanked."""
    record = (found or {}).get("version", {})
    if record.get("checksum") != expected:
        raise SystemExit(f"crates.io serves {CRATE} {version} with checksum {record.get('checksum')}, "
                         f"not the verified {expected}")
    if record.get("yanked") is not False:
        raise SystemExit(f"crates.io has yanked {CRATE} {version}")


def decide(version, crate_file):
    """-> (action, reason) for this version and the bytes verified for it. 'publish': the
    registry holds the crate but not this version; 'published': it serves this version, as
    exactly these bytes and not yanked; 'claim': the crate is not there yet. A crate owned by
    anyone but OWNERS, or a version served otherwise, stops here: neither can be undone, so the
    run must not read as a success."""
    expected = digest(crate_file)
    if fetch(f"/crates/{CRATE}") is None:
        return "claim", f"{CRATE} is not on crates.io yet. " + CLAIM.format(
            version=version, sha256=expected, owners=", ".join(sorted(OWNERS)))
    owners = {user.get("login") for user in (fetch(f"/crates/{CRATE}/owners") or {}).get("users", [])}
    if owners != OWNERS:
        raise SystemExit(f"crates.io lists {sorted(owners)} as the owners of {CRATE}, not {sorted(OWNERS)}")
    found = fetch(f"/crates/{CRATE}/{version}")
    if found is not None:
        served_as(found, expected, version)
        return "published", f"crates.io already serves {CRATE} {version} as verified (sha256 {expected})"
    return "publish", f"publishing {CRATE} {version} to crates.io (sha256 {expected})"


def served(version, crate_file, attempts=30, pause=10):
    """Wait until the registry serves the version, then require it to be the bytes this workflow
    verified, not yanked."""
    expected = digest(crate_file)
    for attempt in range(attempts):
        found = fetch(f"/crates/{CRATE}/{version}")
        if found is not None:
            served_as(found, expected, version)
            return expected
        if attempt + 1 < attempts:
            time.sleep(pause)
    raise SystemExit(f"crates.io does not serve {CRATE} {version} yet")


def output(**values):
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
            for key, value in values.items():
                stream.write(f"{key}={value}\n")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--plan", action="store_true", help="decide what crates.io takes from this commit")
    parser.add_argument("--crate", type=pathlib.Path, metavar="FILE",
                        help="the verified .crate file the plan holds the registry to")
    parser.add_argument("--served", type=pathlib.Path, metavar="FILE",
                        help="require crates.io to serve exactly this .crate file")
    parser.add_argument("--version", help="the version the release planner published")
    parser.add_argument("--source-directory", type=pathlib.Path,
                        help="read version and package metadata from this verified candidate checkout")
    options = parser.parse_args(argv)
    source = options.source_directory.resolve() if options.source_directory else ROOT
    # The code remains from the trusted workflow revision. Only metadata comes
    # from the checked candidate, whose version need not be on main yet.
    found = release.versions_in({name: (source / name).read_text(encoding="utf-8")
                                 for name in release.VERSION_FILES})
    if len(set(found.values())) != 1:
        raise SystemExit("the version files disagree: " + json.dumps(found))
    version = found["VERSION"]
    if options.version and options.version != version:
        raise SystemExit(f"the release planner published {options.version}, but this tree carries {version}")
    name = PACKAGE_NAME.search((source / "native" / "Cargo.toml").read_text(encoding="utf-8"))
    if not name or name.group(1) != CRATE:
        raise SystemExit(f"native/Cargo.toml does not name the package {CRATE}")
    if options.served:
        print(f"crates.io serves {CRATE} {version}, sha256 {served(version, options.served)}")
        return 0
    if options.plan:
        if options.crate is None:
            parser.error("--plan needs the verified crate: --crate FILE")
        action, reason = decide(version, options.crate)
        print(reason)
        output(action=action, version=version)
        if action == "claim":
            print("::error title=crates.io::" + reason)
            return 1
        return 0
    parser.error("choose --plan or --served")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
