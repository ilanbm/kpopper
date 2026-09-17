"""Versions of a subject, the acts on them, and the state computed from both.

A record is a set of immutable versions, kept under `.kpopper/history/` beside the entry
file - the record's source, from which the entry file is built as the current view; not a
backup, and not a directory to delete. A version is one claim about one subject - a
reading, a judgment, or an act on other versions - identified by the hash of everything it
says, kept as one file of its own, so that two branches writing one subject merge under git
without a textual conflict and the disagreement is met by the reader. Two things git does
not do: it carries a history file only once it is committed, so a write is not kept until
the files it added are committed with the record; and its clean merge of history files is
not an agreement on the content - the state is computed from the union, its disputes shown,
and the entry file rebuilt, and a clean merge can hold a dispute nobody has decided. A version keeps three
relations apart: what its writer saw (`saw`, versions of the same subject known when it was
written), what its claim rests on (`rests_on`, dependency versions by id), and what an act
replaced (`over`). It carries the operation that wrote it (`op`, minted once, so a retry is
the same version) and the moment it was recorded (`on`, a timestamp with no authority to
decide anything). Merge is the union of the files. The state - which versions stand, with
what status, and what each judgment's dependencies have become - is computed from the union
and the rules, so the same contributions under the same rules give the same state whatever
the order or number of merges. The entry file is a rendering of that state carrying the
heads it was rendered from, and a hand edit is reconciled three ways against them. This
module is the core under examination; nothing existing reads it.
"""
import copy
import datetime
import hashlib
import io
import json
import os
import re
import uuid

import yaml

HOME = ".kpopper"
STORE = "history"
ENTRY = "GROUNDING.yaml"
RULES = {"version": 2, "self_review_counts": False}
RESERVATIONS = ("corrected", "refuted", "contested", "divergent", "unreviewed", "proposed", "fired",
                "reserved", "unresolved")
ID = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_.\-]*$")
RULE_ACTOR = "rule:source_clock"


class IntegrityError(ValueError):
    """A file that does not say what its name says, or a name that would hold two contents."""


class EditRefused(ValueError):
    """A hand edit the reconciliation does not take, with the file left as it is."""


# ── identity ─────────────────────────────────────────────────────────────────
def canonical(obj):
    return json.dumps(obj, sort_keys=True, ensure_ascii=False, separators=(",", ":"), default=str)


def ident(fields):
    """The identity of a version: the hash of everything it says, the moment and the
    operation included, so that one file name holds one content and a retry of one
    operation is the same file."""
    return hashlib.sha256(canonical({k: v for k, v in fields.items() if k != "id"}).encode("utf-8")).hexdigest()[:40]


def _now():
    return datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat()


def version(subject, kind, by, body, saw=(), at=None, applies=None, on=None, op=None):
    """One claim about a subject -> the version, with its id. `kind` is reading or judgment;
    `saw` the ids of this subject's versions the writer knew; `at` the source's own clock
    value, {day|stamp|revision|commit: ...}, when the source has one; `applies` the time the
    datum is about, when it matters; `on` the moment recorded; `op` the operation - minted
    once by the writer and sent again on a retry, so the retry is this very version, while a
    new observation of the same value is a new operation and a new version."""
    if kind not in ("reading", "judgment"):
        raise ValueError("kind is reading or judgment")
    if not isinstance(subject, str) or not ID.match(subject):
        raise ValueError("a subject is an id: letters, digits, dots, dashes, underscores")
    if not isinstance(body, dict):
        raise ValueError("a body is a mapping")
    fields = {"subject": subject, "kind": kind, "by": by, "on": on or _now(), "op": op or uuid.uuid4().hex,
              "body": copy.deepcopy(body), "saw": sorted(saw)}
    if at is not None:
        fields["at"] = at
    if applies is not None:
        fields["applies"] = applies
    fields["id"] = ident(fields)
    return fields


def act(subject, by, what, of=None, over=(), because="", read=None, saw=(), on=None, op=None):
    """An act on versions of a subject -> a version of kind act. `what` is accept (of one
    version, over the ones it replaces), refute (of one), review (of one, having read the
    dependency versions in `read`), or correct (of one, over the version it corrects). `saw`
    names the acts and versions the actor knew, so a later act that answers an earlier one
    can be told from two that never met."""
    if what not in ("accept", "refute", "review", "correct"):
        raise ValueError("an act is accept, refute, review or correct")
    if not isinstance(subject, str) or not ID.match(subject):
        raise ValueError("a subject is an id")
    body = {"act": what, "of": of, "over": sorted(over), "because": because}
    if read:
        body["read"] = dict(read)
    fields = {"subject": subject, "kind": "act", "by": by, "on": on or _now(), "op": op or uuid.uuid4().hex,
              "body": body, "saw": sorted(saw)}
    fields["id"] = ident(fields)
    return fields


# ── the store: one file per version ──────────────────────────────────────────
class Store(object):
    parsed = 0                      # files parsed by this process, for the cost tests

    def __init__(self, root):
        self.root = os.path.abspath(root)
        self.dir = os.path.join(self.root, HOME, STORE)
        self.problems = []

    def _path(self, subject, vid):
        return os.path.join(self.dir, subject, vid + ".yaml")

    def keep(self, v):
        """The version kept, once. A file already there must say the same: one name, one
        content, or the store refuses rather than hold two."""
        if v.get("id") != ident(v):
            raise IntegrityError("the version's id is not the hash of what it says")
        p = self._path(v["subject"], v["id"])
        text = yaml.safe_dump(v, sort_keys=True, allow_unicode=True)
        if os.path.isfile(p):
            with io.open(p, encoding="utf-8") as f:
                if yaml.safe_load(f) != v:
                    raise IntegrityError("a version file named " + v["id"] + " already holds other content")
            return v["id"]
        os.makedirs(os.path.dirname(p), exist_ok=True)
        readme = os.path.join(self.dir, "README")
        if not os.path.isfile(readme):
            with io.open(readme, "w", encoding="utf-8") as f:
                f.write("This directory is the record's source: every claim, decision and act ever kept, one "
                        "file each, named by what it says. The entry file beside it is the current view built "
                        "from these files. It is not a backup: without it the view cannot be rebuilt, and "
                        "what stood before is gone.\n")
        with io.open(p, "w", encoding="utf-8") as f:
            f.write(text)
        return v["id"]

    def _load(self, subject, name):
        data = self._contents(subject, name)
        return self._parse(subject, name, data) if data is not None else None

    def _contents(self, subject, name):
        p = os.path.join(self.dir, subject, name)
        try:
            with io.open(p, "rb") as f:
                return f.read()
        except OSError as error:
            self.problems.append(subject + "/" + name + ": cannot read history file: " + str(error))
            return None

    def _parse(self, subject, name, data):
        Store.parsed += 1
        try:
            v = yaml.safe_load(data.decode("utf-8"))
        except (yaml.YAMLError, UnicodeError) as error:
            self.problems.append(subject + "/" + name + ": cannot parse history file: " + str(error))
            return None
        if not isinstance(v, dict) or v.get("subject") != subject or v.get("id") != name[:-5] \
                or v.get("id") != ident(v):
            self.problems.append(subject + "/" + name + ": the file does not say what its name says")
            return None
        return v

    def ids(self):
        """Every subject with a directory, and the ids of its files - a listing, no parsing:
        what the index is checked against, and what a checkout that swapped files changes."""
        out = {}
        if not os.path.isdir(self.dir):
            return out
        for subject in sorted(os.listdir(self.dir)):
            d = os.path.join(self.dir, subject)
            if subject.startswith(".") or not os.path.isdir(d):
                continue
            out[subject] = sorted(n[:-5] for n in os.listdir(d) if n.endswith(".yaml"))
        return out

    def subjects(self):
        return {s: len(ids) for s, ids in self.ids().items()}

    def read(self, subjects=None):
        """Every version kept - of the subjects named, else all -> {subject: {id: version}},
        read from the files alone; a file that fails its own identity is a problem, not a
        version."""
        out = {}
        listing = self.ids()
        for subject in sorted(subjects if subjects is not None else listing):
            for vid in listing.get(subject, []):
                v = self._load(subject, vid + ".yaml")
                if v is not None:
                    out.setdefault(subject, {})[vid] = v
        return out

    def get(self, subject, vid):
        """One version by id, or None."""
        p = self._path(subject, vid)
        return self._load(subject, vid + ".yaml") if os.path.isfile(p) else None

    def union_from(self, other):
        """Every version the other store holds and this one does not, copied in - and one
        the two hold under one name with other content refused. Validate the complete
        source listing before copying anything; an unreadable or invalid source is not
        a successful partial union."""
        source = Store(other)
        try:
            held = source.read()
        except OSError as error:
            raise IntegrityError("cannot read union source: " + str(error)) from error
        if source.problems:
            raise IntegrityError("cannot union incomplete source: " + "; ".join(source.problems))
        added = 0
        for subject, versions in held.items():
            for v in versions.values():
                if not os.path.isfile(self._path(subject, v["id"])):
                    self.keep(v)
                    added += 1
                else:
                    self.keep(v)             # the same, or an integrity error
        return added

    @property
    def index_path(self):
        return os.path.join(self.dir, ".index.json")

    def state(self, rules=None, ancestry=None, fresh=False):
        """Reuse a subject's computed entry only when its ids and byte hashes still match.
        Every call reads and hashes every history file, even a warm call with no YAML
        parsing. Stale subjects are parsed from those same bytes; `fresh` parses all.
        The index is this checkout's cache, never the source."""
        rules = dict(RULES, **(rules or {}))
        listing = self.ids()
        index = {}
        if not fresh and os.path.isfile(self.index_path):
            with io.open(self.index_path, encoding="utf-8") as f:
                index = json.load(f)
        if index.get("rules") != rules or index.get("ancestry") != bool(ancestry):
            index = {}
        entries, stale, problems, fingerprints = {}, [], {}, {}
        for subject, ids in listing.items():
            before = len(self.problems)
            contents = {vid: self._contents(subject, vid + ".yaml") for vid in ids}
            fingerprints[subject] = {vid: hashlib.sha256(data).hexdigest()
                                     for vid, data in contents.items() if data is not None}
            cached = (index.get("subjects") or {}).get(subject)
            if (cached and cached.get("ids") == ids and len(fingerprints[subject]) == len(ids)
                    and cached.get("bytes") == fingerprints[subject]):
                if "entry" in cached:
                    entries[subject] = cached["entry"]
                problems[subject] = list(cached.get("problems") or [])
            else:
                stale.append(subject)
                held = {}
                for vid, data in contents.items():
                    v = self._parse(subject, vid + ".yaml", data) if data is not None else None
                    if v is not None:
                        held[vid] = v
                problems[subject] = self.problems[before:]
                if held:
                    e = _subject_entry(subject, held, rules, ancestry)
                    if e is not None:
                        entries[subject] = e
        entries = {s: entries[s] for s in sorted(entries)}
        st = _cross(entries, rules)
        st["problems"] = sorted({m for ms in problems.values() for m in ms})
        if stale or not index:
            os.makedirs(self.dir, exist_ok=True)
            ignore = os.path.join(self.dir, ".gitignore")
            if not os.path.isfile(ignore):
                with io.open(ignore, "w", encoding="utf-8") as f:
                    f.write(".index.json\n")
            kept = {}
            for s in listing:
                kept[s] = {"ids": listing[s], "bytes": fingerprints[s], "problems": problems.get(s, [])}
                if s in entries:
                    kept[s]["entry"] = entries[s]
            with io.open(self.index_path, "w", encoding="utf-8") as f:
                json.dump({"rules": rules, "ancestry": bool(ancestry), "subjects": kept}, f, sort_keys=True)
        return st

    def open_acts(self, subject, ids, st=None):
        """The acts on these claims of a subject that no later act answered - what an act
        laid over them names in `saw`, so that it answers them rather than meets them as a
        dispute. Read from the state, never from the files."""
        st = st or self.state()
        e = st["subjects"].get(subject) or {}
        return sorted({i for c in ids for i in (e.get("open_acts") or {}).get(c, [])})

    def settle(self, rules=None, ancestry=None, on=None):
        """The acts a source rule implies, recorded as acts - so an automatic decision stands
        on the same surface as a person's, and a later reader finds it without recomputing
        it. -> the ids written."""
        st = self.state(rules, ancestry)
        written = []
        for i in st["implied"]:
            a = act(i["subject"], RULE_ACTOR, "accept", of=i["by"], over=[i["superseded"]],
                    because="a later state of the source: " + i["why"], on=on or _now(),
                    saw=self.open_acts(i["subject"], [i["superseded"]], st),
                    op=hashlib.sha256((i["subject"] + i["superseded"] + i["by"]).encode()).hexdigest()[:32])
            self.keep(a)
            written.append(a["id"])
        return written


# ── the source's own clock ───────────────────────────────────────────────────
def _revision_key(v):
    return tuple(int(p) if p.isdigit() else p for p in re.split(r"[.\-]", str(v)))


def _instant(s):
    s = str(s)
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    try:
        t = datetime.datetime.fromisoformat(s)
    except ValueError:
        return None
    if t.tzinfo is None:
        t = t.replace(tzinfo=datetime.timezone.utc)
    return t


def clock_key(at):
    """A sortable key for a clock value of a totally ordered kind - a day, a stamp as an
    instant, a revision as a tuple - or None when the kind orders only along ancestry or the
    value does not read."""
    if not isinstance(at, dict) or len(at) != 1:
        return None
    (k, v), = at.items()
    if k == "day":
        return str(v) if re.match(r"^\d{4}-\d{2}-\d{2}$", str(v)) else None
    if k == "stamp":
        return _instant(v)
    if k == "revision":
        return _revision_key(v)
    return None


def clock_order(a, b, ancestry=None):
    """How two clock values of one source relate -> -1, 0, 1 when the source's clock orders
    them, None when it does not: a day, a stamp and a revision are totally ordered; a commit
    is ordered only along ancestry, which `ancestry(x, y)` answers - is x an ancestor of y."""
    if not isinstance(a, dict) or not isinstance(b, dict) or len(a) != 1 or len(b) != 1:
        return None
    (ka, va), (kb, vb) = list(a.items())[0], list(b.items())[0]
    if ka != kb:
        return None
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
    x, y = clock_key(a), clock_key(b)
    if x is None or y is None:
        return None
    return (x > y) - (x < y)


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


def _claim(v):
    return canonical(v["body"])


def state(versions, rules=None, ancestry=None):
    """The state of every subject from the versions and the acts alone -> {"subjects": {...},
    "implied": [...]}: per subject its heads, their status, the value or body that stands, and
    for a judgment the disposition of each dependency and the reservations it carries."""
    rules = dict(RULES, **(rules or {}))
    entries = {}
    for subject, held in sorted(versions.items()):
        e = _subject_entry(subject, held, rules, ancestry)
        if e is not None:
            entries[subject] = e
    return _cross(entries, rules)


def _subject_entry(subject, held, rules, ancestry, *, claim_key=None):
    """What a subject's own files say -> its entry: heads, status, marks, proposals, the body
    that stands, the reviews on its heads - and the ids it holds, so the cross-subject pass
    never reads a file."""
    claims = {i: v for i, v in held.items() if v["kind"] != "act"}
    acts = {i: v for i, v in held.items() if v["kind"] == "act"}
    if not claims:
        return None
    ordered = sorted(acts.values(), key=lambda a: (a["on"], a["id"]))
    roots = sorted(i for i, v in claims.items() if not v["saw"])
    # every act says one word about a claim: an acceptance says it stands, a refutation says
    # it is out, an acceptance laid over it says it is replaced (or corrected). What decides
    # the claim is the words no later act answered - an act answers another by naming it in
    # `saw`. All open words agree that it stands: it stands. None says so: it is out, marked
    # by the strongest word. Both: a dispute between acts, and the claim stands as contested.
    # A root claim - written knowing nothing of the subject - stands until a word says
    # otherwise; a replaced claim stays replaced when its replacer falls, since a return is
    # a word of its own, given with a reason, never implied.
    words, reviews = {}, []
    for a in ordered:
        b = a["body"]
        if b["act"] in ("accept", "correct") and b["of"] in claims:
            words.setdefault(b["of"], []).append((a, "stands"))
            for o in b["over"]:
                if o in claims and o != b["of"]:
                    words.setdefault(o, []).append((a, ("corrected:" if b["act"] == "correct" else "replaced:")
                                                    + b["of"]))
        elif b["act"] == "refute" and b["of"] in claims:
            words.setdefault(b["of"], []).append((a, "out"))
        elif b["act"] == "review" and b["of"] in claims:
            reviews.append({"of": b["of"], "by": a["by"], "read": b.get("read") or {}})
    standing, proposals, disputed, mark, accepted_by, open_acts = [], [], set(), {}, {}, {}
    for c in sorted(claims):
        ws = words.get(c, [])
        if not ws:
            if c in roots:
                standing.append(c)
                accepted_by[c] = None
            else:
                proposals.append(c)
            continue
        answered = {a["id"] for a, _ in ws for a2, _ in ws if a2 is not a and a["id"] in a2["saw"]}
        open_words = [(a, w) for a, w in ws if a["id"] not in answered]
        open_acts[c] = sorted(a["id"] for a, _ in open_words)
        kinds = {w.split(":")[0] for _, w in open_words}
        if kinds == {"stands"}:
            standing.append(c)
            accepted_by[c] = open_words[-1][0]["by"]
        elif "stands" not in kinds:
            mark[c] = "corrected" if "corrected" in kinds else "refuted" if "out" in kinds else "replaced"
        else:
            standing.append(c)
            disputed.add(c)
            accepted_by[c] = next(a["by"] for a, w in open_words if w == "stands")
    frontier = [claims[i] for i in standing]
    kind = next(iter(claims.values()))["kind"]
    implied = []
    divergent = False
    if kind == "reading" and len(frontier) > 1:
        # the source's own clock, among readings of one source: the latest state stands and
        # the readings of earlier states are superseded by rule - acts implied here, recorded
        # by `settle`. A clock the kind orders totally is settled by its key; a commit clock
        # only along ancestry, pairwise among the few that share a source
        groups = {}
        for v in frontier:
            at = v.get("at")
            source = v["body"].get("from") if isinstance(v["body"], dict) else None
            k = (str(source), list(at)[0] if isinstance(at, dict) and len(at) == 1 else None)
            groups.setdefault(k, []).append(v)
        keep = set(v["id"] for v in frontier)
        for (src, ck), vs in groups.items():
            if ck is None or len(vs) < 2:
                continue
            if ck == "commit":
                for v in vs:
                    for w in vs:
                        if v is not w and v["id"] in keep and clock_order(v["at"], w["at"], ancestry) == -1:
                            keep.discard(v["id"])
                            implied.append({"rule": "source_clock", "subject": subject, "superseded": v["id"],
                                            "by": w["id"], "why": "commit " + str(w["at"]["commit"]) + " descends from "
                                            + str(v["at"]["commit"])})
                            break
                continue
            keyed = [(clock_key(v["at"]), v) for v in vs]
            if any(k is None for k, _ in keyed):
                continue
            top = max(k for k, _ in keyed)
            latest = sorted(v["id"] for k, v in keyed if k == top)[0]
            for k, v in keyed:
                if k < top:
                    keep.discard(v["id"])
                    implied.append({"rule": "source_clock", "subject": subject, "superseded": v["id"],
                                    "by": latest, "why": str(ck) + " " + str(v["at"][ck]) + " before "
                                    + str(claims[latest]["at"][ck])})
        frontier = [v for v in frontier if v["id"] in keep]
        for v in implied:
            mark.setdefault(v["superseded"], "superseded")
        if len(frontier) > 1 and all(isinstance(v.get("at"), dict) and "commit" in v["at"] for v in frontier):
            divergent = True
    heads = [v["id"] for v in frontier]
    entry = {"kind": kind, "heads": heads, "versions": len(claims), "acts": len(acts), "marks": mark,
             "proposals": proposals, "ids": sorted(claims), "implied": implied,
             "writers": {h: claims[h]["by"] for h in heads}, "bodies": {h: claims[h]["body"] for h in heads},
             "reviews": [r for r in reviews if r["of"] in heads], "disputed_acts": sorted(disputed & set(heads)),
             "roots": roots, "open_acts": open_acts}
    # heads that say the same are one claim held by several - agreement, not a dispute
    by_claim = {}
    for v in frontier:
        by_claim.setdefault((claim_key or _claim)(v), []).append(v["id"])
    if len(by_claim) == 1 and heads and not (disputed & set(heads)):
        h = sorted(heads)[0]
        entry.update({"head": h, "body": claims[h]["body"], "agreed": len(heads),
                      "accepted_by": accepted_by.get(h), "writer": claims[h]["by"], "root": h in entry["roots"]})
        entry["status"] = "accepted" if kind == "reading" else "pending"
    elif len(heads) > 1 or (disputed & set(heads)):
        entry["status"] = "divergent" if divergent else "contested"
    else:
        entry["status"] = "empty"
    return entry


def _cross(entries, rules):
    """The pass across subjects, from their entries alone: which reviews still cover their
    version, each judgment's falsifier against the accepted readings, and what each of its
    dependencies became - a chain of assumptions carrying its reservations all the way."""
    subjects = {s: dict(e) for s, e in sorted(entries.items())}
    implied = sorted((i for e in subjects.values() for i in e.get("implied", [])),
                     key=lambda i: (i["subject"], i["superseded"], i["by"]))
    values = {s: e["body"].get("v") for s, e in subjects.items()
              if e["kind"] == "reading" and e.get("status") == "accepted"}
    judgments = [s for s, e in subjects.items() if e["kind"] == "judgment"]
    for s in judgments:
        e = subjects[s]
        if "head" not in e:
            continue
        pred = e["body"].get("wrong_if")
        e["fired"] = _evaluate(pred, values) if pred else None
        # a review covers its version while the dependency versions it read are the heads
        others = set()
        for r in e["reviews"]:
            if r["of"] not in e["heads"]:
                continue
            if not rules["self_review_counts"] and r["by"] == e["writers"].get(r["of"]):
                continue
            if all(vid in (subjects.get(d) or {}).get("heads", ()) for d, vid in r["read"].items()):
                others.add(r["by"])
        e["reviewed_by"] = sorted(others)
    for _ in range(len(judgments) + 1):
        changed = False
        for s in judgments:
            e = subjects[s]
            if "head" not in e:
                continue
            deps = {}
            for dep, vid in (e["body"].get("rests_on") or {}).items():
                d = subjects.get(dep)
                if d is None or vid not in d.get("ids", ()):
                    deps[dep] = "unresolved"
                elif d.get("status") in ("contested", "divergent"):
                    deps[dep] = d["status"]
                elif d["marks"].get(vid) == "corrected":
                    deps[dep] = "corrected"
                elif d["marks"].get(vid) == "refuted":
                    deps[dep] = "refuted"
                elif d.get("head") == vid or vid in d.get("heads", ()):
                    if d["kind"] == "judgment":
                        deps[dep] = ("fired" if d.get("fired") is True else
                                     "unreviewed" if d.get("status") == "unreviewed" else
                                     "reserved" if d.get("reservations") else "same")
                    else:
                        deps[dep] = "same"
                elif vid in d["proposals"]:
                    deps[dep] = "proposed"
                else:
                    deps[dep] = "moved"
            reservations = sorted(dep for dep, st in deps.items() if st in RESERVATIONS)
            if any(st == "unresolved" for st in deps.values()):
                status = "unresolved"
            elif e.get("fired") is True:
                status = "fired"
            elif e.get("root") or e.get("reviewed_by"):
                status = "accepted"
            else:
                status = "unreviewed"
            if e.get("deps") != deps or e.get("status") != status or e.get("reservations") != reservations:
                e["deps"], e["status"], e["reservations"] = deps, status, reservations
                changed = True
        if not changed:
            break
    return {"subjects": subjects, "implied": implied, "rules": rules}


def history(versions, subject):
    """Every version of a subject in the order recorded, acts included -> [version]."""
    return sorted(versions.get(subject, {}).values(), key=lambda v: (v["on"], v["id"]))


# ── the entry file: a rendering that carries its base, and a hand edit reconciled against it ──
def state_hash(st):
    return hashlib.sha256(canonical({"rules": st["rules"], "subjects": {
        s: [e["heads"], e.get("status")] for s, e in st["subjects"].items()}}).encode("utf-8")).hexdigest()[:40]


def render(store, st=None, rules=None, ancestry=None):
    """The state as the entry file - one entry per subject with what stands, the disputes
    beside it, and in its head the heads it was rendered from, so the file carries its own
    base and no snapshot of the state is kept anywhere else -> (text, state hash)."""
    st = st or store.state(rules, ancestry)
    stamp = state_hash(st)
    doc = {"meta": {"state": stamp, "heads": {s: e["heads"] for s, e in st["subjects"].items()},
                    "acts": {s: sorted({i for h in e["heads"] for i in (e.get("open_acts") or {}).get(h, [])})
                             for s, e in st["subjects"].items()}},
           "subjects": {}, "disputes": {}}
    for s, e in st["subjects"].items():
        if "head" in e:
            doc["subjects"][s] = dict(e["body"], version=e["head"], status=e["status"])
        else:
            doc["disputes"][s] = {"status": e["status"],
                                  "heads": [dict(e["bodies"][h], version=h, by=e["writers"][h]) for h in e["heads"]]}
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


def ingest(store, text, by="hand", on=None, rules=None, ancestry=None, op=None):
    """A hand-edited entry file taken in as versions: each subject the edit changed becomes a
    version whose `saw` is the head the editor's copy showed - read from the heads the file
    carries - with an acceptance over that head. A head that moved since is not pretended
    seen: the edit lands beside it and the two are contested. A file that carries no heads
    is an unknown base: nothing is assumed seen. A subject the file dropped is refused - a
    deletion is not an edit - and the file is left as it is. -> [version ids]"""
    try:
        doc = yaml.safe_load(text) or {}
    except yaml.YAMLError as e:
        raise ValueError("the entry file does not read: " + str(e))
    if any(l.startswith(("<<<<<<<", ">>>>>>>", "=======")) for l in text.splitlines()):
        raise ValueError("the entry file carries merge markers - it is rebuilt from the versions, never read")
    meta = doc.get("meta") if isinstance(doc.get("meta"), dict) else {}
    base_heads = meta.get("heads") if isinstance(meta.get("heads"), dict) else None
    base_acts = meta.get("acts") if isinstance(meta.get("acts"), dict) else {}
    st = store.state(rules, ancestry)
    edited_subjects = doc.get("subjects") or {}
    if base_heads is not None:
        dropped = [s for s in base_heads if s in st["subjects"] and s not in edited_subjects
                   and s not in (doc.get("disputes") or {})]
        if dropped:
            raise EditRefused("the edit dropped " + ", ".join(sorted(dropped)) + " - a deletion is not an edit; "
                              "refute or replace what stands, and the file is left as it is")
    minted = []
    for subject, edited in edited_subjects.items():
        if not isinstance(edited, dict):
            continue
        edited = {k: v for k, v in edited.items() if k not in ("version", "status")}
        current = st["subjects"].get(subject)
        if current and "body" in current and _same(edited, current["body"]):
            continue                                    # nothing changed against what stands
        base_ids = base_heads.get(subject) if base_heads is not None else None
        # a rendered agreement is one claim held by several heads: any of them is the base
        base = store.get(subject, base_ids[0]) if base_ids else None
        if base is not None and _same(edited, base["body"]):
            continue                                    # the editor left it as they saw it
        kind = "judgment" if "rests_on" in edited else "reading"
        seed = op or hashlib.sha256((meta.get("state") or "") .encode()).hexdigest()[:16]
        v = version(subject, kind, by, edited, saw=list(base_ids or []), on=on,
                    op=hashlib.sha256((seed + subject + canonical(edited)).encode()).hexdigest()[:32])
        store.keep(v)
        minted.append(v["id"])
        # the edit answers the acts its own copy showed and nothing that arrived since: an act
        # that answered a refutation the editor never saw would erase a reservation by mistake
        over = list(base_ids) if base_ids else []
        a = act(subject, by, "accept", of=v["id"], over=over, because="edited by hand", on=on,
                saw=sorted(set(base_acts.get(subject, [])) | set(over)),
                op=hashlib.sha256(("accept" + v["id"]).encode()).hexdigest()[:32])
        store.keep(a)
        minted.append(a["id"])
    return minted


def take_in(store, other=None, rules=None, ancestry=None):
    """The merge, as the tool runs it: a pending hand edit ingested first against the heads
    its file carries, then the union of the other store's files when one is named, the acts
    a source rule implies recorded, and the entry file rebuilt from the union - never read
    back as an edit. A refused edit stops it with the file untouched.
    -> (ingested, added, stamp)"""
    entry = os.path.join(store.root, ENTRY)
    ingested = []
    if os.path.isfile(entry):
        with io.open(entry, encoding="utf-8") as f:
            text = f.read()
        if not any(l.startswith(("<<<<<<<", ">>>>>>>")) for l in text.splitlines()):
            ingested = ingest(store, text, rules=rules, ancestry=ancestry)
    added = store.union_from(other) if other else 0
    store.settle(rules, ancestry)
    stamp = write_entry(store, rules, ancestry)
    return ingested, added, stamp
