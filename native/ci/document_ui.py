#!/usr/bin/env python3
"""Run the offline DOM assertions against this checkout's compiled native library."""
import json
import os
from pathlib import Path
import subprocess
import sys


def main():
    root = Path(__file__).resolve().parents[2]
    build = subprocess.run(["cargo", "test", "--locked", "--lib", "--no-run", "--message-format=json"],
                           cwd=root / "native", text=True, encoding="utf-8", stdout=subprocess.PIPE)
    if build.returncode:
        return build.returncode
    binaries = []
    for line in build.stdout.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-message":
            print(message["message"].get("rendered", ""), file=sys.stderr)
        if (message.get("reason") == "compiler-artifact" and message.get("executable")
                and message["target"]["name"] == "kpop_native" and message["profile"]["test"]):
            binaries.append(message["executable"])
    if len(binaries) != 1:
        raise RuntimeError("expected exactly one checkout-built native library test binary")
    environment = dict(os.environ, KPOP_DOCUMENT_TEST_BINARY=binaries[0], PYTHON=sys.executable)
    return subprocess.run(["node", "--test", "tests/test_document_ui.cjs"], cwd=root, env=environment).returncode


if __name__ == "__main__":
    raise SystemExit(main())
