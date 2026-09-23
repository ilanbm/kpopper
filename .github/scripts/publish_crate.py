#!/usr/bin/env python3
"""Publish a release's crate to crates.io, from the commit its GitHub release was made from.

Runs in the publish workflow once that release is out, and only for the version this commit
moved to. crates.io never takes a version back, so a version the registry already serves is
left alone, and nothing is uploaded that the workflow did not build from its packaged files
first. The first publication claims the name and needs an owner's own token; until the crate
exists this says so and publishes nothing. Later versions are published with a short-lived
token the workflow's identity is exchanged for, so the repository holds no registry credential.

    python3 .github/scripts/publish_crate.py --plan                  # what this commit asks of crates.io
    python3 .github/scripts/publish_crate.py --served FILE.crate     # crates.io serves exactly these bytes
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
API = os.environ.get("KPOPPER_CRATES_API", "https://crates.io/api/v1")
# crates.io asks every client to say who it is.
AGENT = "kpopper release workflow (https://github.com/ilanbm/kpopper)"
PACKAGE_NAME = re.compile(r'^\[package\]\n(?:[^\[\n][^\n]*\n)*?name = "([^"]+)"', re.M)
CLAIM = (
    "The first version on crates.io claims the name, which needs an owner's token: check out "
    "the tag v{version}, then run `cargo publish --locked` from native/ after `cargo login`. "
    "Later releases publish from this workflow once crates.io trusts it (crate settings, "
    "Trusted Publishing: repository ilanbm/kpopper, workflow publish.yml, environment crates-io)."
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


def decide(version):
    """-> (action, reason). 'publish': the registry holds the crate but not this version;
    'published': it already serves this version; 'claim': the crate is not there yet."""
    if fetch(f"/crates/{CRATE}") is None:
        return "claim", f"{CRATE} is not on crates.io yet. " + CLAIM.format(version=version)
    if fetch(f"/crates/{CRATE}/{version}") is not None:
        return "published", f"crates.io already serves {CRATE} {version}; nothing to publish"
    return "publish", f"publishing {CRATE} {version} to crates.io"


def digest(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def served(version, crate_file, attempts=30, pause=10):
    """Wait until the registry serves the version, then require its checksum to be the one
    of the bytes this workflow verified."""
    expected = digest(crate_file)
    for attempt in range(attempts):
        found = fetch(f"/crates/{CRATE}/{version}")
        if found is not None:
            checksum = found.get("version", {}).get("checksum")
            if checksum != expected:
                raise SystemExit(f"crates.io serves {CRATE} {version} with checksum {checksum}, "
                                 f"not the verified {expected}")
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
    parser.add_argument("--served", type=pathlib.Path, metavar="FILE",
                        help="require crates.io to serve exactly this .crate file")
    parser.add_argument("--version", help="the version the release planner published")
    options = parser.parse_args(argv)
    found = release.versions_in(release.read_texts())
    if len(set(found.values())) != 1:
        raise SystemExit("the version files disagree: " + json.dumps(found))
    version = found["VERSION"]
    if options.version and options.version != version:
        raise SystemExit(f"the release planner published {options.version}, but this tree carries {version}")
    name = PACKAGE_NAME.search((ROOT / "native" / "Cargo.toml").read_text(encoding="utf-8"))
    if not name or name.group(1) != CRATE:
        raise SystemExit(f"native/Cargo.toml does not name the package {CRATE}")
    if options.served:
        print(f"crates.io serves {CRATE} {version}, sha256 {served(version, options.served)}")
        return 0
    if options.plan:
        action, reason = decide(version)
        print(reason)
        if action == "claim":
            print("::warning title=crates.io::" + reason)
        output(action=action, version=version)
        return 0
    parser.error("choose --plan or --served")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
