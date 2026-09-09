"""Mapping tasks returned to the calling host agent, with explicit execution receipts.

The CLI is not an LLM runner. A ready task has been returned to its caller; it is not
running or complete until that same agent accepts and reports the actual work.
"""
import os
from pathlib import Path
import re
import subprocess
import sys
import uuid

try:
    from . import onboarding as O
except ImportError:
    import onboarding as O


def session():
    value = os.environ.get("KPOPPER_AGENT_SESSION") or os.environ.get("CODEX_THREAD_ID")
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,200}", value):
        return None
    return value


def guide():
    return Path(__file__).with_name("start-guide.md").read_text(encoding="utf-8")


def read(location):
    value = O._read(O.project_dir(location) / "mapping.json")
    if value is None:
        return None
    if value.get("mode") not in ("map", "deep") or value.get("mapping") not in ("ready", "running", "complete", "failed"):
        raise ValueError("Invalid mapping task state")
    if not isinstance(value.get("request"), str) or not re.fullmatch(r"[a-f0-9]{32}", value["request"]):
        raise ValueError("Invalid mapping request ID")
    if not isinstance(value.get("owner"), str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,200}", value["owner"]):
        raise ValueError("Invalid mapping owner")
    return value


def packet(location, job, instructions=None):
    base = [sys.executable, str(Path(__file__).with_name("cli.py")),
            "--workspace", location["workspace"], "_agent"]
    request = job["request"]
    return {"request": request, "status": job["mapping"], "mode": job["mode"],
            "owner": job["owner"], "workspace": location["workspace"], "record": location["record"],
            "instructions": guide() if instructions is None else instructions,
            "protocol": {"accept": base + ["accept", "--request", request],
                         "complete": base + ["complete", "--request", request, "--report", "REPORT_PATH"],
                         "fail": base + ["fail", "--request", request, "--reason", "REASON"]}}


def request(location, deep=False):
    owner = session()
    if not owner:
        raise ValueError("No calling agent session is bound. Mapping runs inside an active agent session; no work was started.")
    if location["status"] == "unavailable":
        raise ValueError(location["reason"])
    instructions = guide()  # Fail before saving a task if the distribution is incomplete.
    with O._choice_lock(location):
        previous = read(location)
        mode = "deep" if deep else "map"
        if previous and previous["mapping"] in ("ready", "running") and previous["owner"] != owner:
            raise ValueError("Another agent session owns a pending mapping. Finish or fail that task before starting another.")
        if previous and previous["mapping"] in ("ready", "running") \
                and previous["owner"] == owner and previous["mode"] == mode:
            return packet(location, previous, instructions)
        value = {"mode": mode, "mapping": "ready", "request": uuid.uuid4().hex, "owner": owner}
        O._write(O.project_dir(location) / "mapping.json", value)
        # Capture the returned request under the same lock as the write.
        return packet(location, value, instructions)


def owned(location, request_id=None):
    value = read(location)
    if not value or not session() or value["owner"] != session() \
            or (request_id is not None and value["request"] != request_id):
        raise ValueError("This mapping belongs to a different request or agent session.")
    return value


def transition(location, request_id, action, report=None, reason=None):
    with O._choice_lock(location):
        value = owned(location, request_id)
        if action == "accept":
            if value["mapping"] not in ("ready", "running"):
                raise ValueError("This mapping cannot be accepted in its current state.")
            value["mapping"] = "running"
        elif action == "fail":
            if value["mapping"] not in ("ready", "running") or not reason or not reason.strip():
                raise ValueError("An active mapping and a failure reason are required.")
            value.update(mapping="failed", error=reason.strip())
        else:
            if value["mapping"] != "running":
                raise ValueError("Accept the mapping before completing it.")
            path = Path(report or "").expanduser().absolute()
            if not report or not path.is_file() or path.stat().st_size == 0:
                raise ValueError("Completion requires the actual, nonempty report or knowledge record.")
            check = None
            if Path(location["record"]).is_file():
                try:
                    result = subprocess.run([sys.executable, str(Path(__file__).with_name("provenance.py")),
                                             "check", location["record"]],
                                            capture_output=True, text=True, encoding="utf-8", timeout=60)
                except (OSError, subprocess.SubprocessError) as error:
                    value["check"] = {"exit_code": None, "error": str(error)}
                    O._write(O.project_dir(location) / "mapping.json", value)
                    raise ValueError("The record check could not finish; mapping remains running. "
                                     "Retry completion or report failure. " + str(error)) from error
                check = {"exit_code": result.returncode, "output": result.stdout, "error": result.stderr}
            value.update(mapping="complete", report=str(path), check=check)
        O._write(O.project_dir(location) / "mapping.json", value)
        return value
