#!/usr/bin/env python3
"""Durable, selective ingestion for explicit provenance reports.

Captured text and queue state live outside the repository by default.  A report may
update one existing scalar reading or an atomic batch of readings and grounded
entries. Explicit replacements pass the canonical writer's permission checks and
retain prior archive/history evidence. Unsupported or ambiguous changes stay pending.

The implementation requires ``fcntl.flock`` for capture, processing, and acknowledgement,
matching provenance.py's directory lock.  Those writes fail closed on platforms without
``fcntl``; read-only status and pending inspection remain available.
"""
import argparse
import base64
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

TERMINAL = {"applied", "project_captured", "needs_primary", "superseded", "error"}
ACTIONABLE = {"MOVED", "UNCHECKED", "BROKEN", "BLOCKED", "UNKNOWN"}
AUTO_STATES = {"captured", "processing"}
EVENT_FIELDS = {"event_id", "session_id", "source_quote", "target", "value", "date",
                "kind", "question", "reason", "updates", "record_sha256",
                "shareability", "privacy", "scope", "source", "at", "profile", "disclosed_locators"}
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
    if state_dir is not None:
        return Path(state_dir).expanduser().resolve()
    rec = _routed_record(record)[0]
    configured = os.environ.get("XDG_STATE_HOME")
    base = Path(configured) if configured and Path(configured).is_absolute() \
        else Path.home() / ".local" / "state"
    return (base / "kpopper" / "ingestion" / _sha(str(rec).encode("utf-8"))).resolve()


def _storage_record(record, state_dir):
    raw = _record_path(record)
    # An explicitly addressed historical store remains inspectable after a mode
    # transition. Processing still checks its captured policy before any write.
    if state_dir is not None and _load(Path(state_dir).expanduser().resolve() / 'record.json') == {'record': str(raw)}:
        return raw
    return _routed_record(raw)[0]


def _layout(record=None, state_dir=None):
    _require_locking()  # Fail before creating/chmod'ing state on unsupported platforms.
    rec = _storage_record(record, state_dir)
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
    rec = _storage_record(record, state_dir)
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
    if 'disclosed_locators' in out:
        disclosures = out['disclosed_locators']
        C = P._peer('history_contract')
        if not isinstance(disclosures, list) or len(disclosures) > 256:
            raise ValueError('disclosed_locators must be a bounded path/hash allowlist')
        for item in disclosures:
            C._mapping(item, ('path', 'sha256'))
            C.relative_path(item['path'])
            C._require(item['sha256'] is None or isinstance(item['sha256'], str) and
                       C.HEX.fullmatch(item['sha256']), 'invalid_locator_disclosures')
    if out.get('profile') not in (None, 'core/v1'):
        raise ValueError('unsupported writer profile')
    scope = out.get('scope')
    if scope is not None:
        if not isinstance(scope, dict) or scope.get('kind') not in ('project', 'external', 'code', 'feature', 'unclear'):
            raise ValueError('report scope needs an explicit supported kind')
        if scope['kind'] in ('project', 'external', 'code') and (not isinstance(scope.get('environment'), str)
                                                               or not scope['environment'].strip()):
            raise ValueError('report scope needs an exact environment')
        if scope['kind'] == 'code' and not re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', str(scope.get('commit', ''))):
            raise ValueError('code-scoped report needs an exact commit')
    for field in ("source", "at"):
        if field in out and (not isinstance(out[field], str) or not out[field].strip()):
            raise ValueError(field + " must be non-empty text")
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
            allowed = {"kind", "id", "at", "value"} if op["kind"] == "set" else {"kind", "id", "at", "body", "into", "drops"}
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
            if 'drops' in op and (not isinstance(op['drops'], dict) or any(
                    not isinstance(k, str) or not isinstance(v, str) or not v.strip()
                    for k, v in op['drops'].items())):
                raise ValueError('drops must name dependencies and nonempty reasons')
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


def _history_active(record):
    return P._peer('history_direct').active([str(record)])


def _record_world(record, *, core_writer=False):
    paths = [str(record)]
    if _history_active(record):
        A = P._peer('history_authoring')
        captured = P._peer('history_store').Store(record).capture()
        if captured.document.get('also') or P.load_hypotheses(paths):
            raise ValueError('multi-file and hypothesis history reports require primary review')
        doc = A._document(P._peer('history_adapter').from_store_capture(captured).document)
        ids, judgments, fields = A.READER.infer(doc)
        world = A._world(doc)
        return doc, ids, judgments, fields, world.raw if world else P.with_builtins(doc, ids, judgments, fields)
    files = [Path(x).resolve() for x in P._files_of(paths)]
    if len(files) != 1 or files[0] != record.resolve():
        raise ValueError("multi-file and pointer records require primary review")
    doc = P._peer('reasoning.authoring').load(P, paths) if core_writer else P.load(paths, read_mode='frozen')
    if doc.hypotheses:
        raise ValueError("a record with hypothesis context requires primary review")
    ids, judgments, fields = P.infer(doc)
    if P._peer('reasoning.authoring').selected(doc):
        original = P._peer('reasoning.snapshot').Snapshot.capture(paths, read_mode='frozen')
        raw = P._peer('reasoning.authoring').World(P, doc, original=original).raw
    else:
        raw = P.with_builtins(doc, ids, judgments, fields)
    return doc, ids, judgments, fields, raw


def _target(record, name, source_collection=None, *, core_writer=False):
    if not isinstance(name, str) or not name:
        raise ValueError("the report does not identify one existing target")
    doc, ids, judgments, fields, raw = _record_world(record, core_writer=core_writer)
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
    if getattr(raw, 'world', None) is None and isinstance(value, str) and P.EXPR.search(value) and \
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
    if source_collection is None:
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
    doc, _, _, _, _ = _record_world(record, core_writer=True)
    raw = P.bodies(doc)
    return _body_hash({op["id"]: raw.get(op["id"]) for op in updates})


def _cited_source_collection(doc, ids, judgments, raw, source):
    """An explicit report citation has the same source role as ordinary set --source."""
    body = raw.get(source)
    if source not in ids or source in judgments or P.is_builtin(source) \
            or not isinstance(body, dict) or any(key in body for key in ("v", "quoted", "rule")) \
            or not any(body.get(key) for key in ("asked", "file", "url", "of", "read")):
        raise ValueError(source + " is not a recorded source; add the source before citing it")
    homes = [name for name, members in P.collections_of(doc).items() if source in members]
    if len(homes) != 1:
        raise ValueError("the recorded source must belong to one existing collection")
    return homes[0]


def _report_target(record, envelope):
    """New claims and explicit source citations bind the whole interpreted record."""
    expected = envelope.get("record_sha256")
    has_additions = any(op["kind"] == "add" for op in envelope.get("updates", []))
    if has_additions and expected is None:
        raise ValueError("new entries require record_sha256 from the primary's prior open --json or search")
    source = envelope.get("source")
    if source is not None and expected is None:
        raise ValueError("an existing source citation requires record_sha256 from the primary's prior open --json or search")
    if expected is not None and _sha(Path(record).read_bytes()) != expected:
        raise ValueError("record changed since the primary read it; reread the premises before resubmitting")
    if "updates" not in envelope and source is None:
        return _target(record, envelope.get("target"), core_writer=True)
    doc, ids, judgments, fields, raw = _record_world(record, core_writer=True)
    citation_home = _cited_source_collection(doc, ids, judgments, raw, source) if source is not None else None
    if source is not None and not envelope.get("at"):
        operations = envelope.get("updates", [{"kind": "set", "id": envelope.get("target")}])
        if any((op["kind"] == "set" or any(key in op["body"] for key in ("v", "quoted")))
               and not op.get("at") for op in operations):
            raise ValueError("an existing source citation requires at for each reading, or a shared envelope.at")
    if "updates" not in envelope:
        return _target(record, envelope.get("target"), citation_home, core_writer=True)
    homes = {citation_home} if citation_home else set()
    for op in envelope["updates"]:
        if op["kind"] == "set":
            old = _target(record, op["id"], citation_home, core_writer=True)
            if _basic_type(op["value"]) != old["type"]:
                raise ValueError(op["id"] + " has a different scalar type")
            homes.add(old["source_collection"])
        else:
            # Existing names are replacement intents. The canonical writer must
            # validate their original body and retain its archive/history evidence.
            body = op["body"]
            if fields.get("snapshot") in body or "seen" in body:
                raise ValueError("judgment snapshots are computed by the writer")
            if any(key in body for key in ("from", "at", "of", "src", "source")):
                raise ValueError("batch citations use envelope.source and update.at, not citation fields inside body")
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
            ("event_id", "state", "target", "captured_at", "finished_at", "reason", "record", "state_dir")}


def _routed_record(record):
    original = _record_path(record)
    views = P._peer('knowledge_views')
    project = views.project_for([str(original)])
    policy = project.config()
    rec = Path(views.write_paths([str(original)])[0]).resolve()
    return rec, project, policy


@contextlib.contextmanager
def _event_lock(rec, event):
    project = P._peer('project_modes').Project(event.get('project_root', rec.parent))
    with P._locked(str(rec), project=project):
        if event.get('project_policy') is not None and project.config() != event['project_policy']:
            raise ValueError('project policy changed after report capture; review the retained report')
        if event.get('project_policy') is None and Path(P._peer('knowledge_views').write_paths([str(rec)])[0]).resolve() != rec:
            raise ValueError('record destination changed since legacy capture; review the retained report')
        yield project


def _report_private(doc, envelope):
    """Check the original permissions before a synthetic source replaces citations."""
    R, G = P._peer('recording'), P._peer('pending_grounding')
    controls = {k: doc[k] for k in ('meta', 'private', 'privacy', 'visibility', 'shareability') if k in doc}
    if R.private_marker(controls) or R.private_marker(envelope):
        return True
    entries = G.entries(doc)
    roots = set()
    if envelope.get('source') in entries:
        roots.add(envelope['source'])
    operations = envelope.get('updates', [{'id': envelope.get('target')}])
    for operation in operations:
        if operation.get('id') in entries:
            roots.add(operation['id'])
        body = operation.get('body') or {}
        roots.update(x for x in G._strings(body) if x in entries)
        roots.update(x for x in P._mentioned(body) if x in entries)
    return bool(roots and R.private_marker(G.closure(doc, sorted(roots))))


def capture(envelope, record=None, state_dir=None, start=True):
    """Durably retain one explicit report and optionally start a finite worker."""
    envelope = _validate_envelope(envelope)
    selected, project, policy = _routed_record(record)
    rec, root = _layout(selected, state_dir)
    eid = _event_id(rec, envelope)
    encoded = _json_bytes(envelope)
    with _file_lock(root / "capture.lock"):
        prior = _load(_event_file(root, eid))
        if prior is not None:
            if _load(root / "envelopes" / (eid + ".json")) != envelope:
                raise ValueError("event_id was reused for different input")
            if start and prior.get("state") in AUTO_STATES:
                _start_worker_locked(rec, root)
            return status(eid, rec, root) if prior.get('state') in TERMINAL else _summary(prior)
        with P._locked(str(rec), project=project):
            if project.config() != policy:
                raise ValueError('project policy changed while capturing report; retry')
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
            'record': str(rec), 'state_dir': str(root), 'project_root': str(project.root), 'project_policy': policy,
            "event_id": eid, "state": "captured", "target": envelope.get("target"),
            "captured_at": time.time(), "order": counter, "record_hash": record_hash,
            "target_snapshot": snapshot, "capture_issue": issue,
            "source_file": str(source), "source_sha256": _sha(source_bytes),
            "envelope_sha256": _sha(encoded), "attempts": 0,
        }
        _save(_event_file(root, eid), event)
        project_report = (policy['mode'] == 'advanced' and envelope.get('shareability') == 'project'
                          and isinstance(envelope.get('scope'), dict)
                          and envelope['scope'].get('kind') in ('project', 'external'))
        if start and not project_report:
            _start_worker_locked(rec, root)
    if project_report:
        # Explicit project-wide capture is acknowledged only after the Git ledger
        # commits; the ingestion journal retains staging/recovery, not a YAML copy.
        process(rec, root, event_id=eid)
        return status(eid, rec, root)
    return _summary(event)


def _graph(record, target=None, measured=False, profile=None):
    doc, ids, judgments, fields, raw = _record_world(record, core_writer=True)
    if getattr(raw, 'world', None) is not None or profile == 'core/v1':
        if _history_active(record):
            return _history_graph(record, target)
        return P._peer('reasoning.ingestion').graph(P, record, target, profile)

    measurement = None
    if measured and set(ids) & set(P.PAGE):
        try:
            from . import page_measurements as M
        except ImportError:
            import page_measurements as M
        measurement = M.snapshot([str(record)])
        doc, ids, judgments, fields, raw = _record_world(record, core_writer=True)
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


def _history_graph(record, target=None, *, capture=None):
    """Internal report graph from committed authority, never a YAML-only reader."""
    A, H = P._peer('history_authoring'), P._peer('history_store')
    captured = capture or H.Store(record).capture()
    adapted = P._peer('history_adapter').from_store_capture(captured)
    doc = A._document(adapted.document)
    return _history_document_graph(doc, _sha(captured.entry_bytes), target)


def _history_document_graph(doc, content_hash, target):
    A = P._peer('history_authoring')
    ids, judgments, fields = A.READER.infer(doc)
    world = A._world(doc)
    raw = world.raw if world else P.with_builtins(doc, ids, judgments, fields)
    seeds = target if isinstance(target, list) else [target] if target else []
    hit, touched, derived = P.reach_of(ids, judgments, raw, seeds)
    for seed in seeds:
        if seed in judgments:
            hit.setdefault(seed, seed)
    states = {}
    for nid, judgment in judgments.items():
        tag, reason = world.state(nid) if world else P._state(nid, judgment, raw, ids, fields, touched=touched)
        evaluation = ({'holds': True, 'does_not_hold': False}.get(
            world.assessment()['nodes'][nid]['state']['falsifier']['status']) if world else
            P.evaluate(judgment['pred'], raw, ids))
        states[nid] = {'tag': tag, 'reason': reason, 'evaluation': evaluation,
                       'name': P.named(judgment['body']), 'verdict': judgment['body'].get('verdict')}
    result = {'hash': content_hash, 'judgments': states,
              'reach': {'judgments': sorted(hit), 'via': hit,
                        'touched': sorted(touched), 'derived': derived}}
    if world:
        report = world.assessment()
        result.update(assessment_profile='core/v1', snapshot_id=report['snapshot_id'],
                      assessment=P._peer('pending_grounding')._encode(report))
    return result


def _history_after_capture(captured, mutation):
    from dataclasses import replace
    C, H = P._peer('history_contract'), P._peer('history_store')
    commits, objects = dict(captured.commits), dict(captured.object_bytes)
    for item in mutation.files:
        if item['role'] == 'history_object':
            obj = C.decode_document(item['after'])
            objects[(obj['subject'], obj['id'])] = item['after']
        elif item['role'] == 'history_commit':
            commits[mutation.to_data()['operation']] = item['after']
        elif item['role'] == 'record':
            after = item['after']
    selected = C.committed_objects(captured.marker, commits, objects)
    state = H.reduce(selected, captured.state['rules'])
    return replace(captured, entry_bytes=after, document=C.decode_document(after), commits=commits,
                   objects=selected, object_bytes=objects, state=state,
                   baseline=H.baseline(captured.marker, commits, state))


def _classify(before, after, *, recorded_pin_subjects=()):
    if before.get('assessment_profile', 'ordinary-reader/v1') != after.get('assessment_profile', 'ordinary-reader/v1'):
        raise ValueError('mixed assessment profiles in ingestion classification')
    reached = after["reach"]["judgments"]
    pin_only = set()
    if recorded_pin_subjects and after.get('assessment_profile') == 'core/v1':
        report = P._peer('pending_grounding')._decode(after['assessment'])
        for subject in recorded_pin_subjects:
            node = report['nodes'].get(subject, {})
            state = node.get('state', {})
            codes = {issue['code'] for issue in state.get('integrity', {}).get('issues', [])}
            dependencies = state.get('basis', {}).get('dependencies', {})
            if codes == {'missing_snapshot'} and state.get('falsifier', {}).get('status') == 'does_not_hold' and \
                    state.get('contention', {}).get('status') == 'none_detected' and all(
                        item.get('computation', {}).get('status') == 'ok' and
                        item.get('comparison') != 'changed' and not item.get('rule_changed') and
                        item.get('basis_comparison') != 'changed' for item in dependencies.values()):
                pin_only.add(subject)
    fired, actionable = [], []
    for jid in reached:
        old = before["judgments"].get(jid, {})
        now = after["judgments"][jid]
        if old.get("evaluation") is not True and now["evaluation"] is True:
            fired.append(jid)
        elif now["tag"] in ACTIONABLE and \
                (old.get("tag"), old.get("reason")) != (now["tag"], now["reason"]):
            if jid not in pin_only:
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
        'record': str(event.get('record', '')), 'state_dir': str(root),
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
    if "source" in envelope:
        receipt["cited_source"] = envelope["source"] if state in ("applied", "project_captured") else None
        receipt["date"] = envelope.get("date")
    if "at" in envelope:
        receipt["at"] = envelope["at"]
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


def recording_source(nid, body, record=None, *, _preparing=None):
    """Scope the purpose exemption to an applied capture, or this worker's staged one."""
    if record is None or not isinstance(body, dict) or not isinstance(nid, str) or not nid.startswith('s.ingest_'):
        return False
    eid = nid[len('s.ingest_'):]
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', eid) or not isinstance(body.get('file'), str):
        return False
    source = Path(body['file'])
    if not source.is_absolute() or source.parent.name != 'sources' or source.name != eid + '.txt':
        return False
    root = source.parent.parent
    try:
        owner = Path(record).expanduser().resolve()
        preparing_root = None
        if _preparing is not None:
            if not isinstance(_preparing, dict) or set(_preparing) != {'record', 'state_dir', 'event_id', 'shadow'}:
                return False
            preparing_root = Path(_preparing['state_dir']).resolve()
            shadow = Path(_preparing['shadow']).resolve()
            if owner != shadow or shadow.parent.parent != preparing_root / 'drafts' \
                    or not shadow.parent.name.startswith(_preparing['event_id'] + '-'):
                return False
            owner = Path(_preparing['record']).resolve()
        # A caller may choose --state-dir. Its ownership marker, rather than the
        # current machine's default state path, binds it to the checked record.
        if _load(root / 'record.json') != {'record': str(owner)}:
            return False
        event = _load(_event_file(root, eid))
        if not isinstance(event, dict) or event.get('event_id') != eid or event.get('source_file') != str(source):
            return False
        _, error = _capture_payload(root, event)
        if error:
            return False
        receipt = _load(root / 'receipts' / (eid + '.json'))
        if isinstance(receipt, dict) and receipt.get('state') == 'applied' \
                and receipt.get('event_id') == eid and receipt.get('source') == nid \
                and all(receipt.get(key) == event.get(key) for key in
                        ('source_file', 'source_sha256', 'envelope_sha256')):
            return True
        # This context comes only from _prepare's call, not YAML or a CLI flag.
        # The final receipt cannot exist yet; the commit still checks the record
        # and input hashes after this gate accepts the isolated staged record.
        return bool(_preparing is not None and root.resolve() == preparing_root
                    and _preparing['event_id'] == eid and event.get('state') == 'processing')
    except (OSError, ValueError, TypeError, KeyError, RuntimeError):
        return False


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


def _legacy_files(rec):
    """Capture every mutable member, including absent members, under the writer lock."""
    T = P._peer('history_transaction')
    paths = P.layout(rec)
    result = []
    for role in ('record', 'view', 'replaced'):
        path = Path(paths['entry' if role == 'record' else role])
        relative = path.relative_to(rec.parent).as_posix()
        data = T._read(T._target(rec.parent, relative))
        result.append({'path': relative, 'role': role, 'before': data})
    return result


def _write_context(rec, project):
    """Retain non-file authority used by preparation; it must survive final recheck."""
    T, C = P._peer('history_transaction'), P._peer('history_contract')
    marker_path = Path(P.layout(rec)['history_authority']).relative_to(rec.parent).as_posix()
    raw = T._read(T._target(rec.parent, marker_path))
    marker = C.validate_authority(C.decode_document(raw)) if raw is not None else T.legacy_authority(rec)
    if marker['authority'] == 'history':
        captured = P._peer('history_store').Store(rec).capture()
        document = P._peer('history_authoring')._document(
            P._peer('history_adapter').from_store_capture(captured).document)
    else:
        document = P.parse(text=rec.read_bytes()) or {}
        if isinstance(document.get('meta'), dict) and 'history' in document['meta']:
            raise ValueError('authority_mismatch: generated history view has no active authority')
    # Validate all declared and historical reasoning requirements before staging.
    P._peer('pending_grounding').document_capabilities(document)
    destination = str(Path(P._peer('knowledge_views').write_paths([str(rec)])[0]).resolve())
    if destination != str(rec.resolve()):
        raise ValueError('record destination changed after report capture')
    pending = (P._peer('pending_grounding').Store(project.root).head()
               if project.git and project.config()['mode'] == 'advanced' else None)
    return {'authority': marker, 'marker_sha256': _sha(raw) if raw is not None else None,
            'destination': destination, 'policy': project.config(), 'pending_ref': pending}


def _shadow(rec, root, event, before_bytes):
    draft = root / 'drafts' / (event['event_id'] + '-' + uuid.uuid4().hex)
    _private_dir(draft)
    shadow = draft / rec.name
    files = event.get('_before_files')
    if files is None:
        files = _legacy_files(rec)
    for item in files:
        data = before_bytes if item['role'] == 'record' else item['before']
        if data is not None:
            _atomic(draft / item['path'], data)
    return shadow


def _prepared_mutation(rec, shadow, event, files, context, before, after):
    T, G = P._peer('history_transaction'), P._peer('pending_grounding')
    members = [{**item, 'after': T._read(shadow.parent / item['path'])} for item in files]
    for role in ('history', 'history_commits', 'history_authority'):
        if Path(P.layout(shadow)[role]).exists():
            raise ValueError('unsupported_capability: shadow history requires history transport')
    receipt = T.semantic_receipt(profile=before.get('assessment_profile', 'ordinary-reader/v1'),
        capabilities={'before': G.meaning_capabilities(P.parse(text=next(x['before'] for x in files if x['role'] == 'record'))),
                      'after': G.meaning_capabilities(P.parse(text=shadow.read_bytes()))},
        before=before, after=after)
    baseline = {'event_id': event['event_id'], 'source_sha256': event['source_sha256'],
                'envelope_sha256': event['envelope_sha256'], 'context': context}
    return T.PreparedMutation(operation='report-' + event['event_id'], entry=rec.name,
        authority=context['authority'], baseline=baseline, files=members, receipt=receipt)


def _mutation_from_journal(journal):
    T = P._peer('history_transaction')
    return T.PreparedMutation.from_bytes(base64.b64decode(journal['mutation'], validate=True))


def _bind_report_mutation(rec, event, journal, data):
    baseline = data['baseline']
    if (data['operation'] != 'report-' + event['event_id'] or data['entry'] != rec.name
            or journal['event_id'] != event['event_id']
            or baseline.get('event_id') != event['event_id']
            or baseline.get('context', {}).get('destination') != str(rec.resolve())
            or any(baseline.get(key) != event[key] for key in ('source_sha256', 'envelope_sha256'))):
        raise ValueError('prepared report operation does not match its retained event')


def _verify_mutation(rec, root, event, envelope, journal, project, supplied):
    mutation = _mutation_from_journal(journal)
    expected = mutation.to_data()
    _bind_report_mutation(rec, event, journal, expected)
    if supplied != expected:
        raise ValueError('prepared report operation does not match its retained event')
    _, error = _capture_payload(root, event)
    if error:
        raise ValueError(error)
    if any(expected['baseline'][key] != event[key] for key in ('source_sha256', 'envelope_sha256')):
        raise ValueError('prepared report capture changed')
    if _write_context(rec, project) != expected['baseline']['context']:
        raise ValueError('report authority, routing, policy or pending ref changed after preparation')
    receipt = expected['receipt']
    if receipt['before'] != journal['before_graph'] or receipt['after'] != journal['prepared_graph']:
        raise ValueError('prepared report assessment differs from retained receipt')
    if 'core_gate' in journal:
        P._peer('reasoning.ingestion').verify_receipt(journal['core_gate'], journal['before_graph'], journal['prepared_graph'])
    # Recovery may see mixed file images. Validate capability compatibility using
    # the retained final document, never a partially published live graph.
    after = next(item['after'] for item in mutation.files if item['role'] == 'record')
    P._peer('reasoning.authoring').pending_compatible(P, [str(rec)], P.parse(text=after))


def _committed(root, event, journal):
    def save(_mutation):
        journal.update(phase='record_committed', committed_mutation=_mutation['digest'])
        journal.setdefault('committed_at', time.time())
        # The shared journal may be removed only after the event receipt's file
        # and directory are durable. The general queue saver tolerates directory
        # fsync failures, which is insufficient for this completion boundary.
        P._peer('history_transaction')._replace(
            root / 'journals' / (event['event_id'] + '.json'), _json_bytes(journal))
    return save


def _report_actions(event, envelope, *, portable_source=None):
    source_id = 's.ingest_' + event['event_id']
    cited = envelope.get('source', source_id)
    if cited == source_id and 'source' in envelope:
        raise ValueError('a report cannot cite its own capture as an existing source')
    seeds = _report_seeds(envelope)
    source = {'name': 'Captured report', 'file': portable_source or event['source_file'], 'read': envelope['date'],
              'recorded_for': 'Update ' + (', '.join(seeds) if isinstance(seeds, list) else seeds) + ' from this captured report.'}
    if portable_source is not None and 'scope' in envelope:
        source['scope'] = copy.deepcopy(envelope['scope'])
    if 'source' in envelope:
        source['from'] = cited
        if 'at' in envelope:
            source['at'] = envelope['at']
    actions = [{'kind': 'add', 'id': source_id, 'body': source, 'as_of': envelope['date'],
                'into': event['target_snapshot']['source_collection']}]
    operations = envelope['updates'] if 'updates' in envelope else [
        {'kind': 'set', 'id': envelope['target'], 'value': envelope['value']}]
    for operation in sorted(operations, key=lambda op: op['kind'] != 'set'):
        action = copy.deepcopy(operation)
        action.update(as_of=envelope['date'], why=envelope.get('reason'))
        location = action.pop('at', envelope.get('at', 'entire captured report'))
        if 'scope' in envelope:
            action['_record_scope'] = copy.deepcopy(envelope['scope'])
        if action['kind'] == 'set':
            action.update(source=cited, at=location)
        else:
            body = action['body']
            if 'scope' in envelope:
                if 'scope' in body and body['scope'] != envelope['scope']:
                    raise ValueError('entry scope differs from report scope: ' + action['id'])
                body['scope'] = copy.deepcopy(envelope['scope'])
            if 'v' in body or 'quoted' in body:
                body.update({'from': cited, 'at': location, 'of': envelope['date']})
        actions.append(action)
    return actions


def _history_report_binding(event, context):
    return {'kind': 'report-history/v1', 'event_id': event['event_id'],
            'source_sha256': event['source_sha256'], 'envelope_sha256': event['envelope_sha256'],
            'context': context}


def _history_shared(rec):
    return rec.parent / P._peer('history_direct').journal(rec)


def _history_report_envelope(rec, root, event, mutation):
    T, C = P._peer('history_transaction'), P._peer('history_contract')
    value = {'version': 1, 'kind': 'report-history/v1', 'record': str(rec), 'state_dir': str(root),
             'event_id': event['event_id'], 'mutation': T._blob(mutation.to_bytes())}
    return {**value, 'digest': P._peer('pending_grounding').identity(value)}


def _history_pin_support(mutation, *, capture=None):
    """Only final newly written claims with complete actual pins waive absent seen."""
    C, G = P._peer('history_contract'), P._peer('pending_grounding')
    document = mutation.to_data()['receipt']['after']['document']
    support = set()
    for item in mutation.files:
        if item['role'] != 'history_object':
            continue
        obj = C.validate_object(C.decode_document(item['after']))
        if obj['kind'] != 'judgment' or obj.get('pin_gaps'):
            continue
        field = obj['authored']['fields']['deps']
        deps = obj['body'].get(field, [])
        current = document.get(obj['authored']['collection'], {}).get(obj['subject'])
        if capture is not None and any(capture.state['subjects'].get(dep, {}).get('acceptance') != 'accepted' or
                version not in capture.state['subjects'].get(dep, {}).get('heads', [])
                for dep, version in obj['pins'].items()):
            continue
        if isinstance(deps, (list, dict)) and set(deps) == set(obj['pins']) and \
                G.identity(current) == G.identity(obj['body']):
            support.add(obj['subject'])
    return sorted(support)


def _verify_history_report(rec, root, event, envelope, journal, project, supplied):
    mutation = _mutation_from_journal(journal)
    expected = mutation.to_data()
    G = P._peer('pending_grounding')
    if G.identity(supplied) != G.identity(expected) or supplied['operation'] != 'report-' + event['event_id']:
        raise ValueError('prepared history report differs from retained event')
    _, error = _capture_payload(root, event)
    if error:
        raise ValueError(error)
    bound = expected['receipt']['before']['authoring']['context']
    if G.identity(bound) != G.identity(_history_report_binding(event, _write_context(rec, project))):
        raise ValueError('history report authority, source, routing, policy or pending ref changed')
    if G.identity(expected['receipt']['before']['authoring']['actions']) != G.identity(
            _report_actions(event, envelope, portable_source=journal.get('portable_source'))):
        raise ValueError('history report intents differ from the retained envelope')
    record = next(item for item in mutation.files if item['role'] == 'record')
    for role, graph_key in (('before', 'before_graph'), ('after', 'prepared_graph')):
        graph = _history_document_graph(expected['receipt'][role]['document'], _sha(record[role]),
                                        _report_seeds(envelope))
        if G.identity(graph) != G.identity(journal[graph_key]):
            raise ValueError('history report graph differs from retained semantic evidence')
    if journal['before_graph'].get('assessment_profile') == 'core/v1':
        gate = P._peer('reasoning.ingestion')
        gate.verify_receipt(journal.get('core_gate'), journal['before_graph'], journal['prepared_graph'])
        failures = gate.gate(P, journal['before_graph'], journal['prepared_graph'],
                             recorded_pin_subjects=_history_pin_support(mutation))
        if failures:
            raise ValueError('; '.join(failures))


def _publish_history_report(rec, root, event, envelope, journal, project):
    T, C, A = (P._peer(name) for name in ('history_transaction', 'history_contract', 'history_authoring'))
    mutation = _mutation_from_journal(journal)
    pending = _history_shared(rec)
    encoded = C.encode_document(_history_report_envelope(rec, root, event, mutation))
    if pending.exists() and pending.read_bytes() != encoded:
        raise ValueError('recovery_required: another history operation owns the record journal')
    _verify_history_report(rec, root, event, envelope, journal, project, mutation.to_data())
    T.publish_immutable(pending.parent / '.gitignore', b'*\n', root=rec.parent)
    T.publish_immutable(pending, encoded, root=rec.parent)
    A.commit(rec, mutation, verify=lambda supplied:
             _verify_history_report(rec, root, event, envelope, journal, project, supplied))
    _committed(root, event, journal)(mutation.to_data())
    pending.unlink()
    T._sync(pending.parent)
    P.forget(rec)


def _finish_history_report(rec, root, event, envelope, journal, *, recovered):
    captured = P._peer('history_store').Store(rec).capture()
    after = _history_graph(rec, _report_seeds(envelope), capture=captured)
    target = (_batch_fingerprint(rec, envelope['updates']) if 'updates' in envelope else
              _target(rec, envelope['target'], core_writer=True)['body_sha256'])
    if target != journal['target_after_sha256']:
        return _question(root, event, envelope, 'the committed report was changed later; it will not be replayed', record_committed=True)
    fired, actionable = _classify(journal['before_graph'], after,
        recorded_pin_subjects=_history_pin_support(_mutation_from_journal(journal), capture=captured))
    return _finish(root, event, envelope, 'applied', None,
        _signals(event['event_id'], envelope, after, fired, actionable),
        graph_before=journal['before_graph']['hash'], graph_after=after['hash'], reach=after['reach'],
        newly_fired_judgments=fired, actionable_judgments=actionable, recovered=recovered,
        diagnostics=journal.get('diagnostics', []), target_after_sha256=target)


def _inventory_digest(captured):
    G = P._peer('pending_grounding')
    return G.identity([{'kind': kind, 'path': path, 'value': value}
                       for (kind, path), value in sorted(captured.inventory.items())])


def _scoped_roots(event, envelope):
    seeds = _report_seeds(envelope)
    return sorted(set((seeds if isinstance(seeds, list) else [seeds]) + ['s.ingest_' + event['event_id']]))


def _scoped_bundle_data(bundle):
    G, T = P._peer('pending_grounding'), P._peer('history_transaction')
    return {'revision': bundle['revision'], 'manifest': G._encode(bundle['manifest']),
            'files': {path: T._blob(raw) for path, raw in bundle['files'].items()}}


def _scoped_bundle(journal):
    G, T = P._peer('pending_grounding'), P._peer('history_transaction')
    value = journal['pending_bundle']
    bundle = {'revision': value['revision'], 'manifest': G._decode(value['manifest']),
              'files': {path: T._unblob(raw) for path, raw in value['files'].items()}}
    G.validate_bundle(bundle)
    if bundle['revision'] != journal['pending_revision']:
        raise ValueError('retained scoped report revision mismatch')
    return bundle


def _scoped_artifact(rec, event, envelope, journal, captured):
    B = P._peer('history_bundle')
    mutation = _mutation_from_journal(journal)
    intent = mutation.to_data()['receipt']['before']['authoring']
    return B.prepare_subset(captured, roots=_scoped_roots(event, envelope), scope=envelope['scope'],
        shareability='project', operation='report-subset-' + event['event_id'],
        recorded_at=intent['recorded_at'], source_entry=rec.name, prepared=mutation,
        disclosed_locators=envelope.get('disclosed_locators', []))


def _prepare_scoped_history_report(rec, root, event, envelope, journal, captured):
    G, B, C, T = (P._peer(name) for name in ('pending_grounding', 'history_bundle', 'history_contract', 'history_transaction'))
    artifact = _scoped_artifact(rec, event, envelope, journal, captured)
    portable = journal['portable_source']
    source_id = journal['source_id']
    selected = B.validate(artifact)
    if any(portable in set(G._files(obj)) for obj in selected.objects.values() if obj['subject'] != source_id):
        raise ValueError('report evidence path collides with another historical source')
    required = {path for raw in artifact['files'].values() for path in G._files(C.decode_document(raw))}
    evidence, observed = {}, {}
    for path in sorted(required):
        if path == portable:
            evidence[path] = envelope['source_quote'].encode('utf-8')
        else:
            location = T._target(rec.parent, path)
            C._require(not location.exists() or location.stat().st_size <= C.MAX_REQUEST_BYTES, 'history_limit')
            raw = T._read(location)
            if raw is None:
                raise ValueError('missing report contribution evidence: ' + path)
            evidence[path] = raw
            observed[path] = C.sha256(raw)
    bundle = G.prepare(B.adapt(artifact).document, _scoped_roots(event, envelope),
                       scope=envelope['scope'], shareability='project', history=artifact, evidence=evidence)
    journal.update(history_contribution=True, source_inventory=_inventory_digest(captured),
                   evidence_hashes=observed, pending_bundle=_scoped_bundle_data(bundle),
                   pending_event_id='report-' + event['event_id'], pending_revision=bundle['revision'])
    C._require(len(_json_bytes(journal)) <= T.MAX_TRANSACTION_BYTES, 'history_limit')


def _capture_scoped_history_report(rec, root, event, envelope, journal, *, crash_after_commit=False):
    """Publish pending evidence after releasing the direct/project preparation lock."""
    G, B, H, A, T = (P._peer(name) for name in
                     ('pending_grounding', 'history_bundle', 'history_store', 'history_authoring', 'history_transaction'))
    bundle = _scoped_bundle(journal)
    store = G.Store(event['project_root'])
    receipt = store.receipt(journal['pending_event_id'])
    if receipt is not None:
        if receipt['revision'] != bundle['revision']:
            raise ValueError('captured contribution differs from retained report')
    else:
        def verify_source():
            # The pending store already owns the policy lock. Acquire only the
            # record participant locks; never recursively acquire project.lock.
            with P._directory_locked(rec):
                live = H.Store(rec).capture()
                if _inventory_digest(live) != journal['source_inventory']:
                    raise ValueError('history report source changed before contribution capture')
                mutation = _mutation_from_journal(journal)
                _verify_history_report(rec, root, event, envelope, journal, store.project, mutation.to_data())
                A.verify_prepared(rec, mutation)
                artifact = _scoped_artifact(rec, event, envelope, journal, live)
                if artifact['revision'] != B.from_contribution(bundle)['revision']:
                    raise ValueError('scoped report history differs from retained source preparation')
                for relative, expected in journal['evidence_hashes'].items():
                    raw = T._read(T._target(rec.parent, relative))
                    if raw is None or _sha(raw) != expected:
                        raise ValueError('scoped report evidence changed before capture: ' + relative)
                portable = journal['portable_source']
                if bundle['files'].get(portable) != envelope['source_quote'].encode('utf-8'):
                    raise ValueError('scoped report source bytes differ from retained quote')
                return True
        receipt = store.capture(bundle, event_id=journal['pending_event_id'],
            contribution_id=journal['pending_event_id'], shareability='project',
            expected_policy=event['project_policy'], expected_sources={str(rec): journal['before_hash']},
            verify_source=verify_source)
    if crash_after_commit:
        raise _CrashAfterCommit('simulated interruption after pending history capture')
    return _finish(root, event, envelope, 'project_captured', 'Complete scoped history report captured in pending_grounding',
                   pending=receipt, source=journal['source_id'], diagnostics=journal.get('diagnostics', []))


def _process_history_report(rec, root, event, envelope, *, crash_after_commit=False):
    with _event_lock(rec, event) as project:
        scope = envelope.get('scope')
        if isinstance(scope, dict) and scope.get('kind') == 'unclear':
            return _question(root, event, envelope, 'unclear report scope; retained privately')
        project_capture = project.config()['mode'] == 'advanced' and envelope.get('shareability') == 'project' and \
                isinstance(scope, dict) and scope.get('kind') in ('project', 'external')
        portable_source = (Path(P.layout(rec)['home']) / 'evidence' / 'reports' /
                           (event['event_id'] + '.txt')).relative_to(rec.parent).as_posix()
        captured = P._peer('history_store').Store(rec).capture()
        document = P._peer('history_authoring')._document(
            P._peer('history_adapter').from_store_capture(captured).document)
        if _report_private(document, envelope):
            return _question(root, event, envelope, 'private or unclear original source permission; report retained privately')
        target = _report_target(rec, envelope)
        if target['body_sha256'] != event['target_snapshot']['body_sha256']:
            return _question(root, event, envelope, 'target changed after capture; report was retained without overwriting it')
        context = _write_context(rec, project)
        before = _history_graph(rec, _report_seeds(envelope), capture=captured)
        A = P._peer('history_authoring')
        mutation = A.prepare_batch(rec, _report_actions(event, envelope, portable_source=portable_source), operation='report-' + event['event_id'],
            capture=captured, context=_history_report_binding(event, context),
            evidence={portable_source: envelope['source_quote'].encode('utf-8')})
        candidate = _history_after_capture(captured, mutation)
        after = _history_graph(rec, _report_seeds(envelope), capture=candidate)
        if _report_private(A._document(P._peer('history_adapter').from_store_capture(candidate).document), envelope):
            return _question(root, event, envelope, 'private or unclear prepared source permission')
        target_bodies = P.bodies(A._document(P._peer('history_adapter').from_store_capture(candidate).document))
        target_hash = (_body_hash({op['id']: target_bodies.get(op['id']) for op in envelope['updates']})
                       if 'updates' in envelope else _body_hash(target_bodies[envelope['target']]))
        journal = {'event_id': event['event_id'], 'phase': 'prepared', 'history': True,
                   'before_hash': _sha(captured.entry_bytes), 'after_hash': _sha(candidate.entry_bytes),
                   'before_graph': before, 'prepared_graph': after, 'source_id': 's.ingest_' + event['event_id'],
                   'target_after_sha256': target_hash, 'diagnostics': [],
                   'portable_source': portable_source,
                   'mutation': base64.b64encode(mutation.to_bytes()).decode('ascii')}
        if before.get('assessment_profile') == 'core/v1':
            gate = P._peer('reasoning.ingestion')
            failures = gate.gate(P, before, after, recorded_pin_subjects=_history_pin_support(mutation))
            if failures:
                return _question(root, event, envelope, '; '.join(failures), validation_issues=failures)
            journal['core_gate'] = gate.receipt(before, after)
        if project_capture:
            journal['portable_source'] = portable_source
            _prepare_scoped_history_report(rec, root, event, envelope, journal, captured)
        _save(root / 'journals' / (event['event_id'] + '.json'), journal)
        if not project_capture:
            _publish_history_report(rec, root, event, envelope, journal, project)
    if project_capture:
        return _capture_scoped_history_report(rec, root, event, envelope, journal,
                                             crash_after_commit=crash_after_commit)
    if crash_after_commit:
        raise _CrashAfterCommit('simulated interruption after history commit')
    return _finish_history_report(rec, root, event, envelope, journal, recovered=False)


def recover_history(record, state_dir, event_id, *, direction='after', _processor_locked=False):
    """Recover/cancel the exact report-history/v1 journal; never regenerate intents."""
    rec, root = _existing_layout(record, state_dir)[:2]
    if not _processor_locked:
        with _file_lock(root / 'processor.lock'):
            return recover_history(rec, root, event_id, direction=direction, _processor_locked=True)
    event = _load(_event_file(root, event_id))
    journal = _load(root / 'journals' / (event_id + '.json'))
    if not event or not journal or not journal.get('history'):
        raise ValueError('no_recovery_pending')
    envelope, error = _capture_payload(root, event)
    if error:
        raise ValueError(error)
    mutation = _mutation_from_journal(journal)
    with _event_lock(rec, event) as project:
        if direction == 'before':
            live = P._peer('history_store').Store(rec).capture()
            if mutation.to_data()['operation'] in live.commits:
                raise ValueError('history_already_committed: committed evidence needs an explicit new act')
            pending = _history_shared(rec)
            encoded = P._peer('history_contract').encode_document(_history_report_envelope(rec, root, event, mutation))
            if not pending.is_file() or pending.read_bytes() != encoded:
                raise ValueError('history report recovery journal mismatch')
            journal['phase'] = 'cancelled'
            P._peer('history_transaction')._replace(root / 'journals' / (event_id + '.json'), _json_bytes(journal))
            result = _question(root, event, envelope, 'report cancelled before history commit')
            pending.unlink()
            P._peer('history_transaction')._sync(pending.parent)
            return result
        if direction != 'after':
            raise ValueError('invalid recovery direction')
        _verify_history_report(rec, root, event, envelope, journal, project, mutation.to_data())
        if journal.get('phase') == 'record_committed':
            live = P._peer('history_store').Store(rec).capture()
            manifest = next(item['after'] for item in mutation.files if item['role'] == 'history_commit')
            if journal.get('committed_mutation') != mutation.to_data()['digest'] or \
                    live.commits.get(mutation.to_data()['operation']) != manifest:
                raise ValueError('history report completion has no matching committed operation')
        if journal.get('phase') != 'record_committed' or _history_shared(rec).exists():
            _publish_history_report(rec, root, event, envelope, journal, project)
    return _finish_history_report(rec, root, event, envelope, journal, recovered=True)


def _prepare_core(rec, root, event, envelope, before_bytes):
    """Prepare one final core world; public legacy mark/gate remain dormant."""
    eid = event['event_id']
    shadow = _shadow(rec, root, event, before_bytes)
    source_id = 's.ingest_' + eid
    cited = envelope.get('source', source_id)
    if cited == source_id and 'source' in envelope:
        raise ValueError('a report cannot cite its own capture as an existing source')
    seeds = _report_seeds(envelope)
    source_body = {'name': 'Captured report', 'file': event['source_file'], 'read': envelope['date'],
        'recorded_for': 'Update ' + (', '.join(seeds) if isinstance(seeds, list) else seeds) + ' from this captured report.'}
    if 'source' in envelope:
        source_body['from'] = cited
        if 'at' in envelope:
            source_body['at'] = envelope['at']
    actions = [{'kind': 'add', 'id': source_id, 'body': source_body,
                'as_of': envelope['date'], 'into': event['target_snapshot']['source_collection']}]
    for operation in envelope.get('updates', [{'kind': 'set', 'id': envelope.get('target'), 'value': envelope.get('value')}]):
        action = copy.deepcopy(operation)
        action['as_of'] = envelope['date']
        location = action.pop('at', envelope.get('at', 'entire captured report'))
        if 'scope' in envelope:
            action['_record_scope'] = copy.deepcopy(envelope['scope'])
        if action['kind'] == 'set':
            action.update(source=cited, at=location)
        else:
            body = action['body']
            if 'scope' in envelope:
                if 'scope' in body and body['scope'] != envelope['scope']:
                    raise ValueError('entry scope differs from report scope: ' + action['id'])
                body['scope'] = copy.deepcopy(envelope['scope'])
            if 'v' in body or 'quoted' in body:
                body.update({'from': cited, 'at': location, 'of': envelope['date']})
        actions.append(action)
    core = P._peer('reasoning.ingestion')
    before = _graph(shadow, seeds, profile=envelope.get('profile'))
    replacements = []
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        after_bytes, diagnostics = core.stage(P, [str(shadow)], actions, profile=envelope.get('profile'),
                                              replacement_sink=replacements.extend)
    _atomic(shadow, after_bytes)
    for replacement in replacements:
        P.keep_replaced([str(shadow)], replacement['id'], replacement['old'], replacement['ended'],
                        replacement['stamp'], replacement['dropped'])
    after = _graph(shadow, seeds)
    failures = core.gate(P, before, after)
    if failures:
        raise PreparationRefused(failures, diagnostics)
    doc = P._peer('reasoning.authoring').load(P, [str(shadow)])
    if _report_private(doc, envelope):
        raise ValueError('private or unclear source permission in prepared core report')
    # This is the same captured-source ownership test used by the ordinary gate.
    if not recording_source(source_id, P.bodies(doc)[source_id], str(shadow), _preparing={
            'record': str(rec), 'state_dir': str(root), 'event_id': eid, 'shadow': str(shadow)}):
        raise ValueError('prepared core report has no verified recording intent')
    target_hash = _batch_fingerprint(shadow, envelope['updates']) if 'updates' in envelope else _target(shadow, envelope['target'], core_writer=True)['body_sha256']
    return shadow, after_bytes, after, target_hash, diagnostics


def _prepare(rec, root, event, envelope, before_bytes):
    document = P.parse(text=before_bytes) or {}
    if P._peer('reasoning.authoring').selected(document, envelope.get('profile')):
        return _prepare_core(rec, root, event, envelope, before_bytes)
    eid = event["event_id"]
    shadow = _shadow(rec, root, event, before_bytes)
    mark = shadow.parent / 'mark.json'
    paths = [str(shadow)]
    source_id = "s.ingest_" + eid
    cited_source = envelope.get("source", source_id)
    if envelope.get("source") == source_id:
        raise ValueError("a report cannot cite its own capture as an existing source")
    date = envelope["date"]
    diagnostics = []
    gate_output = io.StringIO()
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        P.mark(str(mark), paths)
        seeds = _report_seeds(envelope)
        source_body = {"name": "Captured report", "file": event["source_file"], "read": date,
                       "recorded_for": "Update " + (", ".join(seeds) if isinstance(seeds, list) else seeds) + " from this captured report."}
        if "source" in envelope:
            source_body["from"] = cited_source
            if "at" in envelope:
                source_body["at"] = envelope["at"]
        P.apply(paths, {
            "kind": "add", "id": source_id, "as_of": date, "why": None,
            "into": event["target_snapshot"]["source_collection"],
            "hypothesis": None, "source": None, "at": None,
            "body": source_body,
        })
        operations = envelope.get("updates", [{"kind": "set", "id": envelope.get("target"),
                                               "value": envelope.get("value")}])
        # Existing readings settle first. New entries follow in dependency order, so a
        # new judgment's snapshot sees the final readings, never an intermediate price.
        operations = sorted(operations, key=lambda op: op["kind"] != "set")
        for op in operations:
            action = {"kind": op["kind"], "id": op["id"], "as_of": date, "why": None,
                      "into": op.get("into"), "hypothesis": None, "source": None, "at": None}
            if 'drops' in op:
                action['drops'] = copy.deepcopy(op['drops'])
            if envelope.get('scope') is not None:
                action['_record_scope'] = copy.deepcopy(envelope['scope'])
            location = op.get("at", envelope.get("at", "entire captured report"))
            if op["kind"] == "set":
                action.update(value=op["value"], source=cited_source, at=location)
            else:
                body = copy.deepcopy(op["body"])
                if envelope.get('scope') is not None:
                    if 'scope' in body and body['scope'] != envelope['scope']:
                        raise ValueError('entry scope differs from report scope: ' + op['id'])
                    body['scope'] = copy.deepcopy(envelope['scope'])
                if "v" in body or "quoted" in body:
                    body.update({"from": cited_source, "at": location, "of": date})
                action["body"] = body
            P.apply(paths, action, diagnostics=diagnostics)
            # A privacy route can return success for retaining a private draft.
            # Batch atomicity requires an actual authored entry for every item.
            saved = P.bodies(P.load(paths, read_mode='frozen')).get(op['id'])
            if op['kind'] == 'set' or 'v' in op.get('body', {}) or 'quoted' in op.get('body', {}):
                wanted = op.get('value') if op['kind'] == 'set' else op['body'].get('v', op['body'].get('quoted'))
                actual = saved.get('v', saved.get('quoted')) if isinstance(saved, dict) else None
                if type(actual) is not type(wanted) or actual != wanted or saved.get('from') != cited_source:
                    raise ValueError('report operation was not applied: ' + op['id'])
            elif not isinstance(saved, dict):
                raise ValueError('report entry was not retained: ' + op['id'])
        with contextlib.redirect_stdout(gate_output), contextlib.redirect_stderr(gate_output):
            gate = P.gate(str(mark), paths, _recording_context={
                'record': str(rec), 'state_dir': str(root), 'event_id': eid, 'shadow': str(shadow)})
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
        return _integrity_question(root, event, integrity, committed)
    T = P._peer('history_transaction')
    if journal.get('history'):
        return recover_history(rec, root, event['event_id'], _processor_locked=True)
    if 'mutation' in journal:
        mutation = _mutation_from_journal(journal)
        data = mutation.to_data()
        _bind_report_mutation(rec, event, journal, data)
        shared = rec.parent / T.journal_for(rec)
        if shared.exists():
            with _event_lock(rec, event) as project:
                retained = T.PreparedMutation.from_bytes(shared.read_bytes())
                if retained.to_bytes() != mutation.to_bytes():
                    raise ValueError('recovery_required: another report operation owns the record journal')
                T.recover_legacy(rec.parent, T.journal_for(rec),
                    verify=lambda supplied: _verify_mutation(rec, root, event, envelope, journal, project, supplied),
                    on_committed=_committed(root, event, journal))
                P.forget(rec)
        if data['receipt']['before'] != journal['before_graph'] or data['receipt']['after'] != journal['prepared_graph']:
            raise ValueError('prepared report assessment differs from retained receipt')
        applied = (journal.get('phase') == 'record_committed'
                   and journal.get('committed_mutation') == data['digest'])
        if not applied:
            # Matching bytes alone cannot establish that this event was published.
            current = [(T._read(T._target(rec.parent, item['path'])), item) for item in mutation.files]
            if all(raw == item['after'] for raw, item in current):
                raise ValueError('after_images_match: report completion has no retained operation receipt')
            if any(raw != item['before'] for raw, item in current):
                raise ValueError('concurrent_edit: unfinished report lost its recovery journal')
            return None
        # The durable operation receipt proves publication. Later unrelated edits
        # need not block acknowledgment, but a changed/reverted target is never
        # replayed. Target/source equality alone did not establish this proof.
        if any(T._read(T._target(rec.parent, item['path'])) != item['after'] for item in mutation.files):
            try:
                target_hash = (_batch_fingerprint(rec, envelope['updates']) if 'updates' in envelope else
                               _target(rec, envelope['target'], core_writer=True)['body_sha256'])
                source = P.bodies(P._peer('reasoning.authoring').load(P, [str(rec)])).get(journal['source_id'], {})
                retained = target_hash == journal['target_after_sha256'] and source.get('file') == event['source_file']
            except (Exception, SystemExit):
                retained = False
            if not retained:
                return _question(root, event, envelope,
                    'the committed report was changed or reverted later; it will not be applied again',
                    record_committed=True)
    else:
        # Older event journals do not certify a multi-file prepared operation.
        # Only their explicit durable completion phase can acknowledge a retry.
        applied = journal.get('phase') == 'record_committed' and _sha(rec.read_bytes()) == journal['after_hash']
    if not applied:
        return None
    try:
        after = _graph(rec, _report_seeds(envelope))
    except P.Refused as error:
        return _question(root, event, envelope, str(error), record_committed=True)
    if journal['before_graph'].get('assessment_profile') == 'core/v1' or after.get('assessment_profile') == 'core/v1':
        try:
            core_gate = P._peer('reasoning.ingestion')
            core_gate.verify_receipt(journal.get('core_gate'), journal['before_graph'], journal['prepared_graph'])
            failures = core_gate.gate(P, journal['before_graph'], after)
            if failures:
                raise ValueError('; '.join(failures))
        except (ValueError, KeyError) as error:
            return _question(root, event, envelope, str(error), record_committed=True)
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
        return _integrity_question(root, event, integrity, committed)
    if journal and journal.get('phase') == 'cancelled':
        return _question(root, event, envelope, 'report cancelled before history commit')
    if journal and journal.get('history_contribution'):
        return _capture_scoped_history_report(rec, root, event, envelope, journal,
                                              crash_after_commit=crash_after_commit)
    if journal and journal.get('pending_event_id'):
        G = P._peer('pending_grounding')
        pending = G.Store(event['project_root']).receipt(journal['pending_event_id'])
        if pending is not None:
            if pending['revision'] != journal['pending_revision']:
                return _question(root, event, envelope, 'captured contribution differs from the prepared report')
            return _finish(root, event, envelope, 'project_captured', 'Complete report captured in pending_grounding',
                           pending=pending, source=journal['source_id'], diagnostics=journal.get('diagnostics', []))
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
    if _history_active(rec):
        return _process_history_report(rec, root, event, envelope, crash_after_commit=crash_after_commit)
    for attempt in range(MAX_REPREPARES + 1):
        with _event_lock(rec, event) as project:
            try:
                original_doc = P._peer('reasoning.authoring').load(P, [str(rec)])
                if _report_private(original_doc, envelope):
                    return _question(root, event, envelope, 'private or unclear original source permission; entire report retained privately')
                current_target = _report_target(rec, envelope)
            except (Exception, SystemExit) as exc:
                return _question(root, event, envelope, "target conflict: " + " ".join(str(exc).split()))
            if current_target["body_sha256"] != event["target_snapshot"]["body_sha256"] and \
                    ("updates" in envelope or not _owned_predecessor(root, event, current_target)):
                return _question(root, event, envelope,
                                 "target changed after capture; the report was retained without overwriting it")
            prepared_target_sha256 = current_target["body_sha256"]
            before_bytes = rec.read_bytes()
            before_files = _legacy_files(rec)
            context = _write_context(rec, project)
            try:
                before_graph = _graph(rec, _report_seeds(envelope), profile=envelope.get('profile'))
            except P.Refused as error:
                return _question(root, event, envelope, str(error))
        try:
            _, integrity = _capture_payload(root, event)
            if integrity:
                return _integrity_question(root, event, integrity)
            shadow, after_bytes, prepared_graph, target_after_sha256, diagnostics = \
                _prepare(rec, root, dict(event, _before_files=before_files), envelope, before_bytes)
            mutation = _prepared_mutation(rec, shadow, event, before_files, context, before_graph, prepared_graph)
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
            "brief_hash": next((_sha(x["before"]) if x["before"] is not None else None) for x in before_files if x["role"] == "view"),
            "diagnostics": diagnostics,
            "mutation": base64.b64encode(mutation.to_bytes()).decode('ascii'),
        }
        if before_graph.get('assessment_profile') == 'core/v1':
            core_gate = P._peer('reasoning.ingestion')
            failures = core_gate.gate(P, before_graph, prepared_graph)
            if failures:
                return _question(root, event, envelope, '; '.join(failures), validation_issues=failures)
            journal['core_gate'] = core_gate.receipt(before_graph, prepared_graph)
        _save(journal_path, journal)
        with _event_lock(rec, event) as project:
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
            scope = envelope.get('scope')
            project_capture = (project.config()['mode'] == 'advanced' and envelope.get('shareability') == 'project'
                               and isinstance(scope, dict) and scope.get('kind') in ('project', 'external'))
            # Store.capture owns the policy lock; project capture follows outside it.
            if not project_capture:
                if 'core_gate' in journal:
                    # A pending contribution can appear without changing record bytes.
                    # Recheck semantic promotion under the final policy/record lock.
                    try:
                        P._peer('reasoning.authoring').prepare(P, [str(rec)], {'profile': envelope.get('profile')})
                    except P.Refused as error:
                        return _question(root, event, envelope, str(error))
                T = P._peer('history_transaction')
                T.publish_legacy(rec.parent, T.journal_for(rec), mutation,
                    verify=lambda data: _verify_mutation(rec, root, event, envelope, journal, project, data),
                    on_committed=_committed(root, event, journal))
                P.forget(rec)
        if project_capture:
            G = P._peer('pending_grounding')
            document = P._peer('reasoning.authoring').load(P, [str(shadow)])
            if P._peer('reasoning.authoring').selected(document):
                return _question(root, event, envelope, 'unsupported_capability: core contribution capture requires the versioned contribution writer')
            roots = _report_seeds(envelope)
            roots = [roots] if isinstance(roots, str) else roots
            # The report is independently retained evidence, including when the
            # readings cite an existing source or contain only qualitative rules.
            source_id = 's.ingest_' + eid
            roots = [*roots, source_id]
            for name in roots:
                G.entries(document)[name][1]['scope'] = copy.deepcopy(scope)
            portable = '.kpopper/evidence/reports/' + eid + '.txt'
            G.entries(document)[source_id][1]['file'] = portable
            closure = G.closure(document, roots)
            if any(portable in set(G._files(body)) for name, (_, body) in G.entries(closure).items()
                   if name != source_id):
                raise ValueError('captured report evidence path conflicts with an existing source')
            evidence = {}
            for name in G._files(closure):
                if name == portable:
                    evidence[name] = envelope['source_quote'].encode('utf-8')
                else:
                    location = (rec.parent / G.M.relative_path(name)).resolve()
                    location.relative_to(rec.parent.resolve())
                    evidence[name] = location.read_bytes()
            bundle = G.prepare(document, roots, scope=scope, shareability='project', evidence=evidence)
            journal.update(pending_event_id='report-' + eid, pending_revision=bundle['revision'])
            _save(journal_path, journal)
            captured = G.Store(project).capture(bundle, event_id='report-' + eid, contribution_id='report-' + eid,
                                                 shareability='project', expected_policy=event.get('project_policy'),
                                                 expected_sources={str(rec): journal['before_hash']})
            return _finish(root, event, envelope, 'project_captured', 'Complete report captured in pending_grounding',
                           pending=captured, source=source_id, diagnostics=diagnostics)
        if crash_after_commit:
            raise _CrashAfterCommit("simulated interruption after record commit")
        try:
            after = _graph(rec, _report_seeds(envelope))
        except P.Refused as error:
            return _question(root, event, envelope, str(error), record_committed=True)
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
            try:
                out.append(_process_event(rec, root, event, _crash_after_commit))
            except (ValueError, OSError, P._peer('history_authoring').P.Refused) as error:
                T = P._peer('history_transaction')
                retained = _load(root / 'journals' / (event['event_id'] + '.json')) or {}
                retry_capture = isinstance(error, OSError) and retained.get('history_contribution')
                if (rec.parent / T.journal_for(rec)).exists() or _history_shared(rec).exists() or retry_capture:
                    # Keep interrupted operations retryable without spawning an
                    # unbounded retry loop or manufacturing a terminal receipt.
                    event.update(state='recovery_required', reason=str(error))
                    _save(_event_file(root, event['event_id']), event)
                    out.append(_summary(event))
                    continue
                envelope, integrity = _capture_payload(root, event)
                out.append(_integrity_question(root, event, integrity) if integrity else
                           _question(root, event, envelope, str(error)))
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
    event = capture(envelope, record, state_dir, start=False)
    if event.get('state') == 'project_captured':
        return event
    rec, root = _layout(event.get('record') or _routed_record(record)[0], event.get('state_dir') or state_dir)
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
    return 0 if answer and answer.get('state') in ('applied', 'project_captured') else 1


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
