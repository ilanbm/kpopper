#!/usr/bin/env python3
"""Sameness is judged, not guessed: the reader never decides that two ids are one subject.
It names the nearest existing entries when one is written and the candidate pairs when
hypotheses consolidate - from declared fields alone - and a person answers each pair with
one of two commands, both of which write the answer down so the pair never comes back.

  python3 provenance.py same     <a> <b> [--keep a|b] [--as-of YYYY-MM-DD] [file]
  python3 provenance.py distinct <a> <b> "<why>" [--as-of YYYY-MM-DD] [file]

A candidate has a reason, and the reasons come in an order of certainty: the same `from` and
`at` under two ids - one reading of one place - is certain, and so is the same `url` or `file`
on two sources; the same `from` is less; the same `rule`, once every id in it is read through
what was retired into what; two judgments on the same premises - one's premises among the
other's, a session source being no premise - with their verdicts compared: equal is a
duplicate, different is a pair to judge. A prefix the base does not hold is a new subject,
not a pair. Overlap in `name` ranks a list; it is never a reason.

`same a b` is a migration: b is retired into a. Every reference to b - in rests_on, in seen
keys, in predicates and rules, in the {{b}} references inside text, across the base, every
hypothesis and the brief - now names a; the two bodies merge, the newer reading winning and
a's name kept; a keeps `also: [b]`, so a later writer of b is pointed at a. Where b's reading
would replace a's, the merge asks may_supersede the way every same-id write does; check runs
afterwards, and the whole migration is undone when it left a problem check did not have.
`distinct a b "why"` writes `distinct_from: b` on a, the why beside it, and the pair returns
as no candidate.
"""
import io, os, re, sys, datetime, yaml, json

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import provenance as P

IDISH = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*$")
# the reasons a pair is named, most certain first
LOCATION, SOURCE, RULE, PREMISES = 0, 1, 2, 3
# the fields that carry one reading of a value: they travel together when a merge takes the
# newer reading, so a value never keeps the place and date of the reading it replaced
READING = ("v", "quoted", "rule", "unit", "from", "at", "of", "read", "url", "file")
STOP = {"the", "and", "for", "its", "this", "that", "with", "from", "not", "are", "was",
        "were", "one", "what", "which", "when", "where", "how", "here", "into", "than",
        "then", "over", "under", "about", "after", "before", "does", "did", "has", "had"}


def _norm(v):
    return " ".join(str(v).split())


def _ids_in(text):
    """The ids a `distinct_from` names: one, or several separated by commas."""
    return [t for t in re.split(r"[,\s]+", str(text or "")) if t]


def _token(nid):
    """`nid` as a whole token: not a suffix of a longer id, not a prefix of one."""
    return re.compile(r"(?<![A-Za-z0-9_.])" + re.escape(nid) + r"(?![A-Za-z0-9_])(?!\.[A-Za-z0-9_])")


def tokens(text):
    return {t for t in re.findall(r"[^\W_]+", str(text or "").lower()) if len(t) > 2 and t not in STOP}


def overlap(a, b):
    """Token overlap of two names, 0..1 - a ranking key, never a reason."""
    ta, tb = tokens(a), tokens(b)
    return len(ta & tb) / len(ta | tb) if ta and tb else 0.0


def retired_into(raws, live, deps=None):
    """{retired id: the id it was retired into}, from every `also:` a body carries - in the
    base and in every hypothesis beside it. An id still held anywhere is not retired: absence
    is the whole condition, and where it fails the field is read as neither reading. A record
    that declares `also` as its dependency field means something else by it entirely, and this
    says nothing about such a record - pass `deps` to be told."""
    if deps == "also":
        return {}
    out = {}
    for raw in raws:
        for k, b in raw.items():
            if not isinstance(b, dict):
                continue
            al = b.get("also")
            al = [al] if isinstance(al, str) else al if isinstance(al, list) else []
            for x in al:
                if isinstance(x, str) and IDISH.match(x) and x not in live and x != k:
                    out.setdefault(x, k)
    return out


def also_held(raws, live, deps=None):
    """Where the retirement reading does not hold -> [(holder, named)], in order: an `also:`
    that names an id the record still holds somewhere. The two readings of the field have one
    shape - the ids folded into this one, and the siblings a record declares - so absence is
    the whole condition that tells them apart, and this is the set it fails on. A write cannot
    make one: `add` of a retired id is refused. A merge can, and textually - one branch retires
    an id, another still holds it, the two touch different lines. A record that declares `also`
    as its dependency field means its premises by it, and holds none of these."""
    if deps == "also":
        return []
    out = []
    for raw in raws:
        for k, b in sorted(raw.items()):
            if not isinstance(b, dict):
                continue
            al = b.get("also")
            al = [al] if isinstance(al, str) else al if isinstance(al, list) else []
            for x in al:
                if isinstance(x, str) and IDISH.match(x) and x in live and x != k \
                        and (k, x) not in out:
                    out.append((k, x))
    return out


def also_lines(doc, deps=None):
    """What `check` says about it: one line per pair, naming the two commands that answer it.
    Never a failure - which of the two readings holds is a person's, the way every other
    question of sameness here is."""
    base, hyps, live = _every_raw(doc)
    raws = [base] + [h["raw"] for h in hyps.values() if not h["error"]]
    return [f"{k} carries also: {x}, and {x} is an entry - a retirement that came back, or a "
            f"sibling this record declares; the reader reads it as neither: same {k} {x} folds "
            f"them, distinct {k} {x} \"why\" tells them apart"
            for k, x in also_held(raws, live, deps)]


def distinct_pairs(raws):
    """Every pair a `distinct_from` declared, either way round."""
    out = set()
    for raw in raws:
        for k, b in raw.items():
            if isinstance(b, dict) and b.get("distinct_from"):
                for x in _ids_in(b["distinct_from"]):
                    if x != k:
                        out.add(frozenset((k, x)))
    return out


def _is_session(b):
    return isinstance(b, dict) and bool(b.get("asked"))


def _rule_of(b):
    """The rule an entry holds: its `rule`, or a formula written under `v`."""
    if not isinstance(b, dict):
        return None
    r = b.get("rule")
    if isinstance(r, dict):
        return r
    if isinstance(r, str) and r.strip():
        return r
    v = b.get("v")
    if isinstance(v, str) and P.EXPR.search(v) and P.ID.search(v):
        return v
    return None


def canon_rule(rule, retired):
    """Canonical expression references, with a legacy fallback for unsupported prose."""
    try:
        tree = P.E.lower(rule) if isinstance(rule, dict) else P.E.lower({'expr': rule})
        for old, new in retired.items():
            tree = P.E.rename(tree, old, new)
        return tree
    except ValueError:
        return _norm(P.ID.sub(lambda m: retired.get(m.group(0), m.group(0)), str(rule)))


def _premises(b, deps_field, raw_all):
    """What a judgment rests on, less the session sources: every judgment rests on the
    session that wrote it, so a session shared is no premise shared."""
    deps = b.get(deps_field) if isinstance(b, dict) and deps_field else None
    if not isinstance(deps, list):
        return set()
    return {d for d in deps if isinstance(d, str) and not _is_session(raw_all.get(d))}


def _field(b, f, retired=None):
    v = b.get(f) if isinstance(b, dict) else None
    if not isinstance(v, str) or not v.strip():
        return None
    v = _norm(v)
    return retired.get(v, v) if retired else v


def near(subject, body, pool, deps_field, raw_all, retired, distinct, limit=None):
    """The entries in `pool` ({id: body}) nearest `subject`, by declared fields alone ->
    [{"id", "rank", "reasons", "score"}], most certain first, ties broken by name overlap."""
    out = []
    s_from, s_at = _field(body, "from", retired), _field(body, "at")
    s_loc = [(f, _field(body, f)) for f in ("url", "file") if _field(body, f)]
    s_rule = _rule_of(body)
    s_rule = canon_rule(s_rule, retired) if s_rule else None
    s_prem = _premises(body, deps_field, raw_all)
    s_jud = isinstance(body, dict) and isinstance(body.get(deps_field), list) if deps_field else False
    s_name = P.named(body)
    for k, b in pool.items():
        if k == subject or not isinstance(b, dict) or P.is_builtin(k) \
                or frozenset((subject, k)) in distinct:
            continue
        reasons, rank = [], None
        o_from, o_at = _field(b, "from", retired), _field(b, "at")
        if s_from and o_from == s_from:
            if s_at and o_at == s_at:
                reasons.append(f"same from and at ({s_from}, {P.scalar(s_at, fold=False)}) - certain")
                rank = LOCATION
            else:
                reasons.append(f"same from ({s_from})")
                rank = SOURCE
        for f, v in s_loc:
            if _field(b, f) == v:
                reasons.append(f"same {f} ({P.short(v, 60)}) - certain")
                rank = LOCATION
        o_rule = _rule_of(b)
        if s_rule and o_rule and canon_rule(o_rule, retired) == s_rule:
            reasons.append(f"same rule ({P.short(o_rule, 60)})")
            rank = RULE if rank is None else min(rank, RULE)
        o_prem = _premises(b, deps_field, raw_all)
        if s_jud and s_prem and o_prem and (s_prem <= o_prem or o_prem <= s_prem):
            shared = ", ".join(sorted(s_prem & o_prem))
            same_verdict = P._same(P._verdict_of(body) or "", P._verdict_of(b) or "")
            reasons.append(f"rests on {shared} too"
                           + (", with the same verdict - a duplicate" if same_verdict
                              else " - verdicts differ, a pair to judge"))
            rank = PREMISES if rank is None else min(rank, PREMISES)
        if reasons:
            out.append({"id": k, "rank": rank, "reasons": reasons, "score": overlap(s_name, P.named(b))})
    out.sort(key=lambda c: (c["rank"], -c["score"], c["id"]))
    return out if limit is None else out[:limit]


def _every_raw(doc):
    """The base's bodies and each readable hypothesis's -> (base, {name: raw}, all ids)."""
    base = P.bodies(doc)
    hyps = {n: h for n, h in (getattr(doc, "hypotheses", None) or {}).items() if not h["error"]}
    live = {k for k in base if not P.is_builtin(k)}
    for h in hyps.values():
        live |= h["ids"]
    return base, hyps, live


# ── the nearest existing, at the moment of writing ───────────────────────────
def nearest_existing(a, doc, ids, jud, fields, raw):
    """One of the write path's validators: a write under an id that was retired is refused
    and pointed at the id it went into; an `add` hears which existing entries are nearest
    the new one - a note in the reply, never a refusal - and nothing else is said."""
    k = a["id"]
    base, hyps, live = _every_raw(doc)
    raws = [base] + [h["raw"] for h in hyps.values()]
    retired = retired_into(raws, live, fields["deps"])
    if k in retired and k not in ids:
        into = retired[k]
        return [f"{k} was retired into {into} - write {into} instead; it carries also: [{k}]"]
    if a["kind"] != "add" or not isinstance(a.get("body"), dict):
        return []
    body = a["body"]
    pool = {x: raw[x] for x in ids if x != k and isinstance(raw.get(x), dict)}
    distinct = distinct_pairs(raws) | {frozenset((k, x)) for x in _ids_in(body.get("distinct_from"))}
    found = near(k, body, pool, fields["deps"], raw, retired, distinct, limit=3)
    if found:
        print("nearest existing:")
        for c in found:
            print(f"  {c['id']}: {'; '.join(c['reasons'])}")
        print(f"  one subject: same <id> {k} folds it in · two: distinct {k} <id> \"why\" keeps them apart")
    return []


# ── the candidates a merger walks ────────────────────────────────────────────
def candidates(doc, hypotheses=None, limit=5):
    """The pairs a merger judges when hypotheses consolidate, from declared fields alone
    -> (pairs, new_subjects). `doc` is the loaded record; `hypotheses` the ones being
    consolidated - names of hypotheses beside the record, or the dicts the reader builds for
    them, as consolidate holds them (default: every readable one beside the record). What
    arrives - an id a hypothesis holds and the base does not - is held against the base and
    against what the other hypotheses bring; a pair two arrivals make is listed once. pairs:
    {arriving id: {"hypothesis": name, "near": [{"id", "hypothesis" (None for the base),
    "rank", "reasons", "score"}, ...]}}, at most `limit` per subject. new_subjects: {prefix:
    [(hypothesis, id), ...]} for every prefix the base does not hold. Pairs a distinct_from
    declared are left out."""
    ids, jud, fields = P.infer(doc)
    base, beside, live = _every_raw(doc)
    chosen = []
    for x in (sorted(beside) if hypotheses is None else hypotheses):
        h = x if isinstance(x, dict) else beside.get(x)
        if h is not None and not h.get("error") and h["name"] not in {c["name"] for c in chosen}:
            chosen.append(h)
    hyps = dict(beside, **{h["name"]: h for h in chosen})
    for h in chosen:
        live |= set(h["ids"])
    raws = [base] + [h["raw"] for h in hyps.values()]
    raw_all = {}
    for r in reversed(raws):
        raw_all.update(r)
    retired = retired_into(raws, live, fields["deps"])
    distinct = distinct_pairs(raws)
    arrivals = [(h["name"], k) for h in chosen for k in sorted(h["ids"]) if k not in ids]
    base_pool = {x: base[x] for x in ids if isinstance(base.get(x), dict)}
    pairs, new_subjects = {}, {}
    for n, k in arrivals:
        body = hyps[n]["raw"][k]
        pool = dict(base_pool)
        where = {x: None for x in pool}
        for m, x in arrivals:
            if x != k and (m, x) > (n, k):        # the pair two arrivals make is listed once
                pool[x] = hyps[m]["raw"][x]
                where[x] = m
        found = near(k, body, pool, fields["deps"], raw_all, retired, distinct, limit=limit)
        if found:
            for c in found:
                c["hypothesis"] = where[c["id"]]
            pairs[k] = {"hypothesis": n, "near": found}
        prefix = k.split(".")[0] if "." in k else k
        if not any((x.split(".")[0] if "." in x else x) == prefix for x in ids if not P.is_builtin(x)):
            new_subjects.setdefault(prefix, []).append((n, k))
    return pairs, new_subjects


def candidate_lines(doc, hypotheses=None, limit=5):
    """`candidates` as the lines the dry run prints under its two headings ->
    (candidates, new_subjects): one line per pair, grouped by the arriving id - "a (h) and b:
    why" - and one per prefix the base does not hold; empty lists when there is nothing."""
    pairs, subjects = candidates(doc, hypotheses, limit)
    lines = []
    for k, got in sorted(pairs.items(), key=lambda kv: (kv[1]["hypothesis"], kv[0])):
        for c in got["near"]:
            tag = f" ({c['hypothesis']})" if c["hypothesis"] else ""
            lines.append(f"{k} ({got['hypothesis']}) and {c['id']}{tag}: {'; '.join(c['reasons'])}")
    news = [f"{p} ({', '.join(sorted({n for n, _ in ks}))}): " + ", ".join(k for _, k in ks)
            for p, ks in sorted(subjects.items())]
    return lines, news


# ── same: the migration ──────────────────────────────────────────────────────
def _read_day(b, raw):
    return P._read_on(b, raw) if isinstance(b, dict) else None


def _claim(b, ids, raw):
    """What an entry claims, for telling two readings apart: its value, else its rule."""
    if not isinstance(b, dict):
        return b
    v = b.get("v")
    if v is None:
        v = b.get("quoted")
    if v is not None and not (isinstance(v, str) and P.EXPR.search(v) and P.ID.search(v)):
        return v
    return _rule_of(b)


def _merge(S, R, sb, rb, ids, jud, fields, raw, as_of):
    """The survivor's body after the retired one is folded in -> (body, notes). The newer
    reading wins - its value with the place and date it was read at; the kept id's name
    stays; whatever only the retired one carried is taken over. A judgment keeps its own
    verdict unless the other may supersede it. Two readings that disagree with nothing to
    order them are a contradiction, not one subject twice, and are refused."""
    notes = []
    if not sb:
        sb = {}                                 # nothing of S here: R's body, under S's name
    if P._judgment_shaped(sb, fields):
        vs, vr = P._verdict_of(sb), P._verdict_of(rb)
        if vs is not None and vr is not None and not P._same(vs, vr):
            ok, why = P.may_supersede(S, sb, rb, raw, ids, jud, fields, as_of)
            if not ok:
                raise P.Refused(f"refused - {S} concludes {P.short(vs, 60)!r} and {R} "
                                f"{P.short(vr, 60)!r} - {why}: two verdicts on one subject are a "
                                f"contradiction, not one subject twice; keep the one that holds, "
                                f"or write the other through add --hypothesis, for a person "
                                f"to fold")
            body = dict(rb)
            notes.append(f"{S} takes {R}'s verdict - {why}")
        else:
            body = dict(sb)
    else:
        cs, cr = _claim(sb, ids, raw), _claim(rb, ids, raw)
        rules = all(isinstance(b, dict) and b.get('rule') is not None and
                    b.get('v') is None and b.get('quoted') is None for b in (sb, rb))
        equal = P.E.same(cs, cr) if rules and isinstance(cs, dict) and isinstance(cr, dict) else P._same(cs, cr)
        ds, dr = _read_day(sb, raw), _read_day(rb, raw)
        winner = S
        if not sb:
            winner = R
        elif cr is not None and (cs is None or not equal):
            if dr is not None and (ds is None or dr >= ds):
                ok, why = P.may_supersede(S, sb, cr, raw, ids, jud, fields, dr.isoformat())
                if not ok:
                    raise P.Refused(f"refused - {S} holds {P.short(cs)} as of {ds} and {R} holds "
                                    f"{P.short(cr)} as of {dr} - {why}: two readings that disagree "
                                    f"are a contradiction, not one subject twice; set the one that "
                                    f"is right, or open a hypothesis, then same")
                winner = R
                notes.append(f"{S} takes {R}'s reading: {P.short(cs)} -> {P.short(cr)} - {why}")
            elif ds is None and dr is None:
                raise P.Refused(f"refused - {S} holds {P.short(cs)} and {R} holds {P.short(cr)}, and "
                                f"nothing dates either reading: two readings that disagree are a "
                                f"contradiction, not one subject twice; set the one that is right, "
                                f"or open a hypothesis, then same")
            else:
                notes.append(f"{S} keeps its reading {P.short(cs)} as of {ds}; {R}'s "
                             f"{P.short(cr)}" + (f" as of {dr}" if dr else ", undated") + " is older")
        elif cs is None and cr is None and dr is not None and (ds is None or dr > ds):
            winner = R                          # two sources: the newer read wins
            notes.append(f"{S} takes {R}'s reading of {dr}")
        if not sb:
            body = dict(rb)                     # the retired body whole, in its own order
        elif winner == R:
            # the newer reading in the place the old one had; its own provenance with it, the
            # survivor's provenance of the reading it replaces gone, the unit kept
            body = {}
            for f, v in sb.items():
                if f in READING and f != "unit":
                    if f in rb:
                        body[f] = rb[f]
                else:
                    body[f] = v
            for f in READING:
                if f in rb and f not in body:
                    body[f] = rb[f]
            for f, v in rb.items():
                if f not in body and f not in READING and f not in ("also", "distinct_from"):
                    body[f] = v
        else:
            body = dict(sb)
            for f, v in rb.items():
                if f not in body and f not in READING and f not in ("also", "distinct_from"):
                    body[f] = v
    if not body.get("name") and P.named(rb):
        body["name"] = P.named(rb)
    for f in ("also", "distinct_from"):
        body.pop(f, None)
    also = []
    for src in (sb, rb):
        al = src.get("also")
        for x in ([al] if isinstance(al, str) else al if isinstance(al, list) else []):
            if x not in also and x != S:
                also.append(x)
    if R not in also:
        also.append(R)
    body["also"] = also
    dist = []
    for src in (sb, rb):
        for x in _ids_in(src.get("distinct_from")):
            if x not in dist and x not in (S, R):
                dist.append(x)
    if dist:
        body["distinct_from"] = ", ".join(dist)
    return body, notes


def _drop_key(lines, s, e, field, key):
    """The pair `key` removed from the mapping field inside the block [s, e) - a flow mapping
    or a block one - -> the new end of the block."""
    span = P._field_span(lines, s, e, field)
    if not span:
        return e
    find, i, j = span
    if P._inline(lines[i]).startswith("{"):
        block = "\n".join(lines[i:j])
        block = re.sub(r"(?<![A-Za-z0-9_.])" + re.escape(key) + r"\s*:\s*" + P.FLOW_VALUE
                       + r"\s*,?\s*", "", block)
        block = re.sub(r",\s*}", "}", block)
        new = block.split("\n")
        lines[i:j] = new
        return e + len(new) - (j - i)
    sub = P._field_span(lines, i, j, key)
    if not sub:
        return e
    _, ki, kj = sub
    del lines[ki:kj]
    return e - (kj - ki)


def _set_fields(lines, nid, body, was):
    """The entry rewritten field by field where it stands: a field the merge changed or
    added is replaced or appended, one it dropped is removed, and every other line - comments
    included - stays as it was."""
    _, ind, s, e = P._locate(lines, nid)
    if P._inline(lines[s]).startswith("{"):
        comments = [l for l in lines[s + 1:e] if l.strip().startswith("#")]
        find = P._members_of(lines, s, e)[0] or ind + 2
        lines[s:e] = P._entry_lines(nid, body, ind, find, True) + comments
        return
    find = P._members_of(lines, s, e)[0] or ind + 2
    for f in list(was):
        if f not in body and f != "also":
            span = P._field_span(lines, s, e, f)
            if span:
                _, i, j = span
                del lines[i:j]
                e -= j - i
    for f, v in body.items():
        if f in was and was[f] == v:
            continue
        e = P._replace_field(lines, s, e, f, P._field_lines(f, v, find))


def _remove_block(lines, nid):
    """The entry's lines cut out, with the blank line that separated it -> its collection
    and its lines, comments included."""
    name, ind, s, e = P._locate(lines, nid)
    block = lines[s:e]
    j = e
    if j < len(lines) and not lines[j].strip() and s > 0 and not lines[s - 1].strip():
        j += 1
    del lines[s:j]
    return name, block


def _rewrite_text(text, R, S, predicate_field="wrong_if", snapshot_field="seen"):
    """Every whole-token mention of R now names S -> (text, how many)."""
    # Tagged expression literals are data, even when they spell an entry ID.
    spans = []
    seen = set()
    def visit(node, expression=False, snapshot=False):
        if id(node) in seen:
            return
        seen.add(id(node))
        if isinstance(node, yaml.MappingNode):
            if expression and len(node.value) == 1 and node.value[0][0].value == 'expr':
                value = node.value[0][1]
                original = {'expr': value.value}
                left, right = value.start_mark.index, value.end_mark.index
                if R in P.E.refs(original):
                    changed = P.E.rename(original, R, S, predicate=P.E.predicate_shape(original))
                    replacement = json.dumps(changed['expr'], ensure_ascii=False)
                    if value.style in ('|', '>') and text[left:right].endswith('\n'):
                        replacement += '\n'
                    spans.append((left, right, replacement, 1))
                else:
                    spans.append((left, right, text[left:right], 0))
            elif expression and len(node.value) == 1 and node.value[0][0].value in {"text", "num", "bool"}:
                value = node.value[0][1]
                left, right = value.start_mark.index, value.end_mark.index
                spans.append((left, right, text[left:right], 0))
            else:
                for key, value in node.value:
                    if snapshot and key.value == "computed" and isinstance(value, yaml.MappingNode):
                        for part, payload in value.value:
                            if part.value == "value":
                                left, right = payload.start_mark.index, payload.end_mark.index
                                spans.append((left, right, text[left:right], 0))
                            elif part.value == "rule":
                                visit(payload, True, False)
                    else:
                        visit(value, expression or key.value in {"rule", predicate_field}, snapshot or key.value == snapshot_field)
        elif isinstance(node, yaml.SequenceNode):
            for value in node.value:
                visit(value, expression, snapshot)
    try:
        visit(yaml.compose(text))
    except yaml.YAMLError:
        pass  # Also called on diagnostic prose, not just YAML.
    count, start, parts = 0, 0, []
    for left, right, replacement, edits in sorted(spans):
        changed, n = _token(R).subn(S, text[start:left])
        parts.extend((changed, replacement)); count += n + edits; start = right
    changed, n = _token(R).subn(S, text[start:])
    return "".join(parts) + changed, count + n


def _dedupe_flow_lists(text, S):
    """A flow list that came to hold S twice holds it once; every other item stays as it was."""
    protected, visited = [], set()
    def visit(node):
        if id(node) in visited: return
        visited.add(id(node))
        if isinstance(node, yaml.ScalarNode):
            protected.append((node.start_mark.index, node.end_mark.index))
        elif isinstance(node, yaml.MappingNode):
            for key, value in node.value: visit(key); visit(value)
        elif isinstance(node, yaml.SequenceNode):
            for value in node.value: visit(value)
    try:
        visit(yaml.compose(text))
    except yaml.YAMLError:
        return text
    def fix(m):
        if any(left <= m.start() < right for left, right in protected):
            return m.group(0)
        inner = m.group(1)
        if len(_token(S).findall(inner)) < 2:
            return m.group(0)
        items, out, had = [x.strip() for x in inner.replace("\n", " ").split(",")], [], False
        for x in items:
            if x == S:
                if had:
                    continue
                had = True
            if x:
                out.append(x)
        return "[" + ", ".join(out) + "]"
    return re.sub(r"\[([^\[\]]*)\]", fix, text)


def _dedupe_distinct(text, S):
    """A distinct_from that came to name S twice names it once."""
    def fix(m):
        ids, out = _ids_in(m.group(3)), []
        for x in ids:
            if x not in out:
                out.append(x)
        return m.group(1) + P.scalar(", ".join(out), fold=False)
    return re.sub(r"^(\s*distinct_from:\s*)([\"']?)([A-Za-z0-9_.,\s]+?)\2\s*$", fix, text, flags=re.M)


def _mentions(R, worlds, fields, brief):
    """Where R is named, before the migration -> {kind: [where, ...]}: what the reply says
    was rewritten. `worlds` is [(name or None, raw)]."""
    out = {}
    tok = _token(R)

    def hit(kind, where):
        out.setdefault(kind, []).append(where)
    for wname, raw in worlds:
        tag = f" (in hypothesis {wname})" if wname else ""
        for k, b in sorted(raw.items()):
            if k == R or not isinstance(b, dict):
                continue
            deps = b.get(fields["deps"]) if fields["deps"] else None
            if isinstance(deps, list) and R in deps:
                hit(fields["deps"], k + tag)
            snap = b.get(fields["snapshot"]) if fields["snapshot"] else None
            if isinstance(snap, dict) and R in snap:
                hit(fields["snapshot"], k + tag)
            pred = b.get(fields["predicate"]) if fields["predicate"] else None
            if (isinstance(pred, dict) and R in P.E.refs(pred)) or (isinstance(pred, str) and tok.search(pred)):
                hit(fields["predicate"], k + tag)
            rule = _rule_of(b)
            if (isinstance(rule, dict) and R in P.E.refs(rule)) or (isinstance(rule, str) and tok.search(rule)):
                hit("rule", k + tag)
            for f, v in b.items():
                if isinstance(v, str) and R in P.refs_in(v):
                    hit("text", f"{k} ({f}){tag}")
            if _field(b, "from") == R:
                hit("from", k + tag)
            if R in _ids_in(b.get("distinct_from")):
                hit("distinct_from", k + tag)
    if brief and os.path.exists(brief):
        b = yaml.safe_load(io.open(brief, encoding="utf-8").read()) or {}
        secs = [s for s in (b.get("sections") or []) if isinstance(s, dict)]
        for t in (b.get("tabs") or []):
            if isinstance(t, dict):
                secs += [s for s in (t.get("sections") or []) if isinstance(s, dict)]
                if R in [str(x) for x in (t.get("serves") or [])]:
                    hit("the brief", f"tab {str(t.get('title') or '?')!r} (serves)")
        for s in secs:
            what = []
            if R in P.refs_in(s.get("text")):
                what.append("text")
            if isinstance(s.get("seen"), dict) and R in s["seen"]:
                what.append("seen")
            p = s.get("pick")
            if R in [str(x) for x in ([p] if isinstance(p, str) else list(p or []))]:
                what.append("pick")
            if what:
                hit("the brief", f"section {str(s.get('title') or '?')!r} ({', '.join(what)})")
        if tok.search(yaml.safe_dump(b.get("groups") or {})) or R in (b.get("labels") or {}):
            hit("the brief", "groups and labels")
    return out


def same(paths, a, b, keep=None, as_of=None):
    project = P._peer('knowledge_views').project_for(paths)
    paths = P._peer('knowledge_views').write_paths(paths)
    if not P._RAW_READS.get():
        with P._locked(paths[0], project=project):
            return same(paths, a, b, keep, as_of)
    doc = P.load(paths, read_mode='frozen')
    selected = {}
    for layer in [doc] + [h['doc'] for h in doc.hypotheses.values() if not h['error']]:
        for collection, members in P.collections_of(layer).items():
            selected.setdefault(collection, {}).update(members)
    R = P._peer('recording')
    G = P._peer('pending_grounding')
    closure = G.closure(selected, [a, b]) if all(n in G.entries(selected) for n in (a, b)) else selected
    if R.private_marker(closure):
        receipt = R.draft(P._peer('knowledge_views').project_for(paths),
                          {'kind': 'same', 'ids': [a, b]}, closure, 'private identity change retained for review')
        raise P.Refused('private draft retained at ' + receipt['path'])
    return _same_unlocked(paths, a, b, keep, as_of)


def _same_unlocked(paths, a, b, keep=None, as_of=None):
    """`b` retired into `a` - or the other way round with --keep - across every file the record
    is: the base, the hypotheses beside it, the brief. Read, validated, edited, written, read
    back through check; undone whole when check reports a problem it did not have before."""
    doc = P.load(paths)
    ids, jud, fields = P.infer(doc)
    raw = P.with_builtins(doc, ids, jud, fields)
    base, hyps, live = _every_raw(doc)
    raws = [base] + [h["raw"] for h in hyps.values()]
    retired = retired_into(raws, live, fields["deps"])
    if a == b:
        raise P.Refused(f"refused - {a} and {b} are one id already")
    if keep is None or keep in ("a", a):
        S, R = a, b
    elif keep in ("b", b):
        S, R = b, a
    else:
        raise P.Refused(f"refused - --keep names the id that survives: {a} or {b}")
    for k in (S, R):
        if P.is_builtin(k):
            raise P.Refused(f"refused - {k} is counted by the reader, never written")
        if k not in live:
            raise P.Refused(f"refused - {k} is not an entry of the record or of any hypothesis beside it"
                            + (f" - it was retired into {retired[k]}" if k in retired else ""))
    if frozenset((S, R)) in distinct_pairs(raws):
        raise P.Refused(f"refused - {a} and {b} were declared distinct; remove the distinct_from that "
                        f"says so before saying otherwise")
    for n, h in sorted((getattr(doc, "hypotheses", None) or {}).items()):
        if h["error"]:
            raise P.Refused(f"refused - hypothesis {n} could not be read ({h['error']}), so the migration "
                            f"could not reach what it holds")
    # the worlds: the base, whole, then each hypothesis file - each holding S, R, both or neither
    files = P._files_of(paths)
    brief = P._brief_beside(paths[0])
    worlds = [(None, base)] + [(n, hyps[n]["raw"]) for n in sorted(hyps)]

    def is_judgment(k):
        bodies = [w[k] for _, w in worlds if k in w]
        return [isinstance(x, dict) and P._judgment_shaped(x, fields) for x in bodies]
    js, jr = is_judgment(S), is_judgment(R)
    if any(js) != all(js) or any(jr) != all(jr) or any(js) != any(jr):
        raise P.Refused(f"refused - {S} is {'a judgment' if any(js) else 'an entry'} and {R} "
                        f"{'a judgment' if any(jr) else 'an entry'}: one subject cannot be both")
    def body_of(k):
        return base[k] if k in base else next(h["raw"][k] for h in hyps.values() if k in h["raw"])
    for k in (S, R):
        if not isinstance(body_of(k), dict):
            raise P.Refused(f"refused - {k} is a line, not an entry with fields - an open question is "
                            f"closed by answering it, not folded into another")
    mentions = _mentions(R, worlds, fields, brief)
    before_fail = P.check_lines(paths)[0]

    # every file read once; each is edited in memory and written only when all of them are
    texts = {}
    for f in files + [h["path"] for h in hyps.values()] + ([brief] if brief else []):
        with io.open(f, encoding="utf-8") as fh:
            texts[f] = fh.read()
    originals = dict(texts)
    stamp = as_of or datetime.date.today().isoformat()
    notes, base_s = [], base.get(S) if S in base else None
    also_of = {}                                # per world: what the survivor's also: holds

    def world_edit(name, wraw, wfiles, ids_w, jud_w, raw_w):
        """One world's files: R's block out, S's body merged or born, both-and-R deduped."""
        has_s, has_r = S in wraw, R in wraw
        merged = None
        if has_r:
            sb = wraw[S] if has_s else (base_s if base_s is not None else {})
            merged, why = _merge(S, R, sb, wraw[R], ids_w, jud_w, fields, raw_w, as_of)
            notes.extend((w + (f" (in hypothesis {name})" if name else "")) for w in why)
            if isinstance(merged, dict):
                also_of[name] = merged.pop("also")
        elif has_s and isinstance(wraw[S], dict):
            al = wraw[S].get("also")
            al = [al] if isinstance(al, str) else list(al) if isinstance(al, list) else []
            also_of[name] = al + [x for x in [R] if x not in al]
        for f in wfiles:
            lines = texts[f].split("\n")
            # judgments naming both: R goes out of rests_on and seen, in the field's own style
            for k, body in sorted(wraw.items()):
                if k in (S, R) or not isinstance(body, dict) or not P._locate(lines, k):
                    continue
                deps = body.get(fields["deps"]) if fields["deps"] else None
                snap = body.get(fields["snapshot"]) if fields["snapshot"] else None
                if not ((isinstance(deps, list) and R in deps and S in deps)
                        or (isinstance(snap, dict) and R in snap and S in snap)):
                    continue
                _, ind, s, e = P._locate(lines, k)
                find = P._members_of(lines, s, e)[0] or ind + 2
                if isinstance(deps, list) and R in deps and S in deps:
                    kept = [d for d in deps if d != R]
                    e = P._replace_field(lines, s, e, fields["deps"], P._field_lines(fields["deps"], kept, find))
                if isinstance(snap, dict) and R in snap and S in snap:
                    e = _drop_key(lines, s, e, fields["snapshot"], R)
            r_block, collection = None, None
            if has_r and P._locate(lines, R):
                collection, r_block = _remove_block(lines, R)
                cols = {n_: (s_, e_) for n_, s_, e_ in P._collections_in(lines)}
                if collection in cols and not P._members_of(lines, *cols[collection])[1]:
                    s_, e_ = cols[collection]
                    del lines[s_:e_]
            if isinstance(merged, dict) and P._locate(lines, S):
                _set_fields(lines, S, merged, wraw[S])
            elif isinstance(merged, dict) and r_block is not None and not has_s:
                P._add_in(lines, S, merged, collection)
            if r_block is not None and P._locate(lines, S):
                # the retired entry's own comments - a reason a set left - stay with the survivor
                kept = [l for l in r_block if l.strip().startswith("#")]
                if kept:
                    _, ind, s, e = P._locate(lines, S)
                    find = P._members_of(lines, s, e)[0] or ind + 2
                    lines[e:e] = [" " * find + l.strip() for l in kept]
            texts[f] = "\n".join(lines)

    world_edit(None, base, files, ids, jud, raw)
    for n in sorted(hyps):
        got, why = P._layer_view(doc, hyps[n])
        if got is None:
            raise P.Refused(f"refused - hypothesis {n} cannot be read over the base: {why}")
        world_edit(n, hyps[n]["raw"], [hyps[n]["path"]], got[0], got[1], got[3])
    # the brief: a section's seen that named both keeps the survivor's; then, everywhere, every
    # whole-token mention of R names S - references in text, seen keys, predicates, rules,
    # picks, serves, groups, labels - and a list that came to hold S twice holds it once
    if brief:
        lines = texts[brief].split("\n")
        bdoc = yaml.safe_load(texts[brief]) or {}
        secs = [x for x in (bdoc.get("sections") or []) if isinstance(x, dict)]
        for tab in (bdoc.get("tabs") or []):
            if isinstance(tab, dict):
                secs += [x for x in (tab.get("sections") or []) if isinstance(x, dict)]
        for sec in secs:
            snap = sec.get("seen")
            if isinstance(snap, dict) and R in snap and S in snap and sec.get("text"):
                span = P._section_span(lines, str(sec.get("title")), having="text")
                if span:
                    _, i, j = span
                    _drop_key(lines, i, j, "seen", R)
        labels = bdoc.get("labels") or {}
        if isinstance(labels, dict) and R in labels and S in labels:
            cols = {n: (s, e) for n, s, e in P._collections_in(lines)}
            if "labels" in cols:
                cs, ce = cols["labels"]
                ind, members = P._members_of(lines, cs, ce)
                for mid, i in members:
                    if mid == R:
                        del lines[i:P._block_end(lines, i, ind, ce)]
                        break
        texts[brief] = "\n".join(lines)
    counts = {}
    for f in list(texts):
        texts[f], n = _rewrite_text(texts[f], R, S, fields["predicate"], fields["snapshot"])
        if n:
            counts[f] = n
        texts[f] = _dedupe_distinct(_dedupe_flow_lists(texts[f], S), S)
    # the survivor carries the retired id, in every copy of it, so a later writer of R is
    # pointed at S - added after the rename, or the rename would have renamed it too
    home = {f: None for f in files}
    home.update({hyps[n]["path"]: n for n in hyps})
    for f in texts:
        if f == brief or home[f] not in also_of:
            continue
        lines = texts[f].split("\n")
        loc = P._locate(lines, S)
        if not loc or not isinstance(body_of(S), dict):
            continue
        _, ind, s, e = loc
        want = also_of[home[f]]
        if P._inline(lines[s]).startswith("{"):
            block = "\n".join(lines[s:e])
            if not re.search(r"\balso:", block):
                k = block.rfind("}")
                block = block[:k].rstrip() + ", also: [" + ", ".join(want) + "]" + block[k:]
            else:
                block = re.sub(r"\balso:\s*(\[[^\]]*\]|" + P.FLOW_VALUE + ")",
                               "also: [" + ", ".join(want) + "]", block)
            lines[s:e] = block.split("\n")
        else:
            find = P._members_of(lines, s, e)[0] or ind + 2
            if P._field_span(lines, s, e, "also"):
                P._replace_field(lines, s, e, "also", P._field_lines("also", want, find))
            else:
                while e > s + 1 and lines[e - 1].strip().startswith("#"):
                    e -= 1
                lines[e:e] = P._field_lines("also", want, find)
        texts[f] = "\n".join(lines)
    for f in files:
        lines = texts[f].split("\n")
        if P._bump_updated(lines, stamp):
            texts[f] = "\n".join(lines)
            break

    written = []

    def undo():
        for f in written:
            P._write_text(f, originals[f])
    # every file written, or none: a write that fails halfway puts the earlier ones back
    try:
        for f, t in texts.items():
            if t != originals[f]:
                P._write_text(f, t)
                written.append(f)
    except OSError as e:
        undo()
        raise P.Refused(f"refused - {os.path.relpath(f, os.path.dirname(os.path.abspath(paths[0])))} "
                        f"could not be written ({e.strerror or e}), so nothing was changed")
    # read back: R gone, S held, and check no worse than it was - with the old lines read
    # through the rename, so a problem that only changed its name is not a new one
    try:
        doc2 = P.load(paths)
        ids2, jud2, fields2 = P.infer(doc2)
        raw2 = P.with_builtins(doc2, ids2, jud2, fields2)
        live2 = _every_raw(doc2)[2]
        if R in live2 or S not in live2:
            raise ValueError(f"{R} is still held" if R in live2 else f"{S} is not held after the write")
        fail, note, moved, cont, summary = P.check_lines(paths)
    except (Exception, SystemExit) as e:
        undo()
        raise P.Refused(f"refused - the migration broke the record and was undone: {e}")
    was = {_rewrite_text(l, R, S)[0] for l in before_fail}
    new = [l for l in fail if l not in was]
    if new:
        undo()
        raise P.Refused("refused - check would fail after the migration, so nothing was changed:\n  "
                        + "\n  ".join(new))
    print(f"same {a} {b}: {R} retired into {S}" + (f" (kept {S})" if keep else ""))
    for l in notes:
        print("  " + l)
    if mentions:
        order = [fields["deps"], fields["snapshot"], fields["predicate"], "rule", "text", "from",
                 "distinct_from", "the brief"]
        parts = []
        for kind in [k for k in order if k] + [k for k in mentions if k not in order]:
            if kind in mentions:
                parts.append(f"{kind}: " + ", ".join(mentions[kind]))
        print("  rewritten - " + " · ".join(parts))
    if counts:
        here = os.path.dirname(os.path.abspath(paths[0]))
        print("  " + " · ".join(f"{n} mention{'s' if n != 1 else ''} in {os.path.relpath(f, here)}"
                                for f, n in counts.items()))
    print(f"  check: {summary}")
    P._report(paths, "set", S, doc2, ids2, jud2, fields2, raw2)
    return 0


# ── distinct: the edge that retires a pair ───────────────────────────────────
def distinct(paths, a, b, why, as_of=None):
    project = P._peer('knowledge_views').project_for(paths)
    paths = P._peer('knowledge_views').write_paths(paths)
    if not P._RAW_READS.get():
        with P._locked(paths[0], project=project):
            return distinct(paths, a, b, why, as_of)
    doc = P.load(paths, read_mode='frozen')
    selected = {}
    for layer in [doc] + [h['doc'] for h in doc.hypotheses.values() if not h['error']]:
        for collection, members in P.collections_of(layer).items():
            selected.setdefault(collection, {}).update(members)
    R = P._peer('recording')
    G = P._peer('pending_grounding')
    closure = G.closure(selected, [a, b]) if all(n in G.entries(selected) for n in (a, b)) else selected
    if R.private_marker(closure):
        receipt = R.draft(P._peer('knowledge_views').project_for(paths),
                          {'kind': 'distinct', 'ids': [a, b]}, closure, 'private identity change retained for review')
        raise P.Refused('private draft retained at ' + receipt['path'])
    return _distinct_unlocked(paths, a, b, why, as_of)


def _distinct_unlocked(paths, a, b, why, as_of=None):
    """`distinct_from: b` written on `a`, the why as a comment beneath it, in the file that
    holds `a` - the base's, else the hypothesis's. Nothing else moves."""
    doc = P.load(paths)
    ids, jud, fields = P.infer(doc)
    base, hyps, live = _every_raw(doc)
    raws = [base] + [h["raw"] for h in hyps.values()]
    retired = retired_into(raws, live, fields["deps"])
    if a == b:
        raise P.Refused(f"refused - {a} and {b} are one id: distinct needs two")
    for k in (a, b):
        if P.is_builtin(k):
            raise P.Refused(f"refused - {k} is counted by the reader; it is nothing to be distinct from")
        if k not in live:
            raise P.Refused(f"refused - {k} is not an entry of the record or of any hypothesis beside it"
                            + (f" - it was retired into {retired[k]}" if k in retired else ""))
    why = _norm(why or "")
    if not why:
        raise P.Refused("refused - distinct takes the why: distinct <a> <b> \"<why>\"")
    body = base.get(a) if a in base else next(h["raw"][a] for h in hyps.values() if a in h["ids"])
    if not isinstance(body, dict):
        raise P.Refused(f"refused - {a} is a line, not an entry with fields; distinct is written on a field")
    if frozenset((a, b)) in distinct_pairs(raws):
        print(f"{a} and {b} are already declared distinct; nothing written")
        return 0
    target, in_hypothesis = None, None
    for f in P._files_of(paths):
        if P._locate(io.open(f, encoding="utf-8").read().split("\n"), a):
            target = f
            break
    if target is None:
        for n in sorted(hyps):
            if a in hyps[n]["ids"]:
                target, in_hypothesis = hyps[n]["path"], n
                break
    original = io.open(target, encoding="utf-8").read()
    lines = original.split("\n")
    stamp = as_of or datetime.date.today().isoformat()
    _, ind, s, e = P._locate(lines, a)
    now = _ids_in(body.get("distinct_from"))
    value = ", ".join(now + [b])
    written = P.scalar(value, fold=False)      # one id bare; several quoted, or a comma would split a flow mapping
    comment = f"# distinct {stamp}: {why}"
    if P._inline(lines[s]).startswith("{"):
        block = "\n".join(lines[s:e])
        if re.search(r"\bdistinct_from:", block):
            block = re.sub(r"\bdistinct_from:\s*" + P.FLOW_VALUE, "distinct_from: " + written, block)
        else:
            k = block.rfind("}")
            block = block[:k].rstrip() + ", distinct_from: " + written + block[k:]
        new = block.split("\n") + [" " * (ind + 2) + comment]
        lines[s:e] = new
    else:
        find = P._members_of(lines, s, e)[0] or ind + 2
        e = P._replace_field(lines, s, e, "distinct_from", [" " * find + "distinct_from: " + written])
        lines[e:e] = [" " * find + comment]
    if in_hypothesis is None:
        P._bump_updated(lines, stamp)
    P._write_text(target, "\n".join(lines))
    try:
        doc2 = P.load(paths)
        ids2, jud2, fields2 = P.infer(doc2)
        raw2 = P.with_builtins(doc2, ids2, jud2, fields2)
        base2, hyps2, _ = _every_raw(doc2)
        held = base2.get(a) if a in base2 else hyps2[in_hypothesis]["raw"].get(a)
        if not isinstance(held, dict) or b not in _ids_in(held.get("distinct_from")):
            raise ValueError(f"{a} does not carry distinct_from: {b} after the write")
    except (Exception, SystemExit) as e2:
        P._write_text(target, original)
        raise P.Refused(f"refused - the write broke the record and was undone: {e2}")
    print(f"distinct {a} from {b}: {why}" + (f" (in hypothesis {in_hypothesis})" if in_hypothesis else ""))
    print(f"  the pair returns as no candidate; {a} carries distinct_from: {value}")
    if in_hypothesis:
        got = P._layer_view(doc2, hyps2[in_hypothesis])[0]
        if got:
            ids2, jud2, fields2, raw2 = got
    P._report(paths, "set", a, doc2, ids2, jud2, fields2, raw2)
    return 0


HELP = {
    "same": """  same <a> <b> [--keep a|b] [--as-of YYYY-MM-DD] [file]

One subject under two ids: b is retired into a - the first named survives unless --keep says
the other. A migration, not a guess: every reference to the retired id - in rests_on, in seen
keys, in predicates and rules, in the {{id}} references inside text - across the base, every
hypothesis beside it and the brief, now names the survivor; the two bodies merge, the newer
reading (its of:, read:, else its source's read date) winning and the survivor's name kept;
the survivor carries `also: [<retired id>]`, so a later write under the old id is pointed at
it. Two readings that disagree with nothing to order them - the same day, or undated - are a
contradiction and are refused, through the one door every same-id write goes through; two
judgments keep the survivor's verdict unless the other may supersede it. check runs after the
write, and the migration is undone whole when it left a problem check did not have. The reply
is what was rewritten, and the reach of the survivor.""",
    "distinct": """  distinct <a> <b> "<why>" [--as-of YYYY-MM-DD] [file]

Two subjects that look alike: `distinct_from: <b>` is written on a, with the why as a comment
beneath it, in the file that holds a - the base's, else the hypothesis's. The pair never
returns as a candidate, in add's nearest-existing note or in the dry run's list. A second
distinct on the same id joins the first: `distinct_from: b, c`.""",
}


def command(cmd, rest):
    """The two commands as the command line runs them, under the record's lock."""
    if "--help" in rest or "-h" in rest:
        print(HELP[cmd].strip("\n"))
        return 0
    opts, args = {}, []
    i = 0
    while i < len(rest):
        x = rest[i]
        if x in ("--keep", "--as-of"):
            if i + 1 >= len(rest):
                raise P.Refused(f"{x} needs a value")
            opts[x[2:].replace("-", "_")] = rest[i + 1]
            i += 2
            continue
        args.append(x)
        i += 1
    # the two ids first; distinct's why is whatever follows them, a record path or not; only
    # what comes after those can name the record
    at = [i for i, x in enumerate(args) if not x.endswith((".yaml", ".yml"))][:2]
    if len(at) != 2:
        raise P.Refused(HELP[cmd].strip("\n"))
    n = 2 if cmd == "same" else 3
    why_at = -1
    if cmd == "distinct" and at[1] + 1 < len(args):
        # the why is whatever follows the ids - unless that is the record itself, given last
        nxt = args[at[1] + 1]
        if not (at[1] + 2 == len(args) and nxt.endswith((".yaml", ".yml")) and os.path.exists(nxt)):
            why_at = at[1] + 1
    given = [args[at[0]], args[at[1]]] + ([args[why_at]] if why_at >= 0 else [])
    files = [x for i, x in enumerate(args) if i not in at and i != why_at]
    if len(given) != n or not all(f.endswith((".yaml", ".yml")) for f in files):
        raise P.Refused(HELP[cmd].strip("\n"))
    as_of = opts.get("as_of")
    if as_of and not re.match(r"^\d{4}-\d{2}-\d{2}$", as_of):
        raise P.Refused("--as-of takes a date, YYYY-MM-DD")
    paths = files or P.default_paths()
    if cmd == "same":
        return same(paths, given[0], given[1], opts.get("keep"), as_of)
    if "\n" in given[2]:
        raise P.Refused("the why is one line: a second line would be a line of the record")
    return distinct(paths, given[0], given[1], given[2], as_of)


if __name__ == "__main__":
    a = sys.argv[1:]
    if not a or a[0] not in HELP:
        print(__doc__.strip("\n"))
        sys.exit(2)
    sys.exit(command(a[0], a[1:]))
