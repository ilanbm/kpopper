#!/usr/bin/env python3
"""Consolidation is a test: the record with its hypotheses laid over it, checked - and then,
when the check is clean, the fold.

  python3 consolidate.py [--dry-run] [<hypothesis> ...] [file]   the union, tested; then written
  python3 consolidate.py --refute <hypothesis> "<why>" [--as s.<source>] [file]
  python3 consolidate.py --from <ref> [--dry-run] [<hypothesis> ...] [file]
  python3 consolidate.py pull <seed> [...] --from <ref> [--budget N] [file]

The dry run lays the hypotheses named - every one beside the record, when none is - over the
base by id, in name order, and runs the reader's own check on the result: what would arrive,
what a hypothesis replaces and what rests on that, what the union moves or breaks, what is
contested. It writes nothing. The fold writes the same union into the base with the reader's
own edits, deletes the folded files, and prints what to commit - only when the dry run is
clean. Every replacement passes the one door a written value passes, `may_supersede`,
asked with the base's own readings: a reading the door refuses is contested, and the base
keeps what it holds; a verdict, or other grounds, laid over a standing judgment is
`reversed` - folded when the base's own condition has broken the judgment, or when a person
names the id with `--take`, and red until one does. What a fold replaces is kept beside the
record, with one line on the judgment.

`--refute` writes the hypothesis's claim into the base as a negative finding and deletes the
file; nothing else of it enters. `--from <ref>` reads another branch's committed record and lays
it over this base as hypotheses named after the ref - a pull, never a push.
"""
import contextlib, datetime, glob, io, os, posixpath, re, shlex, subprocess, sys, tempfile
import yaml

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import provenance as P  # noqa: E402
import sameness  # noqa: E402

NEVER = "never"


# ── what is read ─────────────────────────────────────────────────────────────
def hypothesis(name, doc, head=None, path=None, text=None):
    """A hypothesis in the reader's own shape, from a document held in memory: what a file
    beside the record would give, without the file. `doc` is a mapping of collections, as a
    record is; `head` the optional `hypothesis:` head - claim, wrong_if, born, folds; `text`
    the file's lines when there is a file, so the fold can carry its blocks over whole. A
    what-if is one of these with `folds: never`, evaluated and never written."""
    h = P._hypothesis(name, path)
    h["head"] = dict(head or {})
    h["doc"] = doc
    h["raw"] = P.bodies(doc)
    h["ids"] = {k for m in P.collections_of(doc).values() for k in m}
    h["text"] = text
    return h


def _view(doc):
    ids, jud, fields = P.infer(doc)
    return ids, jud, fields, P.with_builtins(doc, ids, jud, fields)


def read(paths, names=(), refs=()):
    """What consolidate reads -> (doc, hyps): the record as the reader loads it, and the
    hypotheses to lay over it in name order - those named, else every one beside the record
    and every one a ref brings. A name nothing holds, or a hypothesis the reader could not
    read, is refused before anything is tested."""
    doc = P.load(paths)
    pool = {}
    for n, h in doc.hypotheses.items():
        if h.get('kind') == 'contribution':
            continue  # publication acceptance/materialization is a separate explicit step
        if h["error"]:
            raise P.Refused(f"refused - hypothesis {n} could not be read: {h['error']}")
        got, why = P._layer_view(doc, h)
        if got is None:
            raise P.Refused(f"refused - hypothesis {n} cannot be read over the base: {why}")
        if h['path'].startswith('git:'):
            h['text'] = P.yaml.safe_dump(h['doc'], allow_unicode=True, sort_keys=False).split('\n')
        else:
            h["text"] = P._text_of_or_none(h["path"]).split("\n")
        pool[n] = h
    for ref in list(dict.fromkeys(refs)):
        for h in from_ref(paths, ref, doc):
            if h["name"] in pool:
                raise P.Refused(f"refused - {h['name']} names both a hypothesis beside the record and "
                                f"what {ref} holds; rename the file, or consolidate them one at a time")
            pool[h["name"]] = h
    if names:
        missing = [n for n in names if n not in pool]
        if missing:
            have = ", ".join(sorted(pool)) or "none"
            raise P.Refused(f"refused - no hypothesis named {', '.join(missing)} beside the record "
                            f"(there: {have})")
        chosen = [pool[n] for n in sorted(set(names))]
    else:
        chosen = [pool[n] for n in sorted(pool)]
    return doc, chosen


# ── the union, tested ────────────────────────────────────────────────────────
class Consolidation(object):
    """The base with hypotheses laid over it by id, in order, and what the reader's check says
    of the result beyond what it says of the base alone.

      doc, ids, jud, fields, raw  the union's view, as `provenance.view` gives it
      base                        the base's own (doc, ids, jud, fields, raw)
      hyps                        the hypotheses laid over, in order
      arrived                     [(id, hypothesis)] - ids the base does not hold
      updates                     [(id, hypothesis, old, new, may, why)] - ids the base holds
                                  that a hypothesis replaces, and what the door says of each
      refused                     the updates the door refused: the base keeps what it holds
      moved, falsified, holes     the check's own lines about the union, beyond the base's
      head_falsified              [(hypothesis, wrong_if)] - heads whose own falsifier holds
      contested                   {id: [(hypothesis, claim)]} - two hypotheses disagree
      reversed                    [(id, hypothesis, old body, new body, may, why)] - judgments
                                  the base holds that a hypothesis lays a verdict, or other
                                  grounds, over: by the base's own condition, by a person's
                                  name, or - untaken - waiting for one
      untaken                     the reversed ones nobody named: the run is red on them
      sourced                     [(id, hypothesis, why)] - readings from another source than
                                  the base's, with another value: refused until named
      drops_needed                {id: [dependency]} - what a replacement no longer rests on,
                                  each to be named with its reason at the fold (--drop)
      untakeable                  ids a name cannot take: a kind changed, an arrangement
                                  replaced by something that is not one
      new_subjects                prefixes the base does not hold
      candidates                  pairs for a person to judge as the same subject or distinct

    `red` is what makes the dry run fail: contested, refused, untaken, falsified, a hole;
    `blocked` is what stops the fold: red, or a premise that moved under a judgment."""

    def __init__(self):
        self.hyps, self.arrived, self.updates, self.refused = [], [], [], []
        self.moved, self.falsified, self.holes, self.head_falsified = [], [], [], []
        self.contested, self.new_subjects, self.candidates = {}, [], []
        self.reversed, self.untaken, self.sourced = [], [], []
        self.drops_needed, self.untakeable = {}, set()
        self.doc = self.base = None
        self.ids = self.jud = self.fields = self.raw = None

    @property
    def red(self):
        return bool(self.contested or self.refused or self.untaken or self.falsified or self.holes
                    or self.head_falsified)

    @property
    def blocked(self):
        return self.red or bool(self.moved)


def _meta_keys(doc):
    m = doc.get("meta")
    return set(m) if isinstance(m, dict) else set()


def _check_of(udoc):
    """The reader's check, run on a document held in memory -> (fail, moved): the union is
    written where nothing else sits - no brief, no hypotheses - and checked as a base is."""
    body = {k: v for k, v in udoc.items() if k not in ("record", "also")}
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, P.ENTRY)
        with io.open(p, "w", encoding="utf-8") as f:
            yaml.safe_dump(body, f, sort_keys=False, allow_unicode=True, width=1000)
        # This disposable reduction world is derived from already captured input.
        # Its short-lived files must not become live publication preconditions.
        token = P._DIRECT_STAGE.set(P._DirectStage())
        try:
            fail, _, moved, _, _ = P.check_lines([p])
        finally:
            P._DIRECT_STAGE.reset(token)
    return fail, moved


def _read_day(body, raw):
    """The day a hypothesis read a value - its own, else its source's, in the world the
    hypothesis makes."""
    return P._read_on(body, raw)


def page_of(paths):
    """What the brief holds every arrangement to, taken once before anything is tested - the
    same facts the write path asks its door with. Empty without a brief, or when the brief
    cannot be built: the fold never fails on the page's account."""
    try:
        return P._page_side(paths)[2]
    except (Exception, SystemExit):
        return {}


def union_of(doc, hyps, base_check=None, as_of=None, page=None, take=(), drops=None):
    """The union and its test -> Consolidation. `doc` is the record as `provenance.load` returns
    it; `hyps` the hypotheses to lay over it, in the order given - in name order, as the
    command does, unless the caller has a reason. `base_check` is the base's own (fail, moved)
    from `provenance.check_lines`, taken once by the caller; without it the base is checked
    here. `as_of` is the day the fold would happen and `page` what the brief holds every
    arrangement to, both handed to the one door so that a fold is refused where a write of
    the same body would be - twice in a day, or with the tab it cites cut. Nothing is
    written: a what-if asks this and reads the answer."""
    c = Consolidation()
    c.hyps = list(hyps)
    bids, bjud, bfields, braw = _view(doc)
    c.base = (doc, bids, bjud, bfields, braw)
    meta = _meta_keys(doc)
    # two hypotheses that hold one id with different claims: nothing decides it but a
    # person, and the union has no value to lay - the run stops here
    holders = P.Record()
    holders.hypotheses = {h["name"]: h for h in c.hyps}
    c.contested = P.contested(holders)
    if c.contested:
        return c
    udoc = doc
    for h in c.hyps:
        udoc = P.layered(udoc, h)
    udoc.hypotheses = {}
    c.doc = udoc
    c.ids, c.jud, c.fields, c.raw = _view(udoc)
    # what arrives, and what replaces what the base holds - each id's last holder in order
    held = {}
    for h in c.hyps:
        for k in sorted(h["ids"]):
            if k in meta:
                continue
            held[k] = h
    for k in sorted(held):
        h = held[k]
        new = h["raw"].get(k)
        if k not in bids:
            c.arrived.append((k, h))
            continue
        old = braw.get(k)
        old_claim, new_claim = P.claim_of(old), P.claim_of(new)
        same = P._same_claim(old_claim, new_claim)
        if same and old == new:
            continue
        if k in bjud:
            if same and isinstance(old, dict) and isinstance(new, dict) \
                    and P._version_core(old) == P._version_core(new) \
                    and not (P.is_arrangement(bjud[k], braw) and old.get("born") != new.get("born")):
                continue        # the same decision, its seen refreshed: a review does not travel
            # a subject does not change kind at the fold, and what replaces an arrangement is
            # an arrangement - and a judgment does not become one here, since an arrangement
            # is born when it is written: none of these is a person's to take by name
            if not isinstance(new, dict) or not P._judgment_shaped(new, bfields):
                may, why = False, ("the base holds a judgment under this id and the hypothesis an "
                                   "entry - a subject does not change kind at the fold, and no name "
                                   "takes this: give it what it rests on and a verdict, or a new id")
                c.untakeable.add(k)
            elif P.is_arrangement(bjud[k], braw) and not P._arrangement_shaped(new, bfields, braw):
                may, why = False, ("what replaces an arrangement is an arrangement - rest on the "
                                   "session sources of the occasion it decides and give it a sign "
                                   "over a count; no name takes a body that drops them")
                c.untakeable.add(k)
            elif not P.is_arrangement(bjud[k], braw) and P._arrangement_shaped(new, bfields, braw):
                may, why = False, ("the base holds a judgment under this id and the hypothesis an "
                                   "arrangement - an arrangement is born when it is written in place, "
                                   "and no name takes one over a judgment: write it as its own decision")
                c.untakeable.add(k)
            else:
                # the base's own condition, on the base's own readings: a hypothesis that
                # brings the reading that breaks a judgment and the verdict that repairs it
                # would have certified itself on the union, so the door is asked with what
                # the base holds. A verdict the base's own sign has not broken waits for a
                # person to take it by name - the day and the page still bind an arrangement
                may, why = P.may_supersede(k, old, new, braw, bids, bjud, bfields, as_of,
                                           page=page, by_hand=(k in take))
                if not may and bjud[k]["pred"] and P.evaluate(bjud[k]["pred"], c.raw, c.ids) is True:
                    why += (f" - it would hold with {h['name']}'s readings "
                            f"({P.short(bjud[k]['pred'], 60)})")
                # what the replacement no longer rests on is a decision with a reason, named
                # at the fold as it is named at a write - never an arrangement's count
                if may and not P.is_arrangement(bjud[k], braw):
                    gone = [d for d in P.dropped_deps(old, new, bfields) if d not in (drops or {})]
                    if gone:
                        c.drops_needed[k] = gone
            c.reversed.append((k, h, old, new, may, why))
            if not may:
                c.untaken.append((k, h, why))
        else:
            day = _read_day(new, c.raw) if isinstance(new, dict) else None
            base_day = P._read_on(old, braw) if isinstance(old, dict) else None
            today = P._as_day(as_of) or P.latest_today()
            kind_changed = isinstance(new, dict) and P._judgment_shaped(new, bfields)
            future = P._as_day(day) if day else None
            other = (not h.get("path") and isinstance(new, dict) and isinstance(old, dict)
                     and new.get("from") and old.get("from") and str(new["from"]) != str(old["from"])
                     and not same)
            if kind_changed:
                may, why = False, ("the base holds an entry under this id and the hypothesis a "
                                   "judgment - a subject does not change kind at the fold: set the "
                                   "entry, or give the judgment a new id")
            elif future and future > today:
                may, why = False, (f"a reading dated {day}, after today ({today.isoformat()}) - a day "
                                   f"is the record's clock, and a day ahead is not read")
            elif other:
                # across the branch line, two sources for one id that disagree are two
                # instruments, and no day orders them: one id follows one source, and a person
                # names which. A hypothesis beside the record is this checkout's own reading
                # of another source - a what-if - and keeps the day rule
                if k in take:
                    may, why = True, (f"a reading from {new['from']} where the base reads from "
                                      f"{old['from']} - taken by name")
                else:
                    may, why = False, (f"a reading from {new['from']} where the base reads from "
                                       f"{old['from']} - one id follows one source; take it by "
                                       f"name: consolidate {h['name']} --take {k}")
                    c.sourced.append((k, h, why))
            elif day is None and base_day is not None:
                # the door tells readings apart by the day; one nothing dates cannot be asked
                # about, and the day of the fold is not the day it was read
                may, why = False, f"the reading is undated, and the base's is from {base_day}"
            else:
                may, why = P.may_supersede(k, old, new_claim, braw, bids, bjud, bfields, day)
        if same and not may and k not in bjud:
            continue            # the same claim, read no later: the base already holds it
        c.updates.append((k, h, old_claim, new_claim, may, why))
        if not may and k not in bjud:
            c.refused.append((k, h, why))
    # the reader's own check on the union, beyond what it says of the base alone
    fail_b, moved_b = base_check if base_check is not None else _check_of(doc)
    fail_u, moved_u = _check_of(udoc)
    fl = P.flags(c.ids, c.jud, c.fields, c.raw)
    for line in fail_u:
        if line in fail_b:
            continue
        name = line.split(":", 1)[0]
        if "falsified" in fl.get(name, ()):
            c.falsified.append(line)
        else:
            m = re.search(r"rests on (\S+), which is not an entry", line)
            others = [n for n in (P._held_by(doc, m.group(1)) if m else [])
                      if n not in holders.hypotheses]
            if others:
                line += (f" - held by hypothes{'is' if len(others) == 1 else 'es'} "
                         f"{', '.join(others)}: consolidate them together")
            c.holes.append(line)
    c.moved = [l for l in moved_u if l not in moved_b]
    # a hypothesis's own falsifier, in its head, evaluated against the union
    for h in c.hyps:
        pred = str(h["head"].get("wrong_if") or "")
        if not pred:
            continue
        got = P.evaluate(pred, c.raw, c.ids)
        if got is True:
            c.head_falsified.append((h, pred))
        elif got is None:
            c.holes.append(f"{h['name']}: its wrong_if is not a comparison this reader decides "
                           f"({P.short(pred, 60)}) - a falsifier nothing evaluates tests nothing")
    # subjects the base does not hold: the first segment of what arrives
    have = {k.split(".")[0] for k in bids if "." in k}
    c.new_subjects = sorted({k.split(".")[0] for k, _ in c.arrived if "." in k} - have)
    # the pairs a person judges: what arrives, held against the base and against each other
    # by declared fields alone; a pair a distinct_from declared is not named
    c.candidates = sameness.candidate_lines(doc, c.hyps)[0]
    return c


# ── the report ───────────────────────────────────────────────────────────────
def _cut(line, n=110):
    return line if len(line) < n else line[:n] + " ..."


def _describe(k, body, raw, ids, fields, suffix=""):
    """An entry as `pull` would say it - its value, name, source and day - or a judgment's
    verdict; `body` is the reading described, `raw` the world it is read in."""
    width = 110 - len(suffix)
    if isinstance(body, dict) and fields.get("deps") and fields["deps"] in body:
        return _cut(f"+ {k}: {P._verdict_of(body) or k}", width) + suffix
    if not isinstance(body, dict):
        return _cut(f"{k}: {body}", width) + suffix
    v = P.value_of(raw, ids, k)
    if v is not None:
        shown = str(v)
    elif body.get("rule") or (isinstance(body.get("v"), str) and P.EXPR.search(body["v"])):
        shown = "= " + P.predicate_text(body.get("rule") or body.get("v"))
    elif body.get("quoted"):
        shown = '"' + str(body["quoted"]) + '"'
    else:
        shown = ""
    nm = P.named(body)
    line = f"{k}: {shown}" + (f" ({nm})" if nm else "")
    if body.get("from"):
        line += f" <- {body['from']}" + (f", at {body['at']}" if body.get("at") else "")
    of = body.get("of") or body.get("read")
    if of:
        line += f" as of {of}"
    return _cut(line, width) + suffix


def _side_lines(who, body, raw, ids, jud, fields):
    """One side of a reversal, for a person to read beside the other: its why and what it
    rests on - the verdict is on the line above."""
    out = []
    if not isinstance(body, dict):
        return out
    because = body.get("because")
    if because:
        out.append(_cut(f"    {who}: because: " + P.said(because, raw, ids, jud, 400)[0], 110))
    deps = body.get(fields["deps"]) if fields.get("deps") else None
    if isinstance(deps, list):
        out.append(_cut(f"    {who}: {fields['deps']}: [" + ", ".join(str(d) for d in deps) + "]", 110))
    pred = body.get("wrong_if")
    if pred:
        out.append(_cut(f"    {who}: wrong_if: {P.predicate_text(pred)}", 110))
    return out


def _head_line(h, today):
    bits = []
    when = h["head"].get("born")
    if when:
        bits.append(f"born {P._as_day(when) or when}, {P._age(when, today)}")
    if str(h["head"].get("folds") or "") == NEVER:
        bits.append("never folds")
    claim = h["head"].get("claim")
    head = f"  {h['name']}" + (f" ({', '.join(bits)})" if bits else "")
    return head + (": " + P.short(claim, 110 - len(head) - 2) if claim else "")


def _sources_of(k, h, doc, bids, fields):
    """One holder's reading of k, with its source, read in the world that holder makes: what
    a person compares side by side."""
    hraw = P.bodies(P.layered(doc, h))
    return _describe(k, h["raw"].get(k), hraw, set(bids) | h["ids"], fields)


def report(c, today=None):
    """The dry run's report, in fixed order -> lines. Arrived, updates, moved / falsified,
    contested, candidates, new subjects; then one line saying whether the fold may run."""
    today = today or datetime.date.today()
    doc, bids, bjud, bfields, braw = c.base
    names = [h["name"] for h in c.hyps]
    out = [f"the base with {', '.join(names)} laid over it"
           + (", in name order" if len(names) > 1 else "")]
    out += [_head_line(h, today) for h in c.hyps]
    out.append("")
    if c.contested:
        n = len(c.contested)
        out.append(f"contested ({n}): an id two hypotheses hold with different claims - the union "
                   f"has no value to lay, so the run stops here")
        for k, hs in c.contested.items():
            out.append(f"  {k}:")
            if k in bids:
                out.append("    the base holds " + _describe(k, braw.get(k), braw, bids, bfields))
            for name, claim in hs:
                h = next(x for x in c.hyps if x["name"] == name)
                out.append(f"    {name} says " + _sources_of(k, h, doc, bids, bfields))
        out.append("")
        out.append("re-read against the merged tree: pull each id to see every reading beside the "
                   "base's, then set what holds today - in the base with a later day, or in the "
                   "hypothesis that read it - or refute one, and consolidate again")
        return out
    ids, jud, fields, raw = c.ids, c.jud, c.fields, c.raw
    out.append(f"arrived ({len(c.arrived)}): what the fold would add")
    for k, h in c.arrived:
        out.append("  " + _describe(k, h["raw"].get(k), raw, ids, fields, f" - from {h['name']}"))
        if k in jud:
            out.append("    " + P._state_line(k, jud[k], raw, ids, fields))
    readings = [u for u in c.updates if u[0] not in bjud]
    out.append(f"updates ({len(readings)}): what the base holds that a hypothesis replaces, and "
               f"what rests on each")
    for k, h, old, new, may, why in readings:
        out.append(f"  {k}: {P.short(old, 36)} -> {P.short(new, 36)}, from {h['name']}")
        out.append(f"    {why}" + ("" if may else " - the base keeps what it holds"))
        hit, touched, derived = P.reach_of(ids, jud, raw, [k])
        if derived:
            out.append(_cut("    worked out from it: " + ", ".join(derived)))
        for name in sorted(hit):
            if name == k:
                continue
            out.append("    " + P._state_line(name, jud[name], raw, ids, fields, touched=touched))
    out.append(f"reversed ({len(c.reversed)}): a verdict, or other grounds, laid over a standing "
               f"judgment - by its own condition, by a person's name, or waiting for one")
    for k, h, old, new, may, why in c.reversed:
        ov, nv = P._verdict_of(old), P._verdict_of(new)
        head = f"  {k}: " + (f"{P.short(ov, 36)} -> {P.short(nv, 36)}" if not P._same(ov, nv)
                             else "the same verdict on other grounds") + f", from {h['name']}"
        out.append(head)
        if may and "by name" in why:
            out.append(f"    taken by name - {why}")
        elif may:
            out.append(f"    by its own condition - {why}")
        elif k in c.untakeable:
            out.append(f"    {why}")
        else:
            out.append(f"    {why} - take it by name: consolidate {h['name']} --take {k}")
        for d in c.drops_needed.get(k, []):
            out.append(f"    no longer rests on {d} - name the reason at the fold: --drop "
                       f"{shlex.quote(d + ': <why>')}")
        for who, body, world, world_ids, world_jud in (("base", old, braw, bids, bjud),
                                                       (h["name"], new, raw, ids, jud)):
            out += _side_lines(who, body, world, world_ids, world_jud, bfields)
        hit, touched, derived = P.reach_of(ids, jud, raw, [k])
        for name in sorted(hit):
            if name != k:
                out.append("    " + P._state_line(name, jud[name], raw, ids, fields, touched=touched))
    n = len(c.moved) + len(c.falsified) + len(c.holes) + len(c.head_falsified)
    out.append(f"moved / falsified ({n}): what the union moves or breaks")
    for line in c.falsified:
        out.append("  FALSIFIED " + line)
    for h, pred in c.head_falsified:
        out.append(f"  FALSIFIED {h['name']}: its own wrong_if holds ({pred})")
    for line in c.holes:
        out.append("  FAIL " + line)
    for line in c.moved:
        out.append("  MOVED " + line)
    out.append(f"contested ({len(c.refused)})"
               + (": the door refuses the reading, so the base keeps what it holds" if c.refused else ""))
    for k, h, why in c.refused:
        out.append(f"  {k}: {why}")
        out.append("    the base holds " + _describe(k, braw.get(k), braw, bids, bfields))
        out.append(f"    {h['name']} says " + _sources_of(k, h, doc, bids, bfields))
    if [r for r in c.refused if r[0] not in {s[0] for s in c.sourced}]:
        out.append("  read again on a later day - set it in the base or in the hypothesis with "
                   "--as-of - or refute the hypothesis")
    out.append(f"candidates ({len(c.candidates)}): pairs for a person to judge as the same subject "
               f"or distinct")
    for line in c.candidates:
        out.append("  " + str(line))
    out.append(f"new subjects ({len(c.new_subjects)}): prefixes the base does not hold")
    for p in c.new_subjects:
        held = sorted(k for k, _ in c.arrived if k.split(".")[0] == p)
        out.append(_cut(f"  {p}: " + ", ".join(held)))
    out.append("")
    foldable = [h["name"] for h in c.hyps if str(h["head"].get("folds") or "") != NEVER]
    if c.red or c.drops_needed:
        what = []
        if c.falsified or c.head_falsified:
            what.append("a falsifier holds")
        if c.holes:
            what.append("a hole")
        if c.refused:
            what.append("a contested reading")
        if c.untaken:
            what.append(f"{len(c.untaken)} reversal{'s' if len(c.untaken) != 1 else ''} to take by name")
        if c.drops_needed:
            what.append("a dropped dependency to name")
        other = bool(c.falsified or c.head_falsified or c.holes or c.refused or c.contested)
        out.append("not clean: " + ", ".join(what)
                   + (" - nothing folds until it is read again" if other
                      else " - a verdict the base's own condition has not broken folds only when a "
                           "person names it" if c.untaken
                      else " - a dependency dropped is a decision with a reason, named at the fold"))
        if c.untaken or c.drops_needed:
            by_hyp = {}
            for k, h, _ in c.untaken:
                if k not in c.untakeable:
                    by_hyp.setdefault(h["name"], []).append(f"--take {k}")
            for k, h, _, _, _, _ in c.reversed:
                for d in c.drops_needed.get(k, []):
                    by_hyp.setdefault(h["name"], []).append("--drop " + shlex.quote(d + ": <why>"))
            for name, args in by_hyp.items():
                out.append(f"  consolidate {name} " + " ".join(args))
    elif c.moved:
        out.append(f"moved: {len(c.moved)} judgment{'s' if len(c.moved) != 1 else ''} to re-review "
                   f"before the fold - a premise moved under {'them' if len(c.moved) != 1 else 'it'}")
    elif not foldable:
        out.append("clean - and nothing here folds: " + ", ".join(names)
                   + (" are what-ifs" if len(names) > 1 else " is a what-if")
                   + ", evaluated and never written")
    elif not c.arrived and not c.updates:
        out.append("clean - and nothing to write: the base already holds everything "
                   + ", ".join(foldable) + f" propose{'s' if len(foldable) == 1 else ''}; consolidate "
                   + " ".join(foldable) + " removes the file" + ("s" if len(foldable) > 1 else ""))
    else:
        out.append("clean: " + ", ".join(foldable) + " may fold - consolidate " + " ".join(foldable))
    return out


# ── the fold ─────────────────────────────────────────────────────────────────
def _renewed(k, c, h, page, stamp, why):
    """A judgment the fold lays over a standing one, as a body -> the hypothesis's own fields
    with the trail a replacement leaves put back: for an arrangement, `born` renewed to the
    day of the fold and one line appended to `replaced:`; for every other judgment, the one
    line. None for an entry, which the fold carries over as the text it is - only a decision
    replacing a decision has a trail to keep, and the trail is the same one `add` leaves when
    the door admits the replacement in place. The body replaced is kept beside the record by
    the fold once the record reads back whole."""
    _, bids, bjud, bfields, braw = c.base
    if k not in bjud:
        return None
    body = h["raw"].get(k)
    if not isinstance(body, dict):
        return None
    snap = bfields["snapshot"] or "seen"
    if P.is_arrangement(bjud[k], braw):
        if not P._arrangement_shaped(body, bfields, braw):
            return None
        extra = P.arrangement_renewal(bjud[k]["body"], why, stamp,
                                      ((page or {}).get(k) or {}).get("stood"))
    else:
        extra = P.judgment_renewal(bjud[k]["body"], why, stamp)
    out = {f: v for f, v in body.items() if f not in extra and f != snap}
    out.update(extra)
    if snap in body:
        out[snap] = body[snap]
    return out


def _block_of(h, k, ids, jud, fields, doc):
    """-> (collection, lines): an id's own lines in the hypothesis, whole - comments, style and
    all - and the collection they sit in; rendered from the body when the hypothesis has no
    text, a what-if held in memory."""
    for text in (h.get("text") or {}).values() if isinstance(h.get("text"), dict) else \
            ([h["text"]] if h.get("text") else []):
        loc = P._locate(text, k)
        if loc:
            name, _, s, e = loc
            block = list(text[s:e])
            while block and not block[-1].strip():
                block.pop()
            return name, block
    body = h["raw"].get(k)
    col = next((cn for cn, m in P.collections_of(h["doc"]).items() if k in m), None) \
        or P._collection_for(doc, ids, jud, fields, k, body, None)
    return col, P._entry_lines(k, body, 2, 4, False)


def _replace_block(lines, nid, block):
    """The entry's lines replaced whole by a block carried over, at the indent of the members
    it stands among; the blank line that separates it from the next stays."""
    _, ind, s, e = P._locate(lines, nid)
    shift = ind - P._indent(block[0])
    block = [(" " * max(0, P._indent(l) + shift) + l.lstrip(" ")) if l.strip() else l
             for l in block]
    lines[s:e] = block


def _rel(paths, p):
    """A path as the commit wants it: relative to the checkout the record sits in, else to the
    record's own directory."""
    here = os.path.dirname(os.path.realpath((sorted(glob.glob(paths[0])) or [paths[0]])[0]))
    code, top, _ = _git(here, "rev-parse", "--show-toplevel")
    root = os.path.realpath(top.strip()) if not code else here
    real = os.path.realpath(p)
    if not real.startswith(root + os.sep):
        root = here
    return os.path.relpath(real, root)


def _text_of(f):
    return P._text_of_or_none(f)


def _guard_private_hypotheses(paths, doc, hyps, action, extra_roots=()):
    """Check original heads and relevant complete source worlds before a lossy write.

    Refutation deliberately keeps only a claim; branch comparison deliberately prunes
    unchanged entries. Neither transformation may strip the permission that governed
    the original claim or an unchanged source it depends on.
    """
    R, G = P._peer('recording'), P._peer('pending_grounding')
    combined = doc
    for hyp in hyps:
        combined = P.layered(combined, hyp)
    for hyp in hyps:
        context = hyp.get('privacy_context', P.layered(combined, hyp))
        heads = {'hypothesis': hyp.get('head', {}),
                 'record': context.get('meta', {}),
                 'original_record': doc.get('meta', {}),
                 'source_record': hyp.get('privacy_head', {})}
        entries = G.entries(context)
        roots = set(G.entries(hyp['doc'])) | set(extra_roots)
        for text in G._strings(heads):
            if text in entries:
                roots.add(text)
            roots.update(name for name in P.refs_in(text) if not P.is_builtin(name))
            roots.update(name for name in P.ID.findall(text) if name in entries)
        # The selected source of a refutation belongs to the current record even when
        # the proposal came from another complete code-world.
        selected, original = {}, {}
        try:
            if roots:
                selected = G.closure(context, sorted(roots))
            # Layering may remove privacy, citations or dependencies. Recheck the
            # original bodies of overwritten entries and every proposed dependency
            # already present in the base, then follow their original source chains.
            original_roots = (roots | set(G.entries(selected))) & set(G.entries(doc))
            if original_roots:
                original = G.closure(doc, sorted(original_roots))
        except ValueError:
            raise P.Refused('refused - hypothesis source closure could not be checked; original retained')
        if any(R.private_marker(value) for value in (heads, hyp['doc'], selected, original)):
            retained = dict(selected, hypothesis=heads['hypothesis'],
                            source_record_metadata=heads['source_record'],
                            record_metadata=heads['record'],
                            original_record_metadata=heads['original_record'],
                            original_record_closure=original)
            intent = dict(action, hypothesis=hyp['name'])
            receipt = R.draft(P._peer('knowledge_views').project_for(paths), intent, retained,
                              'private hypothesis or source permission; original retained without publication')
            raise P.Refused('private draft retained at ' + receipt['path'] + '; original hypothesis retained')


def bjud_of(c):
    return c.base[2]


def _stray(c, take):
    """What is wrong with the ids --take names, as one refusal, or None: an id nothing lays
    over a standing judgment or reads from another source, or one no name can take. A run
    that stopped on a contested id read no reversal, and says that instead."""
    if c.contested:
        return None
    named = {r[0] for r in c.reversed} | {u[0] for u in c.updates if u[0] not in bjud_of(c)
                                             and "taken by name" in u[5]}
    stray = [k for k in take if k not in named]
    if stray:
        return (f"refused - --take names {', '.join(stray)}, which no hypothesis here lays over a "
                f"standing judgment, or reads from another source than the base")
    cannot = [k for k in take if k in c.untakeable]
    if cannot:
        return "refused - no name takes " + ", ".join(cannot) + ": " + "; ".join(
            why for k, _, why in c.untaken if k in cannot)
    return None


def _dirty(paths, files):
    """The record files a fold would write that carry uncommitted changes -> [relative
    path], empty outside a checkout. A branch's record folds onto a committed base, so the
    fold is a commit of its own and nothing of it is mixed with other edits or lost."""
    here = os.path.dirname(os.path.abspath(paths[0]))
    code, top, _ = _git(here, "rev-parse", "--show-toplevel")
    if code:
        return []
    out = []
    for f in list(files) + [P.replaced_path(paths)]:
        f = os.path.abspath(f)
        if not os.path.exists(f):
            continue
        code, status, _ = _git(here, "status", "--porcelain", "--", f)
        if not code and status.strip():
            out.append(_rel(paths, f))
    return out


def _next_command(paths, files, refs):
    """What to run once the fold is written: the commit, so the fold is one commit of its
    own; and for a branch's record, the reminder that the code merges as it would."""
    here = os.path.dirname(os.path.abspath(paths[0]))
    code, _, _ = _git(here, "rev-parse", "--show-toplevel")
    if code or not files:
        return []
    what = " ".join(files)
    out = [f"next: git add {what} && git commit"]
    if refs:
        out.append(f"  then merge {', '.join(refs)} as you would - its record is folded here, "
                   f"and the merge carries only its code")
    return out


def fold(paths, names=(), refs=(), stamp=None, take=(), drops=None):
    project = P._peer('knowledge_views').project_for(paths)
    paths = P._peer('knowledge_views').write_paths(paths)
    if P._peer('history_direct').active(paths):
        if refs:
            raise P.Refused('history_branch_adoption_required: capture the branch history contribution explicitly')
        result = P._peer('history_direct').finish_hypotheses(paths, names, kind='fold',
            because='explicit consolidation', take=take, drops=drops)
        print(result['state'] + ': ' + ', '.join(result['hypotheses']))
        return 0
    with P._locked(paths[0], project=project):
        return P._mutate_legacy(paths, {'kind': 'consolidate', 'as_of': stamp},
            lambda: _fold_candidate(paths, names, refs, stamp, take, drops))


def _fold_candidate(paths, names=(), refs=(), stamp=None, take=(), drops=None):
    """The union written into the base, under one lock from the reading to the deletion:
    the record and its hypotheses read, the union tested and its report printed, then -
    only when the test is clean - every id that arrived or was replaced carried over whole
    from the hypothesis that holds it, placed in id order beside its siblings, entries first
    so each door is asked with the values already in place; the files read back and checked,
    and restored whole if the check says anything it did not say before; the folded files
    deleted. Refused when the dry run is not clean, or when what is named never folds."""
    stamp = stamp or datetime.date.today().isoformat()
    project = P._peer('knowledge_views').project_for(paths)
    paths = P._peer('knowledge_views').write_paths(paths)
    with contextlib.nullcontext():
        doc, hyps = read(paths, names, refs)
        _guard_private_hypotheses(paths, doc, hyps, {'kind': 'consolidate'})
        if not hyps:
            print("no hypotheses beside the record - nothing to consolidate")
            return 0
        files = P._files_of(paths)
        if refs:
            dirty = _dirty(paths, files)
            if dirty:
                raise P.Refused(f"refused - {', '.join(dirty)} carr{'ies' if len(dirty) == 1 else 'y'} "
                                f"uncommitted changes: a branch's record folds onto a committed base, "
                                f"so the fold is a commit of its own - commit first, then consolidate "
                                f"again")
        fail_b, _, moved_b, _, _ = P.check_lines(paths)
        page = page_of(paths)
        c = union_of(doc, hyps, (fail_b, moved_b), stamp, page, take, drops)
        for l in report(c):
            print(l)
        print()
        if c.contested:
            raise P.Refused("refused - a contested id stops the fold: " + ", ".join(c.contested))
        stray = _stray(c, take)
        if stray:
            raise P.Refused(stray)
        if c.drops_needed and not c.blocked:
            raise P.Refused("refused - a replacement no longer rests on what the judgment it replaces "
                            "rested on, and a dependency dropped is a decision with a reason: "
                            + "; ".join(f"consolidate {h['name']} " + " ".join(
                                f"--drop {shlex.quote(d + ': <why>')}" for d in c.drops_needed[k])
                                for k, h, _, _, _, _ in c.reversed if k in c.drops_needed))
        if c.untaken and not (c.falsified or c.holes or c.head_falsified or c.refused):
            cannot = [k for k, _, _ in c.untaken if k in c.untakeable]
            if cannot:
                raise P.Refused("refused - no name takes " + ", ".join(cannot) + ": "
                                + "; ".join(why for k, _, why in c.untaken if k in cannot))
            raise P.Refused("refused - a verdict the base's own condition has not broken folds only "
                            "when a person names it: " + "; ".join(
                                f"consolidate {h['name']} --take {k}" for k, h, _ in c.untaken))
        if c.blocked:
            raise P.Refused("refused - the dry run is not clean; nothing folds until it is")
        never = [h["name"] for h in c.hyps if str(h["head"].get("folds") or "") == NEVER]
        if never:
            raise P.Refused(f"refused - {', '.join(never)} never fold{'s' if len(never) == 1 else ''}: "
                            f"a what-if is evaluated and never written; consolidate the others by name")
        originals = {f: _text_of(f) for f in files}
        texts = {f: originals[f].split("\n") for f in files}
        writes = [(k, h, False) for k, h in c.arrived] + [(k, h, True) for k, h, _, _, _, _ in c.updates]
        writes.sort(key=lambda w: (w[0] in c.jud, w[0]))
        whys = {k: why for k, _, _, _, _, why in c.updates}
        out, added, replaced, trailed = [], 0, 0, set()
        for k, h, replace in writes:
            collection, block = _block_of(h, k, c.ids, c.jud, c.fields, doc)
            if replace:
                target = next(f for f in files if P._locate(texts[f], k))
                renewed = _renewed(k, c, h, page, stamp, whys.get(k, ""))
                if renewed is None:
                    _replace_block(texts[target], k, block)
                else:
                    P._replace_in(texts[target], k, renewed)
                    trailed.add(k)
                replaced += 1
                out.append(f"replace {k} with what {h['name']} holds, where it stands"
                           + (" - born renewed, and what it replaced kept" if renewed and "born" in renewed
                              else " - what it replaced kept" if renewed else ""))
                if renewed is not None:
                    out += P.trail_lines(paths, k, c.base[2][k]["body"], renewed, c.fields)
            else:
                target = P._file_for(files, k, collection, texts)
                where = P._insert_block(texts[target], collection, k, block)
                out.append("carry " + where.replace(f"{k} into", f"{k} from {h['name']} into", 1))
                added += 1
        if writes:
            for f in files:                     # meta lives in one of the files, not always the first
                if P._bump_updated(texts[f], stamp):
                    break
        changed = [f for f in files if "\n".join(texts[f]) != originals[f]]
        for f in changed:
            P._write_text(f, "\n".join(texts[f]))
        try:
            doc2 = P.load(paths)
            ids2, jud2, fields2 = P.infer(doc2)
            raw2 = P.with_builtins(doc2, ids2, jud2, fields2)
            for k, h, _ in writes:
                if k not in ids2:
                    raise ValueError(f"{k} is not in the record after the fold")
                want, got = P.claim_of(h["raw"].get(k)), P.claim_of(raw2.get(k))
                if not P._same_claim(want, got):
                    raise ValueError(f"{k} reads back as {P.short(got)!r}")
            fail_a, _, _, _, _ = P.check_lines(paths)
            worse = [l for l in fail_a if l not in fail_b]
            if worse:
                raise ValueError("check fails on what was folded: " + "; ".join(worse[:3]))
        except (Exception, SystemExit) as e:
            for f in changed:
                P._write_text(f, originals[f])
            raise P.Refused(f"the fold broke the record and was undone: {e}")
        # the bodies the fold replaced, kept whole beside the record - once the record is
        # safely written; a keeping that fails undoes the record too, so the trail on a
        # judgment and the version beside the record never disagree
        kept, side = [], P.replaced_path(paths)
        side_before = _text_of(side) if P._exists(side) else None
        try:
            for k, h, replace in writes:
                if replace and k in trailed and isinstance(h["raw"].get(k), dict):
                    old_body = c.base[2][k]["body"]
                    named_drops = {d: (drops or {})[d] for d in P.dropped_deps(old_body, h["raw"][k], c.fields)
                                   if d in (drops or {})}
                    P.keep_replaced(paths, k, old_body, whys.get(k, ""), stamp, named_drops or None)
                    kept = [side]
        except (Exception, SystemExit) as e:
            for f in changed:
                P._write_text(f, originals[f])
            if side_before is None:
                if P._exists(side):
                    P._remove_file(side)
            else:
                P._write_text(side, side_before)
            raise P.Refused(f"the fold could not keep what it replaced and was undone: {e}")
        deleted = []
        for h in c.hyps:
            if h.get("path") and P._exists(h["path"]):
                P._remove_file(h["path"])
                P.forget(h["path"])
                deleted.append(h["path"])
        d = P.hypothesis_dir(paths)
        if P._DIRECT_STAGE.get() is None and os.path.isdir(d) and not os.listdir(d):
            os.rmdir(d)
    names_ = ", ".join(h["name"] for h in c.hyps)
    if not writes:
        out.append(f"nothing to write: the base already holds everything {names_} propose"
                   f"{'s' if len(c.hyps) == 1 else ''}")
    else:
        nj = sum(1 for k, _, _ in writes if k in c.jud)
        ne = len(writes) - nj
        out.append(f"folded {names_}: {ne} entr{'y' if ne == 1 else 'ies'} and {nj} judgment"
                   f"{'' if nj == 1 else 's'} - {added} added, {replaced} replaced")
    out.append("files to commit: " + (", ".join([_rel(paths, f) for f in changed]
                                                 + [_rel(paths, f) for f in kept]
                                                 + [_rel(paths, p) + " (deleted)" for p in deleted])
                                       or "none"))
    for h in c.hyps:
        if not h.get("path"):
            out.append(f"  nothing to delete for {h['name']}: another branch keeps its own record")
    out += _next_command(paths, [_rel(paths, f) for f in changed] + [_rel(paths, f) for f in kept]
                         + [_rel(paths, p) for p in deleted], refs)
    n = P.counts(doc2, ids2, jud2, fields2, raw2)["graph.flagged"]
    out.append("")
    out.append(f"the record needs a person on {n} judgment{'s' if n != 1 else ''} - check says the rest")
    for l in out:
        print(l)
    return 0


# ── the refutation ───────────────────────────────────────────────────────────
def _session_source(doc, ids, raw, given=None):
    """The session source a finding is from: the one named, which must carry what it was
    asked; else the newest one the base holds - by the day it was read, then by id."""
    if given:
        b = raw.get(given)
        if not (isinstance(b, dict) and b.get("asked")):
            raise P.Refused(f"refused - {given} is not a session source carrying what it was asked")
        return given
    sessions = [(P._read_on(b, raw) or datetime.date.min, k) for k, b in raw.items()
                if k in ids and isinstance(b, dict) and b.get("asked") and not P.is_builtin(k)]
    if not sessions:
        raise P.Refused("refused - a finding is from a session, and this record holds no session "
                        "source: add s.<date>_<slug> asked=\"...\" name=\"...\" first, or name one "
                        "with --as")
    return max(sessions)[1]


def refute(paths, name, why, source=None, stamp=None):
    project = P._peer('knowledge_views').project_for(paths)
    paths = P._peer('knowledge_views').write_paths(paths)
    if P._peer('history_direct').active(paths):
        result = P._peer('history_direct').finish_hypotheses(paths, [name], kind='refute', because=why, by=source)
        return [result['state'] + ': ' + name]
    with P._locked(paths[0], project=project):
        return P._mutate_legacy(paths, {'kind': 'refute', 'as_of': stamp},
            lambda: _refute_candidate(paths, name, why, source, stamp))


def _refute_candidate(paths, name, why, source=None, stamp=None):
    """The hypothesis's claim written into the base as a negative finding - `hyp.<name>`,
    `v: refuted`, its claim as the name, the why as `at:`, from a session source, dated - and
    its file deleted, under one lock and undone together: a finding without the deletion, or
    the other way round, is never left behind. Nothing else of the hypothesis enters. -> the
    lines to print."""
    if not why or not why.strip():
        raise P.Refused("refused - a refutation says why: consolidate --refute <hypothesis> \"<why>\"")
    stamp = stamp or datetime.date.today().isoformat()
    with contextlib.nullcontext():
        doc = P.load(paths)
        h = doc.hypotheses.get(name)
        if h is None:
            raise P.Refused(f"refused - no hypothesis named {name} beside the record")
        if h["error"]:
            raise P.Refused(f"refused - hypothesis {name} could not be read: {h['error']}")
        ids, jud, fields = P.infer(doc)
        raw = P.with_builtins(doc, ids, jud, fields)
        src = _session_source(doc, ids, raw, source)
        _guard_private_hypotheses(paths, doc, [h],
            {'kind': 'refute', 'why': why, 'source': src}, extra_roots=[src])
        nid, n = "hyp." + re.sub(r"[^A-Za-z0-9_]", "_", name), 1
        while nid in ids:
            n += 1
            nid = f"hyp.{re.sub(r'[^A-Za-z0-9_]', '_', name)}_{n}"
        claim = h["head"].get("claim")
        body = {"v": "refuted", "name": str(claim) if claim else f"hypothesis {name}", "from": src,
                "at": " ".join(why.split()), "of": stamp}
        # The optional head claim is prose; add --hypothesis need not write one. Keep the
        # identities of the judgments the file actually held before that file disappears.
        refutes = sorted(k for k, b in h["raw"].items()
                         if isinstance(k, str) and P._judgment_shaped(b, fields))
        if refutes:
            body["refutes"] = refutes
        action = {"kind": "add", "id": nid, "body": body, "as_of": stamp, "why": None, "into": None,
                  "hypothesis": None}
        files = P._files_of(paths)
        originals = {f: _text_of(f) for f in files}
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            P._apply(paths, action)              # the write path, inside the lock already held
        try:
            P._remove_file(h["path"])
            P.forget(h["path"])
        except OSError as e:
            for f in files:
                if _text_of(f) != originals[f]:
                    P._write_text(f, originals[f])
            raise P.Refused(f"refused - {h['path']} could not be deleted, so the finding was not "
                            f"written either: {e}")
        d = P.hypothesis_dir(paths)
        if P._DIRECT_STAGE.get() is None and os.path.isdir(d) and not os.listdir(d):
            os.rmdir(d)
        written = [f for f in files if _text_of(f) != originals[f]]
    out = [l for l in buf.getvalue().split("\n") if l.strip()]
    out.append(f"refuted {name}: {nid} holds its claim as a negative finding, from {src}; "
               f"{_rel(paths, h['path'])} deleted, and nothing else of it enters")
    out.append("files to commit: " + ", ".join([_rel(paths, f) for f in written]
                                               + [_rel(paths, h["path"]) + " (deleted)"]))
    return out


# ── another branch's record ──────────────────────────────────────────────────
def _git(d, *args):
    p = subprocess.run(["git", "-C", d] + list(args), capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


def _pointers(d):
    out = []
    for key in ("record", "also"):
        v = (d or {}).get(key)
        v = [v] if isinstance(v, str) else v if isinstance(v, list) else \
            list(v.values()) if isinstance(v, dict) else []
        out += [c for c in v if isinstance(c, str) and c.endswith((".yaml", ".yml"))]
    return out


def from_ref(paths, ref, doc=None):
    """Another branch's committed record, read as hypotheses -> [hypothesis]: its base as one
    named after the ref, holding only what differs from this base in its claim, is newer by
    its day, or is new to it; and each hypothesis file it carries as one more, named
    `<ref>:<name>`, unless this record holds the same file. Read with `git show` from the
    ref's tree, never written: the branch keeps its own record."""
    doc = doc if doc is not None else P.load(paths)
    first = (sorted(glob.glob(paths[0])) or [paths[0]])[0]
    rec = os.path.abspath(first)
    here = os.path.dirname(rec)
    code, top, err = _git(here, "rev-parse", "--show-toplevel")
    if code:
        raise P.Refused(f"refused - --from reads a committed record, and {rec} is not in a git "
                        f"checkout")
    root = os.path.realpath(top.strip())
    if not os.path.realpath(rec).startswith(root + os.sep):
        raise P.Refused(f"refused - --from reads a committed record, and {rec} is not in the tree at "
                        f"{root}")
    code, sha, err = _git(here, "rev-parse", "--verify", "--quiet", ref + "^{commit}")
    if code:
        raise P.Refused(f"refused - {ref} is not a commit this checkout knows")
    sha = sha.strip()
    day = _git(here, "log", "-1", "--format=%as", sha)[1].strip()
    rel = posixpath.join(*os.path.relpath(os.path.realpath(rec), root).split(os.sep))
    rdir = posixpath.dirname(rel)

    def show(path):
        code, text, _ = _git(here, "show", f"{sha}:{path}")
        return None if code else text

    # The ref may keep its record under the other entry name - one side renamed it - so the
    # file is looked for under both, and its hypotheses where that name keeps them.
    if show(rel) is None and posixpath.basename(rel) in P.ENTRY_NAMES:
        for name in P.ENTRY_NAMES:
            alt = posixpath.join(rdir, name) if rdir else name
            if alt != rel and show(alt) is not None:
                rel = alt
                break

    with tempfile.TemporaryDirectory() as t:
        texts, queue, seen = {}, [rel], set()
        while queue:
            path = queue.pop(0)
            if path in seen:
                continue
            seen.add(path)
            text = show(path)
            if text is None:
                if path == rel:
                    raise P.Refused(f"refused - {ref} holds no {rel}")
                continue
            texts[path] = text.split("\n")
            local = os.path.join(t, *path.split("/"))
            os.makedirs(os.path.dirname(local), exist_ok=True)
            with io.open(local, "w", encoding="utf-8") as f:
                f.write(text)
            try:
                d = yaml.safe_load(text) or {}
            except yaml.YAMLError as e:
                raise P.Refused(f"refused - {ref}:{path} does not read as a record: "
                                + " ".join(str(e).split())[:120])
            queue += [posixpath.normpath(posixpath.join(posixpath.dirname(path), c))
                      for c in _pointers(d if isinstance(d, dict) else {})]
        hdir = posixpath.join(rdir, P.hypotheses_rel(rel)) if rdir else P.hypotheses_rel(rel)
        _, listing, _ = _git(here, "ls-tree", "--name-only", sha, hdir + "/")
        htexts = {}
        for path in listing.split("\n"):
            if not re.search(r"\.ya?ml$", path):
                continue
            text = show(path)
            if text is None:
                continue
            local = os.path.join(t, *path.split("/"))
            os.makedirs(os.path.dirname(local), exist_ok=True)
            with io.open(local, "w", encoding="utf-8") as f:
                f.write(text)
            htexts[re.sub(r"\.ya?ml$", "", posixpath.basename(path))] = text.split("\n")
        token = P._DIRECT_STAGE.set(P._DirectStage())
        try:
            rdoc = P.load([os.path.join(t, *rel.split("/"))])
        finally:
            P._DIRECT_STAGE.reset(token)
    bids, bjud, bfields, braw = _view(doc)
    rraw = P.bodies(rdoc)
    pruned = {}
    for col, members in P.collections_of(rdoc).items():
        if col == "meta":
            continue
        for k, b in members.items():
            keep = k not in bids
            if not keep:
                keep = not P._same_claim(P.claim_of(b), P.claim_of(braw.get(k)))
            if not keep and k in bjud and isinstance(b, dict) and isinstance(braw.get(k), dict):
                # the same verdict on other grounds is a decision written again, and an
                # arrangement re-decided in place carries a new born; the same decision with
                # its seen refreshed is the branch's own review, which stays
                keep = P._version_core(b) != P._version_core(braw[k]) or (
                    P.is_arrangement(bjud[k], braw) and b.get("born") != braw[k].get("born"))
            if not keep and k not in bjud and isinstance(b, dict):
                dr, db = P._read_on(b, rraw), P._read_on(braw.get(k), braw)
                keep = bool(dr) and (db is None or dr > db)
            if keep:
                pruned.setdefault(col, {})[k] = b
    head = {"claim": f"what {ref} committed ({sha[:7]}), read as a hypothesis", "born": day}
    out = [hypothesis(ref, pruned, head, None, texts)]
    out[0]['privacy_context'] = rdoc
    out[0]['privacy_head'] = {key: rdoc[key] for key in ('meta', 'hypothesis') if key in rdoc}
    for n, rh in sorted(rdoc.hypotheses.items()):
        if rh["error"]:
            raise P.Refused(f"refused - hypothesis {n} at {ref} could not be read: {rh['error']}")
        mine = doc.hypotheses.get(n)
        if mine and not mine["error"] and mine["doc"] == rh["doc"] and mine["head"] == rh["head"]:
            continue
        imported = hypothesis(f"{ref}:{n}", rh["doc"], rh["head"], None, {n: htexts.get(n, [])})
        imported['privacy_context'] = P.layered(rdoc, rh)
        imported['privacy_head'] = {key: rdoc[key] for key in ('meta', 'hypothesis') if key in rdoc}
        out.append(imported)
    return out


def pull_from(paths, ref, seeds, budget=40):
    """`pull`, with another branch's committed record laid beside this one: what it proposes
    for the seed, said the way a hypothesis file's proposal is - the record loaded once, the
    branch's hypotheses laid over it, and handed to the reader's own pull."""
    hyps = from_ref(paths, ref)
    doc = P.load(paths)
    doc.hypotheses = dict(doc.hypotheses, **{h["name"]: h for h in hyps})
    return P.pull(paths, seeds, budget, doc=doc)


# ── the command ──────────────────────────────────────────────────────────────
HELP = """  consolidate [--dry-run] [<hypothesis> ...] [--from <ref>] [--take <id>] [--drop "<id>: <why>"] [--as-of YYYY-MM-DD] [file]
  consolidate --refute <hypothesis> "<why>" [--as s.<source>] [--as-of YYYY-MM-DD] [file]
  pull <seed> [...] --from <ref> [--budget N] [file]

Consolidation is a test. The dry run lays the hypotheses named - every one beside the
record, when none is - over the base by id, in name order, and runs the reader's own check
on the result; it writes nothing. The report, in fixed order: arrived (what the fold would
add), updates (readings the base holds that a hypothesis replaces, with what rests on
each), reversed (a verdict, or other grounds, laid over a standing judgment - both sides'
because, rests_on and wrong_if beside each other), moved / falsified (what the union moves
or breaks, in check's own words), contested, candidates, new subjects. An id two hypotheses
hold with different claims stops the run: both readings are shown side by side, to re-read
against the merged tree. The exit code is non-zero on a contested id, a reversal nobody
named, a falsifier that holds, or a hole; a premise that moved under a judgment leaves it
green and blocks the fold, since a move is for a person to re-review.

The fold - no --dry-run - runs the same test first and writes only when it is clean: every
id carried over whole from its hypothesis into the base, in id order beside its siblings,
through the reader's own edits; the files read back and checked; the folded files deleted;
the files to commit printed, and the commit that makes the fold a commit of its own. Every
replacement passes the one door a written value passes, asked with the base's own
readings: an entry when its reading is newer than the base's - so a reading of the same day
as the base's is contested, and stays beside the record until someone reads again; one
nothing dates is not compared with a dated one, and is contested until --as-of dates it. A
judgment when the standing one is broken by its own condition on what the base holds - a
hypothesis that brings both the reading that breaks a judgment and the verdict that repairs
it has not broken it here - or when a person names the id: `--take <id>` folds that reversal
by name, and the trail says so. A replacement that rests on less names each dependency it
drops with its reason, `--drop "<id>: <why>"`, kept with the replaced version. A subject does
not change kind at the fold - an entry under a judgment's id, or a judgment under an entry's -
and what replaces an arrangement is an arrangement; no name takes either. A reading dated
after today is not read. Nothing written inside a hypothesis opens the door; the
report says when the hypothesis's own readings would fire the base's condition, so the
person deciding knows. What a fold replaces is kept whole in .kpopper/replaced.yaml, with
one line on the judgment. A judgment's review - the same decision, its seen refreshed -
does not travel: it is the branch's own. A head's own wrong_if is evaluated against the
union: one that holds, or one the reader cannot decide, is red. A hypothesis with `folds:
never` in its head is a what-if: evaluated by the dry run, never written.

--refute writes the hypothesis's claim into the base as a negative finding and deletes the
file - nothing else of it enters. The finding is one entry:

  hyp.<name>:
    v: refuted              the value every negative finding of this kind carries
    name: <its claim>       from the head, else "hypothesis <name>"
    from: s.<session>       the session that refuted it - the newest the record holds,
                            or the one --as names
    at: <why>               the reason, verbatim
    of: <day>
    refutes: [<id>, ...]    judgment ids held by the hypothesis, when it held any;
                            identities, not dependencies of the finding

--from <ref> reads another branch's committed record - `git show <ref>:GROUNDING.yaml`, or the
older PROVENANCE.yaml, the files it points at, and its hypotheses directory - and lays it over
this base as hypotheses: the
ref's base as one named after the ref, holding only what differs from this base in its
claim, is newer by its day, or is new to it; each hypothesis file it carries as one more,
named <ref>:<name>. The same dry run, the same report, the same refusal; the fold writes
what the branch knew into this base and deletes nothing, since the branch keeps its own
record. It folds onto a committed base only - a record file with uncommitted changes
refuses it - so the fold is a commit of its own, and the branch's code merges after it
carrying only the code. `pull <seed> --from <ref>` shows what it proposes beside the base's
values."""


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv or argv[0] in ("-h", "--help") or "--help" in argv or "-h" in argv:
        print(HELP.strip("\n"))
        return 0
    if argv[0] == "pull":
        rest, ref, budget, seeds, files = argv[1:], None, 40, [], []
        i = 0
        while i < len(rest):
            if rest[i] == "--from":
                if i + 1 >= len(rest):
                    raise P.Refused("--from needs a ref")
                ref = rest[i + 1]; i += 2; continue
            if rest[i] == "--budget":
                if i + 1 >= len(rest):
                    raise P.Refused("--budget needs a number")
                budget = int(rest[i + 1]); i += 2; continue
            (files if rest[i].endswith((".yaml", ".yml")) else seeds).append(rest[i])
            i += 1
        if not ref:
            raise P.Refused("pull here takes --from <ref>: the reader's own pull reads the record "
                            "beside you")
        if not seeds:
            raise P.Refused("pull needs a seed: an entry, or a prefix")
        return pull_from(files or P.default_paths(), ref, seeds, budget)
    dry, refs, refute_, source, as_of, names, files, why = False, [], None, None, None, [], [], None
    take, drops = [], {}
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--dry-run":
            dry = True; i += 1
        elif a in ("--from", "--refute", "--as", "--as-of", "--take", "--drop"):
            if i + 1 >= len(argv):
                raise P.Refused(f"{a} needs a value")
            v = argv[i + 1]
            if a == "--from":
                refs.append(v)
            elif a == "--refute":
                refute_ = v
            elif a == "--as":
                source = v
            elif a == "--take":
                take.append(v)
            elif a == "--drop":
                if ":" not in v or not v.split(":", 1)[0].strip() or not v.split(":", 1)[1].strip():
                    raise P.Refused('--drop takes "<id>: <why>" - the dependency the replacement no '
                                    'longer rests on, and the reason')
                d, w = v.split(":", 1)
                drops[d.strip()] = w.strip()
            else:
                as_of = v
            i += 2
        elif a.startswith("--"):
            raise P.Refused(f"{a} is not an option of consolidate\n\n" + HELP.strip("\n"))
        elif a.endswith((".yaml", ".yml")):
            files.append(a); i += 1
        elif refute_ and why is None:
            why = a; i += 1
        else:
            names.append(a); i += 1
    if as_of and not re.match(r"^\d{4}-\d{2}-\d{2}$", as_of):
        raise P.Refused("--as-of takes a date, YYYY-MM-DD")
    if as_of and P._as_day(as_of) and P._as_day(as_of) > P.latest_today():
        raise P.Refused(f"--as-of {as_of} is after today ({P.latest_today().isoformat()}) - a day "
                        f"is the record's clock, and a fold is not dated ahead")
    paths = files or P.default_paths()
    if refute_:
        if refs or names or dry:
            raise P.Refused("--refute takes one hypothesis and its why, and nothing else")
        for l in refute(paths, refute_, why or "", source, as_of):
            print(l)
        return 0
    if source:
        raise P.Refused("--as names the session source of a refutation: it goes with --refute")
    if not dry:
        return fold(paths, names, refs, as_of, take, drops)
    routed = P._peer('knowledge_views').write_paths(paths)
    if P._peer('history_direct').active(routed):
        if refs:
            raise P.Refused('history_branch_adoption_required: capture the branch history contribution explicitly')
        result = P._peer('history_direct').finish_hypotheses(routed, names, kind='fold',
            because='consolidation preview', take=take, drops=drops, dry=True)
        print(result['state'] + ': ' + ', '.join(result['hypotheses']))
        return 0
    doc, hyps = read(paths, names, refs)
    if not hyps:
        print("no hypotheses beside the record - nothing to consolidate")
        return 0
    fail_b, _, moved_b, _, _ = P.check_lines(paths)
    c = union_of(doc, hyps, (fail_b, moved_b), as_of, page_of(paths), take, drops)
    for l in report(c):
        print(l)
    stray = _stray(c, take)
    if stray:
        print()
        print(stray)
        return 1
    return 1 if c.red or c.drops_needed else 0


if __name__ == "__main__":
    sys.exit(main())
