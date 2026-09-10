"""Local evidence discovery. The index is disposable; records and sources stay authoritative.

No model, network fetch, persistent index or inferred graph links. Search returns bounded
excerpts; exact reads are tied to the same corpus revision and can be paged explicitly.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import sys

try:
    from . import ingestion as I
except ImportError:
    import ingestion as I

P = I.P
SOURCE_BYTES = 1024 * 1024
TEXT_SUFFIXES = {".txt", ".md", ".markdown", ".rst", ".csv", ".tsv", ".json", ".yaml", ".yml"}
SCOPE_NOTE = "Matches locate evidence; recorded claims, hypotheses and captured reports retain their own status. Source truth is not verified."


def encode(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True)


def digest(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _rule_ids(nid, body, ids):
    if not isinstance(body, dict):
        return []
    return sorted({key for field in ("rule", "v")
                   if isinstance(body.get(field), str) and P.EXPR.search(body[field])
                   for key in P.ID.findall(body[field]) if key in ids and key != nid})


def _source_ids(nid, raw, judgments, ids):
    """Declared citations and dependencies only; lexical matches never add graph edges."""
    found, seen, pending = set(), set(), [nid]
    while pending:
        key = pending.pop()
        if key in seen:
            continue
        seen.add(key)
        body = raw.get(key)
        if not isinstance(body, dict):
            continue
        if key not in judgments and not any(k in body for k in ("v", "quoted", "rule")) \
                and any(body.get(k) for k in ("file", "url", "asked", "read")):
            found.add(key)
        citation = body.get("from")
        if isinstance(citation, str) and citation in ids:
            pending.append(citation)
        pending.extend(judgments.get(key, {}).get("deps", []))
        if key not in judgments:
            pending.extend(_rule_ids(key, body, ids))
    return sorted(found)


def _record_files(record):
    files = {Path(p).resolve() for p in P._files_of([str(record)])}
    files.update(Path(h["path"]).resolve() for h in P.load_hypotheses([str(record)]).values())
    brief = P._brief_beside(str(record))
    if brief:
        files.add(Path(brief).resolve())
    return sorted(files)


def corpus(record=None, state_dir=None):
    rec = I._record_path(record)
    if not rec.is_file():
        raise ValueError("no record found; open or map this workspace first")
    files = _record_files(rec)
    stamps = {str(path): I._sha(path.read_bytes()) for path in files}
    doc = P.load([str(rec)])
    origins = {}
    # Origin follows the reader's merge order, not sorted filenames. A later pointer
    # can replace a source ID, including the base directory of its relative locator.
    for filename in P._files_of([str(rec)]):
        path = Path(filename).resolve()
        body = P.yaml.safe_load(path.read_text(encoding="utf-8")) or {}
        for members in P.collections_of(body).values():
            for nid in members:
                origins[nid] = path.parent
    rows, diagnostics, captured_files, invalid_files = [], [], {}, set()
    _, state, exists = I._existing_layout(rec, state_dir)
    if exists:
        for path in sorted((state / "events").glob("*.json")):
            event = I._load(path)
            if not isinstance(event, dict):
                raise ValueError("invalid captured event: " + str(path))
            envelope, error = I._capture_payload(state, event)
            if error:
                invalid_files.add(event.get("source_file"))
                diagnostics.append({"ref": "capture:" + event["event_id"], "reason": error})
                continue
            captured_files[event["source_file"]] = event["state"]
            rows.append({"ref": "capture:" + event["event_id"], "id": event["event_id"],
                         "kind": "capture", "scope": "captured_report", "status": event["state"],
                         "name": envelope.get("question") or "Captured report",
                         "content": envelope["source_quote"], "file": event["source_file"],
                         "sources": [], "dependencies": [], "targets": I._report_seeds(envelope),
                         "question": envelope.get("question"), "reason": event.get("reason")})

    worlds = [("record", doc, None)]
    for name, hyp in sorted(doc.hypotheses.items()):
        if hyp["error"]:
            diagnostics.append({"ref": "hypothesis:" + name, "reason": hyp["error"]})
        else:
            worlds.append(("hypothesis:" + name, P.layered(doc, hyp), hyp["ids"]))
    for scope, world, own in worlds:
        ids, judgments, fields = P.infer(world)
        raw = P.with_builtins(world, ids, judgments, fields)
        for nid in sorted(own if own is not None else ids):
            if nid not in raw or P.is_builtin(nid):
                continue
            body = raw[nid]
            sources = _source_ids(nid, raw, judgments, ids)
            judgment = judgments.get(nid)
            tag, reason = P._state(nid, judgment, raw, ids, fields) if judgment else ("RECORDED", "")
            prefix = "node:" if scope == "record" else scope + ":node:"
            row = {"ref": prefix + nid, "id": nid, "kind": "judgment" if judgment else "entry",
                   "scope": scope, "status": tag if scope == "record" else "HYPOTHESIS",
                   "name": P.named(body), "sources": sources,
                   "dependencies": judgment["deps"] if judgment else [], "reason": reason,
                   "rule_dependencies": [] if judgment else _rule_ids(nid, body, ids),
                   "content": P.yaml.safe_dump(body, allow_unicode=True, sort_keys=False)}
            if judgment:
                row.update(assessment=tag, falsifier_holds=P.evaluate(judgment["pred"], raw, ids))
            rows.append(row)
            if not isinstance(body, dict) or nid not in sources:
                continue
            source_ref = ("source:" if scope == "record" else scope + ":source:") + nid
            filename = body.get("file")
            if not isinstance(filename, str):
                if body.get("url"):
                    diagnostics.append({"ref": source_ref, "reason": "remote source not fetched"})
                continue
            source = Path(filename).expanduser()
            if not source.is_absolute():
                directory = origins.get(nid, rec.parent) if scope == "record" else rec.parent
                source = directory / source
            source = source.resolve()
            if str(source) in invalid_files:
                diagnostics.append({"ref": source_ref, "reason": "captured source integrity check failed"})
                continue
            try:
                if source.suffix.lower() not in TEXT_SUFFIXES:
                    raise ValueError("only local UTF-8 text sources are indexed")
                if not source.is_file():
                    raise ValueError("local source file is unavailable")
                with source.open("rb") as handle:
                    data = handle.read(SOURCE_BYTES + 1)
                if len(data) > SOURCE_BYTES:
                    raise ValueError("source exceeds the 1 MiB search limit; read it directly")
                content = data.decode("utf-8")
                if "\0" in content:
                    raise ValueError("binary content is not indexed")
            except (OSError, UnicodeError, ValueError) as error:
                diagnostics.append({"ref": source_ref, "reason": str(error)})
                continue
            # An applied capture also has a canonical source entry: index its quote once.
            rows = [r for r in rows if not (r["kind"] == "capture" and r.get("file") == str(source))]
            rows.append({"ref": source_ref, "id": nid, "kind": "source", "scope": scope,
                         "status": "SOURCE" if scope == "record" else "HYPOTHESIS",
                         "name": P.named(body), "content": content, "file": str(source),
                         "sources": [nid], "dependencies": [],
                         "capture_state": captured_files.get(str(source))})
    if _record_files(rec) != files or any(I._sha(Path(path).read_bytes()) != stamp for path, stamp in stamps.items()):
        raise ValueError("record changed during search; retry")
    revision = digest(encode({"record": str(rec), "files": stamps, "rows": rows, "unindexed": diagnostics}))
    return {"record": str(rec), "revision": revision, "revision_kind": "search-corpus",
            "record_sha256": stamps[str(rec)], "rows": rows, "unindexed": diagnostics}


def _excerpt(content, terms, size=320):
    pattern = re.compile("|".join(re.escape(term) for term in terms), re.IGNORECASE) if terms else None
    match = pattern.search(content) if pattern else None
    anchor = match.start() if match else 0
    start = max(0, anchor - 80)
    line_start = content.rfind("\n", start, anchor) + 1
    if line_start:
        start = line_start
    end = min(len(content), start + size)
    return {"excerpt": content[start:end], "offset": start,
            "line": content.count("\n", 0, anchor) + 1,
            "excerpt_truncated": start > 0 or end < len(content),
            "total_characters": len(content)}


def search(query, record=None, state_dir=None, limit=5, chars=6000):
    if not isinstance(query, str) or not query.strip() or len(query) > 2000:
        raise ValueError("query must contain 1..2000 characters")
    if not 1 <= limit <= 50 or not 500 <= chars <= 100000:
        raise ValueError("limit must be 1..50; chars must be 500..100000")
    terms = list(dict.fromkeys(re.findall(r"[^\W_]+", query.casefold())))[:32]
    data = corpus(record, state_dir)
    rows = data.pop("rows")
    matches = []
    if terms:
        connection = sqlite3.connect(":memory:")
        try:
            connection.execute("CREATE VIRTUAL TABLE evidence USING fts5(identity, name, content, tokenize='unicode61')")
            connection.executemany("INSERT INTO evidence(rowid, identity, name, content) VALUES (?, ?, ?, ?)",
                                   [(i + 1, row["id"], row["name"], row["content"]) for i, row in enumerate(rows)])
            expression = " OR ".join('"' + term.replace('"', '""') + '"' for term in terms)
            hits = connection.execute("SELECT rowid FROM evidence WHERE evidence MATCH ? ORDER BY bm25(evidence, 8, 4, 1), rowid", (expression,))
            matches = [rows[index - 1] for (index,) in hits]
        finally:
            connection.close()
    result = {**data, "query": query, "scope": SCOPE_NOTE, "results": [], "omitted": len(matches),
              "unindexed_count": len(data["unindexed"]), "unindexed_omitted": 0}
    for row in matches[:limit]:
        hit = {key: value for key, value in row.items() if key != "content"}
        hit.update(_excerpt(row["content"], terms))
        hit["sha256"] = digest(row["content"])
        result["results"].append(hit)
        result["omitted"] -= 1
    # Preserve complete result objects and an explicit count of everything excluded.
    while len(encode(result)) > chars and result["unindexed"]:
        result["unindexed"].pop()
        result["unindexed_omitted"] += 1
    while len(encode(result)) > chars and result["results"]:
        result["results"].pop()
        result["omitted"] += 1
    if len(encode(result)) > chars:
        raise ValueError("chars budget cannot carry search metadata; increase --chars")
    return result


def read(ref, revision, record=None, state_dir=None, offset=0, length=4000):
    data = corpus(record, state_dir)
    if revision != data["revision"]:
        raise ValueError("record, capture state or source changed; search again")
    row = next((r for r in data["rows"] if r["ref"] == ref), None)
    if row is None:
        raise ValueError("unknown search reference")
    content = row["content"]
    if not 0 <= offset <= len(content) or not 1 <= length <= 100000:
        raise ValueError("offset must address this text; length must be 1..100000")
    end = min(len(content), offset + length)
    return {"ref": ref, "revision": revision, "scope": row["scope"], "status": row["status"],
            "content": content[offset:end], "offset": offset, "next_offset": end if end < len(content) else None,
            "complete": offset == 0 and end == len(content), "total_characters": len(content),
            "sha256": digest(content)}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("query", nargs="?")
    parser.add_argument("--record")
    parser.add_argument("--state-dir")
    parser.add_argument("--limit", type=int, default=5)
    parser.add_argument("--chars", type=int, default=6000)
    parser.add_argument("--read")
    parser.add_argument("--revision")
    parser.add_argument("--offset", type=int, default=0)
    parser.add_argument("--length", type=int, default=4000)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.read:
            if args.query or not args.revision:
                raise ValueError("--read requires --revision and no query")
            result = read(args.read, args.revision, args.record, args.state_dir, args.offset, args.length)
        else:
            if args.revision or args.offset:
                raise ValueError("--revision and --offset require --read")
            result = search(args.query, args.record, args.state_dir, args.limit, args.chars)
    except (ValueError, OSError, sqlite3.Error, P.yaml.YAMLError) as error:
        print(encode({"error": str(error)}) if args.json else str(error), file=sys.stderr)
        return 2
    # The structured result is also the ordinary CLI output: no second prose rendering
    # that could drop scope labels, source anchors, or the omitted-result count.
    print(encode(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
