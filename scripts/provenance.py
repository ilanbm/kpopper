#!/usr/bin/env python3
"""Read a PROVENANCE record: check its invariants, and report what a change reaches.

  python3 provenance.py open    [file ...]        the whole opening: head + what moved
  python3 provenance.py check   [file ...]
  python3 provenance.py affects <entry> [entry ...]
  python3 provenance.py pull    <entry> [entry ...]   values and sources for a subject
  python3 provenance.py where                     the record this directory answers for

Without a file argument the record is PROVENANCE.yaml here, else the path this checkout
registered in `<git common dir>/kpopper-record` - for a project whose tree cannot hold it.

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
import io, os, re, sys, glob, subprocess, yaml
from decimal import Decimal, InvalidOperation

ID = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+")
EXPR = re.compile(r"[<>=!+\-*/()]|\bor\b|\band\b|\bnot\b")
DEFAULT = ["PROVENANCE.yaml"]
POINTER = "kpopper-record"   # in the git common dir: shared by every worktree, never tracked


def registered_record():
    """The path a project registered when its tree cannot hold the record: one line in
    `<git common dir>/kpopper-record`. Nothing when there is no git, no pointer, or the
    file it points at is gone - the hooks read the same line in shell."""
    try:
        g = subprocess.run(["git", "rev-parse", "--git-common-dir"], capture_output=True,
                           text=True, timeout=5)
    except (OSError, subprocess.SubprocessError):
        return None
    if g.returncode:
        return None
    p = os.path.join(g.stdout.strip(), POINTER)
    if not os.path.isfile(p):
        return None
    with io.open(p, encoding="utf-8") as f:
        rec = os.path.expanduser(f.readline().strip())
    return rec if rec and os.path.isfile(rec) else None


def default_paths():
    """The record at the project root - else the one the checkout registered."""
    if os.path.exists(DEFAULT[0]):
        return DEFAULT
    rec = registered_record()
    return [rec] if rec else DEFAULT


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
            if not os.path.exists(f):
                sys.exit(f"{f}: no record here. Run this from the directory the record sits "
                         "in, or name the record file as an argument.")
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


def _no_deps(unresolved):
    """What to say when no field reads as a dependency list.

    A record whose references are broken has the same symptom as one that declares
    nothing at all: no field is a list of names that are *all* entries, so none of them
    votes for the role. Telling a reader that nothing declares what it rests on, when
    every judgment carries a `rests_on:`, sends them looking for the wrong thing - so
    name the fields that did list names, and the names that were not found.

    Which of them is the dependency field is not something a shape can settle: a list of
    filenames and a list of mistyped references look exactly alike. So this names the
    candidates and leaves the choice to a person, the same way an ambiguous role does.
    """
    if not unresolved:
        return ("no dependency field found: nothing declares what it rests on, "
                "so there is no graph to walk")
    ranked = sorted(unresolved.items(), key=lambda kv: (-len(kv[1]), str(kv[0])))
    lines = []
    for f, where in ranked[:3]:
        nid, miss = where[0]
        names = ", ".join(miss[:3]) + (" ..." if len(miss) > 3 else "")
        lines.append(f"  {f}: {names if len(names) < 90 else names[:90] + ' ...'} (in {nid}"
                     + (f", and {len(where) - 1} more" if len(where) > 1 else "") + ")")
    if len(ranked) > 3:
        rest = ", ".join(str(f) for f, _ in ranked[3:])
        lines.append(f"  ... and {len(ranked) - 3} more: "
                     + (rest if len(rest) < 90 else rest[:90] + " ..."))
    return ("no dependency field found: no field lists names that are all entries in this "
            "record, so there is no graph to walk.\nThese list names that are not entries:\n"
            + "\n".join(lines)
            + "\n\nEither those names are wrong, or one of these is a dependency field this "
              "reader cannot see by shape - and it does not guess between them. Fix the "
              "names, or say which:\n\nschema:\n  deps: <field name>")


def infer(doc):
    groups = groups_of(doc)
    ids = {k for m in groups.values() for k in m}
    cand = {"deps": {}, "snapshot": {}, "predicate": {}}
    # A field with the shape of a dependency list whose names resolve to nothing votes
    # for no role at all. When that is why the record ends up with no dependency field,
    # it is the whole story - so keep what each one listed and what was missing from it.
    unresolved, present = {}, set()
    for members in groups.values():
        for nid, body in members.items():
            if not isinstance(body, dict):
                continue
            for f, val in body.items():
                present.add(f)
                if isinstance(val, list) and val and all(isinstance(x, str) for x in val):
                    if all(x in ids for x in val):
                        cand["deps"][f] = cand["deps"].get(f, 0) + 1
                    else:
                        unresolved.setdefault(f, []).append(
                            (nid, [x for x in val if x not in ids]))
                elif isinstance(val, dict) and val and all(k in ids for k in val):
                    cand["snapshot"][f] = cand["snapshot"].get(f, 0) + 1
                elif isinstance(val, str) and val:
                    named = [t for t in ID.findall(val) if t in ids]
                    if named and val.strip() not in named and EXPR.search(val):
                        cand["predicate"][f] = cand["predicate"].get(f, 0) + 1
    sch = doc.get("schema") or {}

    def pick(role):
        if sch.get(role):
            # Only the dependency field empties a record when it is named wrongly: every
            # judgment is found through that one name, so a name nothing carries leaves
            # nothing to check and the record passes by default - the quiet pass this
            # method refuses. A snapshot or predicate named the same way costs one check.
            if role == "deps" and sch[role] not in present:
                seen = ", ".join(sorted(str(x) for x in present))
                raise SystemExit(
                    f"schema names '{sch[role]}' for 'deps', and nothing this reader can "
                    f"see carries it: no judgment would be found, and the record would "
                    f"pass by having nothing left to check."
                    + (f"\nFields it can see: {seen if len(seen) < 300 else seen[:300] + ' ...'}"
                       if seen else ""))
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
        raise SystemExit(_no_deps(unresolved))
    jud = {}
    for members in groups.values():
        for nid, body in members.items():
            if isinstance(body, dict) and fields["deps"] in body:
                snap = body.get(fields["snapshot"]) if fields["snapshot"] else None
                snap = dict(snap) if isinstance(snap, dict) else {}
                jud[nid] = {
                    "deps": list(body.get(fields["deps"]) or []),
                    "seen": set(snap),
                    "snap": snap,      # the values too: what each dependency held at review
                    "pred": str(body.get(fields["predicate"]) or "") if fields["predicate"] else "",
                    "body": body,
                }
    return ids, jud, fields


# A judgment may decline any of these, but only out loud.
BLOCKED = ("blocked_on", "unverified", "status")
OPEN = ("open", "questions")

# The one field this method asks for by name rather than inferring by shape - a sentence
# has no distinctive shape. render_page.py owns the real version of this; this is the
# same idea kept minimal for a reader that only prints text.
NAMES = ("name", "title", "label", "what", "desc")


def named(body):
    if not isinstance(body, dict):
        return ""
    return next((str(body[f]).strip() for f in NAMES if body.get(f)), "")


CMP = re.compile(r"^\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*(<=|>=|==|!=|<|>)\s*(.+?)\s*$")


def bodies(doc):
    out = {}
    for v in doc.values():
        if isinstance(v, dict):
            for nid, b in v.items():
                out[nid] = b
    return out


def value_of(raw, ids, k):
    """The comparable value an entry holds right now - v, else the quoted text, else
    nothing: a rule has no value of its own."""
    b = raw.get(k)
    if not isinstance(b, dict):
        return b if k in ids else None
    v = b.get("v")
    if v is None:                      # absent or null: the quoted text is the value
        v = b.get("quoted")
    if isinstance(v, str) and EXPR.search(v) and any(t in ids for t in ID.findall(v)):
        return None
    return v


def evaluate(pred, raw, ids):
    """-> True when the falsifier holds, False when it does not, None when it is not a
    single comparison this reader can decide. Richer predicates are surfaced, never
    guessed at."""
    m = CMP.match(str(pred or ""))
    if not m:
        return None
    a = value_of(raw, ids, m.group(1))
    if a is None:
        return None
    rhs = m.group(3).strip()
    b = value_of(raw, ids, rhs) if ID.fullmatch(rhs) else rhs.strip("\"'")
    if b is None:
        return None

    def num(x):
        try:
            return float(str(x).replace(",", ""))
        except ValueError:
            return None
    na, nb = num(a), num(b)
    a, b = (na, nb) if na is not None and nb is not None else (str(a), str(b))
    op = m.group(2)
    return {"<": a < b, ">": a > b, "<=": a <= b, ">=": a >= b,
            "==": a == b, "!=": a != b}[op]


def short(v, n=40):
    s = " ".join(str(v).split())
    return s if len(s) <= n else s[:n - 1] + "…"


def moved_deps(j, raw, ids):
    """Dependencies whose value differs from the snapshot taken when the judgment was
    written -> [(dep, seen, now, state)]. `state` is what the predicate makes of the move:
    "muted" when it names the dependency and still evaluates false - it moved, not across
    the line the judgment drew; "crossed" when it evaluates true - the judgment is broken
    and that is reported on its own; "moved" otherwise - nothing decides this one, so a
    person must. A dependency with no comparable value now (a rule, a source, a judgment)
    is skipped: there is nothing to compare."""
    def num(x):
        # exact, so a change beyond float precision is still a change
        try:
            return Decimal(str(x).replace(",", ""))
        except InvalidOperation:
            return None
    out = []
    refs = {t for t in ID.findall(j["pred"]) if t in ids}
    verdict = evaluate(j["pred"], raw, ids) if refs else None
    for dep, old in j["snap"].items():
        if dep not in ids or isinstance(old, (list, dict)):
            continue
        now = value_of(raw, ids, dep)
        if now is None or isinstance(now, (list, dict)):
            continue
        if " ".join(str(old).split()) == " ".join(str(now).split()):
            continue
        if num(old) is not None and num(old) == num(now):
            continue
        state = "moved"
        if dep in refs and verdict is not None:
            state = "crossed" if verdict else "muted"
        out.append((dep, old, now, state))
    return out


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
    raw = bodies(doc)
    open_ids = {k for g in OPEN for k in (doc.get(g) or {})}
    fail, note, moved = [], [], []
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
        elif evaluate(j["pred"], raw, ids) is True:
            fail.append(f"{name}: wrong_if holds ({j['pred']}) - broken by its own condition")
        # A dependency that moved since the snapshot puts the judgment in front of a
        # person; it does not fail the build. Movement is a question and a crossed line
        # is the answer, and only the predicate can say which line matters.
        for dep, old, now, state in moved_deps(j, raw, ids):
            if state == "moved":
                moved.append(f"{name}: {dep} differs from its snapshot "
                             f"({short(old)} -> {short(now)}) - re-review, or refresh seen")
    for n in note:
        print("NOTE", n)
    for m in moved:
        print("MOVED", m)
    for f in fail:
        print("FAIL", f)
    print(f"\n{len(jud)} judgments, {len(ids)} entries, {len(fail)} problems"
          + (f", {len(moved)} moved" if moved else "")
          + (f", {len(note)} declared" if note else ""))
    return 1 if fail else 0


def opening(paths, budget=25, chars=None):
    """
    What a session should read instead of the whole record.

    Reading a record whole costs its full size every session, and most of it has not
    moved. This returns orientation plus only what needs a person: ranked, cut to a
    budget, and saying how many it dropped. Ground the subject with `pull`, or trace
    what a change reaches with `affects` - both seeded by whatever the work is about.

    Ranked highest first: a broken reference is worse than a dependency nothing was
    ever checked against, which is worse than a hole someone already declared.

    `chars` is a second, harder budget: a character ceiling on the whole printed
    output, for a caller that cannot afford to page through a large record even at
    the default item budget. Without it every section below the head prints in full
    except `standing`, which only exists to fill room a character budget makes room
    for. With it, sections are cut at line granularity in order, the head always
    survives whole, and one line says how much was left out.
    """
    doc = load(paths)
    ids, jud, fields = infer(doc)
    raw = bodies(doc)
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
        evaluable = bool([t for t in ID.findall(j["pred"]) if t in ids])
        if not evaluable and not blocked:
            items.append((40, name, "nothing evaluable would falsify it"))
        elif evaluable and evaluate(j["pred"], raw, ids) is True:
            items.append((95, name, f"wrong_if holds ({j['pred'][:60]}) - broken by its own "
                                    f"condition"))
        for dep, old, now, state in moved_deps(j, raw, ids):
            if state == "moved":
                items.append((70, name, f"{dep} differs from what it last saw: "
                                        f"{short(old, 28)} -> {short(now, 28)}"))
    items.sort(key=lambda x: (-x[0], x[1]))
    kept, dropped = items[:budget], max(0, len(items) - budget)

    # The record's own head. Orientation and what-moved are one read, not two, and
    # this section is never cut - a caller tight enough on chars to lose it would
    # have nothing left to orient by.
    head = []
    meta = doc.get("meta") or {}
    scope = str(meta.get("scope") or meta.get("about") or "").strip()
    if scope:
        head.append(scope if len(scope) < 300 else scope[:300] + " ...")

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
        head.append("  " + " | ".join(where))
    # The namespace, not the values. Without this a session cannot turn a question
    # into a seed: it has no idea what this record even holds. One line, and
    # "what about the mortgage" becomes mtg.
    groups = {}
    for k in ids:
        if k in jud:                       # judgments are reported separately
            continue
        g = k.split(".")[0] if "." in k else k
        groups[g] = groups.get(g, 0) + 1
    heavy = [(g, n) for g, n in groups.items() if n > 1]
    loose = sum(n for g, n in groups.items() if n == 1)
    if heavy:
        head.append("holds: " + " · ".join(f"{g} ({n})" for g, n in
                                            sorted(heavy, key=lambda kv: (-kv[1], kv[0])))
                    + (f" · and {loose} standalone" if loose else ""))
    head.append(f"{len(ids)} entries, {len(jud)} judgments"
               + (f", {len(open_ids)} open questions" if open_ids else "")
               + (f", updated {meta['updated']}" if meta.get("updated") else ""))
    if not fields["snapshot"]:
        head.append("no snapshot field: drift cannot be detected in this record")

    needs = ["nothing needs a person right now."] if not items else (
        [f"needs a person ({len(items)}):"] + [f"  {name}: {why}" for _, name, why in kept]
        + ([f"  ... {dropped} more - raise the budget to see them"] if dropped else []))

    # Open questions: what a person left open on purpose. One line each - they are
    # already as short as this method gets, so there is nothing to rank or cut here
    # short of the overall budget.
    qmap = {}
    for g in OPEN:
        qmap.update(doc.get(g) or {})
    quests = []
    for qid in sorted(qmap):
        text = qmap[qid] if isinstance(qmap[qid], str) else str(qmap[qid])
        line = f"  ? {qid}: {text}"
        quests.append(line if len(line) < 100 else line[:100] + " ...")

    footer = ("next: pull <entry|prefix> (values with sources) · affects <entry> "
             "(what a change reaches) · check")

    for l in head:
        print(l)

    if chars is None:
        print()
        for l in needs:
            print(l)
        if quests:
            print()
            for l in quests:
                print(l)
        print()
        print(footer)
        return 0

    # standing: the verdict of every judgment that is not above - what the record
    # currently concludes and nothing contests. A judgment listed as needing a person
    # is not standing, so it is not repeated here with a sign that says it is. Only
    # built when there is a character budget to spend on it; it is the last thing
    # cut and the first thing skipped.
    flagged = {name for _, name, _ in items}
    standing = []
    live = [n for n in sorted(jud) if n not in flagged]
    if live:
        standing.append("standing:" + (f"  ({len(flagged)} above are contested)" if flagged else ""))
        for name in live:
            b = jud[name]["body"]
            v = str(b.get("verdict") or b.get("title") or name)
            line = f"  = {name}: {v}"
            standing.append(line if len(line) < 80 else line[:80] + " ...")

    body = [""] + needs
    if quests:
        body += [""] + quests
    if standing:
        body += [""] + standing

    used = sum(len(l) + 1 for l in head)
    shown = len(body)
    for i, l in enumerate(body):
        if used + len(l) + 1 > chars:
            shown = i
            break
        used += len(l) + 1
    for l in body[:shown]:
        print(l)
    cut = len(body) - shown
    if cut:
        print(f"  ... {cut} more - raise --chars")
    print()
    print(footer)
    return 0


def affects(paths, changed):
    doc = load(paths)
    ids, jud, _ = infer(doc)
    raw = bodies(doc)
    # A worked-out value carries a change the same way a judgment does: anything its
    # rule reads flows on to anything that reads it.
    feeds = {}
    for k in ids:
        if k in jud:
            continue
        b = raw.get(k)
        if not isinstance(b, dict):
            continue
        for fld in ("rule", "v"):
            s = b.get(fld)
            if isinstance(s, str) and EXPR.search(s):
                for t in set(ID.findall(s)):
                    if t in ids and t != k:
                        feeds.setdefault(t, []).append(k)
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
    hit, seen_e, frontier = {}, set(changed), list(changed)
    while frontier:
        m = frontier.pop()
        for e in sorted(feeds.get(m, [])):
            if e not in seen_e:
                seen_e.add(e)
                frontier.append(e)
        for name, j in sorted(jud.items()):
            if m in j["deps"] and name not in hit:
                hit[name] = m
                frontier.append(name)
    if not hit:
        print("nothing rests on that")
        return 0
    moved = seen_e | set(hit)
    for name, via in hit.items():
        j = jud[name]
        named = [d for d in j["deps"] if d in moved and re.search(rf"\b{re.escape(d)}\b", j["pred"])]
        why = ("evaluate the predicate against " + ", ".join(named)) if named else "flagged only"
        print(f"{name}\n    via {via} -> {why}"
              + (f"\n    predicate: {j['pred']}" if j["pred"] else ""))
    print(f"\n{len(hit)} judgments reached")
    return 0


def pull(paths, seeds, budget=40):
    """
    The seeded projection, with values and sources - ground a session on a subject
    instead of reading the whole record for it. Where `affects` walks forward from a
    seed to what depends on it, this reads the seed itself: its entries, as recorded,
    and the judgments that rest on them.
    """
    doc = load(paths)
    ids, jud, fields = infer(doc)
    raw = {}
    for v in doc.values():
        if isinstance(v, dict):
            for nid, b in v.items():
                if isinstance(b, dict):
                    raw[nid] = b
                elif nid in ids:
                    raw[nid] = {"v": b}

    # Seed resolution matches `affects`: an exact id, or a prefix expanded over the
    # namespace. A judgment seed pulls in what it rests on - the point of `pull` is
    # to ground, and a judgment without its own entries is not grounded in anything.
    expanded = []
    for c in seeds:
        if c in ids:
            expanded.append(c)
            continue
        hits = sorted(k for k in ids if k.split(".")[0] == c or k.startswith(c + "."))
        if not hits:
            raise SystemExit(f"{c} is not an entry or a prefix in this record. "
                             f"Run `open` to see what it holds.")
        expanded += hits

    entries, judgments = set(), set()
    for k in expanded:
        if k in jud:
            judgments.add(k)
            entries |= {d for d in jud[k]["deps"] if d in ids and d not in jud}
        else:
            entries.add(k)
    judgments |= {name for name, j in jud.items() if set(j["deps"]) & entries}

    def cut(line, n):
        return line if len(line) < n else line[:n] + " ..."

    def resolved(body):
        """-> (v, rule). A value that is itself a formula is a rule wearing a `v:` field -
        same shape render_page.py already treats that way - so it is never compared as a
        literal here either."""
        v, rule = body.get("v"), body.get("rule")
        if v is not None and isinstance(v, str) and EXPR.search(v) and \
                any(t in ids for t in ID.findall(v)):
            v, rule = None, rule or v
        return v, rule

    lines = []
    for k in sorted(entries):
        b = raw.get(k) or {}
        v, rule = resolved(b)
        if v is not None:
            shown = str(v)
        elif rule:
            shown = "= " + str(rule)
        elif b.get("quoted"):
            shown = '"' + str(b["quoted"]) + '"'
        else:
            shown = ""
        nm = named(b)
        line = f"{k}: {shown}" + (f" ({nm})" if nm else "")
        if b.get("from"):
            line += f" <- {b['from']}" + (f", at {b['at']}" if b.get("at") else "")
        of = b.get("of") or b.get("read")
        if of:
            line += f" as of {of}"
        lines.append(cut(line, 110))

    for name in sorted(judgments):
        j = jud[name]
        body = j["body"]
        verdict = str(body.get("verdict") or body.get("title") or name)
        lines.append(cut(f"+ {name}: {verdict}", 110))

        # State, derived the same way `check` derives a problem: a dependency that
        # is not an entry is broken, unless the judgment declares it missing, in
        # which case it is blocked; short of that, a dependency present but never
        # snapshotted is unchecked; short of that, the judgment holds.
        blocked = _blocked_text(body)
        missing = [d for d in j["deps"] if d not in ids]
        if missing:
            state = (f"blocked: {blocked}" if blocked else
                     f"broken: rests on {', '.join(missing)}, which is not an entry")
        elif evaluate(j["pred"], raw, ids) is True:
            state = f"broken: wrong_if holds ({j['pred']})"
        elif fields["snapshot"] and any(d not in j["seen"] for d in j["deps"]):
            stale = [d for d in j["deps"] if d not in j["seen"]]
            state = f"unchecked: never checked against {', '.join(stale)}"
        else:
            state = "holds"
        lines.append(cut("    " + state, 110))

        if j["pred"]:
            lines.append(cut(f"    wrong_if: {j['pred']}", 110))

        # Every move since the snapshot, with what the predicate made of it - pull is
        # the grounding surface, so here even a muted move is worth a line.
        for dep, old, now, state in sorted(moved_deps(j, raw, ids)):
            tag = {"muted": " - within wrong_if", "crossed": " - across wrong_if"}.get(state, "")
            lines.append(cut(f"    moved since review: {dep} {old} -> {now}{tag}", 110))

    kept, remain = lines[:budget], max(0, len(lines) - budget)
    for l in kept:
        print(l)
    if remain:
        print(f"... {remain} more lines - raise the budget")
    print("\naffects <entry> shows what a change reaches")
    return 0


if __name__ == "__main__":
    a = sys.argv[1:] or ["check"]
    cmd, rest = a[0], a[1:]
    if cmd == "where":
        # the record this directory answers for: at the root, or registered with the
        # checkout. Silent and non-zero when there is none - a hook's guard, not an error.
        p = default_paths()[0]
        if os.path.exists(p):
            print(os.path.abspath(p))
            sys.exit(0)
        sys.exit(1)
    if cmd == "affects":
        files = [x for x in rest if x.endswith((".yaml", ".yml"))] or default_paths()
        sys.exit(affects(files, [x for x in rest if not x.endswith((".yaml", ".yml"))]))
    if cmd == "pull":
        b, seeds, files = 40, [], []
        i = 0
        while i < len(rest):
            if rest[i] == "--budget":
                b = int(rest[i + 1]); i += 2; continue
            (files if rest[i].endswith((".yaml", ".yml")) else seeds).append(rest[i])
            i += 1
        sys.exit(pull(files or default_paths(), seeds, b))
    files = [x for x in rest if x.endswith((".yaml", ".yml"))] or default_paths()
    if cmd == "open":
        b = int(rest[rest.index("--budget") + 1]) if "--budget" in rest else 25
        c = int(rest[rest.index("--chars") + 1]) if "--chars" in rest else None
        sys.exit(opening(files, b, c))
    sys.exit(check(files))
