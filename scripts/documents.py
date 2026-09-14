"""Standalone authored documents, finite evidence snapshots and reviewed text edits.

Author code is kept in an opaque sandboxed frame. Evidence is data throughout this
module; neither source files nor manifests supply executable expressions or paths
outside the caller's source root. A snapshot checks a reading, not world truth.
"""
from __future__ import annotations

import copy
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation, ROUND_HALF_UP, localcontext
import errno
import hashlib
from html import escape
from html.parser import HTMLParser
import json
import math
import os
from pathlib import Path
import re
import tempfile

try:
    from .document_html import DocumentError, parse_html, patch_html, text_content
except ImportError:
    from document_html import DocumentError, parse_html, patch_html, text_content

HERE = Path(__file__).resolve().parent
MAX_SOURCE = 8_000_000
MAX_ARTIFACT = 40_000_000
ID = re.compile(r"[A-Za-z][A-Za-z0-9_.-]{0,127}\Z")
KINDS = {"value", "quote", "sum", "difference", "product", "ratio", "inference"}
FRAME_CSP = ("default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' data: blob:; "
             "style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; "
             "connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; "
             "form-action 'none'; worker-src 'none'")


def timestamp():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def digest(raw):
    return hashlib.sha256(raw.encode("utf-8") if isinstance(raw, str) else raw).hexdigest()


def encoded(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, allow_nan=False, separators=(",", ":"))


def safe_json(value):
    # This applies to ALL strings, especially the authored HTML and bridge code.
    return encoded(value).replace("<", "\\u003c").replace("\u2028", "\\u2028").replace("\u2029", "\\u2029")


def read_json(raw, decimal=False):
    def pairs(items):
        obj = {}
        for key, value in items:
            if key in obj:
                raise DocumentError("Duplicate JSON key: " + key)
            obj[key] = value
        return obj

    def constant(value):
        raise DocumentError("Non-finite JSON number")

    def floating(value):
        result = Decimal(value) if decimal else float(value)
        finite = result.is_finite() if decimal else math.isfinite(result)
        if not finite:
            raise DocumentError("Non-finite JSON number")
        return result

    try:
        return json.loads(raw, object_pairs_hook=pairs, parse_constant=constant, parse_float=floating)
    except (ValueError, RecursionError) as error:
        raise DocumentError("Invalid JSON: " + str(error)) from error


def read_bytes(path, limit=MAX_SOURCE):
    with Path(path).open("rb") as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise DocumentError("Input exceeds the supported size limit")
    return data


def scoped(root, relative, directory=False):
    if not isinstance(relative, str) or not relative or Path(relative).is_absolute() or ".." in Path(relative).parts:
        raise DocumentError("Source paths must be relative paths inside --root")
    root = Path(root).resolve()
    candidate = root / relative
    for part in (candidate, *candidate.parents):
        if part == root:
            break
        if part.is_symlink():
            raise DocumentError("Source paths must not traverse symlinks")
    resolved = candidate.resolve()
    try:
        resolved.relative_to(root)
    except ValueError as error:
        raise DocumentError("Source path escapes --root") from error
    if resolved.exists() and not (resolved.is_dir() if directory else resolved.is_file()):
        raise DocumentError("Source must be a regular " + ("directory" if directory else "file"))
    return resolved


def _text(value, name, empty=False, limit=20000):
    if not isinstance(value, str) or (not empty and not value.strip()) or len(value) > limit:
        raise DocumentError(name + " must be " + ("" if empty else "nonempty ") + "text")
    return value


def _id(value):
    if not isinstance(value, str) or not ID.fullmatch(value):
        raise DocumentError("IDs must start with a letter and use letters, digits, dot, dash or underscore")
    return value


def _keys(obj, allowed, name):
    if not isinstance(obj, dict) or set(obj) - set(allowed):
        raise DocumentError(name + " has unknown fields or is not an object")


def _number(value):
    if isinstance(value, bool) or not isinstance(value, (int, float, str, Decimal)):
        raise DocumentError("Expected a finite number")
    if len(str(value)) > 512:
        raise DocumentError("Number is outside the supported finite range")
    try:
        result = Decimal(str(value))
    except InvalidOperation as error:
        raise DocumentError("Expected a finite number") from error
    if not result.is_finite() or (result and abs(result.adjusted()) > 100):
        raise DocumentError("Number is outside the supported finite range")
    return result


def _format_spec(spec):
    _keys(spec, {"decimals", "scale", "prefix", "suffix", "thousands", "decimal"}, "format")
    if type(spec.get("decimals", 0)) is not int or not 0 <= spec.get("decimals", 0) <= 8:
        raise DocumentError("format.decimals must be an integer from 0 to 8")
    if "scale" in spec:
        _number(spec["scale"])
    for key in ("prefix", "suffix"):
        if key in spec:
            _text(spec[key], "format." + key, empty=True, limit=64)
    if spec.get("thousands", "") not in ("", ",", ".", " ", "\u00a0") or spec.get("decimal", ".") not in (".", ","):
        raise DocumentError("Unsupported numeric separators")
    if spec.get("thousands") == spec.get("decimal", "."):
        raise DocumentError("Thousands and decimal separators must differ")
    return copy.deepcopy(spec)


def _input(inp):
    _keys(inp, {"source", "pointer", "quote"}, "claim input")
    _id(inp.get("source"))
    if "pointer" in inp and "quote" in inp:
        raise DocumentError("A source input has one selector, not both pointer and quote")
    if "pointer" in inp:
        p = _text(inp["pointer"], "pointer", empty=True, limit=2000)
        if p and not p.startswith("/") or re.search(r"~(?![01])", p):
            raise DocumentError("Expected a JSON pointer")
    if "quote" in inp:
        _text(inp["quote"], "source quote")
    return copy.deepcopy(inp)


def selection_key(inp):
    return encoded({k: v for k, v in inp.items() if k != "source"})


def normalize_claims(claims):
    if not isinstance(claims, list) or len(claims) > 500:
        raise DocumentError("Expected at most 500 claims")
    result, ids = [], set()
    for claim in claims:
        _keys(claim, {"id", "label", "kind", "inputs", "format", "reason", "group"}, "claim")
        cid = _id(claim.get("id"))
        if cid in ids:
            raise DocumentError("Duplicate claim ID: " + cid)
        ids.add(cid)
        kind = claim.get("kind")
        if kind not in KINDS:
            raise DocumentError("Unsupported check kind: " + str(kind))
        if kind == "ratio" and (not isinstance(claim.get("format"), dict) or "decimals" not in claim["format"]):
            raise DocumentError("A ratio requires an explicit display precision in format.decimals")
        inputs = claim.get("inputs", [])
        if not isinstance(inputs, list) or len(inputs) > 32:
            raise DocumentError("A claim needs at most 32 explicit source inputs")
        inputs = [_input(i) for i in inputs]
        arity = {"value": 1, "quote": 1, "difference": 2, "ratio": 2}
        if kind in arity and len(inputs) != arity[kind] or kind in {"sum", "product"} and not inputs:
            raise DocumentError("Wrong number of inputs for " + kind)
        if kind != "inference" and any(len(i) != 2 for i in inputs):
            raise DocumentError("A mechanical check requires an exact source selector")
        if kind == "quote" and "quote" not in inputs[0]:
            raise DocumentError("A quotation check requires an exact text quote")
        out = {"id": cid, "label": _text(claim.get("label"), "claim label", limit=300),
               "kind": kind, "inputs": inputs}
        if "format" in claim:
            out["format"] = _format_spec(claim["format"])
        if "group" in claim:
            out["group"] = _id(claim["group"])
        if kind == "inference":
            out["reason"] = _text(claim.get("reason"), "inference reason")
            if "format" in claim:
                raise DocumentError("An inference does not have a numeric format")
        elif "reason" in claim:
            out["reason"] = _text(claim["reason"], "claim reason")
        result.append(out)
    return result


def _pointer(data, pointer):
    if not pointer:
        return data
    for component in pointer[1:].split("/"):
        key = component.replace("~1", "/").replace("~0", "~")
        if isinstance(data, dict) and key in data:
            data = data[key]
        elif isinstance(data, list) and re.fullmatch(r"0|[1-9][0-9]*", key) and int(key) < len(data):
            data = data[int(key)]
        else:
            raise DocumentError("The selected field is unavailable")
    return data


def _atom(value):
    if type(value) is bool:
        return {"type": "boolean", "value": value}
    if isinstance(value, (int, float, Decimal)):
        return {"type": "number", "value": str(_number(value))}
    if isinstance(value, str):
        return {"type": "string", "value": _text(value, "selected value", empty=True)}
    if value is None:
        raise DocumentError("The selected field has no value")
    raise DocumentError("The selected field is not a scalar value")


def _source_spec(spec):
    _keys(spec, {"name", "path", "format", "uri", "unavailable", "representation", "event_id", "state_dir"}, "source")
    _text(spec.get("name"), "source name", limit=300)
    if "uri" in spec:
        uri = _text(spec["uri"], "source URI", limit=4000)
        if not re.match(r"https?://[^\s]+\Z", uri, re.I):
            raise DocumentError("Original-source links must be HTTP(S) citation URLs")
    if "unavailable" in spec:
        _text(spec["unavailable"], "unavailable-source reason")
        if "path" in spec or "event_id" in spec or "state_dir" in spec:
            raise DocumentError("An unavailable source cannot supply a path or event")
    elif spec.get("format") not in {"json", "text", "record"} or "path" not in spec:
        raise DocumentError("A source needs a local path and json, text or record format")
    if spec.get("representation", "file") not in {"file", "extraction"}:
        raise DocumentError("representation must be file or extraction")
    if "event_id" in spec:
        if not isinstance(spec["event_id"], str) or not re.fullmatch(r"[0-9a-f]{32}", spec["event_id"]):
            raise DocumentError("event_id must be the canonical 32-character ID returned by ingestion")
        if spec.get("format") != "record":
            raise DocumentError("An ingestion event must bind a record source")
    if "state_dir" in spec and "event_id" not in spec:
        raise DocumentError("state_dir requires an ingestion event")


def _record_selection(path, inp):
    # Reuse the canonical reader's bounded record and scalar/citation rules.
    try:
        from . import ingestion as I
    except ImportError:
        import ingestion as I
    pointer = inp.get("pointer", "")
    pieces = [x.replace("~1", "/").replace("~0", "~") for x in pointer.split("/")[1:]]
    if len(pieces) != 2 or pieces[1] not in {"v", "quoted"}:
        raise DocumentError("Record selectors are /ENTRY/v or /ENTRY/quoted for stored readings")
    nid, field = pieces
    try:
        target = I._target(path, nid)
        doc, _, _, _, _ = I._record_world(path)
        body = I.P.bodies(doc)[nid]
        if field not in body:
            raise DocumentError("The selected record field is unavailable")
        cited = I.P.bodies(doc).get(target["source"])
        if not isinstance(cited, dict):
            raise DocumentError("The reading has no recorded source")
        citation = {"source": target["source"], "name": I.P.named(cited) or target["source"],
                    "at": str(body.get("at", "Location not recorded")),
                    "date": str(body.get("of", cited.get("read", "Date not recorded")))}
        return _atom(target["value"]), citation
    except (ValueError, KeyError, SystemExit) as error:
        raise DocumentError("Record reading is unavailable: " + str(error)) from error


def _record_selections(path, raw, inputs):
    """Give the canonical reader one frozen file, never a pointer or glob path.

    The record reader deliberately supports following other records/hypotheses. A
    document source has narrower authority: refuse those layouts before that reader
    can open anything, then reuse its scalar and citation rules on the captured bytes.
    """
    try:
        from . import ingestion as I
    except ImportError:
        import ingestion as I
    try:
        body = I.P.yaml.safe_load(raw)
    except I.P.yaml.YAMLError as error:
        raise DocumentError("The source could not be read as a record") from error
    if not isinstance(body, dict) or any(body.get(key) for key in ("record", "also")):
        raise DocumentError("Pointer records require an explicit source extraction")
    hypotheses = Path(I.P.layout(path)["hypotheses"])
    if hypotheses.is_symlink() or (hypotheses.is_dir() and any(
            p.suffix in {".yaml", ".yml"} for p in hypotheses.iterdir())):
        raise DocumentError("Records with hypotheses require an explicit source extraction")
    result = {}
    with tempfile.TemporaryDirectory(prefix="kpopper-record-snapshot-") as directory:
        snapshot = Path(directory) / I.P.ENTRY
        snapshot.write_bytes(raw)
        for key, inp in inputs.items():
            if len(inp) == 1:
                continue
            try:
                value, citation = _record_selection(snapshot, inp)
                result[key] = {"status": "available", "value": value, "citation": citation}
            except DocumentError as error:
                result[key] = {"status": "unavailable", "reason": str(error)}
    return result


def _event_binding(path, spec, selections, root):
    try:
        from . import ingestion as I
    except ImportError:
        import ingestion as I
    state_dir = scoped(root, spec["state_dir"], directory=True) if "state_dir" in spec else None
    try:
        receipt = I.status(spec["event_id"], record=path, state_dir=state_dir)
    except (ValueError, OSError, RuntimeError) as error:
        raise DocumentError("The ingestion outcome could not be read; inspect or recover the event before refreshing") from error
    if not receipt or receipt.get("state") != "applied":
        raise DocumentError("The ingestion event is not durably applied; inspect it by event ID")
    readings = []
    if "updates" in receipt:
        for operation in receipt["updates"]:
            if operation["kind"] == "set":
                readings.append((operation["id"], operation["value"], ("v", "quoted")))
            elif operation["kind"] == "add":
                body = operation["body"]
                for field in ("v", "quoted"):
                    if field in body:
                        readings.append((operation["id"], body[field], (field,)))
    else:
        readings.append((receipt.get("target"), receipt.get("value"), ("v", "quoted")))
    selected, targets = [], set()
    for target, value, fields in readings:
        wanted = "/" + str(target).replace("~", "~0").replace("/", "~1")
        pointers = {wanted + "/" + field for field in fields}
        for selection in selections.values():
            if selection["selector"].get("pointer") in pointers:
                selected.append((selection, value))
                targets.add(target)
    if not selected or any(v.get("status") != "available" or v.get("value") != _atom(value) or
                           v.get("citation", {}).get("source") != receipt.get("source") for v, value in selected):
        raise DocumentError("Applied event does not match the selected current reading and citation")
    binding = {k: receipt.get(k) for k in ("event_id", "state", "target", "source_sha256", "envelope_sha256")}
    if "updates" in receipt:
        binding["targets"] = sorted(targets)
        binding["target"] = next(iter(targets)) if len(targets) == 1 else None
    return binding


def capture_sources(specs, claims, root, now=None, previous=None):
    if not isinstance(specs, dict) or len(specs) > 100:
        raise DocumentError("Expected at most 100 sources")
    previous = previous or {}
    needed = {}
    for claim in claims:
        for inp in claim["inputs"]:
            needed.setdefault(inp["source"], {})[selection_key(inp)] = inp
    if set(specs) - set(needed):
        raise DocumentError("Source inputs include an unreferenced source")
    if set(needed) - (set(specs) | set(previous)):
        raise DocumentError("Every source reference must be declared, including unavailable sources")
    result, now = {}, now or timestamp()
    for sid, inputs in needed.items():
        _id(sid)
        if sid not in specs:
            result[sid] = copy.deepcopy(previous[sid])
            result[sid]["reread"] = False
            continue
        spec = specs[sid]
        _source_spec(spec)
        source = {"name": spec["name"], "uri": spec.get("uri"), "format": spec.get("format"),
                  "representation": spec.get("representation", "file"), "read_at": None,
                  "attempted_at": now, "reread": True, "sha256": None, "status": "unavailable",
                  "selections": {}, "reason": spec.get("unavailable", "Source unavailable")}
        result[sid] = source
        if "unavailable" in spec:
            continue
        path = scoped(root, spec["path"])
        try:
            raw = read_bytes(path)
            text = raw.decode("utf-8-sig")
            data = read_json(text, decimal=True) if spec["format"] == "json" else None
        except (OSError, UnicodeError, DocumentError):
            source["reason"] = "The source could not be read as " + spec["format"]
            continue
        source.update(status="available", sha256=digest(raw), read_at=now, reason=None)
        record_values = {}
        if spec["format"] == "record":
            try:
                record_values = _record_selections(path, raw, inputs)
            except DocumentError as error:
                source.update(status="unavailable", reason=str(error))
                continue
        for key, inp in inputs.items():
            selector = {k: v for k, v in inp.items() if k != "source"}
            selection = {"selector": selector, "status": "unavailable"}
            source["selections"][key] = selection
            try:
                if not selector:  # An inference can cite the source without claiming an extraction.
                    selection.update(status="available", note="Source identity captured; no excerpt selected")
                elif spec["format"] == "record":
                    selection.update(record_values[key])
                elif "pointer" in inp and spec["format"] == "json":
                    selection.update(status="available", value=_atom(_pointer(data, inp["pointer"])))
                elif "quote" in inp and spec["format"] == "text":
                    quote = inp["quote"]
                    start = text.find(quote)
                    if start < 0 or text.find(quote, start + 1) >= 0:
                        raise DocumentError("The selected quotation is absent or ambiguous")
                    selection.update(status="available", value=_atom(quote),
                                     location="lines " + str(text[:start].count("\n") + 1) + "–" + str(text[:start + len(quote)].count("\n") + 1))
                else:
                    raise DocumentError("The selector does not match the source format")
            except DocumentError as error:
                selection["reason"] = str(error)
        # All fields share one captured file revision; do not combine a raced read.
        try:
            stable = read_bytes(path) == raw
        except OSError:
            stable = False
        if not stable:
            raise DocumentError("A source changed during capture; retry with a stable source copy")
        if "event_id" in spec:
            source["event"] = _event_binding(path, spec, source["selections"], root)
    return result


def _selection(inp, sources):
    source = sources.get(inp["source"], {})
    selected = source.get("selections", {}).get(selection_key(inp), {})
    if source.get("status") != "available" or selected.get("status") != "available":
        raise DocumentError(selected.get("reason") or source.get("reason") or "Selected evidence is unavailable")
    return selected


def _literal(atom, spec=None):
    if atom["type"] != "number":
        if spec:
            raise DocumentError("A numeric format cannot be applied to a nonnumeric reading")
        return str(atom["value"]).lower() if atom["type"] == "boolean" else atom["value"]
    number = _number(atom["value"])
    if spec is None:
        result = format(number, "f")
        if "." in result:
            result = result.rstrip("0").rstrip(".")
        return "0" if result in {"-0", ""} else result
    with localcontext() as context:
        context.prec = 240
        number *= _number(spec.get("scale", 1))
        decimals = spec.get("decimals", 0)
        number = number.quantize(Decimal(1).scaleb(-decimals), rounding=ROUND_HALF_UP)
    if number == 0:
        number = abs(number)
    result = format(number, ("," if spec.get("thousands") else "") + "." + str(decimals) + "f")
    integer, sep, fraction = result.partition(".")
    integer = integer.replace(",", spec.get("thousands", ""))
    return spec.get("prefix", "") + integer + (spec.get("decimal", ".") + fraction if sep else "") + spec.get("suffix", "")


def expected_text(claim, sources):
    atoms = [_selection(i, sources).get("value") for i in claim["inputs"]]
    if claim["kind"] in {"value", "quote"}:
        return _literal(atoms[0], claim.get("format"))
    if any(not a or a.get("type") != "number" for a in atoms):
        raise DocumentError("Arithmetic needs numeric source readings, not interpreted text")
    numbers = [_number(a["value"]) for a in atoms]
    with localcontext() as context:
        context.prec = 240
        kind = claim["kind"]
        if kind == "sum":
            value = sum(numbers, Decimal(0))
        elif kind == "difference":
            value = numbers[0] - numbers[1]
        elif kind == "product":
            value = Decimal(1)
            for number in numbers:
                value *= number
        elif kind == "ratio":
            if numbers[1] == 0:
                raise DocumentError("The divisor is zero; no ratio can be checked")
            value = numbers[0] / numbers[1]
        else:
            raise DocumentError("This claim has no supported mechanical check")
    return _literal(_atom(value), claim.get("format"))


def evaluate(claims, parsed, sources, now, previous=None):
    checks = {}
    for claim in claims:
        cid = claim["id"]
        anchor = parsed["anchors"][cid]
        changed = bool(previous is not None and any(
            sources[i["source"]].get("sha256") != previous.get(i["source"], {}).get("sha256") or
            sources[i["source"]].get("status") != previous.get(i["source"], {}).get("status")
            for i in claim["inputs"]))
        check = {"actual": anchor["text"], "status": "unchecked", "expected": None,
                 "detail": "", "checked_at": None, "source_changed": changed}
        checks[cid] = check
        if claim["kind"] == "inference":
            check["detail"] = claim["reason"]
            if changed:
                check["detail"] += " Sources changed; this inference needs another review."
            continue
        if not anchor["text_only"]:
            raise DocumentError("Mechanical claim " + cid + " must anchor a text-only element")
        try:
            expected = expected_text(claim, sources)
            check.update(expected=expected, status="match" if anchor["text"] == expected else "mismatch",
                         checked_at=now, detail="Exact " + claim["kind"] + " check against the captured source readings.")
        except (DocumentError, InvalidOperation, OverflowError) as error:
            check.update(status="unavailable", detail=str(error))
    return checks


def proposal_groups(html, claims, parsed, sources, checks, now):
    mechanical = [c for c in claims if c["kind"] != "inference"]
    parent = {c["id"]: c["id"] for c in mechanical}

    def find(key):
        while parent[key] != key:
            parent[key] = parent[parent[key]]
            key = parent[key]
        return key

    def union(a, b):
        parent[find(b)] = find(a)

    bindings, groups, contexts = {}, {}, {}
    for claim in mechanical:
        cid = claim["id"]
        for inp in claim["inputs"]:
            key = (inp["source"], selection_key(inp))
            if key in bindings:
                union(cid, bindings[key])
            bindings[key] = cid
        if claim.get("group"):
            if claim["group"] in groups:
                union(cid, groups[claim["group"]])
            groups[claim["group"]] = cid
        anchor = parsed["anchors"][cid]
        key = (anchor["context_start"], anchor["context_end"])
        if key in contexts:
            union(cid, contexts[key])
        contexts[key] = cid
    components = {}
    for claim in mechanical:
        components.setdefault(find(claim["id"]), []).append(claim)
    result = []
    for members in components.values():
        if all(checks[c["id"]]["status"] == "match" for c in members):
            continue
        ids = [c["id"] for c in members]
        blocked = any(checks[cid]["status"] == "unavailable" for cid in ids)
        edits = []
        if not blocked:
            for cid in ids:
                if checks[cid]["status"] == "mismatch":
                    a = parsed["anchors"][cid]
                    edits.append({"id": cid, "start": a["start"], "end": a["end"],
                                  "before_raw": a["raw"], "after_raw": escape(checks[cid]["expected"], quote=False),
                                  "before": a["text"], "after": checks[cid]["expected"]})
        candidate_checks = {}
        context_rows = []
        if edits:
            candidate = patch_html(html, edits)
            candidate_parsed = parse_html(candidate, [c["id"] for c in claims])
            candidate_checks = evaluate(claims, candidate_parsed, sources, now)
            if any(candidate_checks[cid]["status"] != "match" for cid in ids):
                raise DocumentError("The proposed group failed its independent candidate recheck")
            # Any other claim must retain its exact text, including unsupported reasoning.
            if any(candidate_parsed["anchors"][c["id"]]["text"] != parsed["anchors"][c["id"]]["text"]
                   for c in claims if c["id"] not in ids):
                raise DocumentError("A proposed edit reaches an unrelated claim")
            seen = set()
            for cid in ids:
                a = parsed["anchors"][cid]
                key = (a["context_start"], a["context_end"])
                if key in seen:
                    continue
                seen.add(key)
                part = html[key[0]:key[1]]
                local_edits = [{**e, "start": e["start"] - key[0], "end": e["end"] - key[0]}
                               for e in edits if key[0] <= e["start"] and e["end"] <= key[1]]
                context_rows.append({"before": a["context_text"], "after": text_content(patch_html(part, local_edits))})
        result.append({"id": "g-" + digest(encoded(sorted(ids)))[:16], "label": " / ".join(c["label"] for c in members),
                       "members": ids, "status": "blocked" if blocked else "ready", "decision": "pending",
                       "decided_at": None, "reason": "Required evidence is unavailable; the related edits stay together." if blocked else None,
                       "edits": edits, "contexts": context_rows,
                       "after_checks": {cid: candidate_checks[cid] for cid in ids} if edits else {}})
    return result


def make_artifact(html, claims, sources, title="Document", language="en", now=None, previous=None, history=None):
    now = now or timestamp()
    claims = normalize_claims(claims)
    parsed = parse_html(html, [c["id"] for c in claims])
    checks = evaluate(claims, parsed, sources, now, previous)
    groups = proposal_groups(html, claims, parsed, sources, checks, now)
    return {"version": 1, "title": _text(title, "title", limit=500), "language": _text(language, "language", limit=40),
            "generated_at": now, "authored_html": html, "authored_sha256": digest(html),
            "head_end": parsed["head_end"], "claims": claims, "sources": sources,
            "checks": checks, "groups": groups, "coverage": parsed["coverage"],
            "history": history or [], "previous_sources": previous}


def build(html, manifest, root, now=None):
    _keys(manifest, {"version", "title", "language", "claims", "sources"}, "manifest")
    if type(manifest.get("version")) is not int or manifest["version"] != 1:
        raise DocumentError("Expected document manifest version 1")
    claims = normalize_claims(manifest.get("claims"))
    now = now or timestamp()
    sources = capture_sources(manifest.get("sources", {}), claims, root, now)
    return make_artifact(html, claims, sources, manifest.get("title", "Document"), manifest.get("language", "en"), now)


def selected_html(data):
    return patch_html(data["authored_html"], [e for g in data["groups"] if g["decision"] == "accepted" for e in g["edits"]])


def _validate_snapshots(sources, claims):
    if not isinstance(sources, dict) or len(sources) > 100:
        raise DocumentError("Invalid embedded source snapshots")
    for claim in claims:
        for inp in claim["inputs"]:
            if inp["source"] not in sources:
                raise DocumentError("An embedded source is missing")
    for sid, source in sources.items():
        _id(sid)
        if not isinstance(source, dict) or source.get("status") not in {"available", "unavailable"}:
            raise DocumentError("Invalid source snapshot")
        _text(source.get("name"), "source name", limit=300)
        if source.get("uri"):
            _source_spec({"name": source["name"], "uri": source["uri"], "unavailable": "snapshot validation"})
        if source["status"] == "available" and (not re.fullmatch(r"[0-9a-f]{64}", str(source.get("sha256"))) or not source.get("read_at")):
            raise DocumentError("An available snapshot needs its revision and read time")
        selections = source.get("selections")
        if not isinstance(selections, dict):
            raise DocumentError("Invalid selected evidence")
        for key, selection in selections.items():
            if not isinstance(selection, dict) or selection.get("status") not in {"available", "unavailable"}:
                raise DocumentError("Invalid selected evidence")
            inp = _input({"source": sid, **selection.get("selector", {})})
            if selection_key(inp) != key:
                raise DocumentError("Evidence selector identity differs")
            if selection.get("status") == "available" and len(inp) > 1:
                atom = selection.get("value", {})
                if atom.get("type") == "number":
                    _number(atom.get("value"))
                elif atom.get("type") == "string":
                    _text(atom.get("value"), "selected value", empty=True)
                elif atom.get("type") != "boolean" or type(atom.get("value")) is not bool:
                    raise DocumentError("Invalid embedded scalar reading")


def validate_artifact(data):
    if not isinstance(data, dict) or type(data.get("version")) is not int or data["version"] != 1:
        raise DocumentError("Unsupported document artifact version")
    html = data.get("authored_html")
    if not isinstance(html, str) or digest(html) != data.get("authored_sha256"):
        raise DocumentError("The embedded authored document changed outside the review state")
    claims = normalize_claims(data.get("claims"))
    _validate_snapshots(data.get("sources"), claims)
    previous = data.get("previous_sources")
    if previous is not None:
        _validate_snapshots(previous, claims)
    regenerated = make_artifact(html, claims, data["sources"], data.get("title"), data.get("language"),
                                _text(data.get("generated_at"), "generation time", limit=50), previous, data.get("history", []))
    for key in ("head_end", "coverage", "checks"):
        if regenerated[key] != data.get(key):
            raise DocumentError("Embedded " + key + " no longer matches its document and evidence")
    stored = data.get("groups")
    if not isinstance(stored, list) or len(stored) != len(regenerated["groups"]):
        raise DocumentError("Embedded proposals no longer match their checks")
    for original, candidate in zip(stored, regenerated["groups"]):
        if not isinstance(original, dict) or original.get("decision") not in {"pending", "accepted", "rejected"}:
            raise DocumentError("Invalid saved review decision")
        if candidate["status"] != "ready" and original["decision"] != "pending":
            raise DocumentError("A blocked proposal cannot have been accepted or rejected")
        normalized = {**original, "decision": "pending", "decided_at": None}
        if normalized != candidate:
            raise DocumentError("An embedded proposal changed outside its review decision")
    return data


def refresh(data, source_inputs, root, now=None):
    validate_artifact(data)
    if any(g["status"] == "ready" and g["decision"] == "pending" for g in data["groups"]):
        raise DocumentError("Decide the pending proposals and export the copy before refreshing again")
    now = now or timestamp()
    html = selected_html(data)
    sources = capture_sources(source_inputs, data["claims"], root, now, data["sources"])
    history = list(data.get("history", []))
    history.append({"generated_at": data["generated_at"], "authored_sha256": data["authored_sha256"],
                    "decisions": [{k: g[k] for k in ("id", "decision", "decided_at")} for g in data["groups"]]})
    if len(history) > 100:
        raise DocumentError("This copy has reached 100 refreshes; author a new document from its current selected content")
    return make_artifact(html, data["claims"], sources, data["title"], data["language"], now, data["sources"], history)


class _Payload(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=False)
        self.values, self.active, self.parts = [], False, []

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        if tag == "script" and values.get("id") == "kp-data":
            if values.get("type") != "application/json" or self.active:
                raise DocumentError("Invalid embedded document payload")
            self.active, self.parts = True, []

    def handle_data(self, value):
        if self.active:
            self.parts.append(value)

    def handle_endtag(self, tag):
        if tag == "script" and self.active:
            self.values.append("".join(self.parts))
            self.active = False


def load_artifact(html):
    parser = _Payload()
    parser.feed(html)
    parser.close()
    if parser.active or len(parser.values) != 1:
        raise DocumentError("Expected one standalone kpopper document payload")
    return validate_artifact(read_json(parser.values[0]))


def render(data):
    validate_artifact(data)
    css = (HERE / "document" / "layer.css").read_text(encoding="utf-8")
    js = (HERE / "document" / "layer.js").read_text(encoding="utf-8")
    bridge = (HERE / "document" / "frame.js").read_text(encoding="utf-8")
    if "</script" in js.lower() or "</style" in css.lower():
        raise DocumentError("The packaged runtime contains an unsafe raw element terminator")
    runtime = safe_json({"bridge": bridge, "csp": FRAME_CSP})
    shell_csp = FRAME_CSP.replace("frame-src 'none'", "frame-src about:")
    return ("<!doctype html>\n<html lang=\"" + escape(data["language"], quote=True) + "\"><head><meta charset=\"utf-8\">"
            "<meta http-equiv=\"Content-Security-Policy\" content=\"" + escape(shell_csp, quote=True) + "\">"
            "<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>" + escape(data["title"]) + "</title>"
            "<style>" + css + "</style></head><body>"
            "<noscript><p class=\"kp-noscript\">JavaScript is required to display this document and its evidence and review controls. "
            "Open this file in a browser with JavaScript enabled. No live sources are checked by opening it.</p></noscript>"
            "<iframe id=\"kp-document\" title=\"" + escape(data["title"], quote=True) + "\" sandbox=\"allow-scripts\"></iframe>"
            "<button id=\"kp-open\" type=\"button\" aria-controls=\"kp-panel\" aria-expanded=\"false\">Evidence</button>"
            "<aside id=\"kp-panel\" hidden aria-label=\"Document evidence\"></aside>"
            "<div id=\"kp-notice\" role=\"status\" aria-live=\"polite\"></div>"
            "<script id=\"kp-data\" type=\"application/json\">" + safe_json(data) + "</script>"
            "<script id=\"kp-runtime\" type=\"application/json\">" + runtime + "</script>"
            "<script>" + js + "</script></body></html>\n")


def summary(data):
    checks = copy.deepcopy(data["checks"])
    for group in data["groups"]:
        if group["decision"] == "accepted":
            checks.update(group["after_checks"])
    return {"title": data["title"], "generated_at": data["generated_at"], "coverage": data["coverage"],
            "checks": checks, "sources": data["sources"],
            "proposals": [{k: g[k] for k in ("id", "status", "decision", "members", "reason")} for g in data["groups"]],
            "history": data["history"]}


def _exclusive_output(output, raw):
    """No-clobber fallback for destinations without hard-link support."""
    fd = os.open(output, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    identity = os.fstat(fd)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
    except BaseException:
        # Remove only the partial file created by this attempt, not another writer's
        # replacement. Unlike link/replace, the exclusive fallback is visible while
        # writing; it still cannot clobber an existing copy.
        try:
            current = output.lstat()
            if (current.st_dev, current.st_ino) == (identity.st_dev, identity.st_ino):
                output.unlink()
        except FileNotFoundError:
            pass
        raise


def write_output(data, output, overwrite=False, protected=()):
    output = Path(output).absolute()
    if output.is_symlink() or output.resolve() in {Path(p).resolve() for p in protected}:
        raise DocumentError("Output must be a separate copy, not an input or source file")
    raw = render(data).encode("utf-8")
    if len(raw) > MAX_ARTIFACT:
        raise DocumentError("Standalone output exceeds the 40 MB limit")
    fd, temporary = tempfile.mkstemp(prefix=".kpopper-document-", dir=output.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        if overwrite:
            os.replace(temporary, output)
        else:
            try:
                os.link(temporary, output)
            except OSError as error:
                if error.errno not in {errno.EPERM, errno.ENOTSUP, errno.EOPNOTSUPP, errno.ENOSYS, errno.EXDEV}:
                    raise
                _exclusive_output(output, raw)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    return output
