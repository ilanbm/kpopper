#!/usr/bin/env python3
"""Durable, selective ingestion for explicit provenance reports.

Captured text and queue state live outside the repository by default.  A report may
update one existing scalar reading or an atomic batch of readings and new grounded
entries. Interpretation belongs to the calling agent. Unsupported or ambiguous changes
are retained for review; existing judgments are never rewritten by ingestion.

The implementation requires ``fcntl.flock`` for capture, processing, and acknowledgement,
matching provenance.py's directory lock.  Those writes fail closed on platforms without
``fcntl``; read-only status and pending inspection remain available.
"""
import argparse
import contextlib
import copy
import datetime
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time
import uuid


HERE = Path(__file__).resolve().parent
_SPEC = importlib.util.spec_from_file_location("kpopper_ingestion_provenance", HERE / "provenance.py")
P = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(P)

TERMINAL = {"applied", "needs_primary", "superseded", "error"}
ACTIONABLE = {"MOVED", "UNCHECKED", "BROKEN", "BLOCKED", "UNKNOWN"}
AUTO_STATES = {"captured", "processing"}
EVENT_FIELDS = {"event_id", "session_id", "source_quote", "target", "value", "date",
                "kind", "question", "reason", "updates", "record_sha256",
                "shareability", "privacy", "scope"}
MAX_REPREPARES = 2


def _sha(data):
    return hashlib.sha256(data).hexdigest()


def _json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True,
                       separators=(",", ":")) + "\n").encode("utf-8")


def _body_hash(body):
    """Hash YAML metadata semantically, including native dates and other safe scalars."""
    return _sha(P.yaml.safe_dump(body, allow_unicode=True, sort_keys=True).encode("utf-8"))


def _private_dir(path):
    path = Path(path)
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    try:
        path.chmod(0o700)
    except OSError:
        pass
    return path


def _atomic(path, data, mode=0o600):
    path = Path(path)
    _private_dir(path.parent)
    fd, name = tempfile.mkstemp(prefix="." + path.name + ".", dir=str(path.parent))
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "wb") as out:
            out.write(data)
            out.flush()
            os.fsync(out.fileno())
        os.replace(name, str(path))
        try:
            dfd = os.open(str(path.parent), os.O_RDONLY)
            try:
                os.fsync(dfd)
            finally:
                os.close(dfd)
        except OSError:
            pass
    finally:
        if os.path.exists(name):
            os.unlink(name)


def _save(path, value):
    _atomic(path, _json_bytes(value))


def _load(path, default=None):
    try:
        with io.open(str(path), encoding="utf-8") as src:
            return json.load(src)
    except FileNotFoundError:
        return default


class LockingUnavailable(RuntimeError):
    """This platform cannot serialize durable ingestion writes."""


def _require_locking():
    try:
        import fcntl
    except ImportError as exc:
        raise LockingUnavailable("durable ingestion writes require fcntl file locking") from exc
    return fcntl


@contextlib.contextmanager
def _file_lock(path):
    fcntl = _require_locking()
    path = Path(path)
    _private_dir(path.parent)
    fd = os.open(str(path), os.O_CREAT | os.O_RDWR, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)


def _record_path(record=None):
    selected = P.default_paths() if record is None else [str(record)]
    if len(selected) != 1:
        raise ValueError("ingestion requires one record path")
    return Path(selected[0]).expanduser().resolve()


def state_path(record=None, state_dir=None):
    """Return the private state directory for a record without creating it."""
    rec = _record_path(record)
    if state_dir is not None:
        return Path(state_dir).expanduser().resolve()
    configured = os.environ.get("XDG_STATE_HOME")
    base = Path(configured) if configured and Path(configured).is_absolute() \
        else Path.home() / ".local" / "state"
    return (base / "kpopper" / "ingestion" / _sha(str(rec).encode("utf-8"))).resolve()


def _layout(record=None, state_dir=None):
    _require_locking()  # Fail before creating/chmod'ing state on unsupported platforms.
    rec = _record_path(record)
    root = state_path(rec, state_dir)
    try:
        rec.relative_to(root)
    except ValueError:
        pass
    else:
        raise ValueError("state directory cannot contain the provenance record")
    marker = root / "record.json"
    if root.exists():
        if not root.is_dir():
            raise ValueError("state path exists and is not a directory")
        if marker.is_file():
            if _load(marker) != {"record": str(rec)}:
                raise ValueError("state directory belongs to another provenance record")
        elif any(root.iterdir()):
            raise ValueError("existing non-empty state directory is not owned by ingestion")
    _private_dir(root)
    expected = {"record": str(rec)}
    if not marker.is_file():
        _save(marker, expected)
    for name in ("envelopes", "sources", "events", "journals", "drafts", "results",
                 "signals", "receipts"):
        _private_dir(root / name)
    return rec, root


def _existing_layout(record=None, state_dir=None):
    """Resolve state for read APIs without creating private storage."""
    rec = _record_path(record)
    root = state_path(rec, state_dir)
    marker = root / "record.json"
    if not marker.is_file():
        return rec, root, False
    if _load(marker) != {"record": str(rec)}:
        raise ValueError("state directory belongs to another provenance record")
    return rec, root, True


def _validate_envelope(envelope):
    if not isinstance(envelope, dict):
        raise ValueError("capture envelope must be a JSON object")
    unknown = set(envelope) - EVENT_FIELDS
    if unknown:
        raise ValueError("unknown envelope fields: " + ", ".join(sorted(unknown)))
    quote = envelope.get("source_quote")
    if not isinstance(quote, str) or not quote.strip():
        raise ValueError("source_quote must be non-empty text")
    out = dict(envelope)
    if out.get("record_sha256") is not None and (not isinstance(out["record_sha256"], str)
            or not re.fullmatch(r"[0-9a-f]{64}", out["record_sha256"])):
        raise ValueError("record_sha256 must be the hash returned by the prior record read")
    for field in ("event_id", "session_id", "target", "question", "reason", "kind"):
        if field in out and out[field] is not None and not isinstance(out[field], str):
            raise ValueError(field + " must be text")
    if "updates" in out:
        if "target" in out or "value" in out:
            raise ValueError("use updates or target/value, not both")
        updates = out["updates"]
        if not isinstance(updates, list) or not 1 <= len(updates) <= 32:
            raise ValueError("updates must contain 1..32 operations")
        names = set()
        for op in updates:
            if not isinstance(op, dict) or op.get("kind") not in {"set", "add"}:
                raise ValueError("each update needs kind=set or kind=add")
            allowed = {"kind", "id", "at", "value"} if op["kind"] == "set" else {"kind", "id", "at", "body", "into"}
            required = "value" if op["kind"] == "set" else "body"
            if set(op) - allowed or required not in op:
                raise ValueError("invalid fields in " + op["kind"] + " update")
            name = op.get("id")
            if not isinstance(name, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*", name):
                raise ValueError("each update needs a valid entry id")
            if name in names:
                raise ValueError("one update per id in a batch: " + name)
            names.add(name)
            for field in ("at", "into"):
                if field in op and (not isinstance(op[field], str) or not op[field].strip()):
                    raise ValueError(field + " must be non-empty text")
            if op["kind"] == "add" and not isinstance(op["body"], dict):
                raise ValueError("add body must be a mapping")
    return out


def _basic_type(value):
    if type(value) is bool:
        return "boolean"
    if type(value) is int:
        return "integer"
    if type(value) is float:
        return "number"
    if type(value) is str:
        return "string"
    if value is None:
        return "null"
    return None


def _record_world(record):
    paths = [str(record)]
    files = [Path(x).resolve() for x in P._files_of(paths)]
    if len(files) != 1 or files[0] != record.resolve():
        raise ValueError("multi-file and pointer records require primary review")
    doc = P.load(paths)
    if doc.hypotheses:
        raise ValueError("a record with hypothesis context requires primary review")
    ids, judgments, fields = P.infer(doc)
    raw = P.with_builtins(doc, ids, judgments, fields)
    return doc, ids, judgments, fields, raw


def _target(record, name):
    if not isinstance(name, str) or not name:
        raise ValueError("the report does not identify one existing target")
    doc, ids, judgments, fields, raw = _record_world(record)
    bodies = P.bodies(doc)
    body = bodies.get(name)
    if name not in ids or not isinstance(body, dict):
        raise ValueError(f"{name} is not an existing stored entry")
    if name in judgments or fields.get("deps") in body:
        raise ValueError(f"{name} is a judgment and cannot be rewritten by ingestion")
    if body.get("rule") is not None:
        raise ValueError(f"{name} is worked out by a rule and cannot be rewritten")
    own = [field for field in ("v", "quoted") if field in body]
    if len(own) != 1:
        raise ValueError(f"{name} does not have one stored scalar value")
    value = body[own[0]]
    if _basic_type(value) is None:
        raise ValueError(f"{name} is not a scalar reading")
    if isinstance(value, str) and P.EXPR.search(value) and \
            any(token in ids for token in P.ID.findall(value)):
        raise ValueError(f"{name} is worked out by an inline expression and cannot be rewritten")
    collections = P.collections_of(doc)
    source_collections = []
    for collection, members in collections.items():
        if any(isinstance(candidate, dict) and candidate_id not in judgments
               and not any(field in candidate for field in ("v", "quoted", "rule"))
               and any(candidate.get(field) for field in ("asked", "file", "url", "read"))
               for candidate_id, candidate in members.items()):
            source_collections.append(collection)
    cited = body.get("from")
    cited_homes = []
    for collection, members in collections.items():
        candidate = members.get(cited) if isinstance(cited, str) else None
        if isinstance(candidate, dict) and cited not in judgments and \
                not any(field in candidate for field in ("v", "quoted", "rule")):
            cited_homes.append(collection)
    if len(cited_homes) == 1:
        source_collection = cited_homes[0]
    elif len(source_collections) == 1:
        source_collection = source_collections[0]
    else:
        raise ValueError("the record does not identify one existing source collection")
    return {
        "id": name,
        "value": value,
        "type": _basic_type(value),
        "body_sha256": _body_hash(body),
        "source_collection": source_collection,
        "source": body.get("from"),
    }


def _report_seeds(envelope):
    return [op["id"] for op in envelope["updates"]] if "updates" in envelope else envelope.get("target")


def _brief_fingerprint(record):
    brief = P._brief_beside(str(record))
    return _sha(Path(brief).read_bytes()) if brief else None


def _batch_fingerprint(record, updates):
    doc, _, _, _, _ = _record_world(record)
    raw = P.bodies(doc)
    return _body_hash({op["id"]: raw.get(op["id"]) for op in updates})


def _report_target(record, envelope):
    """Reading updates bind their targets; new claims bind the whole interpreted record."""
    expected = envelope.get("record_sha256")
    has_additions = any(op["kind"] == "add" for op in envelope.get("updates", []))
    if has_additions and expected is None:
        raise ValueError("new entries require record_sha256 from the primary's prior open --json or search")
    if expected is not None and _sha(Path(record).read_bytes()) != expected:
        raise ValueError("record changed since the primary read it; reread the premises before resubmitting")
    if "updates" not in envelope:
        return _target(record, envelope.get("target"))
    doc, ids, judgments, fields, raw = _record_world(record)
    homes = set()
    for op in envelope["updates"]:
        if op["kind"] == "set":
            old = _target(record, op["id"])
            if _basic_type(op["value"]) != old["type"]:
                raise ValueError(op["id"] + " has a different scalar type")
            homes.add(old["source_collection"])
        else:
            if op["id"] in ids:
                raise ValueError(op["id"] + " already exists; batch add never replaces an entry or judgment")
            body = op["body"]
            if fields.get("snapshot") in body or "seen" in body:
                raise ValueError("judgment snapshots are computed by the writer")
            if any(key in body for key in ("from", "at", "of", "src", "source")):
                raise ValueError("batch readings cite the captured report; use update.at for its location")
            stored = [key for key in ("v", "quoted") if key in body]
            derived = "rule" in body
            judgment = fields.get("deps") in body
            if judgment and isinstance(body.get(fields["deps"]), list) and any(
                    isinstance(dep, str) and P.is_builtin(dep) for dep in body[fields["deps"]]):
                raise ValueError("batch judgments about reader/page counts require primary review; use domain entries as premises")
            if len(stored) + int(derived) + int(judgment) != 1:
                raise ValueError("add one stored reading, rule, or judgment per entry")
            if stored and _basic_type(body[stored[0]]) is None:
                raise ValueError("new stored readings must be scalar")
    if not homes:
        for collection, members in P.collections_of(doc).items():
            if any(isinstance(body, dict) and nid not in judgments
                   and not any(key in body for key in ("v", "quoted", "rule"))
                   and any(body.get(key) for key in ("file", "url", "asked", "read"))
                   for nid, body in members.items()):
                homes.add(collection)
    if len(homes) != 1:
        raise ValueError("the batch needs one unambiguous existing source collection")
    fingerprint = _sha(Path(record).read_bytes()) if has_additions else _batch_fingerprint(record, envelope["updates"])
    return {"body_sha256": fingerprint,
            "source_collection": next(iter(homes)), "type": "batch"}


def _event_id(record, envelope):
    supplied = envelope.get("event_id")
    if supplied is not None:
        if not supplied.strip():
            raise ValueError("event_id must be non-empty when supplied")
        seed = str(record) + "\0" + supplied
        return _sha(seed.encode("utf-8"))[:32]
    return uuid.uuid4().hex


def _event_file(root, eid):
    return root / "events" / (eid + ".json")


def _summary(event):
    return {k: event.get(k) for k in
            ("event_id", "state", "target", "captured_at", "finished_at", "reason")}


def capture(envelope, record=None, state_dir=None, start=True):
    """Durably retain one explicit report and optionally start a finite worker."""
    envelope = _validate_envelope(envelope)
    rec, root = _layout(record, state_dir)
    eid = _event_id(rec, envelope)
    encoded = _json_bytes(envelope)
    with _file_lock(root / "capture.lock"):
        prior = _load(_event_file(root, eid))
        if prior is not None:
            if _load(root / "envelopes" / (eid + ".json")) != envelope:
                raise ValueError("event_id was reused for different input")
            if start and prior.get("state") in AUTO_STATES:
                _start_worker_locked(rec, root)
            return _summary(prior)
        with P._locked(str(rec)):
            record_hash = _sha(rec.read_bytes()) if rec.is_file() else None
            issue = None
            snapshot = None
            try:
                snapshot = _report_target(rec, envelope)
            except (Exception, SystemExit) as exc:
                issue = " ".join(str(exc).split())
        counter_path = root / "counter.json"
        counter = int((_load(counter_path) or {}).get("value", 0)) + 1
        _save(counter_path, {"value": counter})
        source = root / "sources" / (eid + ".txt")
        source_bytes = envelope["source_quote"].encode("utf-8")
        _atomic(source, source_bytes)
        _atomic(root / "envelopes" / (eid + ".json"), encoded)
        event = {
            "event_id": eid, "state": "captured", "target": envelope.get("target"),
            "captured_at": time.time(), "order": counter, "record_hash": record_hash,
            "target_snapshot": snapshot, "capture_issue": issue,
            "source_file": str(source), "source_sha256": _sha(source_bytes),
            "envelope_sha256": _sha(encoded), "attempts": 0,
        }
        _save(_event_file(root, eid), event)
        if start:
            _start_worker_locked(rec, root)
    return _summary(event)


def _graph(record, target=None, measured=False):
    doc, ids, judgments, fields, raw = _record_world(record)
    measurement = None
    if measured and set(ids) & set(P.PAGE):
        try:
            from . import page_measurements as M
        except ImportError:
            import page_measurements as M
        measurement = M.snapshot([str(record)])
        doc, ids, judgments, fields, raw = _record_world(record)
        page, _ = M.read(measurement)
        for key, value in page.items():
            if key in ids:
                raw.setdefault(key, {'name': P.PAGE[key]})['v'] = value
    if target:
        seeds = target if isinstance(target, list) else [target]
        hit, touched, derived = P.reach_of(ids, judgments, raw, seeds)
        for seed in seeds:
            if seed in judgments:
                hit.setdefault(seed, seed)
    else:
        hit, touched, derived = {}, set(), []
    states = {}
    for jid, judgment in judgments.items():
        tag, reason = P._state(jid, judgment, raw, ids, fields, touched=touched)
        states[jid] = {
            "tag": tag, "reason": reason,
            "evaluation": P.evaluate(judgment["pred"], raw, ids),
            "name": P.named(judgment["body"]),
            "verdict": judgment["body"].get("verdict"),
        }
    if measurement is not None and not M.unchanged(measurement, M.snapshot([str(record)])):
        raise ValueError('record or page inputs changed during pending read; retry')
    return {
        "hash": _sha(Path(record).read_bytes()), "judgments": states,
        "reach": {"judgments": sorted(hit), "via": hit,
                  "touched": sorted(touched), "derived": derived},
    }


def _classify(before, after):
    reached = after["reach"]["judgments"]
    fired, actionable = [], []
    for jid in reached:
        old = before["judgments"].get(jid, {})
        now = after["judgments"][jid]
        if old.get("evaluation") is not True and now["evaluation"] is True:
            fired.append(jid)
        elif now["tag"] in ACTIONABLE and \
                (old.get("tag"), old.get("reason")) != (now["tag"], now["reason"]):
            actionable.append(jid)
    return fired, actionable


def _signal_id(eid, category, names):
    return _sha((eid + "\0" + category + "\0" + "\0".join(names)).encode("utf-8"))[:32]


def _signals(eid, envelope, after, fired, actionable):
    out = []
    if fired:
        out.append({
            "id": _signal_id(eid, "contradiction", fired), "event_id": eid,
            "category": "contradiction", "target": envelope.get("target"),
            "source_quote": envelope["source_quote"],
            "affected_judgments": after["reach"]["judgments"],
            "newly_fired_judgments": fired, "actionable_judgments": [],
            "reason": "; ".join(after["judgments"][x]["reason"] for x in fired),
        })
    if actionable:
        question = "; ".join(
            f"{x} requires review: {after['judgments'][x]['reason']}" for x in actionable)
        out.append({
            "id": _signal_id(eid, "question", actionable), "event_id": eid,
            "category": "question", "target": envelope.get("target"),
            "source_quote": envelope["source_quote"],
            "affected_judgments": after["reach"]["judgments"],
            "newly_fired_judgments": [], "actionable_judgments": actionable,
            "reason": question, "question": question,
        })
    return out


def _publish(root, result):
    for signal in result["signals"]:
        path = root / "signals" / (signal["id"] + ".json")
        old = _load(path)
        if old is not None and old != signal:
            raise ValueError("published signal conflicts with durable result")
        if old is None:
            _save(path, signal)
    receipt = result["receipt"]
    path = root / "receipts" / (receipt["event_id"] + ".json")
    old = _load(path)
    if old is not None and old != receipt:
        raise ValueError("published receipt conflicts with durable result")
    if old is None:
        _save(path, receipt)


def _finish(root, event, envelope, state, reason, signals=None, **extra):
    signals = list(signals or [])
    eid = event["event_id"]
    receipt = {
        "id": _sha(("receipt\0" + eid).encode("utf-8"))[:32],
        "event_id": eid, "state": state, "target": envelope.get("target"),
        "value": envelope.get("value"),
        "source": "s.ingest_" + eid if state == "applied" else None,
        "source_file": event["source_file"], "reason": reason,
        "source_sha256": event.get("source_sha256"),
        "envelope_sha256": event.get("envelope_sha256"),
        "signal_ids": [x["id"] for x in signals],
        **extra,
    }
    if "updates" in envelope:
        receipt["updates"] = envelope["updates"]
    result = {"receipt": receipt, "signals": signals}
    _save(root / "results" / (eid + ".json"), result)
    _publish(root, result)
    event.update(state=state, reason=reason, finished_at=time.time())
    _save(_event_file(root, eid), event)
    return receipt


def _question(root, event, envelope, reason, **extra):
    eid = event["event_id"]
    question = envelope.get("question") or reason
    signal = {
        "id": _signal_id(eid, "question", [question]), "event_id": eid,
        "category": "question", "target": envelope.get("target"),
        "source_quote": envelope["source_quote"], "affected_judgments": [],
        "newly_fired_judgments": [], "actionable_judgments": [],
        "reason": reason, "question": question,
    }
    return _finish(root, event, envelope, "needs_primary", reason, [signal], reach={}, **extra)


def _capture_payload(root, event):
    """Read a capture only when its envelope and plain-text source retain their bytes."""
    eid = event["event_id"]
    envelope_path = root / "envelopes" / (eid + ".json")
    source_path = root / "sources" / (eid + ".txt")
    try:
        envelope_bytes = envelope_path.read_bytes()
        source_bytes = source_path.read_bytes()
    except OSError as exc:
        return None, "captured input is unavailable: " + " ".join(str(exc).split())
    if str(source_path) != event.get("source_file"):
        return None, "captured source path changed after capture"
    if _sha(envelope_bytes) != event.get("envelope_sha256"):
        return None, "captured envelope changed after capture"
    if _sha(source_bytes) != event.get("source_sha256"):
        return None, "captured source text changed after capture"
    try:
        return json.loads(envelope_bytes.decode("utf-8")), None
    except (UnicodeError, json.JSONDecodeError) as exc:
        return None, "captured envelope cannot be read: " + " ".join(str(exc).split())


def recording_source(nid, body):
    """A recording-purpose exemption belongs only to a retained, intact capture."""
    if not isinstance(nid, str) or not nid.startswith('s.ingest_'):
        return False
    eid = nid[len('s.ingest_'):]
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', eid) or not isinstance(body.get('file'), str):
        return False
    source = Path(body['file'])
    if not source.is_absolute() or source.parent.name != 'sources' or source.name != eid + '.txt':
        return False
    root = source.parent.parent
    try:
        event = _load(_event_file(root, eid))
        if not isinstance(event, dict) or event.get('event_id') != eid or event.get('source_file') != str(source):
            return False
        _, error = _capture_payload(root, event)
    except (OSError, ValueError):
        return False
    return error is None


def _integrity_question(root, event, reason, record_committed=False):
    envelope_path = root / "envelopes" / (event["event_id"] + ".json")
    source_path = root / "sources" / (event["event_id"] + ".txt")
    envelope = {"target": event.get("target"), "source_quote": ""}
    try:
        envelope_bytes = envelope_path.read_bytes()
        if _sha(envelope_bytes) == event.get("envelope_sha256"):
            envelope = json.loads(envelope_bytes.decode("utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        pass
    if not envelope.get("source_quote"):
        try:
            source_bytes = source_path.read_bytes()
            if _sha(source_bytes) == event.get("source_sha256"):
                envelope["source_quote"] = source_bytes.decode("utf-8")
        except (OSError, UnicodeError):
            pass
    return _question(root, event, envelope, reason, record_committed=record_committed,
                     record_source=("s.ingest_" + event["event_id"] if record_committed else None))


def _owned_predecessor(root, event, current_target):
    """Whether the current target is exactly an earlier applied ingestion result."""
    source = current_target.get("source")
    prefix = "s.ingest_"
    if not isinstance(source, str) or not source.startswith(prefix):
        return False
    prior_id = source[len(prefix):]
    prior_event = _load(_event_file(root, prior_id))
    prior = _load(root / "receipts" / (prior_id + ".json"))
    return bool(prior_event and prior and prior.get("state") == "applied"
                and prior.get("target") == event.get("target")
                and prior_event.get("order", 0) < event.get("order", 0)
                and prior.get("target_after_sha256") == current_target.get("body_sha256"))


class PreparationRefused(ValueError):
    """The staged record failed checks; retain the actionable prospective failures."""
    def __init__(self, issues, diagnostics):
        self.issues = issues
        self.diagnostics = diagnostics
        super().__init__('prepared update failed checks: ' + '\n'.join(issues))


def _prepare(rec, root, event, envelope, before_bytes):
    eid = event["event_id"]
    draft = root / "drafts" / (eid + "-" + uuid.uuid4().hex)
    _private_dir(draft)
    shadow = draft / rec.name
    _atomic(shadow, before_bytes)
    brief = P._brief_beside(str(rec))
    if brief:
        _atomic(Path(P.layout(shadow)["view"]), Path(brief).read_bytes())
    mark = draft / "mark.json"
    paths = [str(shadow)]
    source_id = "s.ingest_" + eid
    date = envelope["date"]
    diagnostics = []
    gate_output = io.StringIO()
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        P.mark(str(mark), paths)
        seeds = _report_seeds(envelope)
        P.apply(paths, {
            "kind": "add", "id": source_id, "as_of": date, "why": None,
            "into": event["target_snapshot"]["source_collection"],
            "hypothesis": None, "source": None, "at": None,
            "body": {"name": "Captured report", "file": event["source_file"], "read": date,
                     "recorded_for": "Update " + (", ".join(seeds) if isinstance(seeds, list) else seeds) + " from this captured report."},
        })
        operations = envelope.get("updates", [{"kind": "set", "id": envelope.get("target"),
                                               "value": envelope.get("value")}])
        # Existing readings settle first. New entries follow in dependency order, so a
        # new judgment's snapshot sees the final readings, never an intermediate price.
        operations = sorted(operations, key=lambda op: op["kind"] != "set")
        for op in operations:
            action = {"kind": op["kind"], "id": op["id"], "as_of": date, "why": None,
                      "into": op.get("into"), "hypothesis": None, "source": None, "at": None}
            location = op.get("at", "entire captured report")
            if op["kind"] == "set":
                action.update(value=op["value"], source=source_id, at=location)
            else:
                body = copy.deepcopy(op["body"])
                if "v" in body or "quoted" in body:
                    body.update({"from": source_id, "at": location, "of": date})
                action["body"] = body
            P.apply(paths, action, diagnostics=diagnostics)
        with contextlib.redirect_stdout(gate_output), contextlib.redirect_stderr(gate_output):
            gate = P.gate(str(mark), paths)
    if gate:
        issues = [line.replace(str(shadow), str(rec)) for line in gate_output.getvalue().splitlines() if line.strip()]
        raise PreparationRefused(issues or ['the canonical gate refused the prepared record'], diagnostics)
    after_bytes = shadow.read_bytes()
    if "updates" in envelope:
        target_hash = _batch_fingerprint(shadow, envelope["updates"])
    else:
        target = _target(shadow, envelope["target"])
        if type(target["value"]) is not type(envelope["value"]) or target["value"] != envelope["value"]:
            raise ValueError("prepared target did not read back with the captured scalar value")
        target_hash = target["body_sha256"]
    doc = P.load(paths)
    source = P.bodies(doc).get(source_id)
    if not isinstance(source, dict) or source.get("file") != event["source_file"]:
        raise ValueError("prepared target did not retain its captured source")
    return shadow, after_bytes, _graph(shadow, seeds), target_hash, diagnostics


def _replace_record(record, data):
    mode = record.stat().st_mode & 0o7777
    fd, name = tempfile.mkstemp(prefix="." + record.name + ".ingestion.", dir=str(record.parent))
    try:
        chmod_fd = getattr(os, 'fchmod', None)
        if chmod_fd is not None:
            chmod_fd(fd, mode)
        else:
            os.chmod(name, mode)
        with os.fdopen(fd, "wb") as out:
            out.write(data)
            out.flush()
            os.fsync(out.fileno())
        os.replace(name, str(record))
        P.forget(record)
        try:
            dfd = os.open(str(record.parent), os.O_RDONLY)
            try:
                os.fsync(dfd)
            finally:
                os.close(dfd)
        except OSError:
            pass
    finally:
        if os.path.exists(name):
            os.unlink(name)


class _CrashAfterCommit(RuntimeError):
    pass


def _recover(rec, root, event, envelope, journal):
    _, integrity = _capture_payload(root, event)
    if integrity:
        committed = journal.get("phase") == "record_committed"
        try:
            committed = committed or _sha(rec.read_bytes()) == journal["after_hash"]
        except (OSError, KeyError):
            pass
        return _integrity_question(root, event, integrity, committed)
    current_hash = _sha(rec.read_bytes())
    expected = journal["after_hash"]
    applied = current_hash == expected
    if not applied:
        try:
            now_hash = _batch_fingerprint(rec, envelope["updates"]) if "updates" in envelope else \
                _target(rec, envelope["target"])["body_sha256"]
            applied = (now_hash == journal["target_after_sha256"] and
                       P.bodies(P.load([str(rec)])).get(journal["source_id"], {}).get("file")
                       == event["source_file"])
        except (Exception, SystemExit):
            applied = False
    if not applied:
        return None
    after = _graph(rec, _report_seeds(envelope))
    fired, actionable = _classify(journal["before_graph"], after)
    signals = _signals(event["event_id"], envelope, after, fired, actionable)
    return _finish(root, event, envelope, "applied", None, signals,
                   graph_before=journal["before_graph"]["hash"], graph_after=after["hash"],
                   reach=after["reach"], newly_fired_judgments=fired,
                   actionable_judgments=actionable, recovered=True,
                   diagnostics=journal.get("diagnostics", []),
                   target_after_sha256=journal["target_after_sha256"])


def _process_event(rec, root, event, crash_after_commit=False):
    eid = event["event_id"]
    result = _load(root / "results" / (eid + ".json"))
    if result is not None:
        _publish(root, result)
        event.update(state=result["receipt"]["state"], reason=result["receipt"].get("reason"),
                     finished_at=event.get("finished_at") or time.time())
        _save(_event_file(root, eid), event)
        return result["receipt"]
    event.update(state="processing", attempts=event.get("attempts", 0) + 1,
                 started_at=time.time())
    _save(_event_file(root, eid), event)
    journal_path = root / "journals" / (eid + ".json")
    journal = _load(journal_path)
    envelope, integrity = _capture_payload(root, event)
    if integrity:
        committed = bool(journal and journal.get("phase") == "record_committed")
        if journal:
            try:
                committed = committed or _sha(rec.read_bytes()) == journal["after_hash"]
            except (OSError, KeyError):
                pass
        return _integrity_question(root, event, integrity, committed)
    if journal and journal.get("phase") in ("prepared", "record_committed"):
        recovered = _recover(rec, root, event, envelope, journal)
        if recovered is not None:
            return recovered
        if journal.get("phase") == "record_committed":
            return _question(root, event, envelope,
                             "the committed report was changed or reverted later; it will not be applied again",
                             record_committed=True)
    issue = event.get("capture_issue")
    if issue:
        return _question(root, event, envelope, issue)
    if P._peer('recording').private_marker(envelope) or ('scope' in envelope and envelope.get('shareability') != 'project'):
        return _question(root, event, envelope, 'private or unclear report permission; retained privately')
    if envelope.get("kind", "report") != "report":
        return _question(root, event, envelope, "only kind=report can update an existing reading")
    if "updates" not in envelope and "value" not in envelope:
        return _question(root, event, envelope, "the report needs an explicit scalar value")
    if "updates" not in envelope and _basic_type(envelope["value"]) != event["target_snapshot"]["type"]:
        return _question(root, event, envelope,
                         "the captured value does not have the target's scalar type")
    date = envelope.get("date")
    try:
        parsed_date = datetime.date.fromisoformat(date) if isinstance(date, str) and \
            re.fullmatch(r"\d{4}-\d{2}-\d{2}", date) else None
    except (TypeError, ValueError):
        parsed_date = None
    if parsed_date is None or parsed_date.isoformat() != date:
        return _question(root, event, envelope, "the report needs an ISO date (YYYY-MM-DD)")
    for attempt in range(MAX_REPREPARES + 1):
        with P._locked(str(rec)):
            try:
                current_target = _report_target(rec, envelope)
            except (Exception, SystemExit) as exc:
                return _question(root, event, envelope, "target conflict: " + " ".join(str(exc).split()))
            if current_target["body_sha256"] != event["target_snapshot"]["body_sha256"] and \
                    ("updates" in envelope or not _owned_predecessor(root, event, current_target)):
                return _question(root, event, envelope,
                                 "target changed after capture; the report was retained without overwriting it")
            prepared_target_sha256 = current_target["body_sha256"]
            before_bytes = rec.read_bytes()
            before_graph = _graph(rec, _report_seeds(envelope))
        try:
            _, integrity = _capture_payload(root, event)
            if integrity:
                return _integrity_question(root, event, integrity)
            shadow, after_bytes, prepared_graph, target_after_sha256, diagnostics = \
                _prepare(rec, root, event, envelope, before_bytes)
        except PreparationRefused as exc:
            return _question(root, event, envelope, str(exc),
                             validation_issues=exc.issues, diagnostics=exc.diagnostics)
        except (Exception, SystemExit) as exc:
            return _question(root, event, envelope,
                             "canonical writer refused the report: " + " ".join(str(exc).split()))
        journal = {
            "event_id": eid, "phase": "prepared", "before_hash": _sha(before_bytes),
            "after_hash": _sha(after_bytes), "before_graph": before_graph,
            "prepared_graph": prepared_graph, "shadow": str(shadow),
            "source_id": "s.ingest_" + eid, "attempt": attempt,
            "target_after_sha256": target_after_sha256,
            "brief_hash": _brief_fingerprint(shadow),
            "diagnostics": diagnostics,
        }
        _save(journal_path, journal)
        with P._locked(str(rec)):
            _, integrity = _capture_payload(root, event)
            if integrity:
                return _integrity_question(root, event, integrity)
            current = rec.read_bytes()
            try:
                unchanged = (_report_target(rec, envelope)["body_sha256"] == prepared_target_sha256)
            except (Exception, SystemExit):
                unchanged = False
            if not unchanged:
                return _question(root, event, envelope,
                                 "target or record layout changed while the report was prepared; it was not overwritten")
            if _sha(current) != journal["before_hash"] or _brief_fingerprint(rec) != journal["brief_hash"]:
                continue
            _replace_record(rec, after_bytes)
            journal["phase"] = "record_committed"
            journal["committed_at"] = time.time()
            _save(journal_path, journal)
        if crash_after_commit:
            raise _CrashAfterCommit("simulated interruption after record commit")
        after = _graph(rec, _report_seeds(envelope))
        fired, actionable = _classify(before_graph, after)
        signals = _signals(eid, envelope, after, fired, actionable)
        return _finish(root, event, envelope, "applied", None, signals,
                       graph_before=before_graph["hash"], graph_after=after["hash"],
                       reach=after["reach"], newly_fired_judgments=fired,
                       actionable_judgments=actionable, recovered=False,
                       diagnostics=diagnostics,
                       target_after_sha256=target_after_sha256)
    return _question(
        root, event, envelope,
        "the record changed repeatedly while this report was prepared; review the retained "
        "source and retry or apply it against the current record",
    )


def process(record=None, state_dir=None, event_id=None, max_events=32,
            _crash_after_commit=False):
    """Process a bounded queue pass.  Returns receipts/event summaries in capture order."""
    rec, root = _layout(record, state_dir)
    out = []
    with _file_lock(root / "processor.lock"):
        events = [_load(path) for path in (root / "events").glob("*.json")]
        events = [e for e in events if e and (event_id is None or e["event_id"] == event_id)
                  and e.get("state") not in TERMINAL]
        events.sort(key=lambda e: e.get("order", 0))
        for event in events[:max_events]:
            out.append(_process_event(rec, root, event, _crash_after_commit))
    return out


def status(event_id=None, record=None, state_dir=None):
    """Return one durable event/receipt, or all events in capture order."""
    _, root, exists = _existing_layout(record, state_dir)
    if not exists:
        return None if event_id else []
    if event_id:
        return (_load(root / "receipts" / (event_id + ".json")) or
                _load(root / "events" / (event_id + ".json")))
    events = [_load(path) for path in (root / "events").glob("*.json")]
    return [_load(root / "receipts" / (e["event_id"] + ".json")) or _summary(e)
            for e in sorted((x for x in events if x), key=lambda x: x.get("order", 0))]


def update(envelope, record=None, state_dir=None):
    """Apply one source report now, using the same durable atomic ingestion path."""
    rec, root = _layout(record, state_dir)
    event = capture(envelope, rec, root, start=False)
    process(rec, root, event_id=event['event_id'])
    return status(event['event_id'], rec, root)


def update_main(argv=None):
    parser = argparse.ArgumentParser(prog='kpopper update', description=update.__doc__)
    parser.add_argument('--file', required=True, help='report JSON file, or - for standard input')
    parser.add_argument('--record')
    parser.add_argument('--state-dir')
    parser.add_argument('--json', action='store_true', help='receipts are always structured JSON')
    args = parser.parse_args(argv)
    try:
        answer = update(_read_json_arg(args.file), args.record, args.state_dir)
    except (ValueError, OSError, LockingUnavailable) as error:
        print(json.dumps({'error': str(error)}, ensure_ascii=False))
        return 2
    print(json.dumps(answer, ensure_ascii=False, sort_keys=True))
    return 0 if answer and answer.get('state') == 'applied' else 1


def pending(record=None, state_dir=None, include_handled=False):
    """Return durable important signals; delivery acknowledgement does not delete them."""
    rec, root, exists = _existing_layout(record, state_dir)
    if not exists:
        return []
    handled = (_load(root / "handled.json") or {}).get("signals", {})
    signals = [_load(path) for path in (root / "signals").glob("*.json")]
    out = []
    graphs = {}
    for signal in sorted((x for x in signals if x), key=lambda x: (x["event_id"], x["id"])):
        if signal["id"] in handled:
            signal = dict(signal, handled_at=handled[signal["id"]])
            if not include_handled:
                continue
        # The signal file is immutable history.  Delivery is derived from the record now:
        # a repaired/reviewed judgment must not keep summoning an obsolete warning.
        names = (signal.get("newly_fired_judgments") or
                 signal.get("actionable_judgments") or [])
        if names:
            target = signal.get("target")
            try:
                if target not in graphs:
                    graphs[target] = _graph(rec, target, measured=True)
                graph = graphs[target]
                if signal["category"] == "contradiction":
                    live = [name for name in names
                            if graph["judgments"].get(name, {}).get("evaluation") is True]
                else:
                    live = [name for name in names
                            if graph["judgments"].get(name, {}).get("tag") in ACTIONABLE]
                if not live:
                    continue
                signal = dict(signal)
                signal["affected_judgments"] = live
                if signal["category"] == "contradiction":
                    signal["newly_fired_judgments"] = live
                    signal["reason"] = "; ".join(
                        graph["judgments"][name]["reason"] for name in live)
                else:
                    signal["actionable_judgments"] = live
                    signal["reason"] = "; ".join(
                        f"{name} requires review: {graph['judgments'][name]['reason']}"
                        for name in live)
                    signal["question"] = signal["reason"]
            except (Exception, SystemExit):
                pass  # uncertainty keeps the durable question visible
        out.append(signal)
    return out


def acknowledge(signal_ids, record=None, state_dir=None):
    """Mark signals handled for delivery only; semantic results remain immutable."""
    _, root = _layout(record, state_dir)
    if isinstance(signal_ids, str):
        signal_ids = [signal_ids]
    if not isinstance(signal_ids, list) or not signal_ids:
        raise ValueError("one or more signal IDs are required")
    known = {path.stem for path in (root / "signals").glob("*.json")}
    if any(x not in known for x in signal_ids):
        raise ValueError("unknown signal ID")
    with _file_lock(root / "delivery.lock"):
        handled = _load(root / "handled.json") or {"signals": {}}
        now = time.time()
        for sid in signal_ids:
            handled["signals"].setdefault(sid, now)
        _save(root / "handled.json", handled)
    return handled


def _lease_active(lease):
    if not lease:
        return False
    pid = lease.get("pid")
    if isinstance(pid, int):
        try:
            os.kill(pid, 0)
            return True
        except OSError:
            return False
    return time.time() - lease.get("started_at", 0) < 5


def _start_worker_locked(rec, root):
    lease_path = root / "worker.lease"
    if _lease_active(_load(lease_path)):
        return
    token = uuid.uuid4().hex
    lease = {"token": token, "started_at": time.time(), "pid": None}
    _save(lease_path, lease)
    try:
        proc = subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()), "_worker", "--record", str(rec),
             "--state-dir", str(root), "--lease-token", token],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            start_new_session=True, close_fds=True,
        )
    except Exception:
        lease_path.unlink(missing_ok=True)
        raise
    lease["pid"] = proc.pid
    _save(lease_path, lease)


def _auto_work(root):
    return any((_load(path) or {}).get("state") in AUTO_STATES
               for path in (root / "events").glob("*.json"))


def _worker(rec, root, token):
    for _ in range(4):
        with _file_lock(root / "capture.lock"):
            lease = _load(root / "worker.lease")
            if not lease or lease.get("token") != token:
                return
        process(rec, root, max_events=32)
        with _file_lock(root / "capture.lock"):
            lease = _load(root / "worker.lease")
            if not lease or lease.get("token") != token:
                return
            if not _auto_work(root):
                (root / "worker.lease").unlink(missing_ok=True)
                return
    with _file_lock(root / "capture.lock"):
        lease = _load(root / "worker.lease")
        if lease and lease.get("token") == token:
            (root / "worker.lease").unlink(missing_ok=True)
            if _auto_work(root):
                _start_worker_locked(rec, root)


def _read_json_arg(value):
    if value == "-":
        return json.load(sys.stdin)
    with io.open(value, encoding="utf-8") as src:
        return json.load(src)


def _delivery_module():
    try:
        from . import ingestion_delivery
    except ImportError:
        import ingestion_delivery
    return ingestion_delivery


def _delivery_commands(answer, record, state_dir):
    job = answer["delivery_job"]
    if job.get("dispatch_required"):
        location = ["--record", str(_record_path(record)),
                    "--state-dir", str(state_path(record, state_dir))]
        executable = [sys.executable, str(Path(__file__).resolve())]
        job["wait_command"] = shlex.join(executable + ["wait-delivery", job["id"]] + location)
        job["complete_command"] = shlex.join(executable + ["complete-delivery", job["id"]] + location)
    return answer


def main(argv=None):
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        if hasattr(stream, 'reconfigure'):
            stream.reconfigure(encoding='utf-8', newline='\n')
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    cap = sub.add_parser("capture")
    cap.add_argument("--file", required=True)
    cap.add_argument("--record")
    cap.add_argument("--state-dir")
    cap.add_argument("--no-start", action="store_true")
    cap.add_argument("--notify-task", metavar="TASK_ID",
                     help="reserve native-agent delivery to this Codex task; dispatch the returned job")
    for name in ("process", "pending", "status"):
        cmd = sub.add_parser(name)
        cmd.add_argument("--record")
        cmd.add_argument("--state-dir")
        if name == "status":
            cmd.add_argument("--event-id")
    ack = sub.add_parser("acknowledge")
    ack.add_argument("signal_ids", nargs="+")
    ack.add_argument("--record")
    ack.add_argument("--state-dir")
    wait = sub.add_parser("wait-delivery", help="bounded wait for a native delivery worker")
    wait.add_argument("job_id")
    wait.add_argument("--record")
    wait.add_argument("--state-dir")
    wait.add_argument("--timeout", type=float, default=60)
    complete = sub.add_parser("complete-delivery", help="record the host's message delivery result")
    complete.add_argument("job_id")
    complete.add_argument("--record")
    complete.add_argument("--state-dir")
    complete.add_argument("--claim-token", required=True)
    complete.add_argument("--outcome", required=True, choices=("sent", "failed", "unknown"))
    worker = sub.add_parser("_worker")
    worker.add_argument("--record", required=True)
    worker.add_argument("--state-dir", required=True)
    worker.add_argument("--lease-token", required=True)
    args = parser.parse_args(argv)
    if args.command == "capture":
        if args.notify_task is not None:
            host_task = os.environ.get("CODEX_SESSION_ID") or os.environ.get("CODEX_THREAD_ID")
            if not host_task or args.notify_task != host_task:
                parser.error("--notify-task must match this host's CODEX_SESSION_ID (or CODEX_THREAD_ID)")
            answer = _delivery_module().capture(_read_json_arg(args.file), args.notify_task,
                                                args.record, args.state_dir, not args.no_start)
            answer = _delivery_commands(answer, args.record, args.state_dir)
        else:
            answer = capture(_read_json_arg(args.file), args.record, args.state_dir, not args.no_start)
    elif args.command == "process":
        answer = process(args.record, args.state_dir)
    elif args.command == "pending":
        answer = pending(args.record, args.state_dir)
    elif args.command == "status":
        answer = status(args.event_id, args.record, args.state_dir)
    elif args.command == "acknowledge":
        answer = acknowledge(args.signal_ids, args.record, args.state_dir)
    elif args.command == "wait-delivery":
        answer = _delivery_module().wait(args.job_id, args.record, args.state_dir, args.timeout)
    elif args.command == "complete-delivery":
        answer = _delivery_module().complete(args.job_id, args.claim_token, args.outcome,
                                             args.record, args.state_dir)
    else:
        _worker(Path(args.record).resolve(), Path(args.state_dir).resolve(), args.lease_token)
        return 0
    print(json.dumps(answer, ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
