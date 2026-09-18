"""Translate shared kpopper hook text into Gemini CLI's JSON protocol."""
import json
from pathlib import Path
import subprocess
import sys


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def main():
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", newline="\n")
    event = sys.argv[1] if len(sys.argv) == 2 else ""
    if sys.stdin is None:
        print("{}")
        return 0
    commands = {
        "SessionStart": [sys.executable, str(ROOT / "scripts" / "session_start.py")],
        "SessionEnd": ["sh", str(HERE / "checknote.sh")],
    }
    output = {}
    try:
        if event not in commands:
            raise ValueError("expected SessionStart or SessionEnd")
        result = subprocess.run(commands[event], input=sys.stdin.read() if sys.stdin else "",
                                capture_output=True, text=True, encoding="utf-8",
                                timeout=55)
        if result.stderr:
            print(result.stderr.rstrip(), file=sys.stderr)
        text = result.stdout.rstrip()
        if text:
            if event == "SessionStart":
                output = {"hookSpecificOutput": {
                    "hookEventName": event, "additionalContext": text}}
            else:
                output = {"systemMessage": text}
        elif result.returncode:
            output = {"systemMessage": "kpopper could not run its " + event + " check."}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print("kpopper Gemini hook: " + str(error), file=sys.stderr)
        output = {"systemMessage": "kpopper could not run its " + event + " check."}
    # Both lifecycle events are advisory. Never put flow-control fields here.
    print(json.dumps(output, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
