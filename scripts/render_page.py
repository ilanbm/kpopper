#!/usr/bin/env python3
"""Render a record as one self-contained HTML page - in two tabs, sharing one provenance layer.

  python3 render_page.py [file ...] > record.html
  python3 render_page.py --brief PROVENANCE.view.yaml [file ...] > record.html
  python3 render_page.py --verify [file ...]

**Now** is the tab the session writes: an arrangement of the record aimed at what this
session is for. It is opinionated on purpose - order, sections, emphasis - and it is
written in a brief the session can rewrite in a moment, because intent is the one input
that is not in the record and dies with the conversation.

**Record** is the tab nobody writes: everything, arranged by nothing but the record's own
shape. It is the fallback when the arrangement is wrong, and it is the only tab when no
brief exists.

Two guarantees keep the opinionated tab honest, and both are mechanical rather than
remembered:

  · **it may order, it may not drop.** Anything flagged that no authored section picked
    up lands in a trailing section the brief cannot switch off.
  · **sections repopulate.** A section holds a selector, not a frozen list, so a new
    blocked judgment appears in it with no edit.

And the arrangement itself can go stale: the brief records the record's *shape* when it
was written, and a shape that moved raises a banner. Shape, not values - a date changing
does not make a layout wrong; a fourth blocked judgment might.
"""
import io, os, re, sys, json, html, hashlib, pathlib, datetime, yaml
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import provenance as P

# The page's own code lives beside this file, in the languages its tools speak. Read
# whole at import and inlined at render, so a page is still one file.
_PAGE = pathlib.Path(__file__).resolve().parent / "page"
CSS = "\n" + (_PAGE / "page.css").read_text(encoding="utf-8")
JS = "\n" + (_PAGE / "page.js").read_text(encoding="utf-8")

# ── what the record says about itself ────────────────────────────────────────
# One reading of state, used by every section selector. The same four conditions
# `provenance.py open` ranks by; named here so a brief can select on them.
STATES = ("broken", "falsified", "unchecked", "moved", "blocked", "no_predicate")


def flags_of(ids, jud, fields, raw):
    """Per judgment: the set of conditions that put it in front of a person - derived the
    way `check` and `open` derive them, so the page never disagrees with the reader."""
    out = {}
    for name, j in jud.items():
        f, blocked = set(), next((str(j["body"][k]) for k in P.BLOCKED if j["body"].get(k)), "")
        for d in j["deps"]:
            if d not in ids:
                f.add("blocked" if blocked else "broken")
            elif fields["snapshot"] and d not in j["seen"]:
                f.add("unchecked")
        if not [t for t in P.ID.findall(j["pred"]) if t in ids] and not blocked:
            f.add("no_predicate")
        elif P.evaluate(j["pred"], raw, ids) is True:
            f.add("falsified")
        if any(s == "moved" for _, _, _, s in P.moved_deps(j, raw, ids)):
            f.add("moved")
        out[name] = f
    return out


RTL = re.compile(r"[\u0590-\u05ff\u0600-\u06ff]")


def direction(doc):
    """A record written in Hebrew should not be read left to right because the tool
    was written in English. This is a decision about the record's *shape*, so it is
    stable: values changing never flips the page."""
    txt = "".join(str(v) for g in (doc or {}).values() if isinstance(g, dict)
                  for b in g.values() for v in (b.values() if isinstance(b, dict) else [b])
                  if isinstance(v, str))
    letters = [c for c in txt if c.isalpha()]
    return "rtl" if letters and len(RTL.findall(txt)) / len(letters) > 0.3 else "ltr"


def fmt(v):
    """Thousands separators on a hero number. Nothing else is touched."""
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return str(v)
    return f"{v:,}" if isinstance(v, int) else f"{v:,.10g}".rstrip()


def shape_of(ids, jud, flags):
    return {"entries": len(ids) - len(jud), "judgments": len(jud),
            "flagged": sum(1 for f in flags.values() if f),
            "blocked": sum(1 for f in flags.values() if "blocked" in f)}


# ── the brief ────────────────────────────────────────────────────────────────
def find_brief(paths, explicit=None):
    if explicit:
        return explicit
    for p in list(paths) + ["PROVENANCE.yaml"]:
        c = re.sub(r"\.ya?ml$", "", p) + ".view.yaml"
        if os.path.exists(c):
            return c
    return None


def resolve(sel, ids, jud, flags):
    """A selector is a state, a group, a prefix, or an exact id. Evaluated now, not frozen -
    which is the whole reason a section keeps up with the record without being edited."""
    sel = str(sel).strip()
    if sel in STATES:
        return {k for k, f in flags.items() if sel in f}
    if sel == "flagged":
        return {k for k, f in flags.items() if f}
    if sel == "judgments":
        return set(jud)
    if sel == "all":
        return set(ids)
    if sel in ids:
        return {sel}
    pre = sel.rstrip(".")
    hits = {k for k in ids if k.split(".")[0] == pre or k.startswith(pre + ".")}
    return hits


# ── anchoring: the reference belongs in the sentence, not in a chip beside it ──
# A row of monospace ids under a card is the graph leaking onto the reading surface.
# Bind what the prose already says - the id if it is written out, the value if it is
# quoted - and the sentence becomes the interface. Candidates are limited to *this*
# judgment's own dependencies: anchoring against the whole record invents links, and a
# false link is worse than a missing one.
NUMISH = re.compile(r"^-?\d+(\.\d+)?$")


def as_date(v):
    if isinstance(v, datetime.datetime):
        return v.date()
    if isinstance(v, datetime.date):
        return v
    s = str(v).strip()
    for f in ("%Y-%m-%d", "%d/%m/%Y", "%d.%m.%Y"):
        try:
            return datetime.datetime.strptime(s, f).date()
        except ValueError:
            pass
    return None


def renderings(v):
    """The strings a value can plausibly appear as in prose. Rounded restatements are
    deliberately not among them - binding '~180,000' to 179,842 would be a lie."""
    d = as_date(v)
    if d:
        out = [d.strftime("%d/%m/%Y"), d.strftime("%d/%m"), d.isoformat(), d.strftime("%d.%m.%Y")]
    else:
        s = str(v).strip()
        if not s:
            return []
        bare = s.replace(",", "")
        if NUMISH.match(bare):
            out = [s, bare] + ([f"{int(bare):,}"] if bare.lstrip("-").isdigit() else [])
        else:
            out = [s] if len(s) >= 6 else []
    return [x for x in dict.fromkeys(out) if len(x) >= 3]


def anchor(text, deps, E):
    """-> (html, set of deps that found a place in the text)."""
    if not text:
        return "", set()
    owner, seen_str = {}, set()
    for d in deps:
        for cand in [d] + renderings((E.get(d) or {}).get("v")):
            if cand in seen_str and owner.get(cand) != d:
                owner[cand] = None                      # two deps could explain it: neither does
            else:
                owner.setdefault(cand, d)
            seen_str.add(cand)
    hits, taken = [], []
    for cand in sorted((c for c, o in owner.items() if o), key=len, reverse=True):
        for m in re.finditer(re.escape(cand), text):
            a, b = m.span()
            if any(a < y and x < b for x, y in taken):
                continue
            taken.append((a, b))
            hits.append((a, b, owner[cand]))
    hits.sort()
    out, pos = [], 0
    for a, b, d in hits:
        out.append(html.escape(text[pos:a]))
        out.append(f'<span class="fx in" data-id="{html.escape(d)}">{html.escape(text[a:b])}</span>')
        pos = b
    out.append(html.escape(text[pos:]))
    return "".join(out), {d for _, _, d in hits}


def human(k):
    return k.split(".")[-1].replace("_", " ")


# ── renderers: a closed set, each declaring the shape of data it can carry ────
# Layout is where intent shows. But a renderer that silently accepts data it cannot
# express produces a page that looks arranged and is not, so each one says what it
# needs and a mismatch is a failure, not a shrug.
def fits(kind, keys, jud, E):
    if kind in ("table", "lines", "cards"):
        return None
    if kind == "timeline":
        bad = [k for k in keys if k not in jud and as_date((E.get(k) or {}).get("v")) is None]
        return (f"timeline needs date values; {len(bad)} of {len(keys)} are not dates "
                f"({', '.join(bad[:4])})") if bad else None
    if kind == "headline":
        n = [k for k in keys if k not in jud]
        if not 1 <= len(n) <= 4:
            return f"headline carries one to four values, not {len(n)}"
        blank = [k for k in n if (E.get(k) or {}).get("v") is None]
        return (f"headline needs values; {', '.join(blank)} "
                f"{'is derived and this reader does not evaluate rules' if len(blank) == 1 else 'are derived'}"
                ) if blank else None
    if kind == "fronts":
        g = {k.split(".")[0] for k in keys if k not in jud}
        return f"fronts lays groups side by side; these are all one group ({g})" if len(g) < 2 else None
    if kind == "alerts":
        e = [k for k in keys if k not in jud]
        return f"alerts ranks judgments; {len(e)} of these are entries" if e else None
    return f"unknown renderer '{kind}'"


URGENCY = {"broken": (100, "stop"), "falsified": (95, "stop"), "unchecked": (80, "stop"),
           "moved": (70, "warn"), "blocked": (60, "warn"), "no_predicate": (40, "mut")}

# What each state means, said the way a person would say it. The machine name stays -
# in the hover, where the keys and the rules live. Nothing on the reading surface is
# named after how the thing is built.
SAYS = {"broken": "rests on something that is not in this record",
        "falsified": "its own condition for being wrong now holds",
        "moved": "something it rests on no longer matches what it last saw",
        "unchecked": "has never been checked against one of the things it rests on",
        "blocked": "waiting on something nobody has recorded yet",
        "no_predicate": "nothing here would show it to be wrong"}

# A human name for an entry belongs to the entry, not to a session: what a thing is
# does not change because someone opened the page for a different reason. This is the
# one field the method asks for by name rather than inferring by shape - a sentence has
# no distinctive shape - and a record without it still works, less well.
NAMES = ("name", "title", "label", "what", "desc")


def named(body):
    if not isinstance(body, dict):
        return ""
    return next((str(body[f]).strip() for f in NAMES if body.get(f)), "")


def blocked_of(body):
    """-> (prose, [keys]). A mapping keeps them apart; a bare string is all prose."""
    for k in P.BLOCKED:
        v = body.get(k)
        if not v:
            continue
        if isinstance(v, dict):
            m = v.get("missing") or []
            m = [m] if isinstance(m, str) else list(m)
            why = " ".join(str(x) for f, x in v.items() if f != "missing" and isinstance(x, str))
            return why, m
        return str(v), []
    return "", []


def _plain(o):
    """YAML gives real dates and numbers; JSON wants strings for the ones it cannot carry."""
    if isinstance(o, (datetime.date, datetime.datetime)):
        return o.isoformat()
    if isinstance(o, dict):
        return {k: _plain(v) for k, v in o.items()}
    if isinstance(o, list):
        return [_plain(v) for v in o]
    return o


def tree_svg(ids, jud, E, J, flags):
    """The record as one growing thing. What was read from the world is the root
    system, below the ground line; what was worked out and concluded branches up
    from it, judgments in the canopy. Same record, same tree - the layout reads
    only the graph, so nothing here moves unless the record does."""
    def jig(k, m, salt=""):
        return int(hashlib.md5((salt + k).encode()).hexdigest(), 16) % m

    parents = {}
    for k in ids:
        if k in jud:
            parents[k] = [d for d in jud[k]["deps"] if d in ids]
        else:
            e = E.get(k) or {}
            ps = [t for t in P.ID.findall(str(e.get("rule") or "")) if t in ids]
            frm = e.get("from")
            if isinstance(frm, str) and frm in ids and frm != k:
                ps.append(frm)
            parents[k] = ps
    depth = {}

    def dep(k, seen=()):
        if k in depth:
            return depth[k]
        if k in seen:
            return 0
        ps = parents.get(k) or []
        depth[k] = 0 if not ps else 1 + max(dep(p, seen + (k,)) for p in ps)
        return depth[k]
    for k in ids:
        dep(k)

    # facts sit below conclusions whatever the raw path lengths: entries keep their
    # depth, judgments stack above the tallest entry, judgment-on-judgment higher.
    de = max([depth[k] for k in ids if k not in jud], default=0)
    jd = {}

    def jdep(k, seen=()):
        if k in jd:
            return jd[k]
        if k in seen:
            return 0
        ps = [p for p in parents.get(k) or [] if p in jud]
        jd[k] = 0 if not ps else 1 + max(jdep(p, seen + (k,)) for p in ps)
        return jd[k]
    for k in jud:
        jdep(k)
    for k in ids:
        if k in jud:
            depth[k] = de + 1 + jd[k]

    roots = sorted((k for k in ids if depth[k] == 0), key=lambda k: (k.split(".")[0], k))
    upper = sorted((k for k in ids if depth[k] > 0), key=lambda k: (depth[k], k))
    # past a certain size the grove is a thicket; keep the canopy and what feeds it.
    dropped = 0
    if len(roots) + len(upper) > 110:
        feed = set()
        for k in upper:
            feed |= set(parents[k])
        kept = [k for k in roots if k in feed]
        dropped = len(roots) - len(kept)
        roots = kept

    W, M = 940, 48
    maxd = max([depth[k] for k in upper], default=1)
    LH = 150 if maxd <= 2 else (112 if maxd == 3 else 92)
    G = 72 + maxd * LH + 26
    H = G + 118
    tx = W / 2 + jig("".join(sorted(ids))[:64], 30) - 15      # the trunk leans, per record
    x, y = {}, {}
    for i, k in enumerate(roots):
        x[k] = M + (i + 0.5) * (W - 2 * M) / max(1, len(roots)) + jig(k, 9) - 4
        y[k] = G + 30 + jig(k, 40, "d")
    for d in range(1, maxd + 1):
        layer = [k for k in upper if depth[k] == d]
        gap = 92 if any(k in jud for k in layer) else 48
        for k in layer:
            ps = [p for p in parents[k] if p in x]
            x[k] = (sum(x[p] for p in ps) / len(ps) if ps
                    else M + jig(k, W - 2 * M)) + jig(k, 21, "x") - 10
        layer.sort(key=lambda k: x[k])
        for i in range(1, len(layer)):          # min gap, one deterministic sweep
            x[layer[i]] = max(x[layer[i]], x[layer[i - 1]] + gap)
        off = max(0, (x[layer[-1]] - (W - M)) / 2) if layer else 0
        for k in layer:
            x[k] = min(W - M, max(M, x[k] - off))
            # the canopy is a dome: the further a node sits from the trunk, the
            # lower it hangs.
            y[k] = G - d * LH - jig(k, 16, "y") + ((x[k] - tx) ** 2) * 30 / (W / 2) ** 2

    def lbl(k, n):
        b = jud.get(k)
        t = (b["body"].get("verdict") or b["body"].get("title") or k.split(".")[-1]) if b \
            else (E.get(k, {}).get("name") or k.split(".")[-1])
        t = str(t)
        return t if len(t) <= n else t[:n] + "…"

    o = [f'<svg viewBox="0 0 {W} {H}" role="img" aria-label="the record as a tree">']
    o.append(f'<rect class="tsoil" x="0" y="{G:.0f}" width="{W}" height="{H - G:.0f}"/>')
    o.append(f'<path class="tground" d="M0 {G:.0f} '
             + " ".join(f"Q {gx + 30} {G + (3 if (gx // 60) % 2 else -3):.0f} {gx + 60} {G:.0f}"
                        for gx in range(0, W, 60)) + '"/>')
    top = G - LH * 0.82
    o.append(f'<path class="ttrunk" d="M{tx - 22:.0f} {G:.0f} '
             f'C{tx - 17:.0f} {G - LH * .32:.0f} {tx - 7:.0f} {G - LH * .5:.0f} '
             f'{tx - 4:.0f} {top:.0f} L{tx + 4:.0f} {top:.0f} '
             f'C{tx + 7:.0f} {G - LH * .5:.0f} {tx + 17:.0f} {G - LH * .32:.0f} '
             f'{tx + 22:.0f} {G:.0f} Z"/>')
    for k in upper:
        for p in parents[k]:
            if p not in x:
                continue
            # a fact reaches its consumers through the trunk: the root curve already
            # carried it to the base, so its limb emerges from the wood - only an
            # above-ground parent branches from where it actually stands.
            if depth[p] == 0:
                px, py = tx + jig(p, 13) - 6, G - LH * 0.45
            else:
                px, py = x[p], y[p]
            cx, cy = x[k], y[k]
            w = 1.5 + min(2.6, 0.4 * len((E.get(p, {}) or {}).get("used", [])))
            m2x = cx * 0.55 + tx * 0.45
            o.append(f'<path class="tlimb" data-lf="{html.escape(p)}" data-lt="{html.escape(k)}" '
                     f'stroke-width="{w:.1f}" d="M{px:.0f} {py:.0f} '
                     f'C{px:.0f} {py - LH * .35:.0f} {m2x:.0f} {cy + LH * .5:.0f} '
                     f'{cx:.0f} {cy:.0f}"/>')
    for k in roots:                              # the root fan spreads from the trunk base
        s = -6 if x[k] < tx else 6
        o.append(f'<path class="troot" data-lt="{html.escape(k)}" '
                 f'stroke-width="2.2" d="M{tx + s:.0f} {G + 2:.0f} '
                 f'C{tx + s * 5:.0f} {G + 26:.0f} {(x[k] + tx) / 2:.0f} {y[k] - 4:.0f} '
                 f'{x[k]:.0f} {y[k]:.0f}"/>')
    show_root_lbl = len(roots) <= 16
    boughs = [k for k in upper if k not in jud]
    show_bough_lbl = len(boughs) <= 10
    ci = ri = 0
    for k in roots + upper:
        f = flags.get(k, set())
        sev = " stopf" if f - {"blocked", "moved"} else (" warnf" if f else "")
        kind = "crown" if k in jud else ("root" if depth[k] == 0 else "bough")
        r = 9 if kind == "crown" else (5 if kind == "root" else 4)
        o.append(f'<g class="tn {kind}{sev}" data-id="{html.escape(k)}">')
        if kind == "crown":
            o.append(f'<circle class="halo" cx="{x[k]:.0f}" cy="{y[k]:.0f}" r="18"/>')
        o.append(f'<circle cx="{x[k]:.0f}" cy="{y[k]:.0f}" r="{r}"/>')
        if kind == "crown":
            ty = y[k] - 22 - 13 * (ci % 2); ci += 1
            o.append(f'<text x="{x[k]:.0f}" y="{ty:.0f}" text-anchor="middle" dir="auto">'
                     f'{html.escape(lbl(k, 20))}</text>')
        elif kind == "root" and show_root_lbl:
            ty = y[k] + 16 + 11 * (ri % 3); ri += 1
            o.append(f'<text x="{x[k]:.0f}" y="{ty:.0f}" text-anchor="middle" dir="auto">'
                     f'{html.escape(lbl(k, 12))}</text>')
        elif kind == "bough" and show_bough_lbl:
            ty = y[k] + 15 + 12 * (ci % 2); ci += 1
            o.append(f'<text x="{x[k]:.0f}" y="{ty:.0f}" text-anchor="middle" dir="auto">'
                     f'{html.escape(lbl(k, 16))}</text>')
        o.append("</g>")
    if dropped:
        o.append(f'<text x="{M}" y="{H - 8:.0f}" class="tn root"><tspan>'
                 f'... and {dropped} more roots below the grass</tspan></text>')
    o.append("</svg>")
    return "".join(o)


def build(paths, brief_path=None):
    doc = P.load(paths)
    ids, jud, fields = P.infer(doc)
    meta = doc.get("meta") or {}
    flags = flags_of(ids, jud, fields, P.bodies(doc))
    shape = shape_of(ids, jud, flags)
    brief = {}
    if brief_path and os.path.exists(brief_path):
        brief = yaml.safe_load(io.open(brief_path, encoding="utf-8").read()) or {}

    groups = {}
    for k in sorted(ids):
        if k in jud:
            continue
        groups.setdefault(k.split(".")[0] if "." in k else "-", []).append(k)

    # the payload: entries and judgments, keyed by id. one copy, shared by every element
    # on every tab.
    raw = {}
    for k, v in doc.items():
        if isinstance(v, dict):
            for nid, b in v.items():
                if isinstance(b, dict):
                    raw[nid] = b
                elif nid in ids:
                    raw[nid] = {"v": b}
    used = {}
    for name, j in jud.items():
        for d in j["deps"]:
            used.setdefault(d, []).append(name)
    E, J = {}, {}
    for k in ids:
        if k in jud:
            continue
        b = raw.get(k, {})
        E[k] = {kk: b.get(kk) for kk in ("v", "rule", "from", "at", "of", "read", "quoted", "url", "file")
                if b.get(kk) is not None}
        if b.get("quoted") and "v" not in E[k]:
            E[k]["v"] = b["quoted"]
        v = E[k].get("v")
        if isinstance(v, str) and P.EXPR.search(v) and [t for t in P.ID.findall(v) if t in ids]:
            E[k].setdefault("rule", v)
            del E[k]["v"]
        E[k]["used"] = sorted(used.get(k, []))
        ps = [t for t in P.ID.findall(str(E[k].get("rule") or "")) if t in ids and t != k]
        frm = b.get("from")
        if isinstance(frm, str) and frm in ids and frm != k:
            ps.append(frm)
        if ps:
            E[k]["par"] = sorted(set(ps))
        if named(b):
            E[k]["name"] = named(b)
        n = next((str(b[f]) for f in ("via", "note", "why") if b.get(f)), "")
        if 0 < len(n) <= 150:
            E[k]["note"] = n
    for name, j in jud.items():
        b = j["body"]
        why, keys = blocked_of(b)
        J[name] = {"deps": j["deps"], "used": sorted(used.get(name, [])), "pred": j["pred"],
                   "verdict": b.get("verdict") or b.get("title") or "",
                   "because": (b.get("because") or b.get("breaks_if") or "")[:400],
                   "blocked": why, "waiting": keys}

    labels = (brief.get("labels") or {}) if brief else {}
    fronts_decl = (brief.get("fronts") or {}) if brief else {}
    front_map = {}
    for fname, sels in fronts_decl.items():
        for sel in ([sels] if isinstance(sels, str) else sels):
            for k in resolve(sel, ids, jud, flags):
                front_map.setdefault(k, str(fname))

    def fx(k, text=None, cls="fx"):
        return (f'<span class="{cls}" data-id="{html.escape(k)}">'
                f'{html.escape(str(text if text is not None else k))}</span>')

    def lbl(k):
        """What to call this on a surface that is not about keys. A label is a
        presentation choice, so the brief may set one; the record only supplies one if
        it happens to carry a name of its own."""
        if k in labels:
            return str(labels[k])
        return named(raw.get(k)) or human(k)

    def refs(text):
        """Link every entry id the text literally names. No inference: the id is there."""
        out, pos = [], 0
        for m in P.ID.finditer(text):
            if m.group(0) not in E and m.group(0) not in J:
                continue
            out.append(html.escape(text[pos:m.start()]))
            out.append(f'<span class="fx in" data-id="{html.escape(m.group(0))}">'
                       f'{html.escape(m.group(0))}</span>')
            pos = m.end()
        out.append(html.escape(text[pos:]))
        return "".join(out)

    def shown(k, pretty=False):
        """-> html for this entry's value, with a rule's own references made live."""
        e = E.get(k) or {}
        if e.get("v") is not None:
            return html.escape(fmt(e["v"]) if pretty else str(e["v"]))[:400]
        return ("= " + refs(str(e["rule"]))) if e.get("rule") else ""

    def front(k):
        return front_map.get(k, "")

    def kicker(k, seen_fronts):
        """Which front this row is under - shown only where it is not already obvious."""
        f = front(k)
        return f'<span class="grp">{html.escape(f)}</span>' if f and len(seen_fronts) > 1 else ""

    def note(k):
        n = (E.get(k) or {}).get("note")
        return f'<div class="nt" dir="auto">{html.escape(n)}</div>' if n else ""

    def val(k):
        e = E.get(k) or {}
        if e.get("v") is not None:
            return str(e["v"])
        return ("= " + str(e["rule"])) if e.get("rule") else ""

    def has_value(k):
        return (E.get(k) or {}).get("v") is not None

    # ── renderers ────────────────────────────────────────────────────────────
    anchored = [0, 0]

    def r_cards(names):
        o = []
        for name in names:
            j, b = jud[name], J[name]
            verdict, h1 = anchor(b["verdict"], j["deps"], E)
            because, h2 = anchor(b["because"], j["deps"], E)
            miss = [d for d in j["deps"] if d not in E and d not in J]
            rest = [d for d in j["deps"] if d not in (h1 | h2) and d not in miss]
            anchored[0] += len(h1 | h2)
            anchored[1] += len([d for d in j["deps"] if d not in miss])
            o.append('<div class="card">'
                     f'<div class="vd fx" data-id="{html.escape(name)}" dir="auto">{verdict}</div>'
                     + (f'<div class="bc" dir="auto">{because}</div>' if because else "")
                     # what is missing stays visible; what is present is reachable by
                     # hovering the words that already mention it.
                     + ('<div class="deps">' + "".join(
                         (f'<span class="dep wait" title="declared missing - this judgment '
                          f'is waiting on it">{html.escape(d)}</span>' if b["blocked"] else
                          f'<span class="dep dead" title="not an entry in this record">'
                          f'{html.escape(d)}</span>') for d in miss) + "</div>" if miss else "")
                     + (f'<div class="rest">rests on {len(rest)} more &mdash; hover the line above</div>'
                        if rest else "")
                     + "</div>")
        return "".join(o)

    def r_alerts(names):
        def rank(n):
            return -max((URGENCY[f][0] for f in flags.get(n, ()) if f in URGENCY), default=0)
        o = ['<div class="alerts">']
        for name in sorted(names, key=lambda n: (rank(n), n)):
            fs = sorted(flags.get(name, ()), key=lambda f: -URGENCY.get(f, (0, ""))[0])
            # the dot shows the worst state the judgment is in, whatever order the
            # reasons are listed in
            tones = {URGENCY.get(f, (0, "ok"))[1] for f in fs}
            tone = next((t for t in ("stop", "warn", "mut") if t in tones), "ok")
            v, _ = anchor(J[name]["verdict"], jud[name]["deps"], E)
            # the key it is waiting on is the whole content of a blocked line - it is
            # what someone has to go and get, so it stays visible here
            miss = J[name]["waiting"] or [d for d in jud[name]["deps"] if d not in E and d not in J]
            why = J[name]["blocked"] or "; ".join(SAYS.get(f, f) for f in fs) or "holds"
            if miss and not J[name]["blocked"]:
                why += " \u2014 " + ", ".join(lbl(d) for d in miss)
            fr = front(name) or next((front(d) for d in jud[name]["deps"] if front(d)), "")
            o.append(f'<div class="al"><span class="dot" style="background:var(--{tone})"></span>'
                     f'<span class="at">'
                     + (f'<span class="grp">{html.escape(fr)}</span>' if fr else "")
                     + f'<span class="fx" data-id="{html.escape(name)}" dir="auto">'
                     f'{v}</span><div class="aw" dir="auto">{html.escape(why)}</div>'
                     f'</span></div>')
        return "".join(o) + "</div>"

    DERIVED = '<span class="derived">worked out</span>'

    def r_table(keys, raw_keys=False):
        rows = []
        for k in keys:
            cell = shown(k, True) if has_value(k) else DERIVED
            head = fx(k) if raw_keys else fx(k, lbl(k))
            cls = "k" if raw_keys else "kl"
            rows.append(f'<tr><td class="{cls}" dir="auto">{head}</td>'
                        f'<td class="v" dir="auto">{cell}</td></tr>')
        return "<table>" + "".join(rows) + "</table>"

    def r_lines(keys):
        return '<div class="deps">' + "".join(
            f'<span class="fx dep" data-id="{html.escape(k)}" dir="auto">{html.escape(lbl(k))}</span>'
            for k in keys) + "</div>"

    def r_timeline(keys):
        today = datetime.date.today()
        seq = sorted(((as_date(val(k)), k) for k in keys), key=lambda t: t[0])
        fs = {front(k) for k in keys if front(k)}
        o, marked = ['<div class="tl">'], False
        for d, k in seq:
            if not marked and d >= today:
                o.append(f'<div class="tlr mark">today &middot; {today.strftime("%d/%m/%Y")}</div>')
                marked = True
            o.append(f'<div class="tlr{" past" if d < today else ""}">'
                     f'<span class="when">{d.strftime("%d/%m/%Y")}</span>'
                     f'<span class="what" dir="auto">{kicker(k, fs)}{fx(k, lbl(k))}'
                     f'{note(k)}</span></div>')
        if not marked:
            o.append(f'<div class="tlr mark">today &middot; {today.strftime("%d/%m/%Y")}</div>')
        return "".join(o) + "</div>"

    def r_fronts(keys):
        g = {}
        for k in keys:
            g.setdefault(front(k) or labels.get(k.split(".")[0])
                         or (k.split(".")[0] if "." in k else "-"), []).append(k)
        o = ['<div class="grid">']
        for name, ks in sorted(g.items(), key=lambda kv: (-len(kv[1]), kv[0])):
            o.append(f'<div class="front"><h3 dir="auto">{html.escape(name)}</h3>' + "".join(
                (f'<div class="kv"><span class="kl">{fx(k, lbl(k))}</span>'
                 f'<span class="kvv">{shown(k, True)}</span></div>'
                 if has_value(k) else
                 f'<div class="kv"><span class="kl">{fx(k, lbl(k))}</span>'
                 f'<span class="kvv">{DERIVED}</span></div>')
                for k in ks) + "</div>")
        return "".join(o) + "</div>"

    def r_headline(keys):
        return '<div class="heads">' + "".join(
            f'<div class="head"><div class="big">{fx(k, fmt((E.get(k) or {}).get("v")))}</div>'
            f'<div class="cap" dir="auto">{kicker(k, {front(x) for x in keys if front(x)})}'
            f'{html.escape(lbl(k))}</div>{note(k)}</div>' for k in keys) + "</div>"

    ENTRY_R = {"table": r_table, "lines": r_lines, "timeline": r_timeline,
               "fronts": r_fronts, "headline": r_headline}
    JUD_R = {"cards": r_cards, "alerts": r_alerts}
    cards = r_cards

    # ── the session's tab ───────────────────────────────────────
    now_html, covered, empty_sections, misfit = [], set(), [], []
    # the record's own name heads the page; a record without one borrows the brief's
    # title, which is presentation and so lives in the brief
    rec_name = named(meta) or str(brief.get("title") or "").strip()
    h1 = (f'<h1 dir="auto">{html.escape(rec_name)}</h1>' if rec_name
          else '<h1 dir="ltr">What is known here</h1>')
    if brief:
        now_html.append(h1)
        if brief.get("intent"):
            # the page is named for the record; the intent is the arrangement's aim,
            # not a title - it is another task on the way, and it reads like one.
            now_html.append('<p class="purpose" dir="auto">Everything on this tab was picked '
                            f'for one purpose — <b dir="auto">{html.escape(str(brief["intent"]))}</b></p>'
                            + '<p class="sub" dir="ltr">'
                            + f'The Record tab has all {len(ids)} entries and judgments, '
                            f'arranged by nothing.</p>')
        was = brief.get("shape") or {}
        if not was:
            now_html.append('<div class="banner">This arrangement records no shape, so nothing '
                            'can tell whether it went stale. Add a <code>shape:</code> block '
                            '(printed on stderr when this page was generated).</div>')
        else:
            moved = [f"{k}: {was[k]} &rarr; {shape[k]}" for k in shape if k in was and was[k] != shape[k]]
            if moved:
                now_html.append('<div class="banner" dir="auto">The record has changed shape since '
                                'this arrangement was written &mdash; ' + "; ".join(moved) +
                                '. The sections below still fill themselves, but the sections '
                                'themselves may no longer be the right ones.</div>')
        for sec in brief.get("sections") or []:
            picks = sec.get("pick")
            picks = [picks] if isinstance(picks, str) else list(picks or [])
            got = set()
            for x in picks:
                got |= resolve(x, ids, jud, flags)
            covered |= got
            jn = sorted(x for x in got if x in jud)
            en = sorted(x for x in got if x not in jud)
            title = str(sec.get("title") or ",".join(picks))
            kind = str(sec.get("as") or "").strip()
            wrong = fits(kind, sorted(got), jud, E) if kind else None
            if wrong:
                misfit.append((title, wrong))
                kind = ""
            if not got:
                empty_sections.append(title)
            now_html.append(f'<h2 dir="auto">{html.escape(title)} <span class="n">{len(got)}</span></h2>')
            if sec.get("why"):
                now_html.append(f'<div class="why" dir="auto">{html.escape(str(sec["why"]))}</div>')
            if wrong:
                now_html.append(f'<div class="bad">{html.escape(wrong)} &mdash; fell back to the '
                                f'default shape</div>')
            if not got:
                now_html.append('<div class="why">Nothing in the record matches this section. '
                                'It is about something the record no longer holds.</div>')
            if jn:
                now_html.append((JUD_R.get(kind) or r_cards)(jn))
            if en:
                now_html.append((ENTRY_R.get(kind) or r_table)(en))
        # It may order. It may not drop. This section is not optional and the brief
        # cannot switch it off: an arrangement that hides what it did not anticipate
        # is worth less than no arrangement.
        spill = sorted(k for k, f in flags.items() if f and k not in covered)
        if spill:
            now_html.append(f'<h2 class="spill" dir="ltr">Not covered by this arrangement '
                            f'<span class="n">{len(spill)}</span></h2>'
                            '<div class="why">Flagged, and no section above picked it up. '
                            'This section is written by the page, not by the brief.</div>')
            now_html.append(r_alerts(spill))

    # ── the record's own tab ─────────────────────────────────────────────────
    rec_html = []
    if jud:
        rec_html.append(f'<h2>Judgments <span class="n">{len(jud)}</span></h2>')
        rec_html.append(cards(sorted(jud)))
    for g, keys in sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0])):
        rec_html.append(f'<h2 id="g-{html.escape(g)}">{html.escape(g)}</h2>' + r_table(keys, True))

    # ── the page ─────────────────────────────────────────────────────────────
    ns = '<nav class="ns" dir="ltr">' + "".join(
        f'<a href="#g-{html.escape(g)}">{html.escape(g)} ({len(v)})</a>'
        for g, v in sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0]))) + "</nav>"
    head = [h1]
    if meta.get("scope"):
        head.append(f'<p class="scope" dir="auto">{html.escape(str(meta["scope"]).strip())}</p>')
    head.append(f'<p class="meta" dir="ltr">{shape["entries"]} entries and {shape["judgments"]} '
                f'judgments'
                + (f'. {shape["flagged"]} need a person' if shape["flagged"] else "")
                + (f'. Last updated {html.escape(str(meta["updated"]))}' if meta.get("updated") else "")
                + "</p>" + ns)

    out = ['<!doctype html><html><head><meta charset="utf-8">',
           '<meta name="viewport" content="width=device-width,initial-scale=1">',
           f'<title>{html.escape(str(brief.get("title") or rec_name or meta.get("scope") or "record")[:60])}</title>',
           f'<style>{CSS}</style></head><body><div class="wrap" dir="{direction(doc)}">']

    tree = (h1 + '<p class="purpose" dir="ltr">The whole record as one growing thing — '
            'roots are what was read from the world, the canopy is what was concluded '
            'from it. Hover anything.</p>'
            '<div class="treewrap">' + tree_svg(ids, jud, E, J, flags) + '</div>'
            '<p class="sub" dir="ltr" style="margin-top:8px">roots — read from the world '
            '&middot; branches — worked out &middot; blossoms — concluded &middot; '
            'the trunk is where they meet</p>')
    tabs = ['<div class="tabs" role="tablist">']
    if brief:
        tabs.append('<button type="button" data-tab="now" aria-selected="true">Now'
                    + (f' <span class="n">{len(covered)}</span>' if covered else "") + "</button>")
    tabs.append(f'<button type="button" data-tab="record" aria-selected='
                f'"{"false" if brief else "true"}">Record <span class="n">{len(ids)}</span></button>')
    tabs.append('<button type="button" data-tab="tree" aria-selected="false">Tree</button></div>')
    out.append("".join(tabs))
    if brief:
        out.append('<section id="panel-now">' + "".join(now_html) + "</section>")
    out.append(f'<section id="panel-record"{" hidden" if brief else ""}>'
               + "".join(head + rec_html) + "</section>")
    out.append('<section id="panel-tree" hidden>' + tree + "</section>")

    out.append('<footer dir="ltr">Hover any key for where it came from. Click to pin, click a dependency '
               'to walk to it, Esc to step back. While a card is open, a solid outline marks '
               'everything that rests on it and a dashed one what it rests on. Generated from '
               'the record - nothing here was typed twice.' + (' The <b>Now</b> tab is an arrangement someone chose; '
               '<b>Record</b> is everything, arranged by nothing.' if brief else '') + "</footer>")
    out.append("</div><script>window.__E=" + json.dumps(_plain(E), ensure_ascii=False)
               + ";window.__J=" + json.dumps(_plain(J), ensure_ascii=False) + ";</script>")
    out.append(f"<script>{JS}</script></body></html>")
    return "\n".join(out), E, J, ids, {"shape": shape, "empty": empty_sections,
                                       "misfit": misfit, "brief": bool(brief), "anchored": tuple(anchored),
                                       "unnamed": sorted(k for k in E if not E[k].get("name")
                                                         and k not in labels),
                                       "covered": covered, "flags": flags}


def verify(paths, brief_path=None):
    """Deterministic, no browser. What only looking can catch is a separate job."""
    page, E, J, ids, info = build(paths, brief_path)
    fail = []
    # what the page SHOWS is markup, not script - the provenance layer's own source
    # mentions the attribute it binds to, and that is not an element.
    dom = re.sub(r"<script>.*?</script>", "", page, flags=re.S)
    shown = set(re.findall(r'data-id="([^"]+)"', dom))
    for k in shown:
        if k not in E and k not in J:
            fail.append(f"page shows {k}, which is not in the record")
    for k in E:
        if k not in shown:
            fail.append(f"{k} is in the payload but nothing on the page shows it")
    note = []
    for name, j in J.items():
        for d in j["deps"]:
            if d not in E and d not in J:
                (note if j["blocked"] else fail).append(
                    f"{name} links to {d}, which the payload does not carry"
                    + (" - declared, so the page shows it as awaited" if j["blocked"] else ""))
    if "window.__E=" not in page or "window.__J=" not in page:
        fail.append("payload missing")
    if info["brief"]:
        u = info["unnamed"]
        if u:
            note.append(f"{len(u)} entries carry no human name, so the page has to fall back to "
                        f"their keys: {', '.join(u[:6])}"
                        + (f" and {len(u) - 6} more" if len(u) > 6 else ""))
        a, t = info["anchored"]
        if t:
            note.append(f"{a} of {t} live dependencies are named in the prose that cites them; "
                        f"the rest are reachable only by hovering the judgment")
        if 'data-tab="now" aria-selected="true"' not in page:
            fail.append("a brief exists but the session tab is not the default")
        # An authored section that picks nothing is the alert row about something already
        # closed: it costs trust on everything else on the page.
        for t, why in info["misfit"]:
            fail.append(f"section '{t}': {why}")
        for t in info["empty"]:
            fail.append(f"section '{t}' picks nothing - it is about something the record "
                        f"no longer holds")
        missed = [k for k, f in info["flags"].items() if f and
                  f'data-id="{k}"' not in dom]
        for k in missed:
            fail.append(f"{k} is flagged but does not appear on the Now tab at all")
    for n in note:
        print("NOTE", n)
    for f in fail:
        print("FAIL", f)
    print(f"{len(shown)} elements, {len(E)} entries, {len(J)} judgments, "
          + ("2 tabs, " if info["brief"] else "1 tab, ") + f"{len(fail)} problems")
    return 1 if fail else 0


if __name__ == "__main__":
    a = sys.argv[1:]
    brief = a[a.index("--brief") + 1] if "--brief" in a else None
    files = [x for x in a if x.endswith((".yaml", ".yml")) and x != brief] or P.default_paths()
    brief = find_brief(files, brief)
    if "--verify" in a:
        sys.exit(verify(files, brief))
    page, _, _, _, info = build(files, brief)
    if info["brief"] and not (yaml.safe_load(io.open(brief, encoding="utf-8").read()) or {}).get("shape"):
        sys.stderr.write("# no shape recorded in the brief. paste this into it, so a later\n"
                         "# render can tell you the arrangement went stale:\nshape:\n"
                         + "".join(f"  {k}: {v}\n" for k, v in info["shape"].items()))
    sys.stdout.write(page)
