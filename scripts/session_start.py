"""Open an existing record, or supply bounded first-use guidance without creating one."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

try:
    from . import onboarding as O
    from . import workspace as W
except ImportError:
    import onboarding as O
    import workspace as W

HERE = Path(__file__).resolve().parent
LEGACY_CHARS = 2000
EMPTY_MARK = {"fails": 0, "failures": [], "unserved": [], "ids": [], "judgments": {}}


def _run(script, args, directory):
    return subprocess.run([sys.executable, str(HERE / script), *args], cwd=directory,
                          capture_output=True, text=True, encoding="utf-8", timeout=55)


def read_view(location, reader_args=None, host=None):
    """The public CLI and hooks use the same checked/legacy opening boundary. `host` names
    the agent host so the opener's next moves read as that host invokes a skill."""
    if reader_args is None:
        result = _run("session_hook.py", [location["record"]], location["workspace"])
        if result.returncode != 3:
            return result, True
        reader_args = ["--chars", str(LEGACY_CHARS), location["record"]]
    if host:
        reader_args = [*reader_args, "--host", host]
    return _run("provenance.py", ["open", *reader_args], location["workspace"]), False


def opening(payload, host=None):
    if not isinstance(payload, dict) or payload.get("agent_id"):
        return ""
    location = W.locate(payload.get("cwd"))
    directory, record = location["workspace"], location["record"]
    output = []
    try:
        first_use = O.context(location, host)
        if first_use:
            output.append(first_use)
    except (OSError, ValueError) as error:
        output.append("kpopper first-use preferences unavailable: " + str(error))
    if location["status"] == "unavailable":
        return "\n".join(output)
    if location["status"] == "found":
        result, _ = read_view(location, host=host)
        if result.stdout:
            output.insert(0, result.stdout.rstrip())
        elif result.returncode:
            output.insert(0, "The knowledge record could not be opened. Read it before relying on it: " + record)
        if result.stderr:
            print(result.stderr.rstrip(), file=sys.stderr)

    try:
        try:
            from .followups import summary
        except ImportError:
            from followups import summary
        followups = summary(location, counts_only=True)
        if followups:
            output.append(followups)
    except (ImportError, OSError, ValueError, KeyError, TypeError) as error:
        output.append("Followups unavailable: " + str(error))

    sid = payload.get("session_id", "")
    if isinstance(sid, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,200}", sid):
        output.append("KPOPPER_AGENT_CONTEXT " + json.dumps({
            "environment": {"KPOPPER_AGENT_SESSION": sid},
            "command": [sys.executable, str(HERE / "cli.py")],
            "workspace": directory}, ensure_ascii=False)
            + "\nFor mapping, pass this session environment to the CLI and execute the returned task. "
              "The identity routes work back to this session; it grants no source access.")
        baseline = Path(tempfile.gettempdir()) / ("kpopper-base-" + sid)
        if not (payload.get("source") in ("compact", "resume") and baseline.exists()):
            if location["status"] == "missing":
                # The first real write can be checked at Stop, even though no file
                # was created in the workspace merely to enable that check.
                baseline.write_text(json.dumps(EMPTY_MARK), encoding="utf-8")
            else:
                _run("provenance.py", ["mark", str(baseline), record], directory)
    return "\n".join(output)


def main():
    args = sys.argv[1:]
    cursor = "--cursor" in args
    # the host, when the hook names it: a host with skills is told the skill for each move
    host = args[args.index("--host") + 1] if "--host" in args and args.index("--host") + 1 < len(args) else None
    try:
        raw = sys.stdin.read()
        payload = json.loads(raw) if raw.strip() else {}
        if cursor and isinstance(payload, dict):
            identity = payload.get("conversation_id")
            payload["session_id"] = "cursor-" + identity if isinstance(identity, str) and identity else ""
        text = opening(payload, host)
        if cursor:
            print(json.dumps({"additional_context": text}, ensure_ascii=False))
        elif text:
            print(text)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        # A failed opener is visible but must not hold the user's session hostage.
        print("kpopper could not open this workspace: " + str(error), file=sys.stderr)
        if cursor:
            print(json.dumps({"additional_context": ""}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
