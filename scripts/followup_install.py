"""Check/install daily review through a host agent, with inspected host receipts.

This module issues bounded host work; it never calls a scheduler or treats a local
plan as an installed schedule. One outstanding mutation per workspace prevents two
cooperating agents from creating duplicate schedules after a lost response.
"""
import datetime as dt
from pathlib import Path
import re
import uuid

try:
    from . import followups as F, followup_daily as D
except ImportError:
    import followups as F
    import followup_daily as D


def _time(value):
    if not isinstance(value, str) or not re.fullmatch(r"(?:[01]\d|2[0-3]):[0-5]\d", value):
        raise F.Refused("Schedule time must be HH:MM")
    return value


def marker(store):
    return "KPOPPER_DAILY_WORKSPACE=" + store.location["key"]


def _packet(store, data, job=None, check=False):
    requested = job.get("time") if job else None
    return {"state": "needs_host", "action": "inspect", "workspace_key": data["workspace_key"],
            "marker": marker(store), "workspace": data["config"]["workspace"],
            "record": data["config"]["record"], "ledger": str(store.path),
            "binding": data["daily"]["binding"], "installation": job,
            "desired": {"cadence": "daily", "time": requested, "new_schedule_default_time": "09:00",
                        "timezone": data["config"]["timezone"], "prompt": D.plan(store)["prompt"]},
            "instruction": "Inspect the bound schedule and all equivalent workspace daily reviews through the host's supported tools. " +
            ("Check-only: report actual status. Do not create, update, resume or reserve anything." if check else
             "Return the actual inventory. Continue through creation/update and readback when authorized; this packet is not installation success.")}


def begin(store, owner=None, time=None, timezone=None, destination=None, private=False, check=False, resume=False):
    if time is not None:
        _time(time)
    data = store.load(required=False)
    if data is None:
        if check:
            return {"state": "not_configured", "action": "inspect", "workspace": store.location["workspace"],
                    "workspace_key": store.location["key"], "record": store.location["record"],
                    "marker": marker(store), "suggested_store": store.suggested_store(),
                    "instruction": "Check host schedules without changing them. Installation can initialize followups after a useful record exists and the user's timezone is known."}
        if timezone is None:
            raise F.Refused("First setup needs --timezone from the user's host context")
        F.text(owner, "unique host/session owner")
        store.setup(destination, timezone, private=private)
        data = store.load()
    elif timezone is not None and timezone != data["config"]["timezone"]:
        raise F.Refused("Timezone differs from the configured workspace; resolve that preference before installing")
    if destination is not None and F.reference(destination) != data["config"]["store"]:
        raise F.Refused("Keep the configured task destination; installation does not migrate tasks")
    if check:
        return _packet(store, data, data["daily"].get("installation"), check=True)
    if not Path(data["config"]["workspace"]).is_dir() or not Path(data["config"]["record"]).is_file():
        raise F.Refused("The pinned workspace or record is unavailable; restore its location before installation")
    destination = data["config"]["store"]
    if not destination.startswith("https://") and not Path(destination).is_dir():
        if Path(destination) == store.root / "items" and not data["items"]:
            Path(destination).mkdir(mode=0o700)
        else:
            raise F.Refused("The configured task destination is unavailable; restore it before installing")
    F.text(owner, "unique host/session owner")
    with store.transaction() as data:
        previous = data["daily"].get("installation")
        if previous and previous["state"] in {"apply", "uncertain"}:
            if previous["owner"] != owner:
                return {"state": "needs_reconciliation", "action": "inspect_only", "installation": previous,
                        "instruction": "A host change may already have happened. Inspect it; never create another schedule. An actual matching readback can finish the retained token."}
            return _packet(store, data, previous)
        if previous and previous["state"] == "inspect" and previous["owner"] == owner:
            if previous["config"] != F.digest(data["config"]):
                previous.update(state="blocked", reason="Configuration changed before host mutation")
            elif previous["time"] != time or previous["resume"] != resume:
                raise F.Refused("An inspection is pending with different options")
            else:
                return _packet(store, data, previous)
        if previous:
            data["daily"].setdefault("installation_history", []).append(previous)
        job = {"token": uuid.uuid4().hex, "owner": owner, "state": "inspect", "time": time,
               "resume": resume, "started_at": F.stamp(store.now()),
               "config": F.digest(data["config"]), "workspace_config": data["config"].copy(),
               "prompt": D.plan(store)["prompt"], "template_hash": F.digest(D.plan(store)["prompt"])}
        data["daily"]["installation"] = job
        return _packet(store, data, job)


def _job(data, token):
    job = data["daily"].get("installation")
    if not job or job["token"] != token:
        raise F.Refused("This token does not own the installation")
    if job["config"] != F.digest(data["config"]):
        raise F.Refused("Workspace configuration changed; reconcile the installation first")
    return job


def _observed(store, value):
    if not isinstance(value, str) or "T" not in value.upper():
        raise F.Refused("Host observations require an offset timestamp")
    when = F.T.parse_time(value)
    if not dt.timedelta(0) <= store.now() - when <= dt.timedelta(minutes=10):
        raise F.Refused("Inspect the host again; its receipt must be current (within 10 minutes)")
    return F.stamp(when)


def _schedule(schedule, data):
    fields = {"id", "state", "workspace_key", "cadence", "time", "timezone", "prompt", "access_verified"}
    if not isinstance(schedule, dict) or set(schedule) != fields:
        raise F.Refused("Host schedule requires id, state, workspace_key, cadence, time, timezone, prompt and access_verified")
    for key in ("id", "timezone", "prompt"):
        F.text(schedule[key], key)
    _time(schedule["time"])
    if schedule["state"] not in {"active", "paused"} or schedule["workspace_key"] != data["workspace_key"]:
        raise F.Refused("The host schedule does not identify this workspace or a supported state")
    if schedule["cadence"] not in {"daily", "weekly", "other"}:
        raise F.Refused("The inspected schedule's cadence is unknown")
    prompt = schedule["prompt"]
    markers = re.findall(r"KPOPPER_DAILY_WORKSPACE=([a-f0-9]{64})", prompt)
    legacy = "Run the daily kpopper review" in prompt and all(data["config"][key] in prompt for key in ("workspace", "record"))
    if (markers and set(markers) != {data["workspace_key"]}) or (not markers and not legacy):
        raise F.Refused("The actual host prompt does not identify this workspace's dedicated kpopper daily review")
    if schedule["access_verified"] is not True:
        raise F.Refused("The scheduled host's access to the pinned workspace and private state is unverified")
    return schedule


def _bind(data, host, schedule, observed_at, evidence, template_hash=None):
    old = data["daily"]["binding"]
    if old and (old["host"], old["id"]) != (host, schedule["id"]):
        if old["state"] != "missing":
            raise F.Refused("A different daily schedule is still bound; reconcile its ownership")
        data["daily"].setdefault("binding_history", []).append(old)
    data["daily"]["binding"] = {"host": host, "id": schedule["id"], "state": schedule["state"],
                                "observed_at": observed_at, "evidence": evidence,
                                "time": schedule["time"], "timezone": schedule["timezone"],
                                "cadence": schedule["cadence"],
                                "prompt_hash": F.digest(schedule["prompt"]),
                                "managed_prompt": F.digest(schedule["prompt"]) == template_hash}


def inspect_host(store, token, report):
    report = F.T.normalize(report)
    fields = {"host", "observed_at", "complete", "schedules", "evidence"}
    if not isinstance(report, dict) or set(report) != fields or not isinstance(report["schedules"], list):
        raise F.Refused("Host inventory requires host, observed_at, complete, schedules and evidence")
    host = F.text(report["host"], "host")
    evidence = F.text(report["evidence"], "host inspection evidence")
    observed_at = _observed(store, report["observed_at"])
    with store.transaction() as data:
        job = _job(data, token)
        if job["state"] not in {"inspect", "apply", "uncertain"}:
            raise F.Refused("This installation is finished; invoke install again for another check")
        old = data["daily"]["binding"]
        if report["complete"] is not True or len(report["schedules"]) > 1:
            if job["state"] == "inspect":
                job.update(state="blocked", reason="Host inventory unavailable, incomplete or ambiguous", evidence=evidence)
            return {"state": "blocked", "action": "none", "reason": "Inspect the missing host capability or conflicting schedules; do not create one."}
        if old and old["host"] != host and (old["state"] != "missing" or
                store.now() - F.T.parse_time(old["observed_at"]) > dt.timedelta(hours=24)):
            raise F.Refused("Inspect the already bound host before proposing a different scheduler")
        if not report["schedules"]:
            if job["state"] != "inspect":
                job.update(state="uncertain", evidence=evidence)
                return {"state": "needs_reconciliation", "action": "none", "reason": "A previous host mutation may have succeeded; an empty inventory does not authorize repeating it."}
            if old and old["host"] == host:
                old.update(state="missing", observed_at=observed_at, evidence=evidence)
            candidate, action = None, "create"
            time = job["time"] or "09:00"
        else:
            candidate = _schedule(report["schedules"][0], data)
            if old and old["id"] != candidate["id"] and old["state"] != "missing":
                raise F.Refused("The bound id and inspected candidate disagree; reconcile the old schedule first")
            if candidate["state"] == "paused" and not job["resume"]:
                _bind(data, host, candidate, observed_at, evidence, job["template_hash"])
                job.update(state="complete", outcome="paused", evidence=evidence)
                return {"state": "paused", "action": "none", "binding": data["daily"]["binding"],
                        "instruction": "Preserved the user's pause. Explicit resume can enable it."}
            if candidate["state"] == "paused" and job["resume"] and job["state"] == "uncertain":
                return {"state": "needs_reconciliation", "action": "none",
                        "reason": "Resume was requested but the host is still paused. Verify that the previous request is no longer pending, then reconcile and retry the requested resume."}
            if candidate["prompt"] != job["prompt"]:
                if job["prompt"] in candidate["prompt"]:
                    job["prompt"] = candidate["prompt"]  # Preserve user additions verbatim.
                elif not (old and old.get("managed_prompt") is True and old.get("prompt_hash") == F.digest(candidate["prompt"])):
                    if job["state"] == "inspect":
                        job.update(state="blocked", reason="Existing prompt needs review before replacement", evidence=evidence)
                    return {"state": "needs_review", "action": "none",
                            "reason": "This existing review has a custom or unrecognized older prompt. Preserve it and resolve the proposed prompt change before replacing it."}
            time = job["time"] or candidate["time"]
            if candidate["state"] == "active" and candidate["cadence"] == "daily" and candidate["prompt"] == job["prompt"] \
                    and candidate["time"] == time and candidate["timezone"] == data["config"]["timezone"]:
                _bind(data, host, candidate, observed_at, evidence, job["template_hash"])
                job.update(state="complete", outcome="already_installed", evidence=evidence)
                return {"state": "already_installed", "action": "none", "binding": data["daily"]["binding"]}
            if job["state"] != "inspect":
                return {"state": "needs_reconciliation", "action": "none", "reason": "The previous mutation has no matching readback; do not repeat it."}
            action = "update"
        job.update(state="apply", host=host, action=action, target=candidate["id"] if candidate else None,
                   expected_time=time, evidence=evidence, inspected_at=observed_at)
        return {"state": "needs_host", "action": action, "token": token, "host": host,
                "id": job["target"], "name": "kpopper daily review " + data["workspace_key"][:12],
                "marker": marker(store), "schedule_state": "active", "cadence": "daily", "time": time, "timezone": data["config"]["timezone"],
                "prompt": job["prompt"], "instruction": "Create or update this schedule once, explicitly setting its state to active (including an authorized resume). Then independently read it back and submit --result. Do not retry an uncertain remote response."}


def finish(store, token, report):
    report = F.T.normalize(report)
    fields = {"host", "observed_at", "schedule", "evidence"}
    if not isinstance(report, dict) or set(report) != fields:
        raise F.Refused("Installation result requires host, observed_at, schedule and evidence")
    host = F.text(report["host"], "host")
    evidence = F.text(report["evidence"], "readback evidence")
    observed_at = _observed(store, report["observed_at"])
    with store.transaction() as data:
        job = _job(data, token)
        schedule = _schedule(report["schedule"], data)
        if job["state"] == "complete":
            if job.get("receipt") == report:
                return {"state": "already_recorded", "binding": data["daily"]["binding"]}
            raise F.Refused("A different installation result is already recorded")
        if job["state"] not in {"apply", "uncertain"} or host != job.get("host"):
            raise F.Refused("No host mutation is pending for this installation")
        if schedule["state"] != "active" or schedule["cadence"] != "daily" or schedule["prompt"] != job["prompt"] \
                or schedule["timezone"] != data["config"]["timezone"] or schedule["time"] != job["expected_time"] \
                or (job["target"] and schedule["id"] != job["target"]):
            raise F.Refused("Host readback does not match the requested daily review")
        _bind(data, host, schedule, observed_at, evidence, job["template_hash"])
        job.update(state="complete", outcome="installed", receipt=report, finished_at=F.stamp(store.now()))
        return {"state": "installed", "binding": data["daily"]["binding"],
                "first_run_verified": False,
                "instruction": "Configuration was read back successfully. A future scheduled execution still needs its own runtime evidence."}


def fail(store, token, reason, unchanged=False):
    F.text(reason, "failure reason")
    with store.transaction() as data:
        job = _job(data, token)
        if job["state"] not in {"inspect", "apply", "uncertain"}:
            raise F.Refused("This installation has already finished")
        if unchanged and job["state"] == "uncertain":
            raise F.Refused("An uncertain host operation needs reconciliation, not an assertion that nothing changed")
        job.update(state="uncertain" if job["state"] in {"apply", "uncertain"} and not unchanged else "blocked",
                   reason=reason, no_host_change=unchanged or job["state"] == "inspect")
        return {"state": job["state"], "instruction": "Report the exact blocker. Inspect any uncertain host operation before another attempt."}


def reconcile(store, token, report):
    """Retire a resolved host attempt; an empty inventory alone is insufficient."""
    report = F.T.normalize(report)
    fields = {"host", "observed_at", "complete", "schedules", "no_pending_request", "evidence"}
    if not isinstance(report, dict) or set(report) != fields or report["complete"] is not True \
            or report["no_pending_request"] is not True or not isinstance(report["schedules"], list) \
            or len(report["schedules"]) > 1:
        raise F.Refused("Reconciliation requires complete host inventory and evidence that the previous request is no longer pending")
    host = F.text(report["host"], "host")
    evidence = F.text(report["evidence"], "reconciliation evidence")
    observed_at = _observed(store, report["observed_at"])
    with store.transaction() as data:
        job = data["daily"].get("installation")
        if not job or job["token"] != token or job["state"] not in {"inspect", "apply", "uncertain"}:
            raise F.Refused("No outstanding installation belongs to this token")
        if job.get("host") and job["host"] != host:
            raise F.Refused("Reconcile the host that received the original request")
        old = data["daily"]["binding"]
        if report["schedules"]:
            schedule = _schedule(report["schedules"][0], {**data, "config": job["workspace_config"]})
            if job.get("target") and schedule["id"] != job["target"]:
                raise F.Refused("The original target schedule has not been reconciled")
            _bind(data, host, schedule, observed_at, evidence, job["template_hash"])
        elif old and old["host"] == host:
            old.update(state="missing", observed_at=observed_at, evidence=evidence)
        job.update(state="reconciled", reconciliation=report, finished_at=F.stamp(store.now()))
        return {"state": "reconciled", "action": "inspect_again",
                "requested_options": {"time": job["time"], "resume": job["resume"], "timezone": job["workspace_config"]["timezone"]},
                "instruction": "The previous host operation is resolved. Invoke install again with the user's requested options and inspect fresh host state before applying anything."}
