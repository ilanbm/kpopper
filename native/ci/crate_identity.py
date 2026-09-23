#!/usr/bin/env python3
"""Require a crate built from its published files to be the program this checkout builds.

Both executables must accept the same verified resources and declare the same adapter
source and schemas. Accepting the resources is itself the check of the Lean source
fingerprints: an executable refuses resources built from sources other than its own.
"""

import argparse
import json
import os
import secrets
import subprocess
from pathlib import Path

VALIDATED = {"core": "archive_validated", "ordinary": "files_validated"}


def declaration(binary, resources):
    result = subprocess.run(
        [str(binary), "history", "capabilities", "--nonce", secrets.token_hex(16)],
        env=dict(os.environ, KPOPPER_NATIVE_RESOURCES=str(resources)),
        text=True,
        encoding="utf-8",
        capture_output=True,
        timeout=300,
    )
    if result.returncode:
        raise SystemExit(f"{binary} refused its runtime declaration: {result.stderr.strip()}")
    value = json.loads(result.stdout)
    for name, status in VALIDATED.items():
        found = value["resources"][name].get("status")
        if found != status:
            raise SystemExit(f"{binary} did not accept the {name} resources: {found}")
    return {
        "adapter_source_sha256": value["artifact"]["adapter_source_sha256"],
        "schemas": value["schemas"],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True, type=Path, help="executable built from this checkout")
    parser.add_argument("--crate", required=True, type=Path, help="executable built from the packaged crate")
    parser.add_argument("--resources", required=True, type=Path, help="verified resources both must accept")
    args = parser.parse_args()
    repository = declaration(args.repository.resolve(), args.resources.resolve())
    crate = declaration(args.crate.resolve(), args.resources.resolve())
    if crate != repository:
        raise SystemExit(
            "the crate declares a different program:\n"
            + json.dumps({"repository": repository, "crate": crate}, indent=2, sort_keys=True)
        )
    print(f"the crate is the program this checkout builds: adapter {crate['adapter_source_sha256']}")


if __name__ == "__main__":
    main()
