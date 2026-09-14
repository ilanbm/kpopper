"""Durable followups linked to a workspace's knowledge, independent of task storage.

Scans are read-only. Claims coordinate host agents; nothing in this module executes
task instructions, creates schedules, contacts providers, or changes the graph.
"""
import contextlib
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile
import uuid
from zoneinfo import ZoneInfo

import yaml

try:
    from . import workspace as W, provenance as P, followup_triggers as T, page_measurements as M
except ImportError:
    import workspace as W
    import provenance as P
    import followup_triggers as T
    import page_measurements as M

UTC = dt.timezone.utc
STATES = {"waiting", "needs_user", "done", "cancelled"}
ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,99}$")
MAX_BYTES = 8 * 1024 * 1024


class Refused(ValueError):
    pass


class StrictLoader(yaml.SafeLoader):
    def construct_mapping(self, node, deep=False):
        self.flatten_mapping(node)
        result = {}
        for key_node, value_node in node.value:
            key = self.construct_object(key_node, deep=deep)
            if not isinstance(key, str) or key in result:
                raise Refused("YAML keys must be unique strings")
            result[key] = self.construct_object(value_node, deep=deep)
        return result


def read_yaml(path):
    path = Path(path)
    if path.stat().st_size > MAX_BYTES:
        raise Refused("File is too large: " + str(path))
    try:
        return yaml.load(path.read_text(encoding="utf-8"), Loader=StrictLoader)
    except yaml.YAMLError as error:
        raise Refused("Unreadable YAML at " + str(path) + ": " + str(error)) from error


def text(value, field):
    if not isinstance(value, str) or not value.strip() or len(value) > 12000:
        raise Refused(field + " must be nonempty text (at most 12000 characters)")
    return value.strip()


def stamp(value):
    return value.astimezone(UTC).isoformat().replace("+00:00", "Z")


def digest(value):
    return hashlib.sha256(json.dumps(T.normalize(value), sort_keys=True, ensure_ascii=False,
                                     allow_nan=False, separators=(",", ":")).encode()).hexdigest()


def atomic(path, value):
    data = yaml.safe_dump(T.normalize(value), allow_unicode=True, sort_keys=False).encode()
    atomic_bytes(path, data)


def atomic_bytes(path, data):
    if len(data) > MAX_BYTES:
        raise Refused("Followups ledger is too large; retain a backup before reducing history")
    fd, name = tempfile.mkstemp(prefix=".followups-", dir=str(path.parent))
    try:
        with os.fdopen(fd, "wb") as out:
            out.write(data)
            out.flush()
            os.fsync(out.fileno())
        os.replace(name, path)
        descriptor = os.open(str(path.parent), os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def _record_view(record):
    doc = P.load([record])
    try:
        ids, judgments, fields = P.infer(doc)
    except (ValueError, SystemExit):
        # A source-only record can still supply explicit scalar readings.
        collections = P.collections_of(doc)
        raw = {key: body for group in collections.values() for key, body in group.items()}
        if any(isinstance(body, dict) and "rests_on" in body for body in raw.values()):
            raise
        ids, judgments, fields = set(raw), {}, {}
    return doc, ids, judgments, fields


def graph(record):
    """Only recorded semantic values; an unreadable graph cannot become an empty success."""
    try:
        doc, ids, judgments, fields = _record_view(record)
        before, page, page_reason = None, {}, ''
        # Ordinary followups do not pay for page/engine cache validation. For page
        # references, reload the record inside the captured measurement boundary.
        if set(ids) & set(P.PAGE):
            try:
                before = M.snapshot([record])
            except (OSError, ValueError, TypeError):
                before, page = None, {}
                page_reason = 'page inputs are unavailable; build the canonical page again'
            doc, ids, judgments, fields = _record_view(record)
            if before is not None:
                page, page_reason = M.read(before)
        raw = P.with_builtins(doc, ids, judgments, fields)
        for key, value in page.items():
            if key in ids:
                raw.setdefault(key, {'name': P.PAGE[key]})['v'] = value
        values = {}
        for key in ids:
            if key in P.PAGE:
                values[key] = page[key] if key in page else {"unavailable": page_reason}
                continue
            try:
                values[key] = T.normalize(P.snapshot_value(key, raw, ids, judgments, page))
            except (P.Refused, ValueError) as error:
                # Keep the ID and the reason, without inventing a null reading or
                # making an unrelated followup depend on this calculation.
                values[key] = {"unavailable": str(error)}
        flags = P.flags(ids, judgments, fields, raw) if judgments else {}
        if before is not None and not M.unchanged(before, M.snapshot([record])):
            raise ValueError('record or page inputs changed during followup read; retry')
        return values, [{"id": key, "reasons": sorted(value)} for key, value in sorted(flags.items()) if value], None
    except (OSError, ValueError, TypeError, AttributeError, yaml.YAMLError, SystemExit) as error:
        return {}, [], "Knowledge record unavailable: " + str(error)


def task_fingerprint(ref):
    if ref.startswith("https://"):
        return None
    path = Path(ref)
    if not path.is_absolute() or not path.is_file() or path.stat().st_size > MAX_BYTES:
        raise Refused("Task file is unavailable: " + ref)
    return hashlib.sha256(path.read_bytes()).hexdigest()


def reference(value):
    value = text(value, "task reference")
    if value.startswith("https://"):
        from urllib.parse import urlsplit
        parsed = urlsplit(value)
        if not parsed.hostname or parsed.username or parsed.password:
            raise Refused("External task reference must be an HTTPS URL without credentials")
        return value
    path = Path(value).expanduser()
    if not path.is_absolute():
        raise Refused("Task files must use absolute paths")
    return str(path.resolve())


def event_values(spec, data):
    return {"external": {ref: {"available": ref in data["observations"],
                               "value": data["observations"].get(ref, {}).get("value")}
                         for ref in T.referenced_external(spec["when"])},
            "completed": {key: data["items"][key]["state"] == "done"
                          for key in T.referenced_tasks(spec["when"])}}


class Store:
    def __init__(self, directory=None, now=None):
        self.location = W.locate(directory)
        configured = os.environ.get("XDG_STATE_HOME", "")
        base = Path(configured) if configured and Path(configured).is_absolute() else Path.home() / ".local/state"
        self.base = base / "kpopper/followups"
        self.registered = False
        requested = Path(directory or Path.cwd()).expanduser().resolve()
        git_root = W._git(requested, "--show-toplevel")
        common = W._git(requested, "--git-common-dir") if git_root else None
        candidates = []
        # Read only small location indexes, never other workspaces' task ledgers.
        for locator in self.base.glob("*/location.yaml"):
            hint = read_yaml(locator)
            if not isinstance(hint, dict) or hint.get("key") != locator.parent.name:
                raise Refused("Invalid followup location index: " + str(locator))
            if common and hint.get("git_common") == str(common):
                anchor = git_root / hint["relative"]
            elif not common and hint.get("git_common") is None:
                anchor = Path(hint["workspace"])
            else:
                continue
            if requested == anchor or anchor in requested.parents:
                candidates.append((len(anchor.parts), hint["key"], anchor))
        if candidates:
            candidates.sort(reverse=True)
            if len(candidates) > 1 and candidates[0][0] == candidates[1][0]:
                raise Refused("More than one followup store owns this workspace; reconcile its location indexes")
            _, key, anchor = candidates[0]
            self.location.update(key=key, workspace=str(anchor))
            self.registered = True
        self.root = self.base / self.location["key"]
        self.path = self.root / "followups.yaml"
        self.now = now or (lambda: dt.datetime.now(UTC))

    def _register(self):
        root = Path(self.location["workspace"])
        git_root = W._git(root, "--show-toplevel")
        common = W._git(root, "--git-common-dir") if git_root else None
        atomic(self.root / "location.yaml", {"key": self.location["key"], "workspace": str(root),
                                             "git_common": str(common) if common else None,
                                             "relative": str(root.relative_to(git_root)) if git_root else None})

    def load(self, required=True, path=None):
        path = Path(path) if path is not None else self.path
        if not path.exists():
            if self.registered:
                raise Refused("Registered followups ledger is unavailable: " + str(self.path) + "; restore it from backup")
            if required:
                raise Refused("Followups are not configured. Run `kpopper followups setup`.")
            return None
        data = read_yaml(path)
        if not isinstance(data, dict) or data.get("version") != 1 or data.get("workspace_key") != self.location["key"]:
            raise Refused("Unknown or mismatched followups ledger; restore it instead of creating a replacement")
        for key in ("config", "items", "observations", "daily"):
            if not isinstance(data.get(key), dict):
                raise Refused("Invalid ledger section: " + key)
        config = data["config"]
        for field in ("record", "workspace", "timezone", "store"):
            text(config.get(field), field)
        ZoneInfo(config["timezone"])
        for key, item in data["items"].items():
            if not ID.fullmatch(key) or not isinstance(item, dict) or item.get("id") != key \
                    or item.get("state") not in STATES or not isinstance(item.get("attempts"), list):
                raise Refused("Invalid followup: " + key)
            self._validate(item["spec"], data, key)
        return data

    def suggested_store(self):
        root = Path(self.location["workspace"])
        common = W._git(root, "--git-common-dir")
        main = common.parent if common and common.name == ".git" else root
        candidate = Path.home() / "docs" / main.name / "followups"
        return str(candidate) if candidate.is_dir() else None

    @contextlib.contextmanager
    def lock(self):
        try:
            import fcntl
        except ImportError as error:
            raise Refused("Followup writes require POSIX file locking; read-only inspection remains available") from error
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        with open(self.root / "ledger.lock", "a+b") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            yield

    @contextlib.contextmanager
    def transaction(self, create=False):
        if not create and not self.path.exists():
            self.load()
        with self.lock():
            data = self.load(required=not create)
            before = digest(data) if data is not None else None
            yield data
            if data is not None and digest(data) != before:
                if self.path.exists():
                    atomic_bytes(self.root / "followups.previous.yaml", self.path.read_bytes())
                atomic(self.path, data)

    def setup(self, store=None, timezone="UTC", record=None, private=False):
        ZoneInfo(timezone)
        rec = Path(record or self.location["record"]).expanduser().resolve()
        if not rec.is_file():
            raise Refused("Create or locate the knowledge record before enabling followups")
        if private and store:
            raise Refused("Choose either an existing store or the private fallback")
        if not store and not private and self.suggested_store() and not self.path.exists():
            raise Refused("An existing followups directory was found: " + self.suggested_store()
                          + ". Confirm its project identity with --store, or deliberately choose --private.")
        destination = reference(store) if store else str(self.root / "items")
        if not destination.startswith("https://"):
            dest = Path(destination)
            workspace = Path(self.location["workspace"])
            if dest == workspace or workspace in dest.parents:
                raise Refused("Choose a followups store outside the product workspace")
            if store and not dest.is_dir():
                raise Refused("An existing task directory must already exist")
        config = {"workspace": self.location["workspace"], "record": str(rec),
                  "store": destination, "timezone": timezone}
        with self.transaction(create=True) as existing:
            if existing:
                if existing["config"] != config:
                    raise Refused("Already configured; preserve the existing store and record. Use `status` to inspect them.")
                self._register()
                return existing["config"]
            data = {"version": 1, "workspace_key": self.location["key"], "config": config,
                    "items": {}, "observations": {},
                    "daily": {"binding": None, "claim": None, "receipts": []}}
            atomic(self.path, data)
            self._register()
        return config

    def _validate(self, spec, data, item_id=None):
        if not isinstance(spec, dict):
            raise Refused("A followup specification must be a mapping")
        allowed = {"id", "title", "why", "how", "task", "related", "when", "scope", "executor"}
        if set(spec) - allowed:
            raise Refused("Unknown followup fields: " + ", ".join(sorted(set(spec) - allowed)))
        key = spec.get("id")
        if not isinstance(key, str) or not ID.fullmatch(key):
            raise Refused("id must contain 1–100 letters, numbers, underscores or dashes")
        text(spec.get("scope"), "authorized scope")
        executor = text(spec.get("executor", "kpopper"), "executor")
        related = spec.get("related")
        if not isinstance(related, list) or not related or len(related) > 100 \
                or not all(isinstance(value, str) and 0 < len(value) <= 200 for value in related) or len(set(related)) != len(related):
            raise Refused("related must be a nonempty unique list of at most 100 graph ids")
        T.validate(spec.get("when"), related)
        if "task" in spec:
            reference(spec["task"])
        else:
            for key in ("title", "why", "how"):
                text(spec.get(key), key)
        refs = T.referenced_tasks(spec["when"])
        if refs - set(data["items"]):
            raise Refused("Unknown prerequisite followups: " + ", ".join(sorted(refs - set(data["items"]))))
        done, active = set(), set()
        stack = [(ref, False) for ref in refs]
        while stack:
            node, leaving = stack.pop()
            if leaving:
                active.remove(node)
                done.add(node)
                continue
            if node == item_id or node in active:
                raise Refused("Followup prerequisites cannot contain a cycle")
            if node in done or node not in data["items"]:
                continue
            active.add(node)
            stack.append((node, True))
            stack.extend((child, False) for child in T.referenced_tasks(data["items"][node]["spec"]["when"]))
        return executor

    def add(self, supplied):
        spec = T.normalize(supplied)
        with self.transaction() as data:
            executor = self._validate(spec, data)
            key = spec["id"]
            if key in data["items"]:
                if data["items"][key]["spec"] == spec:
                    return data["items"][key]
                raise Refused("Followup id already exists; use refresh to change it")
            values, _, error = graph(data["config"]["record"])
            if error or set(spec["related"]) - set(values):
                raise Refused(error or "Some related graph ids do not exist")
            if "task" in spec:
                ref = reference(spec["task"])
                if any(item["task"] == ref for item in data["items"].values()):
                    raise Refused("This canonical task is already linked; refresh its existing followup")
            else:
                dest = data["config"]["store"]
                if dest.startswith("https://"):
                    raise Refused("Create the task through the existing task-system connector, then add its task URL")
                root = Path(dest)
                if root == self.root / "items":
                    root.mkdir(parents=True, exist_ok=True, mode=0o700)
                elif not root.is_dir():
                    raise Refused("The configured task store is unavailable; restore its location: " + dest)
                ref = str(root / ("kp-" + key + ".md"))
                content = ("# " + spec["title"] + "\n\n- **When**: " + json.dumps(spec["when"], ensure_ascii=False)
                           + "\n- **Why**: " + spec["why"] + "\n- **How**: " + spec["how"]
                           + "\n- **Routine**: Managed by kpopper followups " + key
                           + "; use its scan/claim/finish commands.\n")
                target = Path(ref)
                # Recover an interrupted creation only when its exact content agrees.
                if target.exists():
                    if target.read_text(encoding="utf-8") != content:
                        raise Refused("Task filename is already occupied: " + ref)
                else:
                    with open(target, "x", encoding="utf-8") as out:
                        out.write(content)
                        out.flush()
                        os.fsync(out.fileno())
            item = {"id": key, "spec": spec, "task": ref, "task_fingerprint": task_fingerprint(ref),
                    "executor": executor, "state": "waiting", "created_at": stamp(self.now()),
                    "baseline": {key: values[key] for key in spec["related"]}, "next_at": None,
                    "baseline_events": event_values(spec, data),
                    "claim": None, "attempts": [], "generation": 1}
            data["items"][key] = item
            return item

    def _row(self, item, data, values, graph_error, now):
        spec = item["spec"]
        completed = {key for key, value in data["items"].items() if value["state"] == "done"}
        assessment = T.evaluate(spec["when"], values, item["baseline"], completed,
                                data["observations"], now, data["config"]["timezone"])
        reasons = assessment["reasons"][:]
        state = "ready" if assessment["value"] is True else "waiting" if assessment["value"] is False else "unknown"
        if item["next_at"]:
            if T.parse_time(item["next_at"]) <= now:
                if assessment["value"] is not None:
                    state = "ready"
                reasons.append("The explicitly requested followup check is due")
            elif digest({key: values.get(key) for key in spec["related"]}) == digest(item["baseline"]) \
                    and digest(event_values(spec, data)) == digest(item.get("baseline_events", {})):
                if state != "unknown":
                    state = "waiting"
                reasons.append("Next justified check: " + item["next_at"])
        try:
            fp = task_fingerprint(item["task"])
            if fp != item["task_fingerprint"]:
                state = "unknown"
                reasons.append("Task content changed; reread it and explicitly refresh the followup")
        except (OSError, ValueError) as error:
            fp = "unavailable"
            state = "unknown"
            reasons.append(str(error))
        if item["task"].startswith("https://"):
            availability = T.evaluate({"external": {"ref": item["task"], "equals": "open", "max_age_hours": 24}},
                                      values, {}, completed, data["observations"], now)
            if availability["value"] is not True:
                state = "unknown"
                reasons.append("Reread the canonical remote task and observe its current open status before acting")
            assessment["inputs"]["task_observation"] = availability["inputs"]
        if graph_error or any(key not in values for key in spec["related"]):
            state = "unknown"
            reasons.append(graph_error or "A related graph entry is missing")
        unavailable = [key for key in spec["related"] if T.unavailable(values.get(key))]
        if unavailable:
            # related links context; when decides which values readiness needs.
            # A scheduled diagnostic may be the work that resolves this absence.
            reasons.append("Related values are unavailable: " + ", ".join(unavailable))
        claim = item["claim"]
        if claim:
            state = "claimed" if T.parse_time(claim["expires_at"]) > now else "interrupted"
        elif item["state"] in {"done", "cancelled", "needs_user"}:
            state = item["state"]
        elif item["executor"] != "kpopper":
            state = "delegated"
            reasons.append("Execution belongs to " + item["executor"])
        external = T.referenced_external(spec["when"])
        if item["task"].startswith("https://"):
            external.add(item["task"])
        # Freshness controls readiness. A same-value reread or passage of time is
        # not a new decision-bearing input for an already claimed action.
        inputs = {"generation": item["generation"], "task": fp,
                  "related": {key: values.get(key) for key in spec["related"]},
                  "completed": {key: key in completed for key in T.referenced_tasks(spec["when"])},
                  "external": {ref: {"available": ref in data["observations"],
                                      "value": data["observations"].get(ref, {}).get("value")} for ref in external},
                  "next_at": item["next_at"]}
        return {"id": item["id"], "task": item["task"], "state": state, "executor": item["executor"],
                "title": spec.get("title", item["id"])[:160], "reasons": [reason[:400] for reason in reasons[:5]],
                "occurrence": digest(inputs), "next_at": item["next_at"], "wake_hint": assessment.get("next_at"),
                "scope": spec["scope"][:600], "related": spec["related"]}

    def scan(self, limit=20, data=None):
        if type(limit) is not int or not 1 <= limit <= 100:
            raise Refused("limit must be between 1 and 100")
        data = data if data is not None else self.load()
        values, maintenance, error = graph(data["config"]["record"])
        rows = [self._row(item, data, values, error, self.now()) for item in data["items"].values()]
        order = {value: index for index, value in enumerate(("interrupted", "unknown", "needs_user", "ready", "claimed", "delegated", "waiting", "done", "cancelled"))}
        rows.sort(key=lambda row: (order[row["state"]], row["next_at"] or "", row["id"]))
        counts = {state: sum(row["state"] == state for row in rows) for state in order}
        return {"workspace": data["config"]["workspace"], "record": data["config"]["record"],
                "ledger": str(self.path), "counts": counts, "items": rows[:limit],
                "omitted": max(0, len(rows) - limit), "maintenance": maintenance[:1],
                "maintenance_omitted": max(0, len(maintenance) - 1), "graph_error": error,
                "daily": data["daily"]["binding"], "notification": bool(error or any(counts[key] for key in ("interrupted", "unknown", "needs_user", "ready")))}

    def observe(self, report):
        if not isinstance(report, dict) or set(report) != {"ref", "value", "observed_at", "evidence"}:
            raise Refused("An observation needs ref, value, observed_at and evidence")
        ref, evidence = text(report["ref"], "ref"), text(report["evidence"], "evidence")
        if not isinstance(report["observed_at"], (str, dt.datetime)) or \
                isinstance(report["observed_at"], str) and not re.search(r"[Tt ]", report["observed_at"]):
            raise Refused("Observation time must be a timestamp with an explicit offset")
        when = T.parse_time(report["observed_at"])
        if when > self.now():
            raise Refused("An observation cannot come from the future")
        value = {"value": T.normalize(report["value"]), "observed_at": stamp(when), "evidence": evidence}
        if len(json.dumps(value["value"], ensure_ascii=False).encode("utf-8")) > 64 * 1024:
            raise Refused("Observation value exceeds 64 KiB; retain a small state or reading and link the full evidence")
        digest(value)
        with self.transaction() as data:
            old = data["observations"].get(ref)
            if old and (T.parse_time(old["observed_at"]) > when or (old["observed_at"] == stamp(when) and old != value)):
                raise Refused("Observation is older than, or conflicts with, the retained observation")
            data["observations"][ref] = value
        return {"ref": ref, **value}

    def claim(self, key, occurrence, owner, daily_token=None):
        text(owner, "session owner")
        with self.transaction() as data:
            item = self._item(data, key)
            values, _, error = graph(data["config"]["record"])
            row = self._row(item, data, values, error, self.now())
            if row["state"] != "ready" or row["occurrence"] != occurrence:
                raise Refused("Followup is not ready or the scan is stale; scan again (" + row["state"] + ")")
            if daily_token:
                daily = data["daily"]["claim"]
                if not daily or daily["token"] != daily_token or T.parse_time(daily["expires_at"]) <= self.now():
                    raise Refused("The daily review is not live or owned by this token")
                if len(daily["actions"]) >= 3:
                    raise Refused("Daily review's three-action budget is exhausted")
                daily["actions"].append(key)
            claim = {"token": uuid.uuid4().hex, "owner": owner, "started_at": stamp(self.now()),
                     "expires_at": stamp(self.now() + dt.timedelta(minutes=30)), "occurrence": occurrence,
                     "baseline": {key: values[key] for key in item["spec"]["related"]}, "daily_token": daily_token}
            claim["baseline_events"] = event_values(item["spec"], data)
            item["claim"] = claim
            return {**row, "claim": claim, "spec": item["spec"], "record": data["config"]["record"]}

    def _item(self, data, key):
        if key not in data["items"]:
            raise Refused("Unknown followup: " + str(key))
        return data["items"][key]

    def _claim(self, item, token, live=True):
        claim = item["claim"]
        if not claim or claim["token"] != token:
            raise Refused("This run token does not own the followup")
        if live and T.parse_time(claim["expires_at"]) <= self.now():
            raise Refused("Run expired; reconcile its effects and use recover")
        return claim

    def renew(self, key, token):
        with self.transaction() as data:
            item = self._item(data, key)
            claim = self._claim(item, token)
            claim["expires_at"] = stamp(self.now() + dt.timedelta(minutes=30))
            return claim

    def finish(self, key, token, outcome, evidence, next_at=None):
        text(evidence, "outcome evidence")
        if outcome not in {"checked", "done", "cancelled", "needs_user", "released"}:
            raise Refused("Unknown outcome")
        with self.transaction() as data:
            item = self._item(data, key)
            request = {"token": token, "outcome": outcome, "evidence": evidence, "next_at": next_at}
            if any(attempt.get("request") == request for attempt in item["attempts"]):
                return {"id": key, "state": item["state"], "already_recorded": True}
            claim = self._claim(item, token)
            if outcome == "checked":
                if not next_at or T.parse_time(next_at, data["config"]["timezone"]) <= self.now():
                    raise Refused("A completed check needs a justified future next_at")
                next_at = stamp(T.parse_time(next_at, data["config"]["timezone"]))
            elif next_at is not None:
                raise Refused("Only checked outcomes take next_at")
            values, _, error = graph(data["config"]["record"])
            current = self._row(item, data, values, error, self.now())
            stale = error is not None or current["occurrence"] != claim["occurrence"]
            effective = "needs_user" if stale and outcome != "released" else outcome
            item["attempts"].append({"run": claim, "finished_at": stamp(self.now()), "request": request,
                                     "outcome": effective, "evidence": evidence, "inputs_changed": stale})
            item["claim"] = None
            item["state"] = effective if effective in STATES else "waiting"
            if effective == "checked":
                item["baseline"] = claim["baseline"]
                item["baseline_events"] = claim["baseline_events"]
                item["next_at"] = next_at
            return {"id": key, "state": item["state"], "outcome": effective, "inputs_changed": stale}

    def recover(self, key, evidence):
        text(evidence, "reconciliation evidence")
        with self.transaction() as data:
            item = self._item(data, key)
            claim = item["claim"]
            if not claim or T.parse_time(claim["expires_at"]) > self.now():
                raise Refused("Only an interrupted run can be recovered")
            item["attempts"].append({"run": claim, "finished_at": stamp(self.now()),
                                     "outcome": "recovered", "evidence": evidence})
            item["claim"] = None
            item["state"] = "waiting"
            return {"id": key, "state": "waiting"}

    def refresh(self, key, supplied, evidence):
        text(evidence, "reread evidence")
        spec = T.normalize(supplied)
        with self.transaction() as data:
            item = self._item(data, key)
            if item["claim"] or item["state"] in {"done", "cancelled"}:
                raise Refused("An active or closed followup cannot be refreshed")
            if spec.get("id") != key:
                raise Refused("Refresh must preserve the followup id")
            executor = self._validate(spec, data, key)
            ref = reference(spec.get("task", item["task"]))
            if any(other["id"] != key and other["task"] == ref for other in data["items"].values()):
                raise Refused("This canonical task is already linked")
            fp = task_fingerprint(ref)
            values, _, error = graph(data["config"]["record"])
            if error or set(spec["related"]) - set(values):
                raise Refused(error or "Some related graph ids do not exist")
            item["attempts"].append({"outcome": "refreshed", "finished_at": stamp(self.now()),
                                     "evidence": evidence, "previous_spec": item["spec"]})
            item.update(spec=spec, executor=executor, task=ref, task_fingerprint=fp, state="waiting",
                        next_at=None, baseline={key: values[key] for key in spec["related"]},
                        baseline_events=event_values(spec, data), generation=item["generation"] + 1)
            return item

    def relocate(self, record, evidence):
        """Deliberately replace a lost/moved pinned record; never migrate task files."""
        text(evidence, "relocation evidence")
        path = Path(record).expanduser().resolve()
        values, _, error = graph(str(path))
        if error:
            raise Refused(error)
        with self.transaction() as data:
            if data["daily"]["claim"] or any(item["claim"] for item in data["items"].values()):
                raise Refused("Reconcile all active/interrupted runs before relocating the record")
            related = {key for item in data["items"].values() for key in item["spec"]["related"]}
            if related - set(values):
                raise Refused("The new record does not contain the existing related graph ids")
            data.setdefault("relocations", []).append({"previous": data["config"].copy(),
                                                       "at": stamp(self.now()), "evidence": evidence})
            data["config"].update(record=str(path), workspace=self.location["workspace"])
            self._register()
            return data["config"]

    def resolve(self, key, outcome, evidence):
        """Reconcile work completed/cancelled outside this runner, retaining its evidence."""
        text(evidence, "external outcome evidence")
        if outcome not in {"done", "cancelled"}:
            raise Refused("Resolution must be done or cancelled")
        with self.transaction() as data:
            item = self._item(data, key)
            if item["claim"]:
                raise Refused("Reconcile the active or interrupted run before resolving its task")
            if item["state"] in {"done", "cancelled"}:
                if item["state"] == outcome:
                    return {"id": key, "state": outcome, "already_recorded": True}
                raise Refused("The obligation is already closed with another outcome")
            item["attempts"].append({"outcome": outcome, "evidence": evidence,
                                     "finished_at": stamp(self.now()), "reconciled_external": True})
            item["state"] = outcome
            return {"id": key, "state": outcome}

    def resume(self, key, evidence):
        """Resume reconciled parked work without erasing its trigger baseline or retry."""
        text(evidence, "resume evidence")
        with self.transaction() as data:
            item = self._item(data, key)
            if item["claim"] or item["state"] != "needs_user":
                raise Refused("Only parked, unclaimed work can be resumed")
            item["attempts"].append({"outcome": "resumed", "evidence": evidence, "at": stamp(self.now())})
            item["state"] = "waiting"
            return {"id": key, "state": "waiting", "next_at": item["next_at"]}

    def restore(self, backup, evidence):
        """Recover an inspected backup without silently replaying uncertain effects."""
        text(evidence, "restore evidence")
        with self.lock():
            recovered = self.load(path=backup)
            past = stamp(self.now() - dt.timedelta(seconds=1))
            for item in recovered["items"].values():
                if item["claim"]:
                    item["claim"]["expires_at"] = past
                elif item["state"] not in {"done", "cancelled"}:
                    item["state"] = "needs_user"
            if recovered["daily"]["claim"]:
                recovered["daily"]["claim"]["expires_at"] = past
            quarantine = None
            if self.path.exists():
                quarantine = self.root / ("quarantine-" + uuid.uuid4().hex + ".yaml")
                atomic_bytes(quarantine, self.path.read_bytes())
            recovered.setdefault("restorations", []).append({"at": stamp(self.now()), "backup": str(Path(backup).resolve()),
                                                             "evidence": evidence, "quarantine": str(quarantine) if quarantine else None})
            atomic(self.path, recovered)
            self._register()
            return {"restored": str(self.path), "quarantine": str(quarantine) if quarantine else None,
                    "message": "Unfinished work requires reconciliation; interrupted claims remain interrupted."}


def summary(location, counts_only=False):
    """Optional bounded opening/event context. No setup, read receipts or actions."""
    store = Store(location["workspace"])
    if not store.path.exists() and not store.registered:
        return ""
    result = store.scan(limit=3)
    visible = [row for row in result["items"] if row["state"] not in {"waiting", "done", "cancelled"}]
    if not visible and not result["graph_error"]:
        return ""
    if counts_only:
        return "KPOPPER_FOLLOWUPS " + json.dumps({"counts": result["counts"], "record": result["record"]}, ensure_ascii=False) \
            + "\n`kpopper followups scan` explains which work is ready and the dependencies behind it."
    compact = [{"id": row["id"], "state": row["state"], "title": row["title"],
                "reasons": [reason[:240] for reason in row["reasons"][:2]]} for row in visible]
    return "KPOPPER_FOLLOWUPS " + json.dumps({"counts": result["counts"], "items": compact,
                                              "record": result["record"], "graph_error": result["graph_error"]}, ensure_ascii=False) \
        + "\nRead the canonical task, rescan and claim before acting within the user's authorized scope. `kpopper followups scan` shows the full queue."
