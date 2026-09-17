"""Adapt the shared opener and stop gate to Copilot CLI's JSON hook protocol."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


HERE = Path(__file__).resolve()
SCRIPTS = HERE.parents[3] / "scripts"


def configuration():
    return {"version": 1, "hooks": {
        event: [{"type": "command", "exec": sys.executable,
                 "args": [str(HERE), mode], "timeoutSec": 65}]
        for event, mode in (("sessionStart", "start"), ("agentStop", "stop"))}}


def handle(mode, payload):
    if not isinstance(payload, dict):
        raise ValueError("the hook payload must be an object")
    payload = dict(payload)
    # Native Copilot events use camelCase; the shared reader uses session_id.
    # source and stop_hook_active already match the shared protocol. In particular,
    # agentStop deliberately uses snake_case for its per-turn continuation flag.
    payload["session_id"] = payload.get("sessionId", payload.get("session_id", ""))
    command = ([sys.executable, str(SCRIPTS / "session_start.py")] if mode == "start"
               else ["sh", str(SCRIPTS / "session_gate.sh")])
    # The shell gate invokes python3. Keep it on the interpreter selected at install.
    env = {**os.environ, "PATH": str(Path(sys.executable).parent) + os.pathsep
           + os.environ.get("PATH", ""), "PYTHONIOENCODING": "utf-8"}
    result = subprocess.run(command, input=json.dumps(payload), env=env,
                            capture_output=True, text=True, encoding="utf-8", timeout=60)
    if mode == "stop" and result.returncode == 2:
        return {"decision": "block", "reason": result.stderr.strip() or result.stdout.strip()}
    if result.stderr:
        print(result.stderr.rstrip(), file=sys.stderr)
    if result.returncode:
        print("kpopper hook failed (exit %s); run kpopper open/check explicitly."
              % result.returncode, file=sys.stderr)
        return {}
    if mode == "start" and result.stdout.strip():
        return {"additionalContext": result.stdout.rstrip()}
    return {}


def main():
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", newline="\n")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("config", "start", "stop"))
    mode = parser.parse_args().mode
    try:
        output = (configuration() if mode == "config" else
                  handle(mode, json.load(sys.stdin) if sys.stdin else None))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print("kpopper Copilot hook unavailable: " + str(error), file=sys.stderr)
        output = {}
    print(json.dumps(output, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
