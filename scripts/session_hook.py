#!/usr/bin/env python3
"""Return3 when checked mode is not enabled; enabled failures stay explicit."""
import importlib
import json
from pathlib import Path
import subprocess
import sys

package = Path(__file__).resolve().parent
sys.path.insert(0, str(package.parent))
settings = importlib.import_module(package.name + ".session.settings")


def main():
    record = sys.argv[1]
    try:
        config = settings.current()
        if not config.get("enabled"):
            return 3
        command = [config["python"], str(package / "session_cli.py"), "hook-open", "--no-settings", "--input", record,
                   "--tokens", str(config["tokens"])]
        for field in ("project", "state", "profile"):
            if config.get(field):
                command += ["--" + field, str(config[field])]
        # The hook has already selected the project's interpreter. An out-of-tree
        # registered record must not redirect execution through another config.
        result = subprocess.run(command, text=True, capture_output=True, timeout=45)
        if result.returncode:
            print("Checked session view unavailable. Read the record before relying on it: " + record)
            print(result.stderr.strip(), file=sys.stderr)
            return 2
        print(result.stdout, end="")
        return 0
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print("Checked session view unavailable. Read the record before relying on it: " + record)
        print(str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
