#!/usr/bin/env python3
"""Render a record as one self-contained HTML page - in two tabs, sharing one provenance layer.

  python3 render_page.py [file ...] > page.html
  python3 render_page.py --brief .kpopper/view.yaml [file ...] > page.html
  python3 render_page.py --verify [file ...]
  python3 render_page.py --profile core/v1 [--verify] [file ...]

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
LOADED_SOURCE_HASH = hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import provenance as P
from page_words import WORDS
from page_lint import lint_output
import page_measurements as MEASUREMENTS

# The page's own code lives beside this file, in the languages its tools speak. Read
# whole at import and inlined at render, so a page is still one file.
_PAGE = pathlib.Path(__file__).resolve().parent / "page"
CSS = "\n" + (_PAGE / "page.css").read_text(encoding="utf-8")
JS = "\n" + (_PAGE / "page.js").read_text(encoding="utf-8")
COMPONENT_CSS = "\n" + (_PAGE / "components.css").read_text(encoding="utf-8")
DATE_JS = "\n" + (_PAGE / "dates.js").read_text(encoding="utf-8")

# ── what the record says about itself ────────────────────────────────────────
# One reading of state, used by every section selector. The same four conditions
# `provenance.py open` ranks by; named here so a brief can select on them.
STATES = ("broken", "falsified", "unchecked", "moved", "blocked", "no_predicate", "unknown", "reversed")
GROUPED_SHAPES = ("grouped", "fronts")


def flags_of(ids, jud, fields, raw, defer_counts=False):
    """Per judgment: the set of conditions that put it in front of a person - derived the
    way `check` and `open` derive them, so the page never disagrees with the reader."""
    return P.flags(ids, jud, fields, raw, defer_counts=defer_counts)


RTL = re.compile(r"[\u0590-\u05ff\u0600-\u06ff]")


def direction(doc):
    """A record written in Hebrew should not be read left to right because the tool
    was written in English. This is a decision about the record's *shape*, so it is
    stable: values changing never flips the page."""
    meta = (doc or {}).get("meta") or {}
    return meta.get("direction") or WORDS.get(language(doc), WORDS["en"])["dir"]


def language(doc):
    """Values, URLs and internal names cannot change the language of a page."""
    meta = (doc or {}).get("meta") or {}
    explicit = meta.get("language") or meta.get("lang")
    if explicit:
        return str(explicit).lower().replace("_", "-").split("-")[0]
    descriptive = {"name", "title", "label", "what", "desc", "scope", "domain", "because",
                   "verdict", "note", "why", "asked", "blocked_on", "reopened_by"}
    parts = []
    def visit(x):
        if isinstance(x, dict):
            for k, v in x.items():
                if k in descriptive and isinstance(v, str):
                    parts.append(v)
                elif isinstance(v, (dict, list)):
                    visit(v)
        elif isinstance(x, list):
            for v in x:
                visit(v)
    visit(doc)
    text = " ".join(parts)
    n = sum(c.isalpha() for c in text)
    for lang, pattern in (("he", r"[\u0590-\u05ff]"), ("ar", r"[\u0600-\u06ff]")):
        if n and len(re.findall(pattern, text)) / n > .3:
            return lang
    return "en"


fmt = P.fmt        # one formatting of a number, shared with every surface that prints one


def counted(words, key, n, lang="en"):
    form = ('one' if n == 1 else 'two' if n == 2 else
            'many' if lang == 'ar' and 11 <= n % 100 <= 99 else
            'other' if lang == 'ar' and not 3 <= n % 100 <= 10 else '')
    return words[key + ('_' + form if form else '')].format(n=n)


# What a card can carry. A reasoning longer than this is drawn to here and marked, because
# a sentence that stops mid-word on the reading surface reads as the whole of what was
# argued. The budget is also the sign `d.reasoning_in_record` named for itself - reasoning
# swelling past what a card holds - so what was cut is said by `--verify` rather than only
# shown.
CARD_CHARS = 400


def clipped(text, n=CARD_CHARS):
    """-> (what a card draws, was it cut). Cut at the last word boundary inside the budget,
    without splitting a record reference, with an ellipsis, so the reader sees that the
    argument continues."""
    s = str(text or "")
    if len(s) <= n:
        return s, False
    cut = s[:n - 1]
    if cut.rfind("{{") > cut.rfind("}}"):
        cut = cut[:cut.rfind("{{")]
    space = cut.rfind(" ")
    return (cut[:space] if space > n // 2 else cut).rstrip(" ,;:-") + "\u2026", True


def shape_of(ids, jud, flags):
    held = [k for k in ids if k not in jud and not P.is_builtin(k)]
    return {"entries": len(held), "judgments": len(jud),
            "flagged": sum(1 for f in flags.values() if f),
            "blocked": sum(1 for f in flags.values() if "blocked" in f)}


# ── the brief ────────────────────────────────────────────────────────────────
def _read_mode(read_mode=None):
    mode = read_mode or ('frozen' if P._RAW_READS.get() else os.environ.get('KPOPPER_READ_MODE', 'live'))
    if mode not in ('live', 'frozen'):
        raise ValueError('read mode must be live or frozen')
    return mode


def record_paths(paths, *, read_mode=None):
    """Resolve the selected knowledge world before deriving its brief or evidence roots."""
    return list(P._peer('knowledge_views').write_paths(paths)) if _read_mode(read_mode) == 'live' else list(paths)


def find_brief(paths, explicit=None, *, read_mode=None):
    """The brief given, else the record's own: `.kpopper/view.yaml` beside a record under the
    new name, `<file>.view.yaml` beside a record - or a file it points at - under the old."""
    return P.brief_for(record_paths(paths, read_mode=read_mode), explicit)


def same_value(old, now):
    """The comparison `check` makes between a snapshot and a value: text after whitespace,
    then number - so 1,702,093 and 1702093 are the same value and 31 and 35 are not."""
    if " ".join(str(old).split()) == " ".join(str(now).split()):
        return True
    try:
        from decimal import Decimal
        return Decimal(str(old).replace(",", "")) == Decimal(str(now).replace(",", ""))
    except Exception:
        return False


def tabs_of(brief):
    """The tabs the page draws, in order. A brief written as `tabs:` draws every one of them
    under its own name, each a reading occasion that `serves` a set of intents; a brief that
    is bare `sections:` is one tab, called Now, aimed at its `intent`. Each tab carries what
    the drawing needs and the key its panel is drawn under - `now` for the first, so a link
    to it keeps working."""
    if not isinstance(brief, dict):
        return []
    out = []
    secs = [s for s in (brief.get("sections") or []) if isinstance(s, dict)]
    if secs or not brief.get("tabs"):
        out.append({"key": "now", "title": "", "occasion": "", "serves": [], "bare": True,
                    "sections": secs, "shape": brief.get("shape"), "intent": brief.get("intent")})
    for t in (brief.get("tabs") or []):
        if not isinstance(t, dict):
            continue
        out.append({"key": "now" if not out else f"now{len(out) + 1}",
                    "title": str(t.get("title") or ""), "occasion": str(t.get("occasion") or ""),
                    "serves": [str(s) for s in (t.get("serves") or [])], "bare": False,
                    "sections": [s for s in (t.get("sections") or []) if isinstance(s, dict)],
                    "shape": t.get("shape"), "intent": None})
    return out


# ── intents, and whether the page still answers them ─────────────────────────
# An intent is a session source: the one entry shape that carries what was asked. What a
# session recorded comes `from:` it, or rests on it. Coverage holds the page against those
# facts and is deliberately dumb - counts and shares, printed, never acted on. Whether two
# intents are one world is a session's judgment, never the mechanism's.
def intent_date(k, body):
    """When a session was read: its `read` or `of` date, else the date its id carries."""
    for f in ("read", "of"):
        d = as_date(body.get(f)) if body.get(f) is not None else None
        if d:
            return d
    m = re.search(r"(\d{4})_(\d{2})_(\d{2})", k)
    if m:
        try:
            return datetime.date(int(m.group(1)), int(m.group(2)), int(m.group(3)))
        except ValueError:
            return None
    return None


def intents_of(ids, jud, raw):
    """Every session source, newest first -> [(id, body, date)]."""
    out = []
    for k in ids:
        b = raw.get(k)
        if k not in jud and isinstance(b, dict) and b.get("asked"):
            out.append((k, b, intent_date(k, b)))
    return sorted(out, key=lambda t: (t[2] or datetime.date.min, t[0]), reverse=True)


def recorded_by(s, ids, jud, raw):
    """What a session wrote: every entry that comes `from:` it, and every judgment that rests
    on it - the two ways the record attributes a thing to the session that produced it."""
    out = set()
    for k in ids:
        b = raw.get(k)
        if isinstance(b, dict) and str(b.get("from") or "") == s:
            out.add(k)
        elif k in jud and s in jud[k]["deps"]:
            out.add(k)
    return out


def born_of(jud):
    """The newest `born` an arrangement judgment carries - the last day the page as a whole
    was decided - or None when none carries one."""
    dates = [as_date(j["body"].get("born")) for j in jud.values() if j["body"].get("born")]
    dates = [d for d in dates if d]
    return max(dates) if dates else None


def coverage(ids, jud, raw, tabs, picks, born):
    """The page held against what its sessions were for. `picks[key]` is what each tab's
    sections pick, taken before anything is drawn. -> {rows, tabs, unpicked_prefixes, empty,
    born, page} where every number is a fact and none is a verdict."""
    picked_all = set().union(*picks.values()) if picks else set()
    rows = []
    for k, b, d in intents_of(ids, jud, raw):
        rec = recorded_by(k, ids, jud, raw)
        claimed = [t for t in tabs if k in t["serves"]]
        earned = [t for t in claimed if rec & picks.get(t["key"], set())]
        prefixes = {}
        for x in sorted(rec):
            p = x.split(".")[0] if "." in x else x
            prefixes[p] = prefixes.get(p, 0) + 1
        rows.append({"id": k, "asked": str(b.get("asked")), "date": d, "recorded": sorted(rec),
                     "picked": sorted(rec & picked_all), "unpicked": sorted(rec - picked_all),
                     "share": (round(len(rec & picked_all) / len(rec), 2) if rec else None),
                     "claimed": [t["title"] or "Now" for t in claimed],
                     "served": [t["title"] or "Now" for t in earned],
                     "unearned": [t["title"] or "Now" for t in claimed if t not in earned],
                     "empty": not rec, "unserved": bool(rec) and not earned,
                     "prefixes": prefixes,
                     "inside": [(t["title"] or "Now", len(rec & picks.get(t["key"], set())), len(rec))
                                for t in tabs]})
    tab_rows = []
    for t in tabs:
        rec = set()
        for r in rows:
            if r["id"] in t["serves"]:
                rec |= set(r["recorded"])
        got = rec & picks.get(t["key"], set())
        tab_rows.append({"title": t["title"] or "Now", "serves": list(t["serves"]),
                         "recorded": len(rec), "picked": len(got),
                         "share": (round(len(got) / len(rec), 2) if rec else None)})
    groups = {}
    for k in ids:
        if P.is_builtin(k):
            continue
        groups.setdefault(k.split(".")[0] if "." in k else k, []).append(k)
    unpicked_prefixes = sorted(g for g, ks in groups.items() if not any(k in picked_all for k in ks))
    # the newest sessions in a row no tab serves, by day - a day is the finest clock the
    # record keeps, so same-day sessions count together, and a day on which any intent is
    # served ends the run
    days = []
    for r in rows:
        if r["empty"]:
            continue
        if days and days[-1][0] == r["date"]:
            days[-1][1].append(r)
        else:
            days.append((r["date"], [r]))
    streak = 0
    for _, rs in days:
        if any(not r["unserved"] for r in rs):
            break
        streak += len(rs)
    drift = None
    if born:
        added = set()
        for r in rows:
            if r["date"] and r["date"] >= born:
                added |= set(r["recorded"])
        drift = round(len(added - picked_all) / len(added), 2) if added else 0.0
    return {"rows": rows, "tabs": tab_rows, "unpicked_prefixes": unpicked_prefixes,
            "empty": [r["id"] for r in rows if r["empty"]], "born": born,
            "page": {"page.unserved": sum(1 for r in rows if r["unserved"]),
                     "page.recent_unserved": streak, "page.drift": drift,
                     "page.covered": len(picked_all)}}


def coverage_lines(cov, full=True):
    """The coverage report as lines: the counts, each tab against what it serves, every
    intent no tab serves with one hint beside it, the prefixes nothing picks. Without
    `full`, only the intents - what `check` and the gate say."""
    out, p = [], cov["page"]
    if full:
        drift = "-" if p["page.drift"] is None else f"{p['page.drift']} since {cov['born']}"
        out.append(f"coverage: {p['page.covered']} covered · spill {p.get('page.spill', '-')} · "
                   f"{p['page.unserved']} intents no tab serves · {p['page.recent_unserved']} "
                   f"recent in a row · drift {drift}")
        for t in cov["tabs"]:
            if not t["serves"]:
                continue
            out.append(f"tab '{t['title']}' serves {', '.join(t['serves'])}: picks {t['picked']} "
                       f"of {t['recorded']} they recorded")
    for r in cov["rows"]:
        if not r["unserved"]:
            continue
        out.append(f"{r['id']} is served by no tab - asked: {P.short(r['asked'], 90)}")
        touched = ", ".join(f"{g}. ({n})" for g, n in sorted(r["prefixes"].items()))
        inside = "; ".join(f"{n} of {m} inside '{t}'" for t, n, m in r["inside"]) or "no tab yet"
        out.append(f"  hint: it wrote {touched} - {inside}")
    for k in cov["empty"]:
        out.append(f"{k} recorded nothing, so no tab can serve it and none needs to")
    if full and cov["unpicked_prefixes"]:
        out.append("no section picks: " + ", ".join(cov["unpicked_prefixes"]))
    return out


# ── arrangements, held against the brief ─────────────────────────────────────
# An arrangement is a judgment by shape (the reader says which): it rests on the session
# sources of the occasion it decides, and its sign is a count the build takes. What is
# special about it is only what it is held against - the page's counts, the record's shape
# kept in the brief, and the decisions that stand - and every fact below is a count or a
# link, never a verdict: whether its sign holds is decided where every sign is.
def arrangements_of(ids, jud, raw, tabs, picks, cov, flags, doc):
    """Every arrangement the record carries -> {id: facts}. Its tabs are the ones whose picks
    *earn* a source it rests on - served as coverage counts it, never merely claimed - and it
    is linked while every source it rests on is earned by some tab: a tab deleted, or gutted
    with its `serves:` line kept, cuts the link the same way. A brief of one bare tab links
    every arrangement to it. `stood` counts the sessions read on a later day than it was
    born that one of its tabs served - the only later sessions that are evidence it held;
    `drift` is the share of what sessions recorded since its own born that nothing picks,
    where the page's drift counts from the newest born on the page."""
    rows = {r["id"]: r for r in (cov["rows"] if cov else [])}
    title_of = {t["key"]: (t["title"] or "Now") for t in tabs}
    bare_one = len(tabs) == 1 and tabs[0]["bare"]
    earned = {t["key"]: sorted(s for s, r in rows.items() if title_of[t["key"]] in r["served"])
              for t in tabs}
    # a tab that claims no occasion - no serves: line at all - is the page's own, and belongs
    # to every arrangement the way the one bare tab does: decided with it, settled by its
    # review, never the ground of a merge
    unclaimed = [t["key"] for t in tabs if not t["serves"]]
    picked_all = set().union(*picks.values()) if picks else set()
    questions = {}
    for g in P.OPEN:
        for k, v in (doc.get(g) or {}).items():
            questions[k] = v if isinstance(v, str) else str(v)
    hyps = getattr(doc, "hypotheses", None) or {}
    out = {}
    for v, j in sorted(jud.items()):
        if not P.is_arrangement(j, raw):
            continue
        srcs = P.intents_of(j, raw)
        if bare_one:
            own, cut = [tabs[0]["key"]], ""
        else:
            own = [t["key"] for t in tabs if set(srcs) & set(earned[t["key"]])]
            unearned = [s for s in srcs if not any(s in earned[k] for k in own)]
            cut = f"no tab's sections earn {', '.join(unearned)}" if unearned else ""
        keys = [t["key"] for t in tabs if t["key"] in own or t["key"] in unclaimed]
        titles = [title_of[k] for k in keys]
        born = as_date(j["body"].get("born"))
        stood = sum(1 for r in rows.values() if born and r["date"] and r["date"] > born
                    and any(t in r["served"] for t in titles))
        added = set()
        for r in rows.values():
            if born and r["date"] and r["date"] >= born:
                added |= set(r["recorded"])
        drift = (round(len(added - picked_all) / len(added), 2) if added else 0.0) if born else None
        contested = []

        def decided(body):
            """What an arrangement decides - everything the session wrote, not what the tool
            stamped - so a re-decision that keeps the verdict and moves the occasion or the
            sign is a contest too."""
            return ({f: x for f, x in body.items() if f not in ("seen", "born", "replaced")}
                    if isinstance(body, dict) else body)
        for name, h in sorted(hyps.items()):
            if h["error"] or v not in h["ids"]:
                continue
            other = h["raw"].get(v)
            if decided(other) != decided(j["body"]):
                contested.append(("contribution" if h.get('kind') == 'contribution' else "hypothesis",
                                  name, str(P.claim_of(other))))
        for q, text in sorted(questions.items()):
            if v in P.ID.findall(text):
                contested.append(("question", q, text))
        parts = P.comparison_parts(j["pred"])
        counted = P.value_of(raw, ids, parts[0]) if parts else None
        out[v] = {"sources": srcs, "tabs": titles, "keys": keys, "own": [title_of[k] for k in own],
                  "linked": not cut, "cut": cut,
                  "fired": "falsified" in flags.get(v, ()), "pred": P.predicate_text(j["pred"]),
                  "reading": (f"{parts[0]} is {counted}" if parts and counted is not None else ""),
                  "born": born, "stood": stood, "drift": drift,
                  # the request is drawn only as the record accepts it: a session source
                  # carrying what was asked, rested on - check fails any other
                  "request": (j["body"].get("request")
                              if isinstance(j["body"].get("request"), str)
                              and P.is_intent(j["body"]["request"], raw)
                              and j["body"]["request"] in j["deps"] else None),
                  "contested": contested, "moved": {}}
    return out, earned


def arrangement_lines(info, page_decides=False):
    """The arrangements held against the brief, as lines -> (fail, note). A brief that no
    longer carries what a standing arrangement decided fails - the record's own claim, so
    `check` fails it too and the Stop gate bounces once on an out-of-tree record, where
    `--verify` never runs; a tab no decision records is a note. With `page_decides` the
    arrangements whose sign the page found holding are said too - for `check`, which
    otherwise only knows the page would decide them; `--verify` fails those itself, and is
    the one surface that says when a brief records no arrangement at all - once, and only
    on a record whose sessions could be served by one."""
    facts = info.get("arrangements") or {}
    cov, tabs = info.get("coverage"), info.get("tabs") or []
    fail, note = [], []
    if not facts:
        if cov and cov.get("rows") and not page_decides:
            note.append("no arrangement decision is recorded, so the brief is held against none - "
                        "an arrangement is a judgment resting on the session sources a tab serves, "
                        "with a sign over a count: add v.<slug> rests_on=[s.<...>, page.unserved] "
                        "verdict=... wrong_if='page.unserved > 0'")
        return fail, note
    by_tab = {}
    for v, f in sorted(facts.items()):
        if not f["linked"]:
            fail.append(f"the brief does not serve {', '.join(f['sources'])} together, as {v} "
                        f"decided ({f['cut']}) - serve them on a tab whose sections pick what they "
                        f"wrote, or re-decide {v}")
        for t in f["own"]:
            by_tab.setdefault(t, []).append(v)
        if f["fired"]:
            if page_decides:
                note.append(f"{v}: wrong_if holds ({f['pred']}) - decided by the page"
                            + (f", {f['reading']}" if f["reading"] else ""))
        else:
            for title, moves in sorted(f["moved"].items()):
                note.append(f"tab '{title}': shape moved ({moves}) - muted, {v}'s sign has not "
                            f"appeared")
        for kind, who, claim in f["contested"]:
            note.append(f"{v} is contested by {kind} {who}: {P.short(claim, 80)}")
    for title, vs in sorted(by_tab.items()):
        for i, a in enumerate(vs):
            for b in vs[i + 1:]:
                if not set(facts[a]["sources"]) & set(facts[b]["sources"]):
                    fail.append(f"the brief reads {a} and {b} on one tab ('{title}'), which neither "
                                f"decided - re-decide the one whose occasion changed, and review "
                                f"the other")
    earned = info.get("earned") or {}
    for t in tabs:
        if t["bare"] or (t["title"] or "Now") in by_tab or not earned.get(t["key"]):
            continue
        note.append(f"tab '{t['title']}' is an arrangement no decision records - add v.<slug> "
                    f"rests_on=[{', '.join(earned[t['key']])}, page.unserved] verdict=... "
                    f"wrong_if='page.unserved > 0'")
    return fail, note


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


def anchor(text, deps, E, label=None):
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
        display = label(d) if label and text[a:b] == d else text[a:b]
        out.append(f'<span class="fx in" data-id="{html.escape(d)}">{html.escape(display)}</span>')
        pos = b
    out.append(html.escape(text[pos:]))
    return "".join(out), {d for _, _, d in hits}


human = P.human


# ── renderers: a closed set, each declaring the shape of data it can carry ────
# Layout is where intent shows. But a renderer that silently accepts data it cannot
# express produces a page that looks arranged and is not, so each one says what it
# needs and a mismatch is a failure, not a shrug.
def fits(kind, keys, jud, E, groups_of=None, words=None, label=None):
    def problem(code, fallback, **values):
        return words[code].format(**values) if words else fallback

    def names(ks):
        return ", ".join(label(k) if label else k for k in ks)

    if kind in ("table", "lines", "cards"):
        return None
    if kind == "timeline":
        bad = [k for k in keys if k not in jud and as_date((E.get(k) or {}).get("v")) is None]
        return problem("fit_timeline", f"timeline needs date values; {len(bad)} of {len(keys)} are not dates "
                       f"({', '.join(bad[:4])})", bad=len(bad), total=len(keys), names=names(bad[:4])) if bad else None
    if kind == "headline":
        n = [k for k in keys if k not in jud]
        if not 1 <= len(n) <= 4:
            return problem("fit_headline_count", f"headline carries one to four values, not {len(n)}", n=len(n))
        blank = [k for k in n if (E.get(k) or {}).get("v") is None]
        return problem("fit_headline_values", f"headline needs values; {', '.join(blank)} "
                       f"{'is derived and this reader does not evaluate rules' if len(blank) == 1 else 'are derived'}",
                       names=names(blank)) if blank else None
    if kind in GROUPED_SHAPES:
        groups = set()
        for k in keys:
            gs = groups_of(k) if groups_of else []
            groups |= set(gs) if gs else {k.split(".")[0]}
        return problem("fit_grouped", "grouped lays groups side by side; these are all one group "
                       f"({', '.join(sorted(groups)) or '-'})", names=', '.join(sorted(groups)) or '-') if len(groups) < 2 else None
    if kind == "alerts":
        entries = [k for k in keys if k not in jud]
        return problem("fit_alerts", f"alerts ranks judgments; {len(entries)} of these are entries", n=len(entries)) if entries else None
    if kind == "axis":
        return None
    if kind == "links":
        bad = [k for k in keys if k in jud or not link_target(E.get(k) or {})]
        return problem("fit_links", f"links needs a safe url or file: {', '.join(bad)}", names=names(bad)) if bad else None
    return problem("fit_unknown", f"unknown renderer '{kind}'", kind=kind)


def link_target(entry, record_root=None, page_path=None):
    """A safe record destination, rebased when the generated page's place is known."""
    from urllib.parse import quote, unquote, urlsplit, urlunsplit
    v = str(entry.get("url") or entry.get("file") or "").strip()
    if not v or any(ord(c) < 32 for c in v):
        return ""
    try:
        parsed = urlsplit(v)
    except ValueError:
        return ""
    scheme = parsed.scheme.lower()
    if scheme:
        if scheme in ("http", "https") and not parsed.netloc:
            return ""
        if scheme not in ("https", "http", "mailto", "file"):
            return ""
        # Encode characters that cannot safely occur literally without changing the URL's
        # path/query/fragment boundaries. Existing escapes remain escapes.
        return urlunsplit((parsed.scheme, parsed.netloc,
                           quote(parsed.path, safe="/.-_~%:@!$&'()*+,;="),
                           quote(parsed.query, safe="/?.-_~%:@!$&'()*+,;="),
                           quote(parsed.fragment, safe="/?.-_~%:@!$&'()*+,;=")))
    if v.startswith("//"):
        return ""
    is_url = bool(entry.get("url"))
    # A fragment-only URL belongs to this generated page. Rebasing it as a filesystem path
    # would break its tabs and tree navigation.
    if is_url and not parsed.path:
        return quote(v, safe="?.-_~%=&#+@!$'()*+,;:/")
    if record_root is not None and page_path is not None:
        local = unquote(parsed.path) if is_url else v
        source = local if os.path.isabs(local) else os.path.join(str(record_root), local)
        # Both ends are spelled the same way before one is subtracted from the other. One
        # directory can be reached under more than one name - on macOS a temporary directory is
        # both /var/... and /private/var/..., one of them a link to the other - and a source
        # named under one spelling, subtracted from a page located under the other, counts the
        # wrong number of levels: the link climbs past the root and arrives nowhere. Resolved on
        # both sides it stays within the directory the two files share, so it keeps working when
        # the page is opened under either name.
        absolute = os.path.realpath(source)
        page_dir = os.path.realpath(os.path.dirname(os.path.abspath(page_path)))
        try:
            relative = os.path.relpath(absolute, page_dir)
            # `local` was decoded for filesystem arithmetic, so '%' is literal here and must be
            # encoded again (an authored %25 must not turn into an incomplete escape). Windows
            # path separators become URL separators; a literal POSIX backslash stays literal.
            if os.sep != "/":
                relative = relative.replace(os.sep, "/")
            target = quote(relative, safe="/.-_~")
        except ValueError:
            # Windows cannot spell a relative path across drive letters. An absolute file URI
            # keeps that source reachable and lets the page itself remain a standalone file.
            target = pathlib.Path(absolute).as_uri()
        if is_url:
            target += ("?" + quote(parsed.query, safe="/?.-_~%:@!$&'()*+,;=")) if parsed.query else ""
            target += ("#" + quote(parsed.fragment, safe="/?.-_~%:@!$&'()*+,;=")) if parsed.fragment else ""
        return target
    # A URL's query and fragment are navigation, while a file field is a literal path.
    return quote(v, safe="/.-_~%?=&#+@" if is_url else "/.-_~")


URGENCY = {"broken": (100, "stop"), "falsified": (95, "stop"), "unchecked": (80, "stop"),
           "moved": (70, "warn"), "blocked": (60, "warn"), "no_predicate": (40, "mut"), "unknown": (75, "warn"),
           "reversed": (72, "warn")}

# What each state means, said the way a person would say it. The machine name stays -
# in the hover, where the keys and the rules live. Nothing on the reading surface is
# named after how the thing is built.
SAYS = {"broken": "rests on something that is not in this record",
        "falsified": "its own condition for being wrong now holds",
        "moved": "something it rests on no longer matches what it last saw",
        "unchecked": "has never been checked against one of the things it rests on",
        "blocked": "waiting on something nobody has recorded yet",
        "no_predicate": "nothing here would show it to be wrong",
        "unknown": "its condition cannot currently be evaluated",
        "reversed": "its verdict was replaced under this id and nobody has reviewed it since"}

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


def tree_svg(ids, jud, E, J, flags, words=None, label=None):
    """The record as one growing thing. What was read from the world is the root
    system, below the ground line; what was worked out and concluded branches up
    from it, judgments in the canopy. Same record, same tree - the layout reads
    only the graph, so nothing here moves unless the record does."""
    w = words or WORDS["en"]

    def jig(k, m, salt=""):
        return int(hashlib.md5((salt + k).encode()).hexdigest(), 16) % m

    parents = {}
    for k in ids:
        if k in jud:
            parents[k] = [d for d in jud[k]["deps"] if d in ids]
        else:
            e = E.get(k) or {}
            ps = [t for t in P.rule_refs(e, ids) if t in ids]
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
        t = (J[k].get("verdict") or (label(k) if label else human(k))) if b \
            else (label(k) if label else E.get(k, {}).get("name") or human(k))
        t = P.ID.sub(lambda m: (label(m.group(0)) if label else human(m.group(0)))
                    if m.group(0) in ids else m.group(0), str(t))
        return t if len(t) <= n else t[:n] + "…"

    o = [f'<svg viewBox="0 0 {W} {H}" role="img" aria-label="{w["tree_alt"]}">']
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
            weight = 1.5 + min(2.6, 0.4 * len((E.get(p, {}) or {}).get("used", [])))
            m2x = cx * 0.55 + tx * 0.45
            o.append(f'<path class="tlimb" data-lf="{html.escape(p)}" data-lt="{html.escape(k)}" '
                     f'stroke-width="{weight:.1f}" d="M{px:.0f} {py:.0f} '
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
        sev = " stopf" if f - {"blocked", "moved", "reversed"} else (" warnf" if f else "")
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
                 f'{w["tree_more"].format(n=dropped)}</tspan></text>')
    o.append("</svg>")
    return "".join(o)


def build(paths, brief_path=None, page_path=None, *, read_mode=None, profile=None, context=None,
          doc=None):
    """The page, and what it counted. `doc` is the record already read for *these* paths in
    this mode - what `P.load(record_paths(paths), read_mode=mode)` returns, nothing else: the
    document carries no paths of its own, so the brief and the record root are still derived
    from `paths` and handing over a document read from somewhere else draws that record under
    this one's brief. Only the stated mode is checked, and the reader's `_page_or_error`
    catches everything, so the refusal below reaches a read command as a note rather than a
    traceback."""
    if profile == 'core/v1':
        if doc is not None:
            raise ValueError('the core page profile builds from its own reading')
        return core_build(paths, brief_path, page_path, read_mode=read_mode, context=context)
    if profile is not None:
        raise ValueError('unsupported page profile: ' + str(profile))
    mode = _read_mode(read_mode)
    paths = record_paths(paths, read_mode=mode)
    # A caller that has just read this record hands its document over instead of paying for a
    # second reading of the same files in the same command - and a second reading is a second
    # chance for the reader's view and the page's to disagree. The write path passes none, so
    # every count it takes still sees the record as it stands at that moment.
    if doc is None:
        doc = P.load(paths, read_mode=mode)
    else:
        if getattr(doc, 'read_mode', mode) != mode:
            raise ValueError('the page was given a record read in another mode: '
                             + str(doc.read_mode) + ', not ' + mode)
        # what `P.load` would have refused on the way in, refused here instead: a document
        # read under the core permission must not be drawn by this renderer just because
        # somebody else did the reading
        P.refuse_dormant_profile(doc)
    record_root = os.path.dirname(P.layout_of(paths)["entry"])
    ids, jud, fields = P.infer(doc)
    meta = doc.get("meta") or {}
    declared_lang, page_dir = language(doc), direction(doc)
    lang = declared_lang if declared_lang in WORDS else "en"
    w = dict(WORDS[lang], dir=page_dir)
    built = P.builtins(doc, ids, jud, fields, P.bodies(doc))
    raw0 = P.bodies(doc)
    raw0.update(built)
    flags = flags_of(ids, jud, fields, raw0, defer_counts=True)
    shape = shape_of(ids, jud, flags)
    brief, tabs = {}, []
    if brief_path and os.path.exists(brief_path):
        brief = yaml.safe_load(pathlib.Path(brief_path).read_text(encoding="utf-8")) or {}
        brief = brief if isinstance(brief, dict) else {}
        tabs = tabs_of(brief)

    prefixes = {}
    for k in sorted(ids):
        if k in jud:
            continue
        prefixes.setdefault(k.split(".")[0] if "." in k else "-", []).append(k)

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
    raw.update(built)
    used = {}
    for name, j in sorted(jud.items()):
        for d in j["deps"]:
            used.setdefault(d, []).append(name)
    E, J = {}, {}
    for k in sorted(ids):
        if k in jud:
            continue
        b = raw.get(k, {})
        E[k] = {kk: b.get(kk) for kk in ("v", "rule", "from", "at", "of", "read", "quoted", "url",
                                          "file", "asked", "unverified", "measure")
                if b.get(kk) is not None}
        if b.get("quoted") and "v" not in E[k]:
            E[k]["v"] = b["quoted"]
        v = E[k].get("v")
        if isinstance(v, str) and P.EXPR.search(v) and [t for t in P.ID.findall(v) if t in ids]:
            E[k].setdefault("rule", v)
            del E[k]["v"]
        if isinstance(b.get("rule"), dict):
            E[k]["rule_text"] = P.predicate_text(b["rule"])
            result = P.E.current(raw, ids, k)
            if result["value"] is not None:
                E[k]["v"] = P.E.display_value(result["value"])
            else:
                E[k]["calculation_error"] = result["reason"]
        E[k]["used"] = sorted(used.get(k, []))
        ps = [t for t in P.rule_refs(E[k], ids) if t in ids and t != k]
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
    swollen = []
    for name, j in sorted(jud.items()):
        b = j["body"]
        why, keys = blocked_of(b)
        written = P.reasoning_of(b)
        because, cut = clipped(written)
        # the budget is over what a card draws, and a reference is drawn as the value or
        # verdict behind it - which can be longer than the `{{id}}` that stands for it. So
        # what is counted is the resolved length: prose cut here, and prose that only
        # overruns once the record is read into it, are the same swelling to a reader.
        drawn = len(P.resolve_refs(because, raw0, ids, jud))
        if cut:
            swollen.append((name, len(written), "cut"))
        elif drawn > CARD_CHARS:
            swollen.append((name, drawn, "resolved"))
        J[name] = {"deps": j["deps"], "used": sorted(used.get(name, [])), "pred": P.predicate_text(j["pred"]),
                   "verdict": str(b.get("verdict") or b.get("title") or ""),
                   "because": because,
                   "blocked": why, "waiting": keys,
                   "unverified": b.get("unverified"),
                   "reopened": next((str(b[k]) for k in P.REOPENED if b.get(k)), "")}

    labels = (brief.get("labels") or {}) if brief else {}
    # what a prefix is called on a surface: the brief's label, else the word the record's
    # own legend gives a letter, else the prefix itself
    legend = P.legend_of(meta, ids)

    def prefix_label(g):
        return labels.get(g) or legend.get(g) or g
    # A grouping is a scheme, and a brief may declare several: `groups:` is either one
    # scheme - group name -> selectors - or several, scheme name -> group name -> selectors.
    # A section says which scheme it reads by, and an id under two groups of one scheme is
    # under both. The older field name still reads as one scheme.
    gdecl = (brief.get("groups") or brief.get("fronts") or {}) if brief else {}
    schemes, index = {}, {}
    if isinstance(gdecl, dict) and gdecl:
        nested = all(isinstance(v, dict) for v in gdecl.values())
        for sname, gs in (gdecl.items() if nested else [("groups", gdecl)]):
            schemes[str(sname)], index[str(sname)] = {}, {}
            if not isinstance(gs, dict):
                continue
            for gname, sels in gs.items():
                members = set()
                for sel in ([sels] if isinstance(sels, str) else list(sels or [])):
                    members |= resolve(sel, ids, jud, flags)
                schemes[str(sname)][str(gname)] = members
                for k in members:
                    index[str(sname)].setdefault(k, []).append(str(gname))
    component_page = bool(schemes) or any(
        str(sec.get("as") or "") in set(GROUPED_SHAPES) | {"headline", "timeline", "alerts", "cards", "axis", "links"}
        for tab in tabs for sec in tab["sections"])
    default_scheme = next(iter(schemes), None)
    reading_by = [default_scheme]        # the scheme the section being drawn reads by

    def fx(k, text=None, cls="fx"):
        return (f'<span class="{cls}" data-id="{html.escape(k)}">'
                f'{html.escape(surface_words(str(text if text is not None else lbl(k))))}</span>')

    def surface_words(text):
        # Only literal ids are named here; never guess a source for an arbitrary value.
        return P.ID.sub(lambda m: lbl(m.group(0)) if m.group(0) in ids else m.group(0), text)

    def lbl(k):
        """What to call this on a surface that is not about keys. A label is a
        presentation choice, so the brief may set one; the record only supplies one if
        it happens to carry a name of its own."""
        if k in labels:
            return str(labels[k])
        name = named(raw.get(k))
        if name:
            return name
        if '.' not in k and k in ids:
            return w.get('label_' + k, w['unnamed_item'].format(name=human(k)))
        return human(k)

    # The record's own words, references as written, are what the cards draw from; the
    # payload carries them resolved, so a hover reads the same sentence a card shows.
    RAW = {name: dict(J[name]) for name in J}

    def resolve_payload():
        """Run once the page has counted, so a reference to a page count reads the number
        on the hover as it does on the card."""
        for name in J:
            for f in ("verdict", "because", "blocked", "reopened"):
                J[name][f] = P.resolve_refs(RAW[name][f], raw0, ids, jud, lbl)
        for k in E:
            if "note" in E[k]:
                E[k]["note"] = P.resolve_refs(E[k]["note"], raw0, ids, jud, lbl)

    def link_ids(text):
        """Link every entry id the text literally names. No inference: the id is there."""
        out, pos = [], 0
        for m in P.ID.finditer(text):
            if m.group(0) not in E and m.group(0) not in J:
                continue
            out.append(html.escape(text[pos:m.start()]))
            out.append(f'<span class="fx in" data-id="{html.escape(m.group(0))}">'
                       f'{html.escape(lbl(m.group(0)))}</span>')
            pos = m.end()
        out.append(html.escape(text[pos:]))
        return "".join(out)

    def shown(k, pretty=False):
        """-> html for this entry's value, with a rule's own references made live."""
        e = E.get(k) or {}
        if e.get("v") is not None:
            value = ((w["yes"] if e["v"] else w["no"]) if component_page or lang != "en" else str(e["v"])) if isinstance(e["v"], bool) else (fmt(e["v"]) if pretty else str(e["v"]))
            return link_ids(value)
        return ("= " + link_ids(P.predicate_text(e["rule"]))) if e.get("rule") else ""

    def groups_of(k, scheme=None):
        """The groups this id is under in the scheme being read by - every one of them,
        since a scheme may overlap. A scheme nobody declared is read off the record itself:
        the id's prefix, or any field the entries carry - `from`, `unit`, `kind` - whose
        value names the group, by its own name when the value is an entry."""
        s = scheme or reading_by[0]
        if s in schemes:
            return list(index.get(s, {}).get(k, []))
        if s == "prefix":
            return [prefix_label(k.split(".")[0])] if "." in k else []
        v = (raw.get(k) or {}).get(s) if isinstance(raw.get(k), dict) else None
        if v is None or isinstance(v, (dict, list)):
            return []
        v = str(v)
        return [lbl(v) if v in E or v in J else v]

    def carried(field):
        """Whether any entry carries this field - what makes `by: <field>` a scheme."""
        return any(isinstance(b, dict) and field in b for b in raw.values())

    def group_of(k):
        gs = groups_of(k)
        return gs[0] if gs else ""

    def hue(k=None, group=None):
        # Identity is taken from the first declaration even when this section groups by
        # a different field. No value, sort position or hash of the record picks a hue.
        names = groups_of(k, default_scheme) if k and default_scheme else []
        gs = list(schemes.get(default_scheme, {}))
        chosen = names[0] if names else group
        if chosen is None:
            return ""
        if chosen not in gs:
            gs = list(schemes.get(reading_by[0], {}))
        i = gs.index(chosen) if chosen in gs else int(hashlib.md5(chosen.encode()).hexdigest(), 16) % 8
        return f' style="--group:var(--g{i % 8})"'

    def kicker(k, seen_groups):
        """Which groups this row is under - shown only where it is not already obvious."""
        gs = groups_of(k)
        return (f'<span class="grp">{html.escape(" · ".join(gs))}</span>'
                if gs and len(seen_groups) > 1 else "")

    def note(k):
        n = (raw.get(k) or {}).get("via") or (raw.get(k) or {}).get("note") or (raw.get(k) or {}).get("why")
        return f'<div class="nt" data-origin="{html.escape(k)}" dir="auto">{prose(str(n), (E.get(k) or {}).get("par", []))[0]}</div>' if n else ""

    def val(k):
        e = E.get(k) or {}
        if e.get("v") is not None:
            return str(e["v"])
        return ("= " + P.predicate_text(e["rule"])) if e.get("rule") else ""

    def has_value(k):
        return (E.get(k) or {}).get("v") is not None

    # ── renderers ────────────────────────────────────────────────────────────
    anchored = [0, 0]

    def ref(k, shown, moved, signed=False):
        """One reference drawn: hoverable, and marked - with what it was - when it moved
        since the text around it was reviewed."""
        cls = "fx in" + (" signed" if signed else "")
        if k in moved:
            return (f'<span class="{cls} mv" data-id="{html.escape(k)}" title="'
                    f'{html.escape(w["was"].format(v=P.short(moved[k], 60)))}">'
                    f'{html.escape(surface_words(str(shown)))}</span>')
        return f'<span class="{cls}" data-id="{html.escape(k)}">{html.escape(surface_words(str(shown)))}</span>'

    def moved_note(pairs, what):
        """The line under a tinted text or card: what moved since it was read, by name.
        A placed judgment says which of its halves moved, so the line does not print one
        clipped verdict twice. A warning, not a source - it is in the markup, so it shows
        wherever the page does."""
        said = []
        for k, o, n in pairs:
            half, was, now = P.which_moved(o, n)
            said.append(f"{html.escape(lbl(k))}{', ' + html.escape(half) if half else ''} "
                        f"{html.escape(P.short(surface_words(str(was))))} &rarr; "
                        f"{html.escape(P.short(surface_words(str(now))))}")
        return (f'<div class="mvd" dir="auto">{w["moved_" + what]}'
                + "; ".join(said) + "</div>")

    def moves_of(name):
        """{dep: what the judgment saw} for every dependency that moved under it - the
        moves nothing decided, which are the ones that put it in front of a person."""
        return {d: o for d, o, _, s in P.moved_deps(jud[name], raw0, ids) if s == "moved"}

    def prose(text, deps, moved=None):
        """Escaped prose with its references drawn and its dependencies anchored. A reference
        shows what every surface shows for it - the value where there is one, the name
        where there is only a rule, the verdict for a judgment - each hoverable, so the
        sentence stays the interface."""
        parts, pos, found = [], 0, set()
        text, moved = text or "", moved or {}
        for m in P.REF.finditer(text):
            a, h = anchor(text[pos:m.start()], deps, E, lbl)
            parts.append(a)
            found |= h
            k = m.group(1)
            shown = P.reference_text(k, raw0, ids, jud, lbl)
            if shown is None:
                parts.append(html.escape(m.group(0)))
            else:
                found.add(k)
                parts.append(ref(k, shown, moved))
            pos = m.end()
        a, h = anchor(text[pos:], deps, E, lbl)
        parts.append(a)
        found |= h
        result = "".join(parts)
        # The source may itself discuss an id outside this judgment's dependencies.
        # Name it without inventing a value edge. Connective prose never takes this path.
        result = re.sub(r'(^|>)([^<>]+)(?=<|$)',
                        lambda m: m.group(1) + html.escape(surface_words(html.unescape(m.group(2)))), result)
        return result, found

    def status(name):
        fs = sorted(flags.get(name, ()), key=lambda f: -URGENCY.get(f, (0, ""))[0])
        parts = [w["says"][f] for f in fs]
        if J[name].get("unverified"):
            parts.append(w["unverified"] + ": " + str(J[name]["unverified"]))
        return "; ".join(parts) or w["holds"]

    def judgment_meta(name):
        j = jud[name]
        unread = [k for k in j["deps"] if k in ids and k not in j["seen"]]
        return (f' data-judgment="{html.escape(name)}"'
                f' data-review="{"unread" if unread else "moved" if moves_of(name) else "current"}"')

    def r_cards(names):
        o = []
        for name in names:
            j, b = jud[name], RAW[name]
            moved = moves_of(name)
            verdict, h1 = prose(b["verdict"], j["deps"], moved)
            because, h2 = prose(b["because"], j["deps"], moved)
            # the sign that would re-open a decided judgment, in its own row: a person
            # reads it here, which is what keeps a bad one from hiding
            reopened, h3 = prose(b["reopened"], j["deps"], moved)
            miss = [d for d in j["deps"] if d not in E and d not in J]
            rest = [d for d in j["deps"] if d not in (h1 | h2 | h3) and d not in miss]
            anchored[0] += len(h1 | h2 | h3)
            anchored[1] += len([d for d in j["deps"] if d not in miss])
            # a judgment something moved under since it was reviewed is tinted, and says
            # what moved - in the markup, so the warning shows where scripts do not run
            mv = [(d, o, n) for d, o, n, s in P.moved_deps(j, raw0, ids) if s == "moved"]
            o.append('<div class="card' + (" moved" if mv else "") + '"' + hue(name) + judgment_meta(name) + '>'
                     f'<div class="cardtop"><span class="judgment-label">{w["judgment"]}</span>'
                     + kicker(name, set(index.get(default_scheme, {}))) + '</div>'
                     f'<div class="vd fx" data-id="{html.escape(name)}" dir="auto">{verdict}</div>'
                     + (f'<div class="bc" dir="auto">{because}</div>' if because else "")
                     + (f'<div class="state" data-warning="true">{html.escape(status(name))}</div>'
                        if flags.get(name) or b.get("unverified") else "")
                     + (f'<div class="rb" dir="auto"><span class="lbl">{w["reopened_by"]}</span>'
                        f'{reopened}</div>' if reopened else "")
                     + (moved_note(mv, "reviewed") if mv else "")
                     # what is missing stays visible; what is present is reachable by
                     # hovering the words that already mention it.
                     + ('<div class="deps">' + "".join(
                         (f'<span class="dep wait" title="{w["awaited"]}: '
                          f'{html.escape(d)}">{html.escape(d)}</span>' if b["blocked"] else
                          f'<span class="dep dead" title="{w["not_here"]}: {html.escape(d)}">'
                          f'{html.escape(d)}</span>') for d in miss) + "</div>" if miss else "")
                     + (f'<div class="rest">{w["rest"].format(n=len(rest))}</div>'
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
            if J[name].get("unverified"):
                tones.add("warn")
            tone = next((t for t in ("stop", "warn", "mut") if t in tones), "ok")
            v, _ = prose(RAW[name]["verdict"], jud[name]["deps"], moves_of(name))
            # the key it is waiting on is the whole content of a blocked line - it is
            # what someone has to go and get, so it stays visible here
            miss = J[name]["waiting"] or [d for d in jud[name]["deps"] if d not in E and d not in J]
            why = J[name]["blocked"] or status(name)
            if J[name].get("unverified"):
                unverified = str(J[name]["unverified"])
                why = ((J[name]["blocked"] + "; ") if J[name]["blocked"] and J[name]["blocked"] != unverified else "")
                why += w["unverified"] + ": " + unverified
            if miss and not J[name]["blocked"]:
                why += " \u2014 " + ", ".join(lbl(d) for d in miss)
            fr = group_of(name) or next((group_of(d) for d in jud[name]["deps"] if group_of(d)), "")
            icon = {"stop": "!", "warn": "△", "mut": "?", "ok": "✓"}[tone]
            o.append(f'<div class="al{" moved" if moves_of(name) else ""}"' + (hue(name) or hue(group=fr or None)) + judgment_meta(name) + '>'
                     f'<span class="ico {tone}" aria-hidden="true">{icon}</span>'
                     f'<span class="at">'
                     + f'<span class="fx" data-id="{html.escape(name)}" dir="auto">'
                     f'{v}</span><div class="aw" data-warning="true" dir="auto">{html.escape(why)}</div>'
                     + (f'<div class="aw">{html.escape(status(name))}</div>' if J[name]["blocked"] and fs else "")
                     + '</span>' + f'<span class="tag {tone}">{w["judgment"]}</span>'
                     + (f'<span class="grp"><i class="group-dot"></i>{html.escape(fr)}</span>' if fr else "")
                     + '</div>')
        return "".join(o) + "</div>"

    DERIVED = f'<span class="derived">{w["derived"]}</span>'
    UNCOUNTED = f'<span class="derived">{w["uncounted"]}</span>'

    def blank(k):
        """What stands where a value would: a rule's entry was worked out; a page count the
        build could not take - nothing dates what was added - is said so, never left empty."""
        return UNCOUNTED if k in P.PAGE else DERIVED

    def r_table(keys):
        rows = []
        for k in keys:
            cell = shown(k, True) if has_value(k) else blank(k)
            head = html.escape(lbl(k))
            cls = "kl"
            rows.append(f'<tr><td class="{cls}" dir="auto">{head}</td>'
                        f'<td class="v" dir="auto"><span class="fx" data-id="{html.escape(k)}">{cell}</span></td></tr>')
        return "<table>" + "".join(rows) + "</table>"

    def r_lines(keys):
        return '<div class="deps">' + "".join(
            f'<span class="fx dep" data-id="{html.escape(k)}" dir="auto">{html.escape(lbl(k))}</span>'
            for k in keys) + "</div>"

    def r_timeline(keys):
        today = datetime.date.today()
        seq = sorted(((as_date(val(k)), k) for k in keys), key=lambda t: t[0])
        fs = {g for k in keys for g in groups_of(k)}
        days = {}
        for date, k in seq:
            days.setdefault(date, []).append(k)
        days.setdefault(today, [])
        o = ['<div class="tl">']
        for date, ks in sorted(days.items()):
            o.append(f'<div class="day{" past" if date < today else " hot" if date == today else ""}" data-day="{date.isoformat()}"'
                     + (' data-calendar-marker="true"' if not ks else '') + '>'
                     '<div class="when"' + (' data-clock="today"' if not ks else '') + '>'
                     + (fx(ks[0], date.strftime('%d/%m/%Y')) if ks else date.strftime('%d/%m/%Y')) + '</div>'
                     f'<div class="day-label">{w["today"] if date == today else ""}</div>')
            for k in ks:
                o.append('<div class="day-item"' + hue(k) + '>' + kicker(k, fs)
                         + fx(k, lbl(k)) + note(k) + '</div>')
            o.append('</div>')
        return "".join(o) + "</div>"

    def r_grouped(keys):
        g = {}
        for k in keys:
            names = groups_of(k) or [prefix_label(k.split(".")[0]) if "." in k else "-"]
            for name in names:               # an id under two groups is drawn under both
                g.setdefault(name, []).append(k)
        o = ['<div class="grid">']
        for name, ks in sorted(g.items(), key=lambda kv: (-len(kv[1]), kv[0])):
            o.append('<div class="group"' + hue(group=name) + f'><h3 dir="auto">{html.escape(name)}</h3>' + "".join(
                (r_cards([k]) if k in jud else
                 f'<div class="kv fx" data-id="{html.escape(k)}"><span class="kl">{html.escape(lbl(k))}</span>'
                 f'<span class="kvv">{shown(k, True)}</span></div>'
                 if has_value(k) else
                 f'<div class="kv fx" data-id="{html.escape(k)}"><span class="kl">{html.escape(lbl(k))}</span>'
                 f'<span class="kvv">{blank(k)}</span></div>')
                for k in ks) + "</div>")
        return "".join(o) + "</div>"

    def r_headline(keys):
        out = ['<div class="heads">']
        for k in keys:
            date = as_date(E[k].get("v"))
            attrs = f' data-countdown="{date.isoformat()}"' if date else ''
            n = (date - datetime.date.today()).days if date else None
            value = (w["today"] if n == 0 else counted(w, "days_left" if n > 0 else "days_ago", abs(n), lang)) if date else (
                (w["yes"] if E[k]["v"] else w["no"]) if isinstance(E[k]["v"], bool) else fmt(E[k]["v"]))
            out.append('<div class="head"' + hue(k) + f'><div class="big"{attrs}>{fx(k, value)}</div>'
                       f'<div class="cap" dir="auto">{kicker(k, {g for x in keys for g in groups_of(x)})}'
                       f'{html.escape(lbl(k))}</div>'
                       + (f'<div class="nt">{fx(k, date.isoformat())}</div>' if date else '')
                       + note(k) + '</div>')
        return ''.join(out) + '</div>'

    def r_links(keys):
        return '<div class="links">' + ''.join(
            '<a class="lk"' + hue(k) + f' href="{html.escape(link_target(E[k], record_root, page_path), quote=True)}">'
            + fx(k, lbl(k)) + note(k) + '</a>' for k in keys) + '</div>'

    page_counts = {}          # filled once the page has counted, before anything is drawn

    def moved_in(sec):
        """What the section's text saw that the record no longer holds: {ref: (was, now)}.
        Compared as the snapshot `review` writes it - a value, a rule, a source's date, and
        for a judgment the sentence the text draws - so an argument rewritten under a
        placement is a move, and not only a verdict that changed above it."""
        out = {}
        for k, old in (sec.get("seen") or {}).items():
            if k not in E and k not in J:
                continue
            now = P.shown_value(k, raw0, ids, jud, page_counts)
            if now is None or isinstance(now, list) or isinstance(old, list):
                continue
            if k not in J and (isinstance(now, dict) or isinstance(old, dict)):
                continue          # an entry whose value is a mapping is not comparable
            if not P.same_seen(old, now):
                out[k] = (old, now)
        return out

    def r_text(sec):
        """Connective prose: its references resolved in place, and a judgment's reasoning
        placed where the text asks for it - marked as a judgment, hoverable as one. Nothing
        is anchored by guessing: the edges are exactly what the text cites. Tinted, with
        what moved, when something it saw is no longer what the record holds.
        -> (html, the ids it placed)."""
        text, moved = str(sec.get("text") or ""), moved_in(sec)

        def segment(text):
            parts, pos, placed = [], 0, set()
            for m in P.REF.finditer(text):
                parts.append(html.escape(text[pos:m.start()]))
                k = m.group(1)
                if k in J:
                    # a judgment with no reasoning still has a conclusion; the text draws that
                    # rather than a hole, and `--verify` says the sentence is not the author's
                    inner, _ = prose(RAW[k]["because"] or RAW[k]["verdict"], jud[k]["deps"], moves_of(k))
                    cls = "fx in rsn" + (" mv" if moves_of(k) else "")
                    parts.append('<span' + judgment_meta(k) + '>'
                                 f'<span class="judgment-label">{w["judgment"]}</span> '
                                 f'<span class="{cls}" data-id="{html.escape(k)}">{inner}</span>'
                                 + (f'<span class="state" data-warning="true"> {html.escape(status(k))}</span>'
                                    if flags.get(k) or J[k].get("unverified") else '') + '</span>')
                    placed.add(k)
                else:
                    shown = P.reference_text(k, raw0, ids, jud, lbl)
                    if shown is None:
                        parts.append(html.escape(m.group(0)))
                    else:
                        signed = sec.get("as") == "axis" and type((E.get(k) or {}).get("v")) in (int, float)
                        if signed:
                            value = E[k]["v"]
                            shown = ("+" if value > 0 else "") + fmt(value)
                        parts.append(ref(k, shown, {x: o for x, (o, _) in moved.items()}, signed=signed))
                        placed.add(k)
                pos = m.end()
            parts.append(html.escape(text[pos:]))
            return "".join(parts), placed

        if sec.get("as") == "axis":
            rows, placed = [], set()
            for line in text.splitlines():
                if not line.strip():
                    continue
                inner, refs = segment(line)
                rows.append('<div class="axis-step">' + inner + '</div>')
                placed |= refs
            content = '<div class="axis">' + ''.join(rows) + '</div>'
        else:
            content, placed = segment(text)
        out = ('<div class="txt' + (" moved" if moved else "") + '" dir="auto">'
               + content + "</div>")
        if moved:
            out += moved_note([(k, o, n) for k, (o, n) in sorted(moved.items())], "read")
        unread = placed - set(sec.get("seen") or {})
        if unread:
            out += f'<div class="state" data-warning="true">{w["review_missing"]}</div>'
        return ('<div data-prose="connective" data-review="'
                + ('unread' if unread else 'moved' if moved else 'current') + '">'
                + out + '</div>'), placed

    ENTRY_R = {"table": r_table, "lines": r_lines, "timeline": r_timeline,
               "grouped": r_grouped, "fronts": r_grouped, "headline": r_headline, "links": r_links}
    JUD_R = {"cards": r_cards, "alerts": r_alerts}
    cards = r_cards

    # ── the session's tabs ───────────────────────────────────────
    # the record's own name heads the page; a record without one borrows the brief's
    # title, which is presentation and so lives in the brief
    rec_name = named(meta) or str(brief.get("title") or "").strip()
    h1 = (f'<h1 dir="auto">{html.escape(rec_name)}</h1>' if rec_name
          else f'<h1 dir="{page_dir}">{w["untitled"]}</h1>')
    # hypotheses beside the record are counted under the heading and drawn nowhere: the page
    # is the base, and what a hypothesis proposes is read with pull until consolidation
    hyps = getattr(doc, "hypotheses", None) or {}
    named_hypotheses = {name: hyp for name, hyp in hyps.items() if hyp.get('kind') != 'contribution'}
    if named_hypotheses:
        n, c = len(named_hypotheses), len(P.contested(doc))
        h1 += (f'<p class="meta" dir="{page_dir}">' + (w["hypothesis_one"] if n == 1 else w["hypotheses"].format(n=n))
               + (w["contested"].format(n=c) if c else '') + w["base_only"] + '</p>')
    for line in P._peer('knowledge_views').lines(doc):
        h1 += '<p class="meta" dir="auto" lang="en">' + html.escape(line) + '</p>'
    if getattr(doc, 'contributions', []):
        h1 += '<p class="meta" lang="en">Project contributions are not expanded on this base page; use open, pull or knowledge snapshot.</p>'
    # what the brief declares beyond what the page draws - checked as the tabs are drawn
    contract = {"tabs": len(tabs), "bad": [], "moved": [], "stale": [], "unread": [], "coverage": []}
    # What the arrangement covers, before anything is drawn: the page's own counts have to
    # exist before what rests on them is decided. So every tab's picks are resolved once
    # here, the counts taken, the flags and the shape decided again - and only then is
    # anything drawn. Spill is what fell through before the arrangement's own falsifiers
    # were decided; the other counts hold the page against what its sessions were for.
    # A section's prose covers what it names - the entry is on the page, and coverage asks
    # whether the arrangement reached it. It does not *account* for it: prose shows a
    # judgment's argument and never that the judgment is broken, unchecked or waiting. So
    # what a section chose is kept apart from what its sentences mentioned, and the spill
    # test - the one an arrangement's own sign is drawn against - reads only what was chosen.
    picks, chosen = {}, {}
    for t in tabs:
        got, sel = set(), set()
        for sec in t["sections"]:
            p = sec.get("pick")
            for x in ([p] if isinstance(p, str) else list(p or [])):
                sel |= resolve(x, ids, jud, flags)
            got |= {r for r in P.refs_in(sec.get("text")) if r in E or r in J}
        picks[t["key"]], chosen[t["key"]] = got | sel, sel
    pre = set().union(*chosen.values()) if chosen else set()
    cov = None
    if brief:
        cov = coverage(ids, jud, raw0, tabs, picks, born_of(jud))
        cov["page"]["page.spill"] = sum(1 for k, f in flags.items() if f and k not in pre)
        for k, v in cov["page"].items():
            if k in E and v is not None:
                E[k]["v"] = v
                raw0[k]["v"] = v
        page_counts.update({k: v for k, v in cov["page"].items() if v is not None})
    flags = flags_of(ids, jud, fields, raw0)
    shape = shape_of(ids, jud, flags)
    # every arrangement held against the brief - after the counts, since its sign is read
    # from them, and before anything is drawn
    arrangements, earned = (arrangements_of(ids, jud, raw0, tabs, picks, cov, flags, doc)
                            if brief else ({}, {}))
    resolve_payload()

    def where_of(t, title):
        return f"'{title}'" if t["bare"] else f"'{title}' (tab '{t['title'] or '?'}')"

    def asked_of(s):
        b = raw.get(s)
        return str(b.get("asked")) if isinstance(b, dict) and b.get("asked") else lbl(s)

    panels, counts, covered, empty_sections, misfit = {}, {}, set(), [], []
    accounted = set()          # what sections chose, apart from what their prose mentioned
    for t in tabs:
        o, got_tab, chosen_tab = [h1], set(), set()
        if t["bare"]:
            if t["intent"]:
                # the page is named for the record; the intent is the arrangement's aim,
                # not a title - it is another task on the way, and it reads like one.
                o.append(f'<p class="purpose" dir="auto">{w["purpose"]}'
                         f'<b dir="auto">{html.escape(str(t["intent"]))}</b></p>')
        else:
            o.append(f'<p class="purpose" dir="auto">{w["occasion"]}'
                     f'<b dir="auto">{html.escape(t["occasion"] or t["title"])}</b></p>')
            if t["serves"]:
                o.append(f'<p class="sub" dir="auto">{w["serves"]}'
                         + " &middot; ".join(fx(s, asked_of(s)) for s in t["serves"]) + "</p>")
        o.append(f'<p class="sub" dir="{page_dir}">'
                 + w["purpose_sub"].format(record=w["tab_record"], n=len(ids)) + '</p>')
        # the decision this tab stands on, quietly: when it was decided, how many later
        # sessions the tab served, whose word it was taken on - and what contests it. Drawn
        # here and counted nowhere: a decision drawn on its tab is not a pick.
        mine = [v for v in sorted(arrangements) if t["key"] in arrangements[v]["keys"]]
        for v in mine:
            f = arrangements[v]
            bits = [w["decided"] + (fx(v, f["born"].isoformat()) if f["born"] else w["as"] + fx(v, lbl(v)))]
            if f["stood"]:
                bits.append(counted(w, "stood", f["stood"], lang))
            if f["request"] and f["request"] in E:
                request = f["request"]
                bits.append(w["word"] + f'<span class="fx" data-id="{html.escape(request)}" '
                            f'data-request="{html.escape(request)}">{html.escape(asked_of(request))}</span>')
            o.append('<p class="sub" dir="auto">' + " &middot; ".join(bits) + "</p>")
            for kind, who, claim in f["contested"]:
                o.append(f'<p class="sub" dir="auto">{w["contests"].format(kind=w.get(kind, kind))}'
                         + (fx(who, P.short(claim, 120)) if who in E or who in J
                            else f'{html.escape(who)}: {html.escape(P.short(claim, 120))}') + "</p>")
        was = t["shape"] or {}
        if not was:
            o.append('<div class="banner">' + w["no_shape"] + '</div>')
        else:
            moved = [f"{w.get(k + '_count', w.get(k, k))}: {was[k]} &rarr; {shape[k]}" for k in shape if k in was and was[k] != shape[k]]
            if moved:
                plain = "; ".join(f"{k}: {was[k]} -> {shape[k]}" for k in shape
                                  if k in was and was[k] != shape[k])
                contract["stale"].append((f"tab '{t['title'] or '?'}'" if not t["bare"] else "the brief")
                                         + " recorded a different shape: " + plain)
                # a tab an arrangement decided reads the move by that arrangement's own
                # sign: fired, or muted - shown either way with the counts beside it as facts
                for v in mine:
                    arrangements[v]["moved"][t["title"] or "Now"] = plain
                p = cov["page"]
                facts = (w["spill_count"].format(n=p.get("page.spill", "-")) + " &middot; "
                         + w["unserved_count"].format(n=p["page.unserved"]) + " &middot; "
                         + w["drift"].format(n="-" if p["page.drift"] is None else p["page.drift"])
                         + (w["since"].format(d=cov["born"]) if p["page.drift"] is not None else "")
                         + "".join(" &middot; " + w["since_decided"].format(name=fx(v, lbl(v)), n=arrangements[v]["drift"])
                                   for v in mine if arrangements[v]["drift"] is not None))
                if mine and not any(arrangements[v]["fired"] for v in mine):
                    o.append('<div class="banner mut" dir="auto">' + w["shape_moved"]
                             + "; ".join(moved) + w["sign_absent"] + facts + '.</div>')
                else:
                    o.append('<div class="banner" dir="auto">' + w["shape_moved"] + "; ".join(moved)
                             + w["shape_moved_end"] + (" " + facts + "." if mine else "") + '</div>')
        for sec in t["sections"]:
            picked = sec.get("pick")
            picked = [picked] if isinstance(picked, str) else list(picked or [])
            got = set()
            for x in picked:
                got |= resolve(x, ids, jud, flags)
            text = str(sec.get("text") or "")
            # a section is text, picks, or both: what the text places counts as picked up
            got_tab |= got | {r for r in P.refs_in(text) if r in E or r in J}
            chosen_tab |= got
            jn = sorted(x for x in got if x in jud)
            en = sorted(x for x in got if x not in jud)
            title = str(sec.get("title") or ",".join(picked))
            kind = str(sec.get("as") or "").strip()
            if kind == "axis" and not text:
                contract["bad"].append(f"section '{title}': axis needs a written sequence in text")
            by = str(sec.get("by") or "")
            reading_by[0] = by if (by in schemes or by == "prefix" or carried(by)) else default_scheme
            wrong = fits(kind, sorted(got), jud, E, groups_of) if kind and got else None
            if wrong:
                misfit.append((where_of(t, title), wrong))
                kind = ""
            if (picked and not got) or not (picked or text):
                empty_sections.append(where_of(t, title))
            o.append(f'<div data-component="{html.escape(kind or ("cards" if jn else "table"))}"'
                     f' data-why="{"present" if sec.get("why") else ""}">')
            o.append(f'<h2 dir="auto">{html.escape(title)}'
                     + (f' <span class="n">{len(got)}</span>' if picked else "") + "</h2>")
            if sec.get("why"):
                o.append(f'<div class="why" dir="auto">{html.escape(str(sec["why"]))}</div>')
            if text:
                o.append(r_text(sec)[0])
            if wrong:
                message = fits(str(sec.get("as") or ""), sorted(got), jud, E, groups_of, w, lbl)
                o.append('<div class="bad">' + html.escape(message) + w["fell_back"] + '</div>')
            if picked and not got:
                o.append('<div class="why">' + w["section_empty"] + '</div>')
            if kind in GROUPED_SHAPES and (jn or en):
                o.append(r_grouped(sorted(got)))
            elif jn:
                o.append((JUD_R.get(kind) or r_cards)(jn))
            if en and kind not in GROUPED_SHAPES:
                o.append((ENTRY_R.get(kind) or r_table)(en))
            o.append('</div>')
        covered |= got_tab
        accounted |= chosen_tab
        panels[t["key"]], counts[t["key"]] = o, len(got_tab)
    # It may order. It may not drop. This section is not optional and the brief cannot
    # switch it off: an arrangement that hides what it did not anticipate is worth less
    # than no arrangement. Flagged judgments nothing picked up, and what a session wrote
    # for an intent no tab serves - counted once for the page, drawn on every tab, since a
    # reader opens a tab and not the page.
    reading_by[0] = default_scheme
    spill = sorted(k for k, f in flags.items() if f and k not in accounted)
    # each id once: a judgment both flagged and written for an unserved intent is the alert,
    # and what two unserved intents share is drawn under the newer
    loose, drawn = [], set(spill)
    for r in (cov["rows"] if cov else []):
        ks = [k for k in r["recorded"] if k not in covered and k not in drawn] if r["unserved"] else []
        if ks:
            loose.append((r, ks))
            drawn |= set(ks)
    if spill or loose:
        n = len(drawn)
        tail = [f'<h2 class="spill" dir="{page_dir}">{w["spill"]} <span class="n">{n}</span></h2>'
                '<div class="why">' + w["spill_loose" if loose else "spill_why"] + '</div>']
        if spill:
            tail.append(r_alerts(spill))
        for r, ks in loose:
            tail.append('<div class="why" dir="auto">' + w["written_for"].format(
                intent=fx(r["id"], r["asked"])) + '</div>')
            jn = [k for k in ks if k in jud]
            en = [k for k in ks if k not in jud]
            if jn:
                tail.append(r_cards(jn))
            if en:
                tail.append(r_table(en))
        for key in panels:
            panels[key] = panels[key] + tail

    if brief:
        # every grouping scheme must group something, and a section that reads by a scheme
        # must name one the brief declares - or one the record carries by its own shape
        for sname, gs in schemes.items():
            for gname, members in gs.items():
                if not members:
                    contract["stale"].append(f"group '{gname}'"
                                             + (f" (scheme '{sname}')" if len(schemes) > 1 else "")
                                             + " picks nothing")
        all_secs = [s for t in tabs for s in t["sections"]]
        for sec in all_secs:
            by = sec.get("by")
            if by and str(by) not in schemes and str(by) != "prefix" and not carried(str(by)):
                contract["bad"].append(f"section '{sec.get('title') or '?'}' reads by '{by}', "
                                       f"which is not a scheme the brief declares"
                                       + (f" (declared: {', '.join(schemes)})" if schemes else "")
                                       + " nor a field an entry carries")
        for t in tabs:
            tname = t["title"] or "?"
            # a tab serves intents, and an intent is a session source: the one entry shape
            # that carries what was asked
            for s in t["serves"]:
                if s not in E or not (raw.get(s) or {}).get("asked"):
                    contract["bad"].append(f"tab '{tname}' serves {s}, which is not a session "
                                           f"source - one carries what was asked")
        # serving is earned by picks: a tab that declares an intent and shows nothing the
        # session wrote for it makes a claim the page cannot keep
        for r in cov["rows"]:
            for tname in r["unearned"]:
                contract["bad"].append(f"tab '{tname}' serves {r['id']} and picks nothing it "
                                       f"recorded - serving is earned by picks"
                                       + (" - it recorded nothing" if r["empty"] else ""))
        contract["coverage"] = coverage_lines(cov)
        for sec in all_secs:
            title = str(sec.get("title") or "?")
            seen = sec.get("seen") or {}
            if sec.get("text"):
                refs = P.refs_in(sec["text"])
                for r in refs:
                    if r not in E and r not in J:
                        contract["bad"].append(f"section '{title}': text references {r}, which "
                                               f"is not an entry")
                # a text with no snapshot can never be told it went stale; one with a
                # snapshot that skips a reference was never read against that reference
                if refs and not seen:
                    contract["unread"].append(f"section '{title}': its text carries no seen, so "
                                              f"nothing can tell when it goes stale - review "
                                              f"\"{title}\" writes one")
                else:
                    for r in refs:
                        if (r in E or r in J) and r not in seen:
                            contract["unread"].append(f"section '{title}': text references {r}, "
                                                      f"which its seen does not carry - never "
                                                      f"read against it")
                # a judgment placed for its reasoning that has none is drawn as its verdict:
                # the sentence a reader meets is then the record's, not the author's
                for r in refs:
                    if r in J and not J[r]["because"]:
                        contract["unread"].append(f"section '{title}': places {r} for its "
                                                  f"reasoning, which it does not carry - its "
                                                  f"verdict is drawn in that sentence instead")
            # the snapshot under a text is compared the way check compares a judgment's:
            # a referenced value that moved since the text was read is said, never failed
            for k in seen:
                if k not in E and k not in J:
                    contract["bad"].append(f"section '{title}': seen names {k}, which is not "
                                           f"an entry")
            for k, (old, now) in sorted(moved_in(sec).items()):
                half, was, is_ = P.which_moved(old, now)
                contract["moved"].append(f"section '{title}': its text saw "
                                         + (f"{half} of {k}" if half else f"{k}")
                                         + f" = {P.short(was, 60)}, now {P.short(is_, 60)} - read "
                                           f"it again, then: review \"{title}\"")

    # ── the record's own tab ─────────────────────────────────────────────────
    rec_html = []
    if jud:
        rec_html.append(f'<h2>{w["judgments"]} <span class="n">{len(jud)}</span></h2>')
        rec_html.append(cards(sorted(jud)))
    # a prefix the legend names is shown by its word, the letter one hover away
    def prefix_heading(g):
        word = legend.get(g)
        return (f' title="{html.escape(g)}."' if word else "", html.escape(word or g))

    for g, keys in sorted(prefixes.items(), key=lambda kv: (-len(kv[1]), kv[0])):
        hover, word = prefix_heading(g)
        rec_html.append(f'<h2 id="g-{html.escape(g)}"{hover}>{word}</h2>' + r_table(keys))

    # ── the page ─────────────────────────────────────────────────────────────
    ns = '<nav class="ns" dir="ltr">' + "".join(
        f'<a href="#g-{html.escape(g)}"{prefix_heading(g)[0]}>{prefix_heading(g)[1]} ({len(v)})</a>'
        for g, v in sorted(prefixes.items(), key=lambda kv: (-len(kv[1]), kv[0]))) + "</nav>"
    head = [h1]
    if meta.get("scope"):
        head.append(f'<p class="scope" dir="auto">{html.escape(str(meta["scope"]).strip())}</p>')
    head.append(f'<p class="meta" dir="{page_dir}">' + w["counts"].format(e=shape["entries"], j=shape["judgments"])
                + (w["need_person"].format(n=shape["flagged"]) if shape["flagged"] else "")
                + (w["updated"].format(d=html.escape(str(meta["updated"]))) if meta.get("updated") else "")
                + "</p>" + ns)

    panel_markup = "".join(part for panel in panels.values() for part in panel)
    has_clocks = 'data-countdown="' in panel_markup or 'data-day="' in panel_markup
    component_styles = component_page or 'class="alerts"' in panel_markup
    styles = CSS + (COMPONENT_CSS if component_styles else "")
    out = [f'<!doctype html><html lang="{html.escape(lang)}" dir="{html.escape(page_dir)}"><head><meta charset="utf-8">',
           '<meta name="viewport" content="width=device-width,initial-scale=1">',
           f'<title>{html.escape(str(brief.get("title") or rec_name or meta.get("scope") or w["tab_record"])[:60])}</title>',
           f'<style>{styles}</style></head><body><div class="wrap" dir="{html.escape(page_dir)}">']

    tree = (h1 + f'<p class="purpose" dir="{page_dir}">{w["tree_lede"]}</p>'
            '<div class="treewrap">' + tree_svg(ids, jud, E, J, flags, w, lbl) + '</div>'
            f'<p class="sub" dir="{page_dir}" style="margin-top:8px">{w["tree_legend"]}</p>')
    bar = ['<div class="tabs" role="tablist">']
    for i, t in enumerate(tabs):
        # a tab written as a tab carries its own name; a bare brief is "Now". The first is
        # the default: a brief exists, so the page opens on what the session chose.
        n = counts[t["key"]]
        bar.append(f'<button type="button" data-tab="{t["key"]}" aria-selected='
                   f'"{"true" if i == 0 else "false"}">' + html.escape(t["title"] or w["tab_now"])
                   + (f' <span class="n">{n}</span>' if n else "") + "</button>")
    bar.append(f'<button type="button" data-tab="record" aria-selected='
               f'"{"false" if brief else "true"}">{w["tab_record"]} <span class="n">{len(ids)}</span></button>')
    bar.append(f'<button type="button" data-tab="tree" aria-selected="false">{w["tab_tree"]}</button></div>')
    out.append("".join(bar))
    for i, t in enumerate(tabs):
        out.append(f'<section id="panel-{t["key"]}"{"" if i == 0 else " hidden"}>'
                   + "".join(panels[t["key"]]) + "</section>")
    out.append(f'<section id="panel-record"{" hidden" if brief else ""}>'
               + "".join(head + rec_html) + "</section>")
    out.append('<section id="panel-tree" hidden>' + tree + "</section>")

    chosen = ("" if not brief else w["footer_brief" if len(tabs) == 1 and tabs[0]["bare"] else "footer_tabs"]
              .format(now=w["tab_now"], record=w["tab_record"]))
    footer = []
    for field in ("truth", "elsewhere"):
        if brief.get(field):
            keys = brief[field] if isinstance(brief[field], list) else [brief[field]]
            links = []
            for key in keys:
                if isinstance(key, str) and key in ids:
                    dest = link_target(E.get(key) or {}, record_root, page_path)
                    label = fx(key, lbl(key))
                    links.append(f'<a href="{html.escape(dest, quote=True)}">{label}</a>' if dest else label)
                else:
                    contract["bad"].append(f'{field}: name a record entry, not unanchored footer prose')
            footer.append('<div>' + w[field] + ' &middot; '.join(links) + '</div>')
    out.append(f'<footer dir="{page_dir}">' + "".join(footer) + (w["snapshot"] + " " if has_clocks else "") + w["footer"] + chosen + '</footer>')
    # sorted keys, so two builds of an unchanged record are the same bytes - the one thing
    # a generated page is for is being diffed against the last one
    script_keys = ("dir", "concludes", "rests_on", "wrong_if", "blocked", "reopened_by", "because", "value",
                   "rule", "measure", "source", "at", "file", "url", "as_of", "used_by", "back", "tree_btn",
                   "tree_btn_title", "whole_tree", "asked")
    if has_clocks:
        script_keys += tuple(key for key in w if key == "today" or key.startswith(("days_left", "days_ago")))
    out.append("</div><script>window.__T=" + json.dumps({key: w[key] for key in script_keys}, ensure_ascii=False).replace("<", "\\u003c")
               + ";window.__E=" + json.dumps(_plain(E), ensure_ascii=False, sort_keys=True).replace("<", "\\u003c")
               + ";window.__J=" + json.dumps(_plain(J), ensure_ascii=False, sort_keys=True).replace("<", "\\u003c") + ";</script>")
    out.append(f"<script>{JS}</script>" + (f"<script>{DATE_JS}</script>" if has_clocks else "") + "</body></html>")
    return "\n".join(out), E, J, ids, {"shape": shape, "empty": empty_sections,
                                       "misfit": misfit, "brief": bool(brief), "anchored": tuple(anchored),
                                       "unnamed": sorted(k for k in E if not E[k].get("name")
                                                         and k not in labels),
                                       "covered": covered, "swollen": swollen,
                                       "flags": flags, "contract": contract,
                                       "language": declared_lang, "direction": page_dir,
                                       "tabs": [{"key": t["key"], "title": t["title"],
                                                 "bare": t["bare"], "serves": t["serves"],
                                                 "shape": t["shape"]} for t in tabs],
                                       "coverage": cov, "page": dict(cov["page"]) if cov else {},
                                       "arrangements": arrangements, "earned": earned}


# ── explicit core/v1 consumer seam ─────────────────────────────────────────────────

def _core_modules():
    """Import the captured consumer contracts without changing legacy imports.

    ``render_page.py`` is both an importable module and a directly executed script, so
    sibling-package imports have to work in both modes.  Keeping this lazy also means an
    ordinary page never initializes the core runtime or changes its compatibility route.
    """
    try:
        from .reasoning.context import CapturedAssessment
        from .reasoning import page_assessment, projection
        from .reasoning.snapshot import Snapshot
    except ImportError:
        # The source-tree CLI executes this file by path, while an installed CLI
        # imports it as ``kpopper.render_page``.  Import the containing package in
        # the former case so reasoning's parent-relative imports retain one identity.
        import importlib
        package = pathlib.Path(__file__).resolve().parent.name
        parent = str(pathlib.Path(__file__).resolve().parent.parent)
        if parent not in sys.path:
            sys.path.insert(0, parent)
        CapturedAssessment = importlib.import_module(
            package + '.reasoning.context').CapturedAssessment
        page_assessment = importlib.import_module(package + '.reasoning.page_assessment')
        projection = importlib.import_module(package + '.reasoning.projection')
        Snapshot = importlib.import_module(package + '.reasoning.snapshot').Snapshot
    return CapturedAssessment, page_assessment, projection, Snapshot


def _core_brief(content):
    """Decode already-captured brief bytes; never reopen the brief from this seam."""
    if content is None:
        return {}
    if isinstance(content, str):
        content = content.encode('utf-8')
    if not isinstance(content, bytes):
        raise ValueError('captured core page brief must be bytes, text, or None')
    try:
        value = yaml.safe_load(content.decode('utf-8')) or {}
    except (UnicodeDecodeError, yaml.YAMLError) as error:
        raise ValueError('invalid captured core page brief: ' + str(error)) from None
    if not isinstance(value, dict):
        raise ValueError('captured core page brief must be a mapping')
    return value


def _core_state_flags(node, view):
    """Compatibility selectors projected only from canonical v3 dimensions.

    These names exist solely so an old page brief can select a core projection.  They
    are page inputs, not canonical findings, and deliberately do not call the legacy
    ``flags`` or predicate evaluator.
    """
    result = set()
    state = node.get('state') if isinstance(node, dict) else {}
    state = state if isinstance(state, dict) else {}
    status = view.get('status') if isinstance(view, dict) else {}
    status = status if isinstance(status, dict) else {}
    body = node.get('body') if isinstance(node, dict) else {}
    fields = node.get('fields') if isinstance(node, dict) else {}
    body = body if isinstance(body, dict) else {}
    fields = fields if isinstance(fields, dict) else {}
    judgment = bool(fields.get('deps') and fields['deps'] in body)

    integrity = state.get('integrity') if isinstance(state.get('integrity'), dict) else {}
    issues = integrity.get('issues') if isinstance(integrity.get('issues'), list) else []
    issue_codes = {item.get('code') for item in issues if isinstance(item, dict)}
    if status.get('integrity') not in ('assessed', 'not_available') \
            or issue_codes.intersection({'missing_dependency', 'invalid_dependencies'}):
        result.add('broken')
    falsifier = status.get('falsifier') if isinstance(status.get('falsifier'), dict) else {}
    if falsifier.get('holds') is True:
        result.add('falsified')
    elif judgment and falsifier.get('status') == 'not_declared':
        result.add('no_predicate')
    elif falsifier.get('status') in ('unknown', 'error', 'unavailable'):
        result.add('unknown')
    basis = status.get('basis')
    if judgment and (basis not in ('assessed', 'not_applicable')
                     or issue_codes.intersection({'missing_snapshot', 'invalid_snapshot'})):
        result.add('unchecked')
    dependencies = state.get('basis', {}).get('dependencies', {}) \
        if isinstance(state.get('basis'), dict) else {}
    if isinstance(dependencies, dict) and any(
            isinstance(item, dict) and (item.get('comparison') == 'changed'
                                        or item.get('basis_comparison') == 'changed'
                                        or item.get('rule_changed') is True)
            for item in dependencies.values()):
        result.add('moved')
    blocked_field = next((name for name in ('blocked_on', 'blocked', 'waiting_for')
                          if body.get(name)), None)
    if blocked_field:
        result.add('blocked')
    computation = status.get('computation') if isinstance(status.get('computation'), dict) else {}
    if computation.get('status') in ('unknown', 'error', 'limit', 'unsupported_capability'):
        result.add('unknown')
    return result


def _core_select(selector, ids, judgments, flags):
    """Resolve the closed legacy page selector vocabulary over supplied projections."""
    if isinstance(selector, list):
        result = set()
        for item in selector:
            result.update(_core_select(item, ids, judgments, flags))
        return result
    if not isinstance(selector, str):
        return set()
    if selector == 'all':
        return set(ids)
    if selector == 'judgments':
        return set(judgments)
    if selector == 'flagged':
        return {identifier for identifier, states in flags.items() if states}
    if selector in STATES:
        return {identifier for identifier, states in flags.items() if selector in states}
    if selector in ids:
        return {selector}
    prefix = selector[:-1] if selector.endswith('.') else selector
    return {identifier for identifier in ids if identifier.startswith(prefix + '.')}


def _core_unresolved_selectors(selector, ids, judgments, flags):
    if isinstance(selector, list):
        return sorted({item for value in selector
                       for item in _core_unresolved_selectors(value, ids, judgments, flags)})
    if not isinstance(selector, str) or selector in {'all', 'judgments', 'flagged'} | set(STATES):
        return []
    return [] if _core_select(selector, ids, judgments, flags) else [selector]


def _core_label(identifier, body, labels):
    if identifier in labels:
        return str(labels[identifier])
    value = named(body) if isinstance(body, dict) else None
    return str(value) if value else human(identifier)


def core_build_from_context(context, brief_content=None, page_path=None, *, record_root=None):
    """Render one source-free core page from one retained captured assessment.

    This is the reviewed cutover seam for the page consumer.  Canonical truth comes
    only from ``context.assessment`` and its normalized ``context.view``.  Brief bytes,
    selection, arrangement and coverage are bound in ``page-secondary/v1`` and can
    change only that revision.  This function performs no source I/O and no evaluation.
    """
    CapturedAssessment, PAGE, PROJECTION, _ = _core_modules()
    if not isinstance(context, CapturedAssessment):
        raise ValueError('core page requires a CapturedAssessment')
    assessment = context.assessment
    view = context.view
    snapshot = context.snapshot.to_data()
    if assessment.get('assessment_profile') != 'core/v1' \
            or view.get('snapshot_id') != context.snapshot_id \
            or view.get('findings_revision') != context.findings_revision:
        raise ValueError('core page requires one matching canonical v3 context')
    brief = _core_brief(brief_content)
    brief_identity = PAGE.capture_brief(brief_content,
                                        operational_limits=assessment.get('operational_limits'))
    nodes = assessment['nodes']
    ids = set(nodes)
    labels = brief.get('labels') if isinstance(brief.get('labels'), dict) else {}
    document = snapshot['document']
    meta = document.get('meta') if isinstance(document.get('meta'), dict) else {}
    lang0, page_dir = language(document), direction(document)
    lang = lang0 if lang0 in WORDS else 'en'
    words = WORDS[lang]
    judgments = {
        identifier for identifier, node in nodes.items()
        if isinstance(node.get('body'), dict) and isinstance(node.get('fields'), dict)
        and node['fields'].get('deps') in node['body']
    }
    flags = {identifier: _core_state_flags(nodes[identifier], view['nodes'][identifier])
             for identifier in sorted(ids)}
    current_shape = {'entries': len(ids - judgments), 'judgments': len(judgments),
                     'flagged': sum(bool(states) for states in flags.values()),
                     'blocked': sum('blocked' in states for states in flags.values())}
    raw_bodies = {identifier: (node['body'] if isinstance(node.get('body'), dict)
                               else {'v': node.get('body')})
                  for identifier, node in nodes.items()}

    display = {}
    root = pathlib.Path(record_root).resolve() if record_root else None
    for identifier in sorted(ids):
        node, projected = nodes[identifier], view['nodes'][identifier]
        body = node['body'] if isinstance(node.get('body'), dict) else {'v': node.get('body')}
        value = projected['status']['computation'].get('value_text')
        rule = body.get('rule')
        rule_text = None
        if rule is not None:
            try:
                rule_text = PROJECTION.render_expression(rule)
            except (ValueError, TypeError, SyntaxError, RecursionError):
                rule_text = 'unsupported expression'
        href = link_target(body, root, page_path) if root else None
        body_deps = body.get(node.get('fields', {}).get('deps')) \
            if isinstance(node.get('fields'), dict) else None
        dependencies = sorted(set(
            ([item for item in body_deps if isinstance(item, str)]
             if isinstance(body_deps, list) else [])
            + [item['id'] for item in projected.get('dependencies', [])
               if isinstance(item, dict) and isinstance(item.get('id'), str)]))
        display[identifier] = {
            'id': identifier,
            'kind': 'judgment' if identifier in judgments else 'entry',
            'label': _core_label(identifier, body, labels),
            'value': value,
            'rule': rule_text,
            'status': projected['status'],
            'status_text': projected['status_text'],
            'dependencies': dependencies,
            'flags': sorted(flags[identifier]),
            'href': href,
        }

    tabs = tabs_of(brief) if brief else []
    arrangements, selected, unresolved_selectors = [], set(), set()
    renderer_misfits, stale_shapes = [], []
    for tab in tabs:
        recorded_shape = tab.get('shape')
        if recorded_shape:
            if not isinstance(recorded_shape, dict):
                stale_shapes.append((tab['title'] or 'Now') + ': invalid recorded shape')
            else:
                moved = [key + ': ' + str(recorded_shape[key]) + ' -> ' + str(current_shape[key])
                         for key in current_shape if key in recorded_shape
                         and recorded_shape[key] != current_shape[key]]
                if moved:
                    stale_shapes.append((tab['title'] or 'Now') + ': ' + '; '.join(moved))
        sections = []
        for section in tab['sections']:
            selector = section.get('pick')
            chosen = sorted(_core_select(selector, ids, judgments, flags))
            unresolved_selectors.update(
                _core_unresolved_selectors(selector, ids, judgments, flags))
            kind = str(section.get('as') or '').strip()
            wrong = fits(kind, chosen, judgments, raw_bodies) if kind and chosen else None
            if wrong:
                renderer_misfits.append(str(section.get('title') or selector or '?') + ': ' + wrong)
            selected.update(chosen)
            sections.append({'title': str(section.get('title') or ''),
                             'why': str(section.get('why') or ''),
                             'shape': kind or 'cards',
                             'ids': chosen})
        arrangements.append({'key': tab['key'], 'title': tab['title'],
                             'occasion': tab['occasion'], 'serves': list(tab['serves']),
                             'sections': sections})
    flagged = {identifier for identifier, states in flags.items() if states}
    spill = sorted(flagged - selected) if tabs else []
    coverage = {'picked': sorted(selected), 'flagged': sorted(flagged),
                'spill': spill, 'covered_count': len(selected), 'spill_count': len(spill)}
    coverage['unresolved_selectors'] = sorted(unresolved_selectors)
    coverage['renderer_misfits'] = sorted(renderer_misfits)
    coverage['stale_shapes'] = sorted(stale_shapes)
    page_values = {
        'consumer_view_version': view['version'],
        'title': str(brief.get('title') or meta.get('name') or meta.get('scope') or 'record'),
        'language': lang0, 'direction': page_dir,
        'nodes': [display[identifier] for identifier in sorted(display)],
        'arrangements': arrangements, 'coverage': coverage,
    }
    bound = PAGE.capture_page_inputs(
        assessment, page_values, operational_limits=assessment.get('operational_limits'))
    page_assessment = PAGE.build(
        assessment, brief_identity, bound,
        operational_limits=assessment.get('operational_limits'))
    page_assessment = PAGE.validate(
        page_assessment, assessment,
        operational_limits=assessment.get('operational_limits'))

    def state_rows(item):
        status = item['status']
        computation = status['computation']['status']
        return ''.join(
            f'<li data-state-dimension="{html.escape(key)}"><span>{html.escape(key)}</span> '
            f'{html.escape(str(value))}</li>'
            for key, value in (
                ('acceptance', status['acceptance']), ('computation', computation),
                ('basis', status['basis']), ('falsifier', status['falsifier']['status']),
                ('contention', status['contention']), ('integrity', status['integrity']),
                ('coverage', status['coverage']), ('assurance', status['assurance']),
                ('support', status['support']['status'] +
                 ((' [' + ', '.join(status['support']['states']) + ']')
                  if status['support']['states'] else ''))))

    def card(identifier):
        item = display[identifier]
        heading = html.escape(item['label'])
        if item['href']:
            heading = f'<a href="{html.escape(item["href"], quote=True)}">{heading}</a>'
        value = item['value']
        rendered = (f'<div class="core-value" data-value-kind="typed">{html.escape(value)}</div>'
                    if value is not None else
                    f'<div class="core-rule">= {html.escape(item["rule"])}</div>'
                    if item['rule'] else '<div class="core-value unavailable">unavailable</div>')
        dependencies = ''.join(
            f'<code class="core-dependency">{html.escape(dependency)}</code>'
            for dependency in item['dependencies'])
        return (f'<article class="core-node {item["kind"]}" data-id="{html.escape(identifier)}">'
                f'<h3>{heading}</h3>{rendered}<ul class="core-state">{state_rows(item)}</ul>'
                + (f'<div class="core-dependencies">{dependencies}</div>' if dependencies else '')
                + '</article>')

    title = page_values['title']
    output = [
        '<!doctype html>',
        f'<html lang="{html.escape(lang)}" dir="{html.escape(page_dir)}"><head><meta charset="utf-8">',
        '<meta name="viewport" content="width=device-width,initial-scale=1">',
        '<meta name="kpopper-assessment-profile" content="core/v1">',
        f'<meta name="kpopper-snapshot-id" content="{context.snapshot_id}">',
        f'<meta name="kpopper-findings-revision" content="{context.findings_revision}">',
        f'<meta name="kpopper-page-assessment-revision" content="{page_assessment["page_assessment_revision"]}">',
        f'<title>{html.escape(title[:60])}</title>',
        '<style>body{font-family:system-ui,sans-serif;margin:0;background:#f7f7f5;color:#20201e}'
        '.core-wrap{max-width:960px;margin:auto;padding:32px}.core-meta{font-family:monospace;font-size:12px;overflow-wrap:anywhere}'
        '.core-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:12px}'
        '.core-node{background:white;border:1px solid #ddd;border-radius:10px;padding:14px}.core-node h3{margin:0 0 10px}'
        '.core-value,.core-rule{font-size:1.1rem;overflow-wrap:anywhere}.unavailable{color:#777}.core-state{padding-left:20px;font-size:13px}'
        '.core-state span{font-weight:600}.core-dependency{display:inline-block;margin:2px;padding:2px 5px;background:#eee;border-radius:4px}'
        '.core-arrangement{margin:28px 0}.core-section{margin:18px 0}'
        '@media(prefers-color-scheme:dark){body{background:#181817;color:#eee}.core-node{background:#242422;border-color:#555}'
        '.core-dependency{background:#383835}a{color:#8fc7ff}}'
        '@media(prefers-reduced-motion:reduce){*{animation:none!important;transition:none!important;scroll-behavior:auto!important}}</style></head>',
        f'<body data-profile="core/v1" data-snapshot-id="{context.snapshot_id}" '
        f'data-findings-revision="{context.findings_revision}" '
        f'data-page-assessment-revision="{page_assessment["page_assessment_revision"]}">',
        f'<main class="core-wrap"><h1>{html.escape(title)}</h1>',
        f'<p class="core-meta">snapshot {context.snapshot_id} · findings {context.findings_revision} '
        f'· page {page_assessment["page_assessment_revision"]}</p>',
    ]
    for tab in arrangements:
        output.append(f'<section class="core-arrangement" data-page-tab="{html.escape(tab["key"])}">'
                      f'<h2>{html.escape(tab["title"] or words["tab_now"])}</h2>')
        if tab['occasion']:
            output.append(f'<p>{html.escape(tab["occasion"])}</p>')
        for section in tab['sections']:
            output.append(f'<section class="core-section"><h3>{html.escape(section["title"])}</h3>')
            if section['why']:
                output.append(f'<p>{html.escape(section["why"])}</p>')
            output.append('<div class="core-grid">' + ''.join(card(identifier)
                                                               for identifier in section['ids']) + '</div></section>')
        output.append('</section>')
    if spill:
        output.append('<section class="core-arrangement" data-page-spill="true"><h2>Flagged outside the arrangement</h2>'
                      '<div class="core-grid">' + ''.join(card(identifier) for identifier in spill)
                      + '</div></section>')
    output.append(f'<section class="core-record"><h2>{html.escape(words["tab_record"])}</h2>'
                  '<div class="core-grid">' + ''.join(card(identifier) for identifier in sorted(ids))
                  + '</div></section>')
    encoded_assessment = json.dumps(page_assessment, ensure_ascii=False, sort_keys=True,
                                    separators=(',', ':')).replace('<', '\\u003c')
    output.append('<script type="application/json" id="kpopper-page-assessment">'
                  + encoded_assessment + '</script></main></body></html>')

    entries = {identifier: display[identifier] for identifier in sorted(ids - judgments)}
    decisions = {identifier: display[identifier] for identifier in sorted(judgments)}
    shape = current_shape
    info = {'profile': 'core/v1', 'shape': shape, 'brief': bool(brief),
            'flags': flags, 'coverage': coverage,
            'page': {'page.covered': len(selected), 'page.spill': len(spill)},
            'snapshot_id': context.snapshot_id, 'findings_revision': context.findings_revision,
            'page_assessment_revision': page_assessment['page_assessment_revision'],
            'page_assessment': page_assessment, 'tabs': arrangements,
            'notes': ['core/v1 generic page seam: arrangement changes only page-secondary/v1']}
    return '\n'.join(output), entries, decisions, ids, info


def core_build(paths, brief_path=None, page_path=None, *, read_mode=None, context=None):
    """Capture once, assess v3 once, then hand the retained context to the pure seam."""
    CapturedAssessment, _, _, Snapshot = _core_modules()
    selected = list(paths)
    if context is None:
        context = (CapturedAssessment.capture(selected, policy='focused-review/v1')
                   if read_mode is None else CapturedAssessment.from_snapshot(
                       Snapshot.capture(selected, read_mode=read_mode),
                       policy='focused-review/v1'))
    chosen = P.brief_for(selected, brief_path)
    brief_content = pathlib.Path(chosen).read_bytes() if chosen and os.path.exists(chosen) else None
    root = pathlib.Path(selected[0]).absolute().parent if selected else None
    return core_build_from_context(context, brief_content, page_path, record_root=root)


def core_verify(paths, brief_path=None, *, read_mode=None, context=None):
    """Verify the explicit core page and its bound secondary revision without rebuilding truth."""
    page, entries, judgments, ids, info = core_build(
        paths, brief_path, read_mode=read_mode, context=context)
    fail = []
    dom_ids = set(re.findall(r'data-id="([^"]+)"', page))
    if dom_ids != set(ids):
        missing, extra = set(ids) - dom_ids, dom_ids - set(ids)
        if missing:
            fail.append('core page omits: ' + ', '.join(sorted(missing)))
        if extra:
            fail.append('core page invents: ' + ', '.join(sorted(extra)))
    for field in ('snapshot_id', 'findings_revision', 'page_assessment_revision'):
        if str(info[field]) not in page:
            fail.append('core page omits ' + field)
    if info['coverage'].get('unresolved_selectors'):
        fail.append('core page selectors unresolved: '
                    + ', '.join(info['coverage']['unresolved_selectors']))
    for item in info['coverage'].get('renderer_misfits', []):
        fail.append('core page renderer does not fit: ' + item)
    for item in info['coverage'].get('stale_shapes', []):
        fail.append('core page shape moved: ' + item)
    embedded = re.search(
        r'<script type="application/json" id="kpopper-page-assessment">(.*?)</script>',
        page, flags=re.S)
    try:
        payload = json.loads(embedded.group(1)) if embedded else None
    except (ValueError, TypeError):
        payload = None
    if payload != info['page_assessment']:
        fail.append('core page does not embed its exact page-secondary/v1 envelope')
    for dimension in ('acceptance', 'computation', 'basis', 'falsifier',
                      'contention', 'integrity', 'coverage', 'assurance'):
        if f'data-state-dimension="{dimension}"' not in page:
            fail.append('core page omits v3 state dimension ' + dimension)
    for item in info['notes']:
        print('NOTE', item)
    for item in fail:
        print('FAIL', item)
    print(f"{len(dom_ids)} elements, {len(entries)} entries, {len(judgments)} judgments, "
          f"{len(info['tabs']) + 1} tabs, {len(fail)} problems")
    return 1 if fail else 0


def measured_build(paths, brief_path=None, page_path=None, *, profile=None, context=None):
    """Explicit page construction publishes counts for the exact inputs it read."""
    if profile == 'core/v1':
        # The core page has a page-secondary/v1 revision over its exact brief and
        # derived values.  It deliberately does not publish that presentation back
        # into the legacy page-count cache during the dormant T4 route.
        return core_build(paths, brief_path, page_path, context=context)
    if profile is not None:
        raise ValueError('unsupported page profile: ' + str(profile))
    if LOADED_SOURCE_HASH != MEASUREMENTS.LOADED_CODE['render_page.py']:
        raise ValueError('loaded renderer changed; restart it before measuring')
    selected = list(paths)
    mode = _read_mode()
    paths = record_paths(selected, read_mode=mode)
    brief_path = P.brief_for(paths, brief_path)
    before = MEASUREMENTS.snapshot(selected, brief_path)
    result = build(paths, brief_path, page_path, read_mode=mode)
    if _read_mode() != mode or record_paths(selected, read_mode=mode) != paths:
        raise ValueError('selected record or read mode changed during page measurement; build again')
    try:
        MEASUREMENTS.publish(before, result[4]['page'])
    except OSError as error:
        # Rendering remains useful on a read-only host; a missing cache stays
        # unavailable to followups and must not be mistaken for a saved result.
        sys.stderr.write('NOTE page measured, but its followup snapshot could not be saved: ' + str(error) + '\n')
    return result


def verify(paths, brief_path=None, *, profile=None, context=None):
    """Deterministic, no browser. What only looking can catch is a separate job."""
    if profile == 'core/v1':
        return core_verify(paths, brief_path, context=context)
    if profile is not None:
        raise ValueError('unsupported page profile: ' + str(profile))
    page, E, J, ids, info = measured_build(paths, brief_path)
    fail, note = lint_output(page, E, J, info)
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
    for name, j in J.items():
        for d in j["deps"]:
            if d not in E and d not in J:
                (note if j["blocked"] else fail).append(
                    f"{name} links to {d}, which the payload does not carry"
                    + (" - declared, so the page shows it as awaited" if j["blocked"] else ""))
    if "window.__E=" not in page or "window.__J=" not in page:
        fail.append("payload missing")
    # two ways past the budget, and they ask different things of an author: prose written
    # long is cut and marked, prose that only overruns once the record resolves into it is
    # drawn whole - what is long there is what the reasoning names, not what it says.
    for kind, said in (("cut", f"longer than the {CARD_CHARS} characters a card carries; each "
                               f"is drawn to its last whole word and marked"),
                       ("resolved", f"within {CARD_CHARS} characters as written and past them "
                                    f"once their references resolve; what each names is long, so "
                                    f"the card is drawn whole")):
        sw = sorted((x for x in info["swollen"] if x[2] == kind), key=lambda x: -x[1])
        if sw:
            note.append(f"{len(sw)} reasoning{'s' if len(sw) != 1 else ''} {said}. Longest first: "
                        + ", ".join(f"{n} ({c})" for n, c, _ in sw[:3])
                        + (f" and {len(sw) - 3} more" if len(sw) > 3 else ""))
    if info["brief"]:
        u = info["unnamed"]
        if u:
            note.append(f"{len(u)} entries carry no human name, so the page has to fall back to "
                        f"generic labels: {', '.join(u[:6])}"
                        + (f" and {len(u) - 6} more" if len(u) > 6 else ""))
        a, t = info["anchored"]
        if t:
            note.append(f"{a} of {t} live dependencies are named in the prose that cites them; "
                        f"the rest are reachable only by hovering the judgment")
        if 'data-tab="now" aria-selected="true"' not in page:
            fail.append("a brief exists but the session tab is not the default")
        c = info["contract"]
        fail += c["bad"]
        # the coverage report: facts the page counted, printed here and never acted on
        note += c["moved"] + c["stale"] + c["unread"] + c["coverage"]
        # a falsifier over a page count is decided here and nowhere else, so here is where
        # it fails
        for name, j in sorted(J.items()):
            if "falsified" in info["flags"].get(name, ()) and \
                    any(t in P.PAGE for t in P.predicate_refs(j["pred"])):
                fail.append(f"{name}: wrong_if holds ({j['pred']}) - decided by the page")
        # the arrangements held against the brief: a reversal made by editing the brief
        # fails, a tab no decision records is said, a muted move is said - and an arrangement
        # whose sign holds fails here whichever count it reads, since the page it is drawn on
        # says it fired
        af, an = arrangement_lines(info)
        fail += af
        note += an
        for v, f in sorted((info.get("arrangements") or {}).items()):
            if f["fired"] and not any(t in P.PAGE for t in P.ID.findall(f["pred"])):
                fail.append(f"{v}: wrong_if holds ({f['pred']}) - the arrangement fired")
        # An authored section that picks nothing is the alert row about something already
        # closed: it costs trust on everything else on the page.
        for w, why in info["misfit"]:
            fail.append(f"section {w}: {why}")
        for w in info["empty"]:
            fail.append(f"section {w} picks nothing - it is about something the record "
                        f"no longer holds")
        missed = [k for k, f in info["flags"].items() if f and
                  f'data-id="{k}"' not in dom]
        for k in missed:
            fail.append(f"{k} is flagged but does not appear on the Now tab at all")
    for n in note:
        print("NOTE", n)
    for f in fail:
        print("FAIL", f)
    drawn = len(info["tabs"]) + 1
    print(f"{len(shown)} elements, {len(E)} entries, {len(J)} judgments, "
          f"{drawn} tab{'s' if drawn != 1 else ''}, {len(fail)} problems")
    return 1 if fail else 0


if __name__ == "__main__":
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", newline="\n")
    supplied, a, page_path, i = sys.argv[1:], [], None, 0
    while i < len(supplied):
        if supplied[i] == "--page-out":
            if i + 1 >= len(supplied):
                sys.exit("--page-out needs a path")
            page_path, i = supplied[i + 1], i + 2
        else:
            a.append(supplied[i]); i += 1
    if '--frozen' in a:
        a.remove('--frozen')
        os.environ['KPOPPER_READ_MODE'] = 'frozen'
    profile = None
    if '--profile' in a:
        index = a.index('--profile')
        if index + 1 >= len(a):
            sys.exit('--profile needs a value')
        profile = a[index + 1]
        del a[index:index + 2]
        if profile != 'core/v1':
            sys.exit('unsupported page profile: ' + profile)
    brief = a[a.index("--brief") + 1] if "--brief" in a else None
    files = [x for x in a if x.endswith((".yaml", ".yml")) and x != brief] or P.default_paths()
    try:
        if "--verify" in a:
            sys.exit(verify(files, brief, profile=profile))
        page, _, _, _, info = measured_build(files, brief, page_path, profile=profile)
    except Exception as error:
        failure = getattr(error, 'envelope', None)
        if profile == 'core/v1' and isinstance(failure, dict):
            print(json.dumps(failure, ensure_ascii=False, sort_keys=True), file=sys.stderr)
            sys.exit(2)
        raise
    for t in ([] if profile == 'core/v1' else info["tabs"]):
        if t["shape"]:
            continue
        if t["bare"]:
            sys.stderr.write("# no shape recorded in the brief. paste this into it, so a later\n"
                             "# render can tell you the arrangement went stale:\nshape:\n"
                             + "".join(f"  {k}: {v}\n" for k, v in info["shape"].items()))
        else:
            sys.stderr.write(f"# no shape recorded for tab '{t['title']}'. paste this into it, so a "
                             "later\n# render can tell you the arrangement went stale:\n    shape: {"
                             + ", ".join(f"{k}: {v}" for k, v in info["shape"].items()) + "}\n")
    sys.stdout.write(page)
