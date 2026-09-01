#!/usr/bin/env python3
"""Read a PROVENANCE record: check its invariants, and report what a change reaches.

  python3 provenance.py open    [file ...]        the whole opening: head + what moved
  python3 provenance.py check   [file ...]
  python3 provenance.py affects <entry> [entry ...]

It does not know your field names. Each role has a distinctive *shape*, and the
shape is enough:

  dependencies  a list of strings that are all ids of other entries
  snapshot      a mapping whose keys are a subset of that entry's dependencies
  predicate     a string containing entry ids *and* something else - an
                operator, a number, a bracket (so a bare reference is not one)

Where two fields genuinely fit the same role it refuses to guess and asks for a
one-line `schema:` block. A checker that quietly passes over what it cannot read
is worse than no checker, so every ambiguity is an error, never a skip.
"""
import io, os, re, sys, glob, yaml

ID = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+")
EXPR = re.compile(r"[<>=!+\-*/()]|\bor\b|\band\b|\bnot\b")
DEFAULT = ["PROVENANCE.yaml"]


def load(paths):
    doc, seen = {}, set()

    def merge(f, d):
        seen.add(os.path.abspath(f))
        for k, v in d.items():
            if isinstance(v, dict) and isinstance(doc.get(k), dict):
                doc[k].update(v)
            else:
                doc[k] = v
        # a pointer is followed so the record can be asked from the project root;
        # non-yaml values are somebody's notes, not records.
        for key in ("record", "also"):
            v = d.get(key)
            v = [v] if isinstance(v, str) else v if isinstance(v, list) else \
                list(v.values()) if isinstance(v, dict) else []
            for c in v:
                if not (isinstance(c, str) and c.endswith((".yaml", ".yml"))):
                    continue
                cf = os.path.join(os.path.dirname(f), c)
                if os.path.abspath(cf) in seen or not os.path.exists(cf):
                    continue
                merge(cf, yaml.safe_load(io.open(cf, encoding="utf-8").read()) or {})

    for p in paths:
        for f in sorted(glob.glob(p)) or [p]:
            merge(f, yaml.safe_load(io.open(f, encoding="utf-8").read()) or {})
    return doc


def groups_of(doc):
    """Any mapping-of-mappings is a candidate collection of entries."""
    out = {}
    for k, v in (doc or {}).items():
        if k in ("schema", "record", "also") or not isinstance(v, dict) or not v:
            continue
        if all(isinstance(x, (dict, str, int, float, bool, type(None))) for x in v.values()):
            out[k] = v
    return out


def infer(doc):
    groups = groups_of(doc)
    ids = {k for m in groups.values() for k in m}
    cand = {"deps": {}, "snapshot": {}, "predicate": {}}
    for members in groups.values():
        for body in members.values():
            if not isinstance(body, dict):
                continue
            for f, val in body.items():
                if isinstance(val, list) and val and all(isinstance(x, str) and x in ids for x in val):
                    cand["deps"][f] = cand["deps"].get(f, 0) + 1
                elif isinstance(val, dict) and val and all(k in ids for k in val):
                    cand["snapshot"][f] = cand["snapshot"].get(f, 0) + 1
                elif isinstance(val, str) and val:
                    named = [t for t in ID.findall(val) if t in ids]
                    if named and val.strip() not in named and EXPR.search(val):
                        cand["predicate"][f] = cand["predicate"].get(f, 0) + 1
    sch = doc.get("schema") or {}

    def pick(role):
        if sch.get(role):
            return sch[role]
        top = sorted(cand[role].items(), key=lambda kv: -kv[1])
        if not top:
            return None
        if len(top) > 1 and top[0][1] == top[1][1]:
            raise SystemExit(
                f"two fields fit '{role}' ({top[0][0]}, {top[1][0]}) and this tool does not "
                f"guess.\nAdd to the record:\n\nschema:\n  {role}: <field name>")
        return top[0][0]

    fields = {r: pick(r) for r in cand}
    if not fields["deps"]:
        raise SystemExit("no dependency field found: nothing declares what it rests on, "
                         "so there is no graph to walk")
    jud = {}
    for members in groups.values():
        for nid, body in members.items():
            if isinstance(body, dict) and fields["deps"] in body:
                jud[nid] = {
                    "deps": list(body.get(fields["deps"]) or []),
                    "seen": set((body.get(fields["snapshot"]) or {}).keys())
                            if fields["snapshot"] else set(),
                    "pred": str(body.get(fields["predicate"]) or "") if fields["predicate"] else "",
                    "body": body,
                }
    return ids, jud, fields


# A judgment may decline any of these, but only out loud.
BLOCKED = ("blocked_on", "unverified", "status")
OPEN = ("open", "questions")


def _blocked_text(body):
    """The declared reason, whether the declaration is prose or a {missing, why} mapping."""
    for k in BLOCKED:
        v = body.get(k)
        if not v:
            continue
        if isinstance(v, dict):
            m = v.get("missing") or []
            m = [m] if isinstance(m, str) else list(m)
            why = " ".join(str(x) for f, x in v.items() if f != "missing" and isinstance(x, str))
            return (why or "waiting on " + ", ".join(m)).strip()
        return str(v)
    return ""


def check(paths):
    doc = load(paths)
    ids, jud, fields = infer(doc)
    open_ids = {k for g in OPEN for k in (doc.get(g) or {})}
    fail, note = [], []
    if not fields["snapshot"]:
        note.append("no snapshot field anywhere: dependencies are declared but never "
                    "captured, so drift can never be detected")
    for name, j in sorted(jud.items()):
        if name in open_ids:
            continue
        blocked = _blocked_text(j["body"])
        for d in j["deps"]:
            if d not in ids:
                (note if blocked else fail).append(
                    f"{name}: rests on {d}, which is not an entry"
                    + (f" - declared: {blocked[:90]}" if blocked else ""))
            elif fields["snapshot"] and d not in j["seen"]:
                fail.append(f"{name}: no snapshot for {d} - never checked against it")
        for tok in sorted(set(ID.findall(j["pred"]))):
            if tok in ids and tok not in j["deps"]:
                fail.append(f"{name}: predicate reads {tok}, which it does not declare as a "
                            f"dependency - a change to it would never reach this")
        # A predicate field holding prose is not a predicate. It reads like one,
        # which is worse than an empty field: nothing evaluates it and nobody notices.
        evaluable = bool([t for t in ID.findall(j["pred"]) if t in ids])
        if not evaluable:
            what = "prose, not an evaluable predicate" if j["pred"] else "no predicate at all"
            (note if blocked else fail).append(
                f"{name}: {what}" + (f" (declared: {blocked[:90]})" if blocked
                else " - and nothing says why not, so it can never be re-checked"))
    for n in note:
        print("NOTE", n)
    for f in fail:
        print("FAIL", f)
    print(f"\n{len(jud)} judgments, {len(ids)} entries, {len(fail)} problems"
          + (f", {len(note)} declared" if note else ""))
    return 1 if fail else 0


def opening(paths, budget=25):
    """
    What a session should read instead of the whole record.

    Reading a record whole costs its full size every session, and most of it has not
    moved. This returns orientation plus only what needs a person: ranked, cut to a
    budget, and saying how many it dropped. Pull the rest on demand with `affects`,
    seeded by whatever the work is actually about.

    Ranked highest first: a broken reference is worse than a dependency nothing was
    ever checked against, which is worse than a hole someone already declared.
    """
    doc = load(paths)
    ids, jud, fields = infer(doc)
    open_ids = {k for g in OPEN for k in (doc.get(g) or {})}
    items = []
    for name, j in sorted(jud.items()):
        if name in open_ids:
            continue
        blocked = _blocked_text(j["body"])
        for d in j["deps"]:
            if d not in ids:
                items.append((60, name, f"waiting on {d} - {blocked[:70]}") if blocked
                             else (100, name, f"rests on {d}, which is not an entry"))
            elif fields["snapshot"] and d not in j["seen"]:
                items.append((80, name, f"never checked against {d}"))
        for tok in sorted(set(ID.findall(j["pred"]))):
            if tok in ids and tok not in j["deps"]:
                items.append((100, name, f"predicate reads {tok}, which it does not declare - "
                                         f"a change to it never reaches this"))
        if not [t for t in ID.findall(j["pred"]) if t in ids] and not blocked:
            items.append((40, name, "nothing evaluable would falsify it"))
    items.sort(key=lambda x: (-x[0], x[1]))
    kept, dropped = items[:budget], max(0, len(items) - budget)

    # The record's own head, printed here because this command already has the file
    # open. Orientation and what-moved are one read, not two.
    meta = doc.get("meta") or {}
    scope = str(meta.get("scope") or meta.get("about") or "").strip()
    if scope:
        print(scope if len(scope) < 300 else scope[:300] + " ...")

    def wv(v):
        vals = v if isinstance(v, list) else list(v.values()) if isinstance(v, dict) else []
        return ", ".join(x for x in vals if isinstance(x, str) and x.endswith((".yaml", ".yml")))

    where = []
    for k, v in doc.items():
        if k not in ("record", "also", "skill", "entry"):
            continue
        s = v if isinstance(v, str) else wv(v)
        if s:
            where.append(f"{k}: {s if len(s) < 90 else s[:90] + ' ...'}")
    if where:
        print("  " + " | ".join(where))
    # The namespace, not the values. Without this a session cannot turn a question
    # into a seed: it has no idea what this record even holds. One line, and
    # "what about the mortgage" becomes mtg.
    groups = {}
    for k in ids:
        if k in jud:                       # judgments are reported separately
            continue
        g = k.split(".")[0] if "." in k else k
        groups[g] = groups.get(g, 0) + 1
    named = [(g, n) for g, n in groups.items() if n > 1]
    loose = sum(n for g, n in groups.items() if n == 1)
    if named:
        print("holds: " + " · ".join(f"{g} ({n})" for g, n in
                                     sorted(named, key=lambda kv: (-kv[1], kv[0])))
              + (f" · and {loose} standalone" if loose else ""))
    print(f"{len(ids)} entries, {len(jud)} judgments"
          + (f", {len(open_ids)} open questions" if open_ids else "")
          + (f", updated {meta['updated']}" if meta.get("updated") else ""))
    if not fields["snapshot"]:
        print("no snapshot field: drift cannot be detected in this record")
    if not items:
        print("\nnothing needs a person right now.")
    else:
        print(f"\nneeds a person ({len(items)}):")
        for _, name, why in kept:
            print(f"  {name}: {why}")
        if dropped:
            print(f"  ... {dropped} more - raise the budget to see them")
    print("\nPull what the work is about:  provenance.py affects <entry>")
    return 0


def affects(paths, changed):
    doc = load(paths)
    ids, jud, _ = infer(doc)
    # A seed may be an exact entry or a prefix. A question names a subject, not a key.
    expanded = []
    for c in changed:
        if c in ids:
            expanded.append(c); continue
        hits = sorted(k for k in ids if k.split(".")[0] == c or k.startswith(c + "."))
        if not hits:
            raise SystemExit(f"{c} is not an entry or a prefix in this record. "
                             f"Run `open` to see what it holds.")
        print(f"# {c} -> {len(hits)} entries: {', '.join(hits[:8])}"
              + (" ..." if len(hits) > 8 else ""))
        expanded += hits
    changed = expanded
    hit, frontier = {}, list(changed)
    while frontier:
        m = frontier.pop()
        for name, j in sorted(jud.items()):
            if m in j["deps"] and name not in hit:
                hit[name] = m
                frontier.append(name)
    if not hit:
        print("nothing rests on that")
        return 0
    moved = set(changed) | set(hit)
    for name, via in hit.items():
        j = jud[name]
        named = [d for d in j["deps"] if d in moved and re.search(rf"\b{re.escape(d)}\b", j["pred"])]
        why = ("evaluate the predicate against " + ", ".join(named)) if named else "flagged only"
        print(f"{name}\n    via {via} -> {why}"
              + (f"\n    predicate: {j['pred']}" if j["pred"] else ""))
    print(f"\n{len(hit)} judgments reached")
    return 0


if __name__ == "__main__":
    a = sys.argv[1:] or ["check"]
    cmd, rest = a[0], a[1:]
    if cmd == "affects":
        files = [x for x in rest if x.endswith((".yaml", ".yml"))] or DEFAULT
        sys.exit(affects(files, [x for x in rest if not x.endswith((".yaml", ".yml"))]))
    files = [x for x in rest if x.endswith((".yaml", ".yml"))] or DEFAULT
    if cmd == "open":
        b = int(rest[rest.index("--budget") + 1]) if "--budget" in rest else 25
        sys.exit(opening(files, b))
    sys.exit(check(files))
