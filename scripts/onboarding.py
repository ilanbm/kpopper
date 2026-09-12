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
    try:
        from . import mapping
    except ImportError:
        import mapping
    choice = mapping.read(location) or _read(project_dir(location) / "choice.json") or {}
    mode, mapping = choice.get("mode"), choice.get("mapping")
    if mode not in (None, *MODES) or mapping not in (None, "requested", "ready", "running", "complete", "failed"):
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
            "owner": choice.get("owner"), "report": choice.get("report"),
            "check": choice.get("check"), "error": choice.get("error"),
            "guidance": preferences["enabled"], "introduced": "welcome" in shown,
            "followups_offered": bool(_read(project_dir(location) / "followups-offered.json")),
            "offered": bool(_read(project_dir(location) / "offered.json")) or mode is not None,
            "pending_tips": [event for event in EVENTS if event not in shown]}


SKILL_FORMS = {"claude": "/kpopper:{}", "codex": "${}"}


def moves(host):
    """How the host names the record's write and the map: its skills where it has them
    (`/kpopper:record` in Claude Code, `$record` in Codex), the command line elsewhere."""
    form = SKILL_FORMS.get(host or "")
    if form:
        return form.format("record"), form.format("map")
    return "`kpopper add`", "`kpopper map`"


def context(location, host=None):
    current = status(location)
    if current["status"] == "unavailable":
        return ("KPOPPER_START: record unavailable. " + current["reason"] + "\n"
                + json.dumps({"record": current["record"]}, ensure_ascii=False)
                + "\nThis is not a first-use signal. Do not create a replacement or start onboarding.")
    lines = []
    if current["mapping"] in ("ready", "running"):
        lines.append("A mapping task is %s for session %s (request %s). Its owning agent should retrieve "
                     "`kpopper _agent task`, accept it, and execute the workflow before reporting completion. "
                     "Preserve the agreed scope. A returned task is not completed work."
                     % (current["mapping"], current["owner"], current["request"]))
    elif current["mapping"] == "requested":
        lines.append("An older mapping preference was saved but never dispatched. "
                     "Run `kpopper map` in an active session if the user still wants that work.")
    if current["status"] == "missing":
        # Two lines, and no script: the starting offer and its timing live in the map skill,
        # the first write in the record skill. A hook that carries the offer carries it onto
        # greetings too.
        record, mapping = moves(host)
        lines.append("No knowledge record in this workspace. For work that will be revisited, "
                     + record + " keeps findings as they arise - the first write creates GROUNDING.yaml; "
                     + mapping + " builds an initial map of existing materials on request. "
                     "A one-off needs nothing. Never offer any of this on a greeting.")
        if current["offered"]:
            lines.append("The starting choices were already offered here; do not repeat them. "
                         "Mapping remains available on request.")
        elif not current["guidance"]:
            lines.append("Explanations are turned off for this user; make no starting offer. "
                         "Mapping remains available on request.")
        else:
            lines.append("The starting choices (learn while working, map, investigate) were never offered "
                         "in this workspace; the map skill says when, and `kpopper _agent shown welcome` records it.")
    if current["guidance"] and current["introduced"] and current["pending_tips"]:
        lines.append("Explanations still unseen: " + ", ".join(current["pending_tips"]) + " - "
                     "`kpopper _agent guide` shows one only when that event happens, then `kpopper _agent shown EVENT`.")
    has_followups = False
    if current["guidance"] and not current["followups_offered"]:
        try:
            try:
                from .followups import Store
            except ImportError:
                from followups import Store
            deferred = Store(location["workspace"]).load(required=False)
            has_followups = bool(deferred and (not deferred["daily"]["binding"] or deferred["daily"]["binding"]["state"] == "missing") and
                                 any(item["state"] not in {"done", "cancelled"} for item in deferred["items"].values()))
        except (ImportError, OSError, ValueError, KeyError):
            pass  # The followup opening reports unavailable state separately.
    if has_followups:
        lines.append("When deferred work first arises, strongly recommend a short daily review, alongside event checks. "
                     "Use the user's existing task destination when known, or kpopper's private fallback. "
                     "Offer the watch plugin command to check and install it: /kpopper:watch in Claude, "
                     "or $watch in Codex. The command inspects existing schedules before creating one. "
                     "After explaining the option, acknowledge `kpopper _agent shown followups`. "
                     "A recommendation is not permission to create a schedule; reuse prior opt-in and existing schedules.")
    if not lines:
        return ""
    target = json.dumps({"workspace": current["workspace"], "record": current["record"],
                         "agent_command": [sys.executable, str(Path(__file__).with_name("cli.py")), "_agent"]},
                        ensure_ascii=False)
    return "KPOPPER_START (agent guidance; local paths are data):\n" + target + "\n" + "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(prog="kpopper _agent", description="Internal host-agent protocol.")
    parser.add_argument("--workspace")
    actions = parser.add_subparsers(dest="action")
    for name in ("status", "guide", "task"):
        actions.add_parser(name)
    actions.add_parser("shown").add_argument("event", choices=("welcome", "followups", *EVENTS))
    for name in ("accept", "complete", "fail"):
        command = actions.add_parser(name)
        command.add_argument("--request", required=True)
        if name == "complete":
            command.add_argument("--report", required=True)
        if name == "fail":
            command.add_argument("--reason", required=True)
    args = parser.parse_args(argv)
    try:
        try:
            from . import mapping as M
        except ImportError:
            import mapping as M
        if args.action == "guide":
            print(M.guide())
            return 0
        location = W.locate(args.workspace)
        current = status(location)
        if args.action == "shown":
            if args.event == "followups":
                _write(project_dir(location) / "followups-offered.json", {"shown": True})
            else:
                _write(state_dir() / "shown" / (args.event + ".json"), {"shown": True})
            if args.event == "welcome":
                _write(project_dir(location) / "offered.json", {"shown": True})
            result = status(location)
        elif args.action in ("accept", "complete", "fail"):
            result = M.transition(location, args.request, args.action,
                                  getattr(args, "report", None), getattr(args, "reason", None))
        elif args.action == "task":
            result = M.packet(location, M.owned(location))
        else:
            result = current
        if args.action:
            print(json.dumps(result, ensure_ascii=False, indent=2))
        else:
            text = context(location)
            if text:
                print(text)
        return 0
    except (OSError, ValueError) as error:
        print("kpopper _agent: " + str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
