"""Versions of a subject, the acts on them, and the state computed from both.

A record is a set of immutable versions. A version is one claim about one subject - a
reading, a judgment, or an act on other versions - identified by the hash of its content
and kept as one file of its own, so that two branches writing one subject merge under git
without a textual conflict and the disagreement is met by the reader. A version keeps three
relations apart: what its writer saw (`saw`, versions of the same subject known when it
was written), what its claim rests on (`rests_on`, dependency versions by id), and what an
act replaced (`over`). Merge is the union of the files. The state - which versions stand,
with what status, and what each judgment's dependencies have become - is computed from the
union and the rules, so the same contributions under the same rules give the same state
whatever the order or number of merges. The entry file is a rendering of that state,
stamped with the state it was rendered from, and a hand edit is reconciled three ways
against the stamp. This module is the core under examination; nothing existing reads it.
"""
import copy
import datetime
import hashlib
import io
import json
import os
import re

import yaml

HOME = ".kpopper"
STORE = "versions"
STAMPS = ".stamps"
ENTRY = "GROUNDING.yaml"
RULES = {"version": 1, "self_review_counts": False}
RESERVATIONS = ("corrected", "refuted", "contested", "divergent", "unreviewed", "proposed")


# ── identity ─────────────────────────────────────────────────────────────────
def canonical(obj):
    return json.dumps(obj, sort_keys=True, ensure_ascii=False, separators=(",", ":"), default=str)


def ident(fields):
    return hashlib.sha256(canonical(fields).encode("utf-8")).hexdigest()[:12]


def _now():
    return datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat()


def version(subject, kind, by, body, saw=(), at=None, applies=None, on=None):
    """One claim about a subject -> the version, with its id. `kind` is reading or judgment;
    `saw` the ids of this subject's versions the writer knew; `at` the source's own clock
    value, {day|stamp|version|commit: ...}, when the source has one; `applies` the time the
    datum is about, when it matters; `on` the moment recorded, a full timestamp with no
    authority to decide anything."""
    if kind not in ("reading", "judgment"):
        raise ValueError("kind is reading or judgment")
    fields = {"subject": subject, "kind": kind, "by": by, "on": on or _now(), "body": copy.deepcopy(body),
              "saw": sorted(saw)}
    if at is not None:
        fields["at"] = at
    if applies is not None:
        fields["applies"] = applies
    fields["id"] = ident(fields)
    return fields


def act(subject, by, what, of=None, over=(), because="", read=None, saw=(), on=None):
    """An act on versions of a subject -> a version of kind act. `what` is accept (of one
    version, over the ones it replaces), refute (of one), review (of one, having read the
    dependency versions in `read`), or correct (of one, over the version it corrects)."""
    if what not in ("accept", "refute", "review", "correct"):
        raise ValueError("an act is accept, refute, review or correct")
    body = {"act": what, "of": of, "over": sorted(over), "because": because}
    if read:
        body["read"] = dict(read)
    fields = {"subject": subject, "kind": "act", "by": by, "on": on or _now(), "body": body, "saw": sorted(saw)}
    fields["id"] = ident(fields)
    return fields


# ── the store: one file per version ──────────────────────────────────────────
class Store(object):
    def __init__(self, root):
        self.root = os.path.abspath(root)
        self.dir = os.path.join(self.root, HOME, STORE)

    def _path(self, v):
        return os.path.join(self.dir, v["subject"], v["id"] + ".yaml")

    def keep(self, v):
        """The version kept, once: a file that exists is never rewritten."""
        p = self._path(v)
        if not os.path.isfile(p):
            os.makedirs(os.path.dirname(p), exist_ok=True)
            with io.open(p, "w", encoding="utf-8") as f:
                yaml.safe_dump(v, f, sort_keys=True, allow_unicode=True)
        return v["id"]

    def read(self):
        """Every version kept -> {subject: {id: version}}, read from the files alone."""
        out = {}
        if not os.path.isdir(self.dir):
            return out
        for subject in sorted(os.listdir(self.dir)):
            d = os.path.join(self.dir, subject)
            if subject.startswith(".") or not os.path.isdir(d):
                continue
            for name in sorted(os.listdir(d)):
                if not name.endswith(".yaml"):
                    continue
                with io.open(os.path.join(d, name), encoding="utf-8") as f:
                    v = yaml.safe_load(f)
                if isinstance(v, dict) and v.get("id") == name[:-5] and v.get("subject") == subject:
                    out.setdefault(subject, {})[v["id"]] = v
        return out

    def union_from(self, other):
        """Every version the other store holds and this one does not, copied in."""
        added = 0
        for subject, versions in Store(other).read().items():
            for v in versions.values():
                if not os.path.isfile(self._path(v)):
                    self.keep(v)
                    added += 1
        return added

    def stamp(self, heads):
        """The heads a rendering showed, kept under the hash that names that state."""
        s = ident(heads)
        d = os.path.join(self.dir, STAMPS)
        os.makedirs(d, exist_ok=True)
        p = os.path.join(d, s + ".json")
        if not os.path.isfile(p):
            with io.open(p, "w", encoding="utf-8") as f:
                json.dump(heads, f, sort_keys=True)
        return s

    def heads_at(self, stamp):
        p = os.path.join(self.dir, STAMPS, str(stamp) + ".json")
        if not stamp or not os.path.isfile(p):
            return None
        with io.open(p, encoding="utf-8") as f:
            return json.load(f)


# ── the source's own clock ───────────────────────────────────────────────────
def _version_key(v):
    return tuple(int(p) if p.isdigit() else p for p in re.split(r"[.\-]", str(v)))


def clock_order(a, b, ancestry=None):
    """How two clock values of one source relate -> -1, 0, 1 when the source's clock orders
    them, None when it does not: a day, a stamp and a version are totally ordered; a commit
    is ordered only along ancestry, which `ancestry(x, y)` answers - is x an ancestor of y."""
    if not isinstance(a, dict) or not isinstance(b, dict) or len(a) != 1 or len(b) != 1:
        return None
    (ka, va), (kb, vb) = list(a.items())[0], list(b.items())[0]
    if ka != kb:
        return None
    if ka in ("day", "stamp"):
        return (str(va) > str(vb)) - (str(va) < str(vb))
    if ka == "version":
        x, y = _version_key(va), _version_key(vb)
        return (x > y) - (x < y)
    if ka == "commit":
        if va == vb:
            return 0
        if ancestry is None:
            return None
        if ancestry(va, vb):
            return -1
        if ancestry(vb, va):
            return 1
        return None
    return None


# ── the state ────────────────────────────────────────────────────────────────
def _evaluate(pred, values):
    """A falsifier decided against accepted reading values -> True, False, or None when the
    reader cannot decide it. One comparison; the record's own evaluator when it is there."""
    try:
        import provenance as P
        raw = {k: {"v": v} for k, v in values.items()}
        return P.evaluate(pred, raw, set(raw))
    except Exception:
        pass
    m = re.match(r"^\s*([\w.]+)\s*(>=|<=|>|<|==|!=)\s*([\w.]+)\s*$", str(pred))
    if not m:
        return None
    a, op, b = m.groups()

    def val(x):
        if x in values:
            return values[x]
        try:
            return float(x)
        except ValueError:
            return None
    x, y = val(a), val(b)
    if x is None or y is None:
        return None
    try:
        return {">": x > y, "<": x < y, ">=": x >= y, "<=": x <= y, "==": x == y, "!=": x != y}[op]
    except TypeError:
        return None


def state(versions, rules=None, ancestry=None):
    """The state of every subject from the versions and the acts alone -> {"subjects": {...},
    "implied": [...]}: per subject its heads, their status, the value or body that stands, and
    for a judgment the disposition of each dependency and the reservations it carries."""
    rules = dict(RULES, **(rules or {}))
    subjects = {}
    implied = []
    for subject, held in sorted(versions.items()):
        claims = {i: v for i, v in held.items() if v["kind"] != "act"}
        acts = [v for v in held.values() if v["kind"] == "act"]
        acts.sort(key=lambda a: (a["on"], a["id"]))
        if not claims:
            continue
        birth = min(claims.values(), key=lambda v: (v["on"], v["id"]))["id"]
        mark, accepted, accepted_by, reviewed_by, refuted = {}, {birth}, {birth: None}, {}, set()
        replacers = {}                  # replaced version -> the accepted versions laid over it
        for a in acts:
            b = a["body"]
            if b["act"] in ("accept", "correct"):
                if b["of"] in claims:
                    accepted.add(b["of"])
                    accepted_by.setdefault(b["of"], a["by"])
                for o in b["over"]:
                    if o in claims and b["of"] in claims:
                        replacers.setdefault(o, set()).add(b["of"])
                        mark[o] = "corrected" if b["act"] == "correct" else "replaced"
            elif b["act"] == "refute":
                if b["of"] in claims:
                    mark[b["of"]] = "refuted"
                    refuted.add(b["of"])
            elif b["act"] == "review":
                if b["of"] in claims:
                    reviewed_by.setdefault(b["of"], set()).add(a["by"])
        # a version is out once an accepted version was laid over it - and stays out when
        # that one falls in turn, since a return is written as a new version. Two versions
        # laid over each other by acts that never met are a dispute between acts: both stand
        replaced = set(refuted)
        for y, xs in replacers.items():
            if any(x in accepted and y not in replacers.get(x, ()) for x in xs):
                replaced.add(y)
        # a claim stands only once accepted - at birth, or by an act naming it - and a claim no
        # act accepted is a proposal: listed, resting on nothing that stands, contesting nothing
        frontier = [claims[i] for i in sorted(claims) if i in accepted and i not in replaced]
        proposals = [i for i in sorted(claims) if i not in accepted and i not in replaced]
        # the source's own clock among readings: a later state of a moving source stands,
        # and the reading of the earlier state is superseded by rule - an act implied here
        kind = next(iter(claims.values()))["kind"]
        divergent = False
        if kind == "reading" and len(frontier) > 1:
            keep = list(frontier)
            for v in frontier:
                for w in frontier:
                    if v is w or v not in keep:
                        continue
                    o = clock_order(v.get("at"), w.get("at"), ancestry)
                    if o == -1:
                        keep.remove(v)
                        implied.append({"rule": "source_clock", "subject": subject, "superseded": v["id"],
                                        "by": w["id"]})
                        mark.setdefault(v["id"], "superseded")
                        break
            if len(keep) > 1 and all(isinstance(v.get("at"), dict) and "commit" in v["at"] for v in keep):
                divergent = True
            frontier = keep
        heads = [v["id"] for v in frontier]
        entry = {"kind": kind, "heads": heads, "versions": len(claims), "acts": len(acts),
                 "marks": mark, "birth": birth, "proposals": proposals}
        if len(heads) == 1:
            h = claims[heads[0]]
            entry["head"] = h["id"]
            entry["body"] = h["body"]
            if kind == "reading":
                entry["status"] = "accepted"
            else:
                who = accepted_by.get(h["id"])
                others = {r for r in reviewed_by.get(h["id"], set())
                          if rules["self_review_counts"] or r != h["by"]}
                entry["status"] = "accepted" if h["id"] == birth or others else "unreviewed"
                entry["accepted_by"] = who
                entry["reviewed_by"] = sorted(others)
        elif len(heads) > 1:
            entry["status"] = "divergent" if divergent else "contested"
        else:
            entry["status"] = "empty"
        subjects[subject] = entry
    # judgments: the falsifier on accepted reading values, and what each dependency became
    values = {s: e["body"].get("v") for s, e in subjects.items()
              if e["kind"] == "reading" and e.get("status") == "accepted"}
    for subject, entry in subjects.items():
        if entry["kind"] != "judgment" or "head" not in entry:
            continue
        body = entry["body"]
        pred = body.get("wrong_if")
        fired = _evaluate(pred, values) if pred else None
        entry["fired"] = fired
        if fired is True and entry["status"] in ("accepted", "unreviewed"):
            entry["status"] = "fired"
        deps = {}
        for dep, vid in (body.get("rests_on") or {}).items():
            d = subjects.get(dep)
            if d is None:
                deps[dep] = "missing"
            elif d.get("status") in ("contested", "divergent"):
                deps[dep] = d["status"]
            elif d["marks"].get(vid) == "corrected":
                deps[dep] = "corrected"
            elif d["marks"].get(vid) == "refuted":
                deps[dep] = "refuted"
            elif d.get("head") == vid:
                deps[dep] = "unreviewed" if d["kind"] == "judgment" and d["status"] == "unreviewed" else "same"
            elif vid in d["proposals"]:
                deps[dep] = "proposed"
            elif vid in d["marks"] or vid in versions.get(dep, {}):
                deps[dep] = "moved"
            else:
                deps[dep] = "unknown"
        entry["deps"] = deps
        entry["reservations"] = sorted(dep for dep, s in deps.items() if s in RESERVATIONS)
    return {"subjects": subjects, "implied": implied, "rules": rules}


def history(versions, subject):
    """Every version of a subject in the order recorded, acts included -> [version]."""
    return sorted(versions.get(subject, {}).values(), key=lambda v: (v["on"], v["id"]))


# ── the entry file: a stamped rendering, and a hand edit reconciled against it ──
def render(store, st=None, rules=None, ancestry=None):
    """The state as the entry file - one entry per subject with what stands, the disputes
    beside it, and the stamp of the state it shows -> (text, stamp)."""
    versions = store.read()
    st = st or state(versions, rules, ancestry)
    heads = {s: e["heads"] for s, e in st["subjects"].items()}
    stamp = store.stamp(heads)
    doc = {"meta": {"state": stamp}, "subjects": {}, "disputes": {}}
    for s, e in st["subjects"].items():
        if "head" in e:
            doc["subjects"][s] = dict(e["body"], version=e["head"], status=e["status"])
        else:
            doc["disputes"][s] = {"status": e["status"], "heads": [dict(versions[s][h]["body"], version=h, by=versions[s][h]["by"])
                                                                for h in e["heads"]]}
    if not doc["disputes"]:
        del doc["disputes"]
    return yaml.safe_dump(doc, sort_keys=False, allow_unicode=True), stamp


def write_entry(store, rules=None, ancestry=None):
    text, stamp = render(store, None, rules, ancestry)
    with io.open(os.path.join(store.root, ENTRY), "w", encoding="utf-8") as f:
        f.write(text)
    return stamp


def _same(a, b):
    return canonical(a) == canonical(b)


def ingest(store, text, by="hand", on=None, rules=None, ancestry=None):
    """A hand-edited entry file taken in as versions: each subject the edit changed becomes a
    version whose `saw` is the head the editor's copy showed - read from the stamp - with an
    acceptance over that head. A head that moved since is not pretended seen: the edit lands
    beside it and the two are contested. An unknown stamp is an unknown base: nothing is
    assumed seen, and every changed subject lands beside what stands now. -> [version ids]"""
    try:
        doc = yaml.safe_load(text) or {}
    except yaml.YAMLError as e:
        raise ValueError("the entry file does not read: " + str(e))
    if any(l.startswith(("<<<<<<<", ">>>>>>>", "=======")) for l in text.splitlines()):
        raise ValueError("the entry file carries merge markers - it is rebuilt from the versions, never read")
    stamp = (doc.get("meta") or {}).get("state")
    base_heads = store.heads_at(stamp)
    versions = store.read()
    st = state(versions, rules, ancestry)
    minted = []
    for subject, edited in (doc.get("subjects") or {}).items():
        edited = {k: v for k, v in edited.items() if k not in ("version", "status")}
        base_ids = (base_heads or {}).get(subject) if base_heads is not None else None
        base_body = versions[subject][base_ids[0]]["body"] if base_ids and len(base_ids) == 1 \
            and subject in versions and base_ids[0] in versions[subject] else None
        current = st["subjects"].get(subject)
        current_body = current.get("body") if current else None
        if base_body is not None and _same(edited, base_body):
            continue                                    # the editor left it as they saw it
        if base_body is None and current_body is not None and _same(edited, current_body):
            continue                                    # nothing changed against what stands
        kind = "judgment" if isinstance(edited, dict) and "rests_on" in edited else "reading"
        saw = list(base_ids) if base_ids else []
        v = version(subject, kind, by, edited, saw=saw, on=on)
        store.keep(v)
        minted.append(v["id"])
        # the editor accepted their own edit over the head their copy showed - and only that
        # one: a head that moved since is untouched, and the two meet as contested; with no
        # stamp to read, the edit replaces nothing and lands beside whatever stands
        over = base_ids if base_ids and len(base_ids) == 1 else []
        a = act(subject, by, "accept", of=v["id"], over=over, because="edited by hand", on=on)
        store.keep(a)
        minted.append(a["id"])
    return minted


def take_in(store, other=None, rules=None, ancestry=None):
    """The merge, as the tool runs it: a pending hand edit ingested first against its own
    stamp, then the union of the other store's files when one is named, then the entry file
    rebuilt from the union - never read back as an edit. -> (ingested, added, stamp)"""
    entry = os.path.join(store.root, ENTRY)
    ingested = []
    if os.path.isfile(entry):
        with io.open(entry, encoding="utf-8") as f:
            text = f.read()
        if not any(l.startswith(("<<<<<<<", ">>>>>>>")) for l in text.splitlines()):
            ingested = ingest(store, text, rules=rules, ancestry=ancestry)
    added = store.union_from(other) if other else 0
    stamp = write_entry(store, rules, ancestry)
    return ingested, added, stamp
