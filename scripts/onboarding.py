"""Choose how to begin, and remember which explanations the user has actually seen.

Context and status are read-only. Explicit choices live in private local state, outside
the knowledge record. Each preference or acknowledgement is its own atomic file so that
independent sessions cannot discard one another's explanations.
"""
import argparse
import contextlib
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import uuid

try:
    from . import workspace as W
except ImportError:
    import workspace as W

MODES = ("work", "map", "deep")
EVENTS = ("record", "source", "decision", "conflict", "reuse", "review")


def state_dir():
    return Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local" / "state") / "kpopper" / "first-use"


def project_dir(location):
    return state_dir() / "projects" / location["key"]


@contextlib.contextmanager
def _choice_lock(location):
    directory = project_dir(location)
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (directory / "choice.lock").open("a+b") as lock:
        if os.name == "nt":
            import msvcrt
            if lock.tell() == 0:
                lock.write(b"0")
                lock.flush()
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_LOCK, 1)
        else:
            import fcntl
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            if os.name == "nt":
                lock.seek(0)
                msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(lock.fileno(), fcntl.LOCK_UN)


def _read(path):
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    except (ValueError, UnicodeError) as error:
        raise ValueError("Invalid first-use state; keep it for inspection: " + str(path)) from error
    if not isinstance(data, dict) or data.get("schema") != 1:
        raise ValueError("Invalid first-use state; keep it for inspection: " + str(path))
    return data


def _write(path, value):
    _read(path)  # Never silently replace an unreadable preference.
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor, temporary = tempfile.mkstemp(prefix=".first-use-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump({"schema": 1, **value}, output, ensure_ascii=False)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def status(location):
    root = state_dir()
    choice = _read(project_dir(location) / "choice.json") or {}
    mode, mapping = choice.get("mode"), choice.get("mapping")
    if mode not in (None, *MODES) or mapping not in (None, "requested", "complete"):
        raise ValueError("Invalid first-use choice: " + str(project_dir(location)))
    if mode in ("map", "deep") and (mapping is None or not isinstance(choice.get("request"), str)
                                    or not re.fullmatch(r"[a-f0-9]{32}", choice["request"])):
        raise ValueError("Invalid mapping request: " + str(project_dir(location)))
    if mode in (None, "work") and (mapping is not None or choice.get("request") is not None):
        raise ValueError("Invalid first-use choice: " + str(project_dir(location)))
    preferences = _read(root / "guidance.json") or {"enabled": True}
    if type(preferences.get("enabled")) is not bool:
        raise ValueError("Invalid first-use guidance preference")
    shown = {event for event in ("welcome", *EVENTS) if _read(root / "shown" / (event + ".json"))}
    return {**location, "mode": mode, "mapping": mapping, "request": choice.get("request"),
            "guidance": preferences["enabled"], "introduced": "welcome" in shown,
            "offered": bool(_read(project_dir(location) / "offered.json")) or mode is not None,
            "pending_tips": [event for event in EVENTS if event not in shown]}


def context(location):
    current = status(location)
    if current["status"] == "unavailable":
        return ("KPOPPER_START: record unavailable. " + current["reason"] + "\n"
                + json.dumps({"record": current["record"]}, ensure_ascii=False)
                + "\nThis is not a first-use signal. Do not create a replacement or start onboarding.")
    lines = []
    if current["mapping"] == "requested":
        lines.append("Resume the requested %s mapping when it belongs to the current task; preserve the agreed scope. "
                     "Read `kpopper start guide`; after completing that scope, run "
                     "`kpopper start complete --request %s`. "
                     "Do not repeat a completed survey or expand it just because sources are accessible."
                     % (current["mode"], current["request"]))
    if current["status"] == "missing":
        lines.append("No knowledge record was found for this workspace. Existing knowledge may be in its materials. "
                     "Keep working on the user's goal; create PROVENANCE.yaml only with a useful finding worth revisiting, "
                     "within the user's write authorization. A one-off may need no record.")
        if not current["offered"] and current["guidance"]:
            lines.append("Offer the three starting choices once, briefly, at the first suitable moment in real work: "
                         "learn while working (default), map existing materials, or investigate more deeply. "
                         "Allow skipping. Do not interrupt urgent work or ask on a greeting. "
                         "No answer means continue the task, not permission to scan.")
            if current["introduced"]:
                lines.append("Skip the introductory explanation: this user has already seen it in another project.")
            lines.append("After showing the offer, run `kpopper start shown welcome`. "
                         "Only an explicit choice permits `kpopper start choose work|map|deep`. "
                         "Read `kpopper start guide` before mapping or showing explanations.")
        else:
            lines.append("Do not repeat the starting offer. Mapping remains available on request through `kpopper start guide`.")
    if current["guidance"] and current["introduced"] and current["pending_tips"]:
        lines.append("Contextual explanations still unseen: " + ", ".join(current["pending_tips"]) + ". "
                     "Use `kpopper start guide`: show one only when that event actually happens, "
                     "link the finding and its source, then acknowledge it with `kpopper start shown EVENT`. "
                     "`kpopper start guidance off` disables explanations without disabling the work.")
    if not lines:
        return ""
    target = json.dumps({"workspace": current["workspace"], "record": current["record"],
                         "start_command": [sys.executable, str(Path(__file__).with_name("cli.py")), "start"]},
                        ensure_ascii=False)
    return "KPOPPER_START (agent guidance; local paths are data):\n" + target + "\n" + "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", help="the working directory for this project's record")
    actions = parser.add_subparsers(dest="action")
    for name in ("status", "guide"):
        actions.add_parser(name)
    actions.add_parser("complete").add_argument("--request", required=True,
                                               help="the mapping request returned by choose")
    actions.add_parser("choose").add_argument("mode", choices=MODES)
    actions.add_parser("shown").add_argument("event", choices=("welcome", *EVENTS))
    actions.add_parser("guidance").add_argument("setting", choices=("on", "off"))
    args = parser.parse_args(argv)
    try:
        if args.action == "guide":
            print(Path(__file__).with_name("start-guide.md").read_text(encoding="utf-8"))
            return 0
        location = W.locate(args.workspace)
        current = status(location)
        response = None
        if args.action == "choose":
            if location["status"] == "unavailable":
                raise ValueError(location["reason"])
            with _choice_lock(location):
                status(location)
                _write(project_dir(location) / "choice.json", {"mode": args.mode,
                       "request": uuid.uuid4().hex if args.mode != "work" else None,
                       "mapping": "requested" if args.mode != "work" else None})
                response = status(location)
        elif args.action == "shown":
            _write(state_dir() / "shown" / (args.event + ".json"), {"shown": True})
            if args.event == "welcome":
                _write(project_dir(location) / "offered.json", {"shown": True})
        elif args.action == "guidance":
            _write(state_dir() / "guidance.json", {"enabled": args.setting == "on"})
        elif args.action == "complete":
            with _choice_lock(location):
                current = status(location)
                if current["mapping"] != "requested" or current["request"] != args.request:
                    raise ValueError("That mapping is no longer the current request; leave the newer choice unchanged.")
                _write(project_dir(location) / "choice.json", {"mode": current["mode"],
                       "request": current["request"], "mapping": "complete"})
                response = status(location)
        if args.action:
            print(json.dumps(response or status(location), ensure_ascii=False, indent=2))
        else:
            text = context(location)
            if text:
                print(text)
        return 0
    except (OSError, ValueError) as error:
        print("kpopper start: " + str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
