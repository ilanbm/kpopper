#!/usr/bin/env python3
"""Read a PROVENANCE record: check its invariants, and report what a change reaches.

  python3 provenance.py open    [file ...]        the whole opening: head + what moved
  python3 provenance.py check   [file ...]
  python3 provenance.py affects <entry> [entry ...]
  python3 provenance.py pull    <entry> [entry ...]   values and sources for a subject
  python3 provenance.py where                     the record this directory answers for
  python3 provenance.py set     <key> <value> [--why "..."] [--as-of DATE]
  python3 provenance.py add     <id> field=value ... [--in COLLECTION]
  python3 provenance.py review  <id | "section title"> [--as-of DATE]
  python3 provenance.py same    <a> <b> [--keep a|b]  one subject under two ids: b retired into a
  python3 provenance.py distinct <a> <b> "<why>"      two subjects that look alike, told apart
  python3 provenance.py mark    <state file> [file]  the hooks' own: where a session began
  python3 provenance.py gate    <state file> [file]  ... and what it left, said once at its end

The five before them change the record, and each answers with the reach: what rests on
what it wrote, what is MOVED now, which predicate fired. `set --help`, `add --help`,
`review --help`, `same --help`, `distinct --help`. Sameness is judged, never guessed: `add`
names the entries nearest a new one, and `same` or `distinct` records the answer
(sameness.py).

Without a file argument the record is PROVENANCE.yaml here, else the path this checkout
registered in `<git common dir>/kpopper-record` - for a project whose tree cannot hold it.

The record forks on a contradiction, never on a session: a write that contradicts the base
is refused into it and goes into a hypothesis - `PROVENANCE.d/<name>.yaml` beside the record,
the record's own shape - with `--hypothesis <name>`. Every command reads the hypotheses over
the base: `pull` shows what each proposes, `check` and `open` say where two disagree
(CONTESTED), the opener counts what waits, and the base alone is what is evaluated until
consolidation tests the union.

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
import io, os, re, sys, glob, json, shlex, subprocess, datetime, textwrap, tempfile, contextlib, yaml
from decimal import Decimal, InvalidOperation

ID = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+")
EXPR = re.compile(r"[<>=!+\-*/()]|\bor\b|\band\b|\bnot\b")
# A reference: an entry named inside prose, `{{heat.loss_kw}}`, resolved wherever the text is
# shown and never retyped. A judgment id placed this way, `{{c.boiler_short}}`, asks for that
# judgment's reasoning at that spot.
REF = re.compile(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}")
DEFAULT = ["PROVENANCE.yaml"]

# Names the reader computes rather than reads: counted from the record alone (graph.*), or
# where the page is built (page.*). Never written, never stored. A judgment may rest on one
# and a falsifier may draw its line against one - and only a name something in the record
# mentions becomes an entry; the rest do not exist until asked for.
GRAPH = {
    "graph.entries":      "entries the record holds",
    "graph.judgments":    "judgments the record holds",
    "graph.open":         "questions left open",
    "graph.flagged":      "judgments that need a person",
    "graph.blocked":      "judgments waiting on something declared missing",
    "graph.broken":       "judgments resting on something that is not an entry",
    "graph.unchecked":    "judgments never checked against one of their dependencies",
    "graph.moved":        "judgments a dependency moved under since they were reviewed",
    "graph.falsified":    "judgments broken by their own condition",
    "graph.no_predicate": "judgments nothing evaluable would falsify",
    "graph.hypotheses":   "hypotheses waiting beside the record",
    "graph.contested":    "ids two hypotheses hold with different claims",
    "graph.prior_reversal_rate": "share of high-confidence prior-resting judgments refuted",
}
PAGE = {
    "page.spill":           "flagged judgments no section of the page picked up",
    "page.unserved":        "intents no tab of the page serves",
    "page.drift":           "share of what was added since the arrangement was born that nothing picks",
    "page.recent_unserved": "recent sessions in a row whose intent no tab serves",
    "page.covered":         "entries and judgments some section of the page picks",
}
COMPUTED = dict(GRAPH, **PAGE)


def is_builtin(k):
    return k in COMPUTED


def refs_in(text):
    """Entry ids a text names as references, in order, once each."""
    out = []
    for m in REF.finditer(str(text or "")):
        if m.group(1) not in out:
            out.append(m.group(1))
    return out


def _mentioned(body):
    """Every id-shaped token a body carries - in its strings, its lists, its mapping keys."""
    if not isinstance(body, dict):
        return
    for val in body.values():
        if isinstance(val, str):
            for t in ID.findall(val):
                yield t
        elif isinstance(val, list):
            for x in val:
                if isinstance(x, str):
                    for t in ID.findall(x):
                        yield t
        elif isinstance(val, dict):
            for k in val:
                if isinstance(k, str):
                    for t in ID.findall(k):
                        yield t
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


HYPOTHESES = "PROVENANCE.d"      # beside the record: one file per hypothesis, the record's shape
HYPOTHESIS_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_\-]*$")


class Record(dict):
    """The base record's document. The hypotheses beside it ride along as an attribute rather
    than a key, so nothing that walks the document's collections mistakes one for an entry:
    every reader sees the base, and asks for the layer by name."""
    def __init__(self, *a, **k):
        super().__init__(*a, **k)
        self.hypotheses = {}


def hypothesis_dir(paths):
    """Where a record's hypotheses live: PROVENANCE.d beside the file the reader opens first -
    the root of a pointer record, the registered file of a project whose tree holds none."""
    first = sorted(glob.glob(paths[0])) or [paths[0]]
    return os.path.join(os.path.dirname(os.path.abspath(first[0])), HYPOTHESES)


def hypothesis_path(paths, name):
    return os.path.join(hypothesis_dir(paths), name + ".yaml")


def _as_day(v):
    """A date the record wrote - a bare date, an ISO string, or a text that opens with one."""
    if isinstance(v, datetime.datetime):
        return v.date()
    if isinstance(v, datetime.date):
        return v
    m = re.match(r"\s*(\d{4})-(\d{2})-(\d{2})", str(v or ""))
    if not m:
        return None
    try:
        return datetime.date(int(m.group(1)), int(m.group(2)), int(m.group(3)))
    except ValueError:
        return None


def _hypothesis(name, path):
    return {"name": name, "path": path, "head": {}, "doc": {}, "ids": set(), "raw": {}, "error": None}


def load_hypotheses(paths):
    """Every hypothesis beside the record -> {name: hypothesis}, in name order. A hypothesis is
    a file of the record's own shape with an optional head - `hypothesis: {claim, wrong_if,
    born, folds}` - saying what it claims and since when; `folds: never` marks a what-if that
    is only ever evaluated. A file the reader cannot read is kept with its error, so `check`
    fails it and nothing passes over it in silence."""
    out = {}
    d = hypothesis_dir(paths)
    if not os.path.isdir(d):
        return out
    for f in sorted(glob.glob(os.path.join(d, "*.yaml")) + glob.glob(os.path.join(d, "*.yml"))):
        name = re.sub(r"\.ya?ml$", "", os.path.basename(f))
        hyp = _hypothesis(name, f)
        try:
            with io.open(f, encoding="utf-8") as fh:
                body = yaml.safe_load(fh.read()) or {}
            if not isinstance(body, dict):
                raise ValueError("not a mapping of collections")
            head = body.pop("hypothesis", None)
            if head is not None and not isinstance(head, dict):
                raise ValueError("hypothesis: must be a mapping - claim, wrong_if, born, folds")
            hyp["head"] = head or {}
            hyp["doc"] = body
            hyp["raw"] = bodies(body)
            hyp["ids"] = {k for m in collections_of(body).values() for k in m}
        except (yaml.YAMLError, ValueError) as e:
            hyp["error"] = " ".join(str(e).split())[:120]
        out[name] = hyp
    return out


def layered(doc, hyp):
    """The record as it stands under one hypothesis: the base with the hypothesis's entries
    laid over it by id - an id both hold is the hypothesis's here, wherever the base kept it."""
    out = Record()
    for k, v in doc.items():
        out[k] = dict(v) if isinstance(v, dict) else v
    for col, members in collections_of(hyp["doc"]).items():
        for k in members:
            for c2, m2 in out.items():
                if c2 != col and isinstance(m2, dict) and k in m2:
                    del m2[k]
        if isinstance(out.get(col), dict):
            out[col].update(members)
        else:
            out[col] = dict(members)
    out.hypotheses = doc.hypotheses
    return out


def view(doc):
    """-> (ids, jud, fields, raw): what every command reads of a document."""
    ids, jud, fields = infer(doc)
    return ids, jud, fields, with_builtins(doc, ids, jud, fields)


def _layer_view(doc, hyp):
    """The view under a hypothesis -> (view, None); or (None, why) when the layer cannot be read
    by shape - a field the base and the hypothesis give different roles, say."""
    try:
        return view(layered(doc, hyp)), None
    except SystemExit as e:
        return None, " ".join(str(e).split())[:160]


def _verdict_of(body):
    """A judgment's conclusion as the record states it: its verdict, else its title."""
    if not isinstance(body, dict):
        return None
    v = body.get("verdict")
    return v if v is not None else body.get("title")


def claim_of(body):
    """What an id claims, for telling two holders of it apart: a judgment's verdict, an entry's
    value or quoted text, a rule, a conclusion kept under title; a body with none of these
    claims all of itself."""
    if not isinstance(body, dict):
        return body
    for f in ("verdict", "v", "quoted", "rule", "title"):
        if body.get(f) is not None:
            return body[f]
    return body


def _same_claim(a, b):
    if isinstance(a, (dict, list)) or isinstance(b, (dict, list)):
        return a == b
    return _same(a, b)


def contested(doc):
    """Ids two or more hypotheses hold with different claims -> {id: [(name, claim), ...]}, in
    name order. Two hypotheses that agree contest nothing; a person is needed only where they
    do not."""
    holders = {}
    for name, h in sorted((getattr(doc, "hypotheses", None) or {}).items()):
        if h["error"]:
            continue
        for k in h["ids"]:
            holders.setdefault(k, []).append((name, claim_of(h["raw"].get(k))))
    out = {}
    for k, hs in sorted(holders.items()):
        if len(hs) > 1 and any(not _same_claim(hs[0][1], c) for _, c in hs[1:]):
            out[k] = hs
    return out


def _age(born, today):
    d = _as_day(born)
    if not d:
        return "undated"
    n = (today - d).days
    return "today" if n <= 0 else ("1 day" if n == 1 else f"{n} days")


def hypothesis_line(doc, today=None, width=110):
    """The opener's one line about hypotheses: how many wait, and for each - one the reader
    could not read first, then the oldest first - its age and how many judgments rest on what
    it holds. Facts, no threshold: when one has waited too long is a person's reading of the
    line."""
    hyps = getattr(doc, "hypotheses", None) or {}
    if not hyps:
        return ""
    today = today or datetime.date.today()
    first, items = (datetime.date.min, ""), []
    for h in hyps.values():
        if h["error"]:
            items.append((first + (h["name"],), f"{h['name']} (unreadable: {h['error']})"))
            continue
        got, why = _layer_view(doc, h)
        if got is None:
            items.append((first + (h["name"],), f"{h['name']} (unreadable over the base: {why})"))
            continue
        k = sum(1 for j in got[1].values() if set(j["deps"]) & h["ids"])
        bits = [_age(h["head"].get("born"), today)]
        if str(h["head"].get("folds") or "") == "never":
            bits.append("never folds")
        bits.append(f"{k} rest{'s' if k == 1 else ''} on it")
        items.append(((_as_day(h["head"].get("born")) or datetime.date.max, "", h["name"]),
                      f"{h['name']} ({', '.join(bits)})"))
    parts = [p for _, p in sorted(items)]
    n = len(hyps)
    line = f"{n} hypothes{'is waits' if n == 1 else 'es wait'} - " + " · ".join(parts)
    return line if len(line) < width else line[:width] + " ..."


def _every_id(doc, ids):
    """The ids the record holds anywhere: the base's, and every hypothesis's."""
    out = {k for k in ids if not is_builtin(k)}
    for h in (getattr(doc, "hypotheses", None) or {}).values():
        out |= h["ids"]
    return out


def load(paths):
    doc, seen = Record(), set()

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
    doc.hypotheses = load_hypotheses(paths)
    return doc


def collections_of(doc):
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
    collections = collections_of(doc)
    ids = {k for m in collections.values() for k in m}
    # A computed name is an entry the moment something in the record mentions it.
    for members in collections.values():
        for body in members.values():
            for t in _mentioned(body):
                if is_builtin(t):
                    ids.add(t)
    cand = {"deps": {}, "snapshot": {}, "predicate": {}}
    # A field with the shape of a dependency list whose names resolve to nothing votes
    # for no role at all. When that is why the record ends up with no dependency field,
    # it is the whole story - so keep what each one listed and what was missing from it.
    unresolved, present = {}, set()
    for members in collections.values():
        for nid, body in members.items():
            if not isinstance(body, dict):
                continue
            for f, val in body.items():
                present.add(f)
                if f == "refutes" and isinstance(nid, str) and nid.startswith("hyp.") \
                        and body.get("v") == "refuted":
                    continue  # a finding's targets are identities, never its premises
                if isinstance(val, list) and val and all(isinstance(x, str) for x in val):
                    if all(x in ids for x in val):
                        cand["deps"][f] = cand["deps"].get(f, 0) + 1
                    else:
                        unresolved.setdefault(f, []).append(
                            (nid, [x for x in val if x not in ids]))
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

    fields = {"deps": pick("deps")}
    if not fields["deps"]:
        raise SystemExit(_no_deps(unresolved))
    # The snapshot and the predicate are judgment fields, so they are voted on among the
    # bodies that carry the dependency field: a derived entry's rule has a predicate's
    # shape and would otherwise outvote them in any record with more rules than
    # judgments. Prose that names entries as references is prose, never a predicate.
    for members in collections.values():
        for nid, body in members.items():
            if not isinstance(body, dict) or fields["deps"] not in body:
                continue
            for f, val in body.items():
                if f in REOPENED or f in ARRANGEMENT_PROSE:
                    continue           # read by name: it never reads as a predicate or a snapshot
                if isinstance(val, dict) and val and all(k in ids for k in val):
                    cand["snapshot"][f] = cand["snapshot"].get(f, 0) + 1
                elif isinstance(val, str) and val and "{{" not in val:
                    named = [t for t in ID.findall(val) if t in ids]
                    if named and val.strip() not in named and EXPR.search(val):
                        cand["predicate"][f] = cand["predicate"].get(f, 0) + 1
    fields["snapshot"] = pick("snapshot")
    fields["predicate"] = pick("predicate")
    jud = {}
    for members in collections.values():
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
# A judgment decided on a prior - or on taste - names the sign that would re-open it, in
# prose a person reads when it appears. Not a predicate and not a hole: the judgment is
# decided, and this says when to look again. Beside blocked_on, which keeps its meaning -
# the predicate cannot be evaluated, and why - so every record written before it still reads.
REOPENED = ("reopened_by",)
# An arrangement carries two fields the reader reads by name and never by shape: the person's
# request it was taken from (`request:`, a session source), and the decisions it replaced
# (`replaced:`, one line each, naming a count and the sign that ended them - which would
# otherwise read as a predicate).
ARRANGEMENT_PROSE = ("request", "replaced")
OPEN = ("open", "questions")

# The one field this method asks for by name rather than inferring by shape - a sentence
# has no distinctive shape. render_page.py owns the real version of this; this is the
# same idea kept minimal for a reader that only prints text.
NAMES = ("name", "title", "label", "what", "desc")


def named(body):
    if not isinstance(body, dict):
        return ""
    return next((str(body[f]).strip() for f in NAMES if body.get(f)), "")


def human(k):
    return k.split(".")[-1].replace("_", " ")


def fmt(v):
    """Thousands separators on a number a reader sees. Nothing else is touched."""
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return str(v)
    return f"{v:,}" if isinstance(v, int) else f"{v:,.10g}".rstrip()


def reference_text(k, raw, ids, jud, label=None, reasoning=False):
    """What a reference to `k` shows - the one way every surface shows it: the value where
    the entry holds one, its name where it holds only a rule, the verdict for a judgment.
    With `reasoning`, a judgment shows its reasoning instead, as written - the caller
    resolves the references inside it. None when `k` is not an entry, so the caller can
    leave the reference as written."""
    if k in jud:
        b = jud[k]["body"]
        if reasoning and b.get("because"):
            return str(b["because"])
        return str(b.get("verdict") or b.get("title") or k)
    if k not in ids:
        return None
    v = value_of(raw, ids, k)
    if v is not None and not isinstance(v, (list, dict)):
        return fmt(v)
    return label(k) if label else (named(raw.get(k)) or human(k))


def resolve_refs(text, raw, ids, jud, label=None):
    """The text with every reference resolved to what it shows. One that names nothing
    stays as written - that is what `check` fails."""
    def sub(m):
        s = reference_text(m.group(1), raw, ids, jud, label)
        return m.group(0) if s is None else s
    return REF.sub(sub, str(text or ""))


def said(text, raw, ids, jud, width=100):
    """A reasoning as plain lines, references resolved, for a surface that prints text."""
    return textwrap.wrap(" ".join(resolve_refs(text, raw, ids, jud).split()), width) or [""]


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


def _reopened_text(body):
    """The declared re-opener, when the judgment carries one."""
    for k in REOPENED:
        v = body.get(k)
        if v:
            return str(v)
    return ""


def _decided(j):
    """The re-opener that stands for a judgment's falsifier: only when the predicate field
    is empty. Prose in the predicate field is still prose, and a re-opener beside it does
    not make it a declaration - that is what blocked_on says, with the reason."""
    return "" if j["pred"] else _reopened_text(j["body"])


COMPARISON = re.compile(r"^\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*(<=|>=|==|!=|<|>)")


def _misfiled_reopener(body, ids):
    """A re-opener written as a comparison over an entry is a predicate in the wrong field:
    the reader would neither evaluate it nor refuse it, and a line nobody evaluates that
    looks evaluated is the one thing this method refuses. -> the text, or ''."""
    r = _reopened_text(body)
    m = COMPARISON.match(r)
    return r if m and m.group(1) in ids else ""


# The line the consequence matrix draws. At or above it a session's confidence in a claim is
# high enough that a cheap-to-reverse judgment is decided on the prior alone; below it the
# judgment is tried, or it waits for a person. The count says the line in the line it prints:
# a number nobody can see is a number nobody can argue with, and it is the only one this
# count draws - the record keeps that as p.prior_count_thresholds, and the judgment drawn on
# it says a second would be wrong.
HIGH_CONFIDENCE = 0.8


def priors_line(ids, jud, raw, *, with_high=False):
    """How much of the record stands on a session's own confidence: how many judgments rest on
    a prior.* claim, and how many of those on one the record holds at HIGH_CONFIDENCE or above.
    -> the line, or '' where nothing rests on a prior and there is nothing to say. With
    with_high, also return the high-confidence judgment ids from this same pass: their count
    is the reversal share's denominator, and their membership bounds its numerator. Facts,
    never a verdict: a judgment draws the line that would demote the kind to a note."""
    n, high = 0, set()
    for name, j in jud.items():
        priors = [d for d in j["deps"] if d.startswith("prior.")]
        if not priors:
            continue
        n += 1
        for d in priors:
            try:
                # read through str(), the way every other comparison in this reader reads a
                # number: a value too large for a float then lands where a falsifier over it
                # would put it, instead of stopping the count with an error
                if float(str(value_of(raw, ids, d))) >= HIGH_CONFIDENCE:
                    high.add(name)
                    break
            except (TypeError, ValueError, OverflowError):
                continue           # a prior the record does not hold as a number says nothing
    line = (f"{n} judgment{'' if n == 1 else 's'} rest{'s' if n == 1 else ''} on prior.* claims, "
            f"{len(high)} of them on a prior at {HIGH_CONFIDENCE} or above") if n else ""
    return (line, high) if with_high else line


# ── arrangements ─────────────────────────────────────────────────────────────
# An arrangement is a judgment by shape: it rests on a session source - the occasion it
# decides - and its sign is a name the build computes, read by its predicate or rested on.
# Nothing depends on the `v.` prefix, which is only the convention the page's reference
# recommends. What is special about an arrangement is only what it is held against: the
# page's counts, the record's shape kept in the brief, and the decisions that stand.
def is_intent(k, raw):
    """A session source: the one entry shape that carries what was asked."""
    b = raw.get(k)
    return isinstance(b, dict) and bool(b.get("asked"))


def intents_of(j, raw):
    """The session sources a judgment rests on - its occasion, when it is an arrangement."""
    return [d for d in j["deps"] if is_intent(d, raw)]


def is_arrangement(j, raw):
    return bool(intents_of(j, raw)) and (any(is_builtin(t) for t in ID.findall(j["pred"]))
                                          or any(is_builtin(d) for d in j["deps"]))


def _arrangement_shaped(body, fields, raw):
    """Whether a body being written would read as an arrangement."""
    if not _judgment_shaped(body, fields):
        return False
    deps = body[fields["deps"]]
    pred = str(body.get(fields["predicate"]) or "") if fields["predicate"] else ""
    return any(is_intent(d, raw) for d in deps) and (any(is_builtin(t) for t in ID.findall(pred))
                                                     or any(is_builtin(d) for d in deps))


def one_comparison(pred, raw=None, ids=None):
    """An arrangement's sign, as the build can decide it: one comparison - a name, an
    operator, a number or a text or another entry - that can hold. -> '' when it is, else
    what is wrong with it. A compound sign reads as evaluable today and is never decided,
    which is freeze in disguise; a count below zero or a share above one never happens; an
    entry with no value to compare would leave the sign green forever."""
    m = CMP.match(str(pred or ""))
    # the right-hand side must be one value too: a number, a quoted text, or an entry - the
    # comparison shape alone would let "a > 0 or b > 0" through with "0 or b > 0" as its value
    rhs = m.group(3).strip() if m else ""
    quoted = len(rhs) > 1 and rhs[0] in "\"'" and rhs[-1] == rhs[0] and not EXPR.search(rhs[1:-1])
    if not m or not (NUMBER.match(rhs) or ID.fullmatch(rhs) or quoted):
        return "is not one comparison this reader decides (a name, an operator, one value)"
    name, op = m.group(1), m.group(2)
    if ID.fullmatch(rhs) and raw is not None and not is_builtin(rhs) \
            and (rhs not in (ids or ()) or value_of(raw, ids, rhs) is None):
        return f"compares against {rhs}, which holds no value the build can compare"
    try:
        x = float(rhs.replace(",", ""))
    except ValueError:
        return ""
    if is_builtin(name):
        if (op == "<" and x <= 0) or (op in ("<=", "==") and x < 0):
            return f"can never hold - a count is never below zero ({name} {op} {rhs})"
        if name in ("page.drift", "graph.prior_reversal_rate") \
                and ((op == ">" and x >= 1) or (op in (">=", "==") and x > 1)):
            return f"can never hold - a share is never above one ({name} {op} {rhs})"
    return ""


def flags(ids, jud, fields, raw):
    """Per judgment: the conditions that put it in front of a person, derived the one way
    every surface derives them. A predicate over a value `raw` does not carry - a count
    not yet taken - is left undecided, never guessed. A judgment decided with a re-opener
    and no predicate is in front of nobody: it is decided, and a person reads the sign."""
    out = {}
    for name, j in jud.items():
        f, blocked = set(), _blocked_text(j["body"])
        for d in j["deps"]:
            if d not in ids:
                f.add("blocked" if blocked else "broken")
            elif fields["snapshot"] and d not in j["seen"]:
                f.add("unchecked")
        named = [t for t in ID.findall(j["pred"]) if t in ids]
        if not named and not blocked and not _decided(j):
            f.add("no_predicate")
        elif named and evaluate(j["pred"], raw, ids) is True:
            f.add("falsified")
        if any(s == "moved" for _, _, _, s in moved_deps(j, raw, ids)):
            f.add("moved")
        out[name] = f
    return out


def counts(doc, ids, jud, fields, raw):
    """Every graph.* value, counted from the record alone - and taken before any judgment
    that reads a count is decided. A line drawn against "how many are flagged" could
    otherwise be crossed by the drawing of it and uncrossed by the crossing, forever; so a
    count never includes what reading it decided, and every surface that then decides
    those judgments - check, the opener, the page - decides them against the same numbers.
    `raw` here is the record's own bodies, without the counts."""
    open_ids = {k for g in OPEN for k in (doc.get(g) or {})}
    held = [k for k in ids if k not in jud and not is_builtin(k)]
    fl = flags(ids, jud, fields, {k: v for k, v in raw.items() if not is_builtin(k)})

    def count(flag):
        return sum(1 for f in fl.values() if flag in f)
    _, high = priors_line(ids, jud, raw, with_high=True)
    refuted = set()
    for k in held:
        body = raw.get(k)
        if not isinstance(k, str) or not k.startswith("hyp.") \
                or not isinstance(body, dict) or body.get("v") != "refuted":
            continue
        # Consolidation preserves the ids its hypothesis held before deleting the file.
        # Older findings may instead link by a bare id or an explicit {{id}} in name.
        # Repeated findings count the judgment once; unmarked prose is never guessed from.
        if "refutes" in body:
            links = body["refutes"]
            if isinstance(links, list):
                refuted.update(k for k in links if isinstance(k, str) and k in high)
            continue
        claim = body.get("name")
        if isinstance(claim, str):
            refuted.update(high.intersection([claim.strip()] + refs_in(claim)))
    return {
        "graph.entries": len(held), "graph.judgments": len(jud), "graph.open": len(open_ids),
        "graph.flagged": sum(1 for f in fl.values() if f),
        "graph.blocked": count("blocked"), "graph.broken": count("broken"),
        "graph.unchecked": count("unchecked"), "graph.moved": count("moved"),
        "graph.falsified": count("falsified"), "graph.no_predicate": count("no_predicate"),
        "graph.hypotheses": len(getattr(doc, "hypotheses", None) or {}),
        "graph.contested": len(contested(doc)),
        "graph.prior_reversal_rate": len(refuted) / len(high) if high else 0.0,
    }


def builtins(doc, ids, jud, fields, raw):
    """Bodies for the computed names the record mentions. graph.* is counted here from the
    record alone; page.* needs the brief and is counted where the page is built, so here it
    carries its name and no value, and a predicate over it stays undecided."""
    wanted = sorted(k for k in ids if is_builtin(k))
    if not wanted:
        return {}
    values = counts(doc, ids, jud, fields, raw)
    out = {}
    for k in wanted:
        body = {"name": COMPUTED[k]}
        if k in values:
            body["v"] = values[k]
            body["from"] = "counted from the record each time it is read"
        else:
            body["from"] = "counted when the page is built"
        out[k] = body
    return out


def with_builtins(doc, ids, jud, fields):
    """The record's bodies plus the computed ones it mentions: what every reader compares
    values against."""
    raw = bodies(doc)
    raw.update(builtins(doc, ids, jud, fields, raw))
    return raw


def check(paths):
    fail, note, moved, cont, summary = check_lines(paths)
    for n in note:
        print("NOTE", n)
    for m in moved:
        print("MOVED", m)
    for c in cont:
        print("CONTESTED", c)
    for f in fail:
        print("FAIL", f)
    print("\n" + summary)
    return 1 if fail else 0


def check_lines(paths):
    """What check finds -> (fail, note, moved, contested, summary), unprinted: the gate reads
    the same lines the command prints."""
    doc = load(paths)
    ids, jud, fields = infer(doc)
    raw = with_builtins(doc, ids, jud, fields)
    open_ids = {k for g in OPEN for k in (doc.get(g) or {})}
    fail, note, moved = [], [], []
    # A reference in prose is a dependency the sentence declares by naming it: it must be
    # an entry, and inside a judgment it must be one the judgment rests on - or a move in
    # it would never reach the sentence that quotes it.
    for nid, body in sorted(raw.items()):
        if not isinstance(body, dict) or is_builtin(nid):
            continue
        for f, val in body.items():
            if not isinstance(val, str):
                continue
            for r in refs_in(val):
                if r not in ids:
                    fail.append(f"{nid}: {f} references {r}, which is not an entry")
                elif nid in jud and r != nid and r not in jud[nid]["deps"]:
                    fail.append(f"{nid}: {f} references {r}, which it does not declare as a "
                                f"dependency - a change to it would never reach this")
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
        misfiled = _misfiled_reopener(j["body"], ids)
        if misfiled:
            fail.append(f"{name}: reopened_by reads as a comparison ({short(misfiled, 60)}) - a "
                        f"predicate belongs in wrong_if, where it is evaluated; a re-opener is the "
                        f"sign a person reads")
        if not evaluable:
            what = "prose, not an evaluable predicate" if j["pred"] else "no predicate at all"
            reopened = _reopened_text(j["body"])
            if blocked:
                note.append(f"{name}: {what} (declared: {blocked[:90]})")
            elif reopened and not j["pred"]:
                # decided, and it says what would re-open it - a sign a person reads, so
                # the judgment is declared rather than failed, and it is not waiting
                note.append(f"{name}: {what} - decided; reopened by: {reopened[:90]}")
            elif reopened:
                # a re-opener stands for an empty predicate field, never for prose in it
                fail.append(f"{name}: {what} - a re-opener does not stand in for it: a predicate "
                            f"is evaluated, or declared un-evaluable with blocked_on")
            else:
                fail.append(f"{name}: {what} - and nothing says why not, so it can never be "
                            f"re-checked")
        elif evaluate(j["pred"], raw, ids) is True:
            fail.append(f"{name}: wrong_if holds ({j['pred']}) - broken by its own condition")
        elif [t for t in ID.findall(j["pred"]) if t in PAGE]:
            named_page = sorted({t for t in ID.findall(j["pred"]) if t in PAGE})
            note.append(f"{name}: wrong_if reads {', '.join(named_page)}, which is counted "
                        f"when the page is built - `page --verify` decides it")
        # An arrangement's sign is decided by the build - by check for a count of the record,
        # by the page for a count of the page - so it is one comparison that can hold. A
        # re-opener may stand beside it, never in its place: an unevaluable sign on an
        # arrangement is decoration, and decoration is freeze in disguise.
        if is_arrangement(j, raw):
            bad = one_comparison(j["pred"], raw, ids)
            if bad:
                fail.append(f"{name}: wrong_if {bad} - an arrangement's sign is decided by the "
                            f"build, or it is decoration")
            req = j["body"].get("request")
            if req is not None and not (isinstance(req, str) and is_intent(req, raw)
                                        and req in j["deps"]):
                fail.append(f"{name}: request: {req} is not a session source carrying what was "
                            f"asked, that it rests on - a person's word is a source, or it is nobody's")
        # A dependency that moved since the snapshot puts the judgment in front of a
        # person; it does not fail the build. Movement is a question and a crossed line
        # is the answer, and only the predicate can say which line matters.
        for dep, old, now, state in moved_deps(j, raw, ids):
            if state == "moved":
                moved.append(f"{name}: {dep} differs from its snapshot "
                             f"({short(old)} -> {short(now)}) - re-review, or refresh seen")
    # What the record stands on: one line, printed and never failed on - the confidences are
    # the record's to defend, and a count of them is not a problem with it.
    priors = priors_line(ids, jud, raw)
    if priors:
        note.append(priors)
    # Coverage, when a brief sits beside the record: which intents no tab of the page serves,
    # and where what each of them wrote falls - facts the page counted, said here so a session
    # that never builds the page still hears them. The page decides its own falsifiers; the
    # brief held against the arrangements that stand is the record's own claim, so a brief
    # that no longer carries what an arrangement decided fails here too.
    info = _page_or_error(paths)
    if info and "error" in info:
        note.append(f"the brief beside the record could not be built: {info['error']}")
    elif info:
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import render_page as R
        if info.get("coverage"):
            note += R.coverage_lines(info["coverage"], full=False)
        af, an = R.arrangement_lines(info, page_decides=True)
        fail += af
        note += an
    # Hypotheses beside the record: a file the reader cannot read fails; an id two of them hold
    # with different claims needs a person and fails nothing - which of them folds, or neither,
    # is decided at consolidation, where the union is tested.
    cont = []
    for name, h in sorted(doc.hypotheses.items()):
        if h["error"]:
            fail.append(f"hypothesis {name} could not be read: {h['error']}")
            continue
        got, why = _layer_view(doc, h)
        if got is None:
            fail.append(f"hypothesis {name} cannot be read over the base: {why}")
    for k, hs in contested(doc).items():
        cont.append(f"{k}: " + ", ".join(f"{n} says {short(c)}" for n, c in hs)
                    + " - one of them folds, or neither; a person decides")
    held = sum(1 for k in ids if not is_builtin(k))
    summary = (f"{len(jud)} judgments, {held} entries, {len(fail)} problems"
               + (f", {len(moved)} moved" if moved else "")
               + (f", {len(cont)} contested" if cont else "")
               + (f", {len(note)} declared" if note else ""))
    return fail, note, moved, cont, summary


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
    raw = with_builtins(doc, ids, jud, fields)
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
        if _misfiled_reopener(j["body"], ids):
            items.append((100, name, "reopened_by reads as a comparison - a predicate belongs in "
                                     "wrong_if"))
        if not evaluable and not blocked and not _decided(j):
            items.append((40, name, "nothing evaluable would falsify it"))
        elif evaluable and evaluate(j["pred"], raw, ids) is True:
            items.append((95, name, f"wrong_if holds ({j['pred'][:60]}) - broken by its own "
                                    f"condition"))
        for dep, old, now, state in moved_deps(j, raw, ids):
            if state == "moved":
                items.append((70, name, f"{dep} differs from what it last saw: "
                                        f"{short(old, 28)} -> {short(now, 28)}"))
    # an id two hypotheses disagree on is ranked above everything: nothing decides it but a
    # person, and consolidation will refuse to run over it
    for k, hs in contested(doc).items():
        items.append((110, k, "CONTESTED - " + ", ".join(f"{n} says {short(c, 30)}" for n, c in hs)))
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
    prefixes = {}
    for k in ids:
        if k in jud or is_builtin(k):      # judgments are reported separately; counts are not held
            continue
        g = k.split(".")[0] if "." in k else k
        prefixes[g] = prefixes.get(g, 0) + 1
    heavy = [(g, n) for g, n in prefixes.items() if n > 1]
    loose = sum(n for g, n in prefixes.items() if n == 1)
    if heavy:
        head.append("holds: " + " · ".join(f"{g} ({n})" for g, n in
                                            sorted(heavy, key=lambda kv: (-kv[1], kv[0])))
                    + (f" · and {loose} standalone" if loose else ""))
    held = sum(1 for k in ids if not is_builtin(k))
    head.append(f"{held} entries, {len(jud)} judgments"
               + (f", {len(open_ids)} open questions" if open_ids else "")
               + (f", updated {meta['updated']}" if meta.get("updated") else ""))
    # of those judgments, how many stand on a session's own confidence - only where any do:
    # a record with no prior.* claims has nothing to say here and keeps the room
    priors = priors_line(ids, jud, raw)
    if priors:
        head.append(priors)
    # hypotheses beside the record: one line, how many wait and for how long - nothing of what
    # they claim, which is pulled by seed or met as a collision on an id
    waiting = hypothesis_line(doc)
    if waiting:
        head.append(waiting)
    if not fields["snapshot"]:
        head.append("no snapshot field: drift cannot be detected in this record")

    # Each judgment that needs a person is followed by its reasoning, once, with its
    # references resolved to what the record holds now - so the re-reading the flag asks
    # for can start here, against the current values, without opening the file.
    needs = ["nothing needs a person right now."] if not items else [f"needs a person ({len(items)}):"]
    reasoned = set()
    for _, name, why in kept:
        needs.append(f"  {name}: {why}")
        because = jud[name]["body"].get("because") if name in jud else None
        if because and name not in reasoned:
            reasoned.add(name)
            line = "      because: " + said(because, raw, ids, jud, 400)[0]
            needs.append(line if len(line) < 100 else line[:100] + " ...")
    if dropped:
        needs.append(f"  ... {dropped} more - raise the budget to see them")

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
    # An intent no tab of the page serves is said at every open, in the one line that is
    # already about what to do next: the newest first and how many more, never the list -
    # the slot is for what needs a person, and check names the rest with a hint each. An
    # arrangement whose sign appeared comes first: it is the gap read by a decision.
    info = _page_or_error(paths)
    facts = (info.get("arrangements") or {}) if info and "error" not in info else {}
    fired = sorted(k for k, f in facts.items() if f["fired"])
    cov = info.get("coverage") if info and "error" not in info else None
    unserved = [r["id"] for r in cov["rows"] if r["unserved"]] if cov else []
    rest = (" · pull <entry|prefix> (values with sources) · affects <entry> "
            "(what a change reaches)")
    if fired:
        footer = (f"next: check - {fired[0]} fired ({short(jud[fired[0]]['pred'], 40)})"
                  + (f" (and {len(fired) - 1} more)" if len(fired) > 1 else "") + rest)
    elif unserved:
        footer = (f"next: check - {unserved[0]} is served by no tab"
                  + (f" (and {len(unserved) - 1} more)" if len(unserved) > 1 else "") + rest)

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
            req = b.get("request")
            line = f"  = {name}" + (f" (on the word of {req})" if req else "") + f": {v}"
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


def reach_of(ids, jud, raw, changed):
    """What a change to `changed` reaches -> (hit, moved, derived): each judgment reached and
    the entry it was reached through, everything that moved on the way, and the worked-out
    entries among them. A worked-out value carries a change the same way a judgment does:
    anything its rule reads flows on to anything that reads it."""
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
    hit, seen_e, frontier, derived = {}, set(changed), list(changed), []
    while frontier:
        m = frontier.pop()
        for e in sorted(feeds.get(m, [])):
            if e not in seen_e:
                seen_e.add(e)
                derived.append(e)
                frontier.append(e)
        for name, j in sorted(jud.items()):
            if m in j["deps"] and name not in hit:
                hit[name] = m
                frontier.append(name)
    return hit, seen_e | set(hit), derived


def affects(paths, changed):
    doc = load(paths)
    ids, jud, _ = infer(doc)
    raw = bodies(doc)
    # every world the record has: the base, and the base under each hypothesis - a hypothesis's
    # judgment rests on the base too, so a change reaches it, and is said with the hypothesis's
    # name; a hypothesis that replaces a base judgment is walked in its own world
    worlds = [(None, ids, jud, raw)]
    for n, h in sorted(doc.hypotheses.items()):
        got = None if h["error"] else _layer_view(doc, h)[0]
        if got:
            worlds.append((n, got[0], got[1], got[3]))
    every = set().union(*(w[1] for w in worlds))
    # A seed may be an exact entry or a prefix. A question names a subject, not a key.
    expanded = []
    for c in changed:
        if c in every:
            expanded.append(c); continue
        hits = sorted(k for k in every if k.split(".")[0] == c or k.startswith(c + "."))
        if not hits:
            raise SystemExit(f"{c} is not an entry or a prefix in this record. "
                             f"Run `open` to see what it holds.")
        print(f"# {c} -> {len(hits)} entries: {', '.join(hits[:8])}"
              + (" ..." if len(hits) > 8 else ""))
        expanded += hits
    total = 0
    for n, ids_w, jud_w, raw_w in worlds:
        hit, moved, _ = reach_of(ids_w, jud_w, raw_w, [c for c in expanded if c in ids_w])
        if n is not None:
            hit = {k: v for k, v in hit.items() if k in doc.hypotheses[n]["ids"]}
        for name, via in hit.items():
            j = jud_w[name]
            named = [d for d in j["deps"] if d in moved and re.search(rf"\b{re.escape(d)}\b", j["pred"])]
            why = ("evaluate the predicate against " + ", ".join(named)) if named else "flagged only"
            print(f"{name}" + (f" (in hypothesis {n})" if n else "")
                  + f"\n    via {via} -> {why}"
                  + (f"\n    predicate: {j['pred']}" if j["pred"] else ""))
        total += len(hit)
    if not total:
        print("nothing rests on that")
        return 0
    print(f"\n{total} judgments reached")
    return 0


def _pull_raw(doc, ids, jud, fields):
    """The bodies `pull` reads: every entry as recorded - a bare value wrapped - plus the
    computed names the record mentions."""
    raw = {}
    for v in doc.values():
        if isinstance(v, dict):
            for nid, b in v.items():
                if isinstance(b, dict):
                    raw[nid] = b
                elif nid in ids:
                    raw[nid] = {"v": b}
    raw.update(builtins(doc, ids, jud, fields, raw))
    return raw


def pull(paths, seeds, budget=40, doc=None):
    """
    The seeded projection, with values and sources - ground a session on a subject
    instead of reading the whole record for it. Where `affects` walks forward from a
    seed to what depends on it, this reads the seed itself: its entries, as recorded,
    and the judgments that rest on them. The hypotheses beside the record are read over
    it: what each proposes for an id the base holds, what it adds, and where two disagree.
    `doc` is the record already loaded, for a caller that laid something over it - another
    branch's record, read and never written.
    """
    doc = load(paths) if doc is None else doc
    ids, jud, fields = infer(doc)
    raw = _pull_raw(doc, ids, jud, fields)
    # the record as it stands under each hypothesis, in name order
    layers, unread = {}, []
    for n, h in sorted(doc.hypotheses.items()):
        if h["error"]:
            unread.append(f"! hypothesis {n} could not be read: {h['error']}")
            continue
        got, why = _layer_view(doc, h)
        if got is None:
            unread.append(f"! hypothesis {n} cannot be read over the base: {why}")
            continue
        under = layered(doc, h)
        ids_h, jud_h, fields_h = got[0], got[1], got[2]
        layers[n] = (h, ids_h, jud_h, fields_h, _pull_raw(under, ids_h, jud_h, fields_h))
    every = set(ids)
    for _, ids_h, _, _, _ in layers.values():
        every |= ids_h
    disputed = contested(doc)

    def in_hypothesis(k):
        """The layers whose hypothesis holds k -> [(name, layer)]."""
        return [(n, l) for n, l in layers.items() if k in l[0]["ids"]]

    def is_judgment(k):
        return k in jud or any(k in l[2] for _, l in in_hypothesis(k))

    # Seed resolution matches `affects`: an exact id, or a prefix expanded over the
    # namespace. A judgment seed pulls in what it rests on - the point of `pull` is
    # to ground, and a judgment without its own entries is not grounded in anything.
    expanded = []
    for c in seeds:
        if c in every:
            expanded.append(c)
            continue
        hits = sorted(k for k in every if k.split(".")[0] == c or k.startswith(c + "."))
        if not hits:
            raise SystemExit(f"{c} is not an entry or a prefix in this record. "
                             f"Run `open` to see what it holds.")
        expanded += hits

    entries, judgments = set(), set()
    for k in expanded:
        if is_judgment(k):
            judgments.add(k)
            for j in [jud.get(k)] + [l[2].get(k) for _, l in in_hypothesis(k)]:
                if j:
                    entries |= {d for d in j["deps"] if d in every and not is_judgment(d)}
        else:
            entries.add(k)
    judgments |= {name for name, j in jud.items() if set(j["deps"]) & entries}
    for n, (h, ids_h, jud_h, fields_h, raw_h) in layers.items():
        judgments |= {name for name, j in jud_h.items() if name in h["ids"] and set(j["deps"]) & entries}

    def cut(line, n):
        return line if len(line) < n else line[:n] + " ..."

    def resolved(body, ids_):
        """-> (v, rule). A value that is itself a formula is a rule wearing a `v:` field -
        same shape render_page.py already treats that way - so it is never compared as a
        literal here either."""
        v, rule = body.get("v"), body.get("rule")
        if v is not None and isinstance(v, str) and EXPR.search(v) and \
                any(t in ids_ for t in ID.findall(v)):
            v, rule = None, rule or v
        return v, rule

    def describe(k, b, ids_):
        """-> (line, shown): an entry's line, and the value it shows."""
        v, rule = resolved(b, ids_)
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
        return line, shown

    def judgment_lines(name, j, raw_, ids_, jud_, fields_, tag=""):
        body = j["body"]
        verdict = str(body.get("verdict") or body.get("title") or name)
        out = [cut(f"+ {name}{tag}: {verdict}", 110)]

        # State, derived the same way `check` derives a problem: a dependency that
        # is not an entry is broken, unless the judgment declares it missing, in
        # which case it is blocked; short of that, a dependency present but never
        # snapshotted is unchecked; short of that, the judgment holds.
        blocked = _blocked_text(body)
        missing = [d for d in j["deps"] if d not in ids_]
        if missing:
            state = (f"blocked: {blocked}" if blocked else
                     f"broken: rests on {', '.join(missing)}, which is not an entry")
        elif evaluate(j["pred"], raw_, ids_) is True:
            state = f"broken: wrong_if holds ({j['pred']})"
        elif fields_["snapshot"] and any(d not in j["seen"] for d in j["deps"]):
            stale = [d for d in j["deps"] if d not in j["seen"]]
            state = f"unchecked: never checked against {', '.join(stale)}"
        else:
            state = "holds"
        out.append(cut("    " + state, 110))

        # The reasoning, with its references resolved to what the record holds now: an
        # agent that never opens the page reads the argument here, under its own flag.
        if body.get("because"):
            for i, l in enumerate(said(body["because"], raw_, ids_, jud_, 96)):
                out.append(("    because: " if i == 0 else "             ") + l)

        if j["pred"]:
            out.append(cut(f"    wrong_if: {j['pred']}", 110))
        reopened = _reopened_text(body)
        if reopened:
            out.append(cut(f"    reopened by: {reopened}", 110))
        # a decision taken on a person's request says so wherever it is read: the request
        # is the door that let it past the arrangement's sign
        if body.get("request"):
            out.append(cut(f"    on the word of: {body['request']}", 110))

        # Every move since the snapshot, with what the predicate made of it - pull is
        # the grounding surface, so here even a muted move is worth a line.
        for dep, old, now, state in sorted(moved_deps(j, raw_, ids_)):
            mark_ = {"muted": " - within wrong_if", "crossed": " - across wrong_if"}.get(state, "")
            out.append(cut(f"    moved since review: {dep} {old} -> {now}{mark_}", 110))
        return out

    def dispute(k):
        return cut("    CONTESTED: " + ", ".join(f"{n} says {short(c)}" for n, c in disputed[k]), 110)

    lines = list(unread)
    for k in sorted(entries):
        holders = in_hypothesis(k)
        if k in raw:
            line, shown = describe(k, raw.get(k) or {}, ids)
            lines.append(cut(line, 110))
            # what each hypothesis proposes for it - read from the record as it stands under
            # that hypothesis, so a rule or a reference resolves there; a source read again,
            # or a value re-sourced, is a proposal too
            for n, l in holders:
                line_h, now = describe(k, l[4].get(k) or {}, l[1])
                if now != shown:
                    lines.append(cut(f"    proposes {shown} -> {now}, from {n}", 110))
                elif line_h != line:
                    lines.append(cut(f"    proposes instead, from {n}: {line_h[len(k) + 2:].strip()}", 110))
        elif holders:
            n, l = holders[0]
            line, _ = describe(k, l[4].get(k) or {}, l[1])
            lines.append(cut(line + " - held by " + ", ".join(n2 for n2, _ in holders), 110))
        if k in disputed:
            lines.append(dispute(k))

    for name in sorted(judgments):
        holders = [(n, l) for n, l in in_hypothesis(name) if name in l[2]]
        if name in jud:
            lines += judgment_lines(name, jud[name], raw, ids, jud, fields)
            was = str(_verdict_of(jud[name]["body"]) or name)
            for n, l in holders:
                now = str(_verdict_of(l[2][name]["body"]) or name)
                if not _same(was, now):
                    lines.append(cut(f"    proposes instead, from {n}: {now}", 110))
        elif holders:
            n, l = holders[0]
            lines += judgment_lines(name, l[2][name], l[4], l[1], l[2], l[3],
                                    tag=f" (in hypothesis {n})")
        if name in disputed:
            lines.append(dispute(name))

    kept, remain = lines[:budget], max(0, len(lines) - budget)
    for l in kept:
        print(l)
    if remain:
        print(f"... {remain} more lines - raise the budget")
    print("\naffects <entry> shows what a change reaches")
    return 0


# ── the write path ───────────────────────────────────────────────────────────
# set, add and review are how the record changes from the command line, and each answers
# with the reach: what rests on what it wrote, what is MOVED now, which predicate fired.
# The file is edited in place - the lines the write names change and nothing else is
# reformatted, so the diff is the change - and `seen` is never typed: add fills it from
# what the dependencies hold now, review refreshes it the same way. One write function,
# one validation step before it, and every refusal comes from that step - which is where a
# later rule plugs in: a value that contradicts what the record holds, the entries nearest
# a new one.

class Refused(SystemExit):
    """A write the record will not take, said before anything is touched."""


NUMBER = re.compile(r"^-?\d+(\.\d+)?$")
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
BARE = re.compile(r"^[A-Za-z_][A-Za-z0-9_.\-]*$")
MEMBER = re.compile(r"^( +)([A-Za-z_][A-Za-z0-9_.]*):(?: |$)")
TOP = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*):(?: |$)")
ITEM = re.compile(r"^( *)- +title:\s*(.+?)\s*$")
FLOW_VALUE = r"(\"(?:[^\"\\]|\\.)*\"|'(?:[^']|'')*'|[^,}\n]+)"


def typed(text):
    """The value a command-line argument means: a plain number, true or false, else the
    text itself - never the loader's own reading, which turns `on` into a boolean and
    `07:30` into a count of seconds."""
    if NUMBER.match(text):
        return float(text) if "." in text else int(text)
    if text in ("true", "false"):
        return text == "true"
    return text


def _indent(line):
    return len(line) - len(line.lstrip(" "))


def _blank(line):
    s = line.strip()
    return not s or s.startswith("#")


def _bare_ok(s):
    """A word that reads back as itself when written without quotes - one token, no
    spaces; prose and anything digit-led stays quoted, the way the records write them."""
    if not BARE.match(s):
        return False
    try:
        return yaml.safe_load(s) == s
    except yaml.YAMLError:
        return False


def _style(raw_value):
    """How a value is written where it stands: '"', "'", 'date' for a bare date, else 'bare'."""
    s = raw_value.strip()
    if s[:1] in ('"', "'"):
        return s[0]
    return "date" if DATE.match(s) else "bare"


def scalar(v, like="bare", col=0, width=100, fold=True):
    """`v` as the record writes it at column `col`: numbers and booleans bare; a string
    quoted the way the value it replaces was - or bare when it reads back as itself - and
    folded under its opening quote at word boundaries when it would run past `width`."""
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float)):
        return repr(v) if isinstance(v, float) else str(v)
    if v is None:
        return "null"
    if isinstance(v, (datetime.date, datetime.datetime)):
        v = v.isoformat()
    s = str(v)
    if like == "'" and "\n" not in s:
        return "'" + s.replace("'", "''") + "'"
    # a date stays bare only where it already was: written new, it is quoted, as the
    # records write them
    if like == "date" and DATE.match(s):
        return s
    if like in ("bare", "date") and _bare_ok(s):
        return s
    q = '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'
    if not fold or col + len(q) <= width:
        return q
    # a double-quoted scalar folds: one line break reads as one space, so the text is
    # split only at single spaces between non-spaces and every run of spaces stays whole
    words = re.split(r"(?<=\S) (?=\S)", q)
    lines, cur = [], words[0]
    for w in words[1:]:
        if col + len(cur) + 1 + len(w) <= width:
            cur += " " + w
        else:
            lines.append(cur)
            cur = w
    lines.append(cur)
    return ("\n" + " " * (col + 1)).join(lines)


def _field_lines(field, v, ind, width=100, like="bare"):
    """One field as lines at indent `ind`: a scalar folded if long, a list as a flow
    sequence, a mapping as a flow mapping when it fits on the line and one pair per
    line when it does not."""
    key = " " * ind + field + ": "
    if isinstance(v, list):
        return [key + "[" + ", ".join(scalar(x, fold=False) for x in v) + "]"]
    if isinstance(v, dict):
        flow = key + "{" + ", ".join(f"{k}: {scalar(x, fold=False)}" for k, x in v.items()) + "}"
        if len(flow) <= width:
            return [flow]
        out = [" " * ind + field + ":"]
        for k, x in v.items():
            out += _field_lines(k, x, ind + 2, width)
        return out
    return (key + scalar(v, like, len(key), width)).split("\n")


# ── finding things in the text ───────────────────────────────────────────────
def _collections_in(lines):
    """-> [(name, start, end)] for every top-level key, in file order."""
    out, cur = [], None
    for i, l in enumerate(lines):
        m = TOP.match(l)
        if m:
            if cur:
                out.append((cur[0], cur[1], i))
            cur = (m.group(1), i)
    if cur:
        out.append((cur[0], cur[1], len(lines)))
    return out


def _members_of(lines, start, end):
    """-> (indent, [(id, line)]): the entries of a collection - or the fields of an
    entry - as the lines at the member indent, which the first one sets."""
    ind, out = None, []
    for i in range(start + 1, end):
        if _blank(lines[i]):
            continue
        n = _indent(lines[i])
        if ind is None:
            ind = n
        m = MEMBER.match(lines[i])
        if m and len(m.group(1)) == ind:
            out.append((m.group(2), i))
    return ind, out


def _block_end(lines, i, ind, end):
    """The line after the block that starts at `i`: everything indented deeper than
    `ind`, and the blank lines inside it - never the ones that separate it from the next."""
    j = i + 1
    while j < end:
        if not lines[j].strip():
            k = j
            while k < end and not lines[k].strip():
                k += 1
            if k < end and _indent(lines[k]) > ind:
                j = k
                continue
            break
        if _indent(lines[j]) > ind:
            j += 1
            continue
        break
    return j


def _locate(lines, nid):
    """-> (collection, member indent, start, end) of the entry `nid`, or None."""
    for name, s, e in _collections_in(lines):
        ind, members = _members_of(lines, s, e)
        for mid, i in members:
            if mid == nid:
                return name, ind, i, _block_end(lines, i, ind, e)
    return None


def _inline(line):
    """What follows the key on its own line - a flow mapping, a scalar, or nothing."""
    return line.split(":", 1)[1].strip() if ":" in line else ""


def _field_span(lines, s, e, field):
    """-> (field indent, i, j): the field's own lines inside the block [s, e), or None."""
    ind, fields = _members_of(lines, s, e)
    for f, i in fields:
        if f == field:
            return ind, i, _block_end(lines, i, ind, e)
    return None


def _replace_field(lines, s, e, field, new_lines, after=None):
    """Replace the field's lines inside [s, e) - or insert them after the field `after`
    (else at the end of the block). -> the new end of the block."""
    span = _field_span(lines, s, e, field)
    if span:
        _, i, j = span
        lines[i:j] = new_lines
        return e + len(new_lines) - (j - i)
    at = e
    if after:
        span = _field_span(lines, s, e, after)
        if span:
            at = span[2]
    lines[at:at] = new_lines
    return e + len(new_lines)


def _bump_updated(lines, date):
    """meta.updated -> `date`, keeping the line's own quoting and any comment on it."""
    for name, s, e in _collections_in(lines):
        if name != "meta":
            continue
        for i in range(s + 1, e):
            m = re.match(r"^(\s+updated:\s*)(\S+)(\s*(?:#.*)?)$", lines[i])
            if m:
                lines[i] = m.group(1) + scalar(date, _style(m.group(2)), fold=False) + m.group(3)
                return True
        ind, _ = _members_of(lines, s, e)
        lines[s + 1:s + 1] = [" " * (ind or 2) + f"updated: {date}"]
        return True
    return False


# ── what a write answers ─────────────────────────────────────────────────────
def _brief_beside(path):
    b = re.sub(r"\.ya?ml$", "", path) + ".view.yaml"
    return b if os.path.exists(b) else None


def _page_info(paths):
    """What the page knows when it is built beside this record - its counts, its shape, the
    coverage report - or None when no brief sits beside the record."""
    brief = _brief_beside(paths[0])
    if not brief:
        return None
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import render_page as R
    return R.build(paths, brief)[4]


def _page_side(paths):
    """page.* as the page counts them - every name, mentioned or not, since a new judgment
    may be the first to rest on one - the record's shape, and every arrangement held against
    the brief: -> (values, shape, arrangements). ({}, None, {}) without a brief; a count the
    page could not take is left out."""
    info = _page_info(paths)
    if info is None:
        return {}, None, {}
    return ({k: v for k, v in info["page"].items() if v is not None}, info["shape"],
            info.get("arrangements") or {})


def _page_or_error(paths):
    """What the page knows, or None without a brief, or {"error": why} when the brief cannot
    be built - the reader never fails on the page's account, it says so in a line."""
    try:
        return _page_info(paths)
    except (Exception, SystemExit) as e:
        return {"error": str(e)}


def _coverage(paths):
    """The page's coverage report, or None: without a brief, or when the brief cannot be
    built."""
    info = _page_or_error(paths)
    if info and "error" in info:
        return info
    return info["coverage"] if info and info.get("coverage") else None


def _unserved(paths):
    """Intents no tab of the page serves, newest first."""
    cov = _coverage(paths)
    return [r["id"] for r in cov["rows"] if r["unserved"]] if cov and "rows" in cov else []


def snapshot_value(dep, raw, ids, jud, page):
    """What `seen` records for a dependency: the value it holds now, its rule where it has
    only a rule, its verdict where it is a judgment, the date a source was read - each
    exactly as the record holds it, so a later comparison sees a move and never a
    paraphrase. None for a page count the page has not taken."""
    if dep in jud:
        b = jud[dep]["body"]
        return str(b.get("verdict") or b.get("title") or dep)
    if dep in PAGE:
        return page.get(dep)
    b = raw.get(dep)
    if not isinstance(b, dict):
        return b
    v = value_of(raw, ids, dep)
    if v is not None:
        return v
    rule = b.get("rule") or b.get("v")
    if isinstance(rule, str) and rule:
        return rule
    when = b.get("read") or b.get("of")
    if when:
        return f"read {when}"
    return named(b) or "present"


def _state(name, j, raw, ids, fields, touched=()):
    """-> (tag, reason): a judgment's state after a write - the reading `check` gives it,
    said in terms of what just moved."""
    blocked = _blocked_text(j["body"])
    missing = [d for d in j["deps"] if d not in ids]
    if missing:
        return (("BLOCKED", f"waiting on {', '.join(missing)} - {blocked[:70]}") if blocked
                else ("BROKEN", f"rests on {', '.join(missing)}, which is not an entry"))
    named_ = [t for t in ID.findall(j["pred"]) if t in ids]
    if named_ and evaluate(j["pred"], raw, ids) is True:
        return "FIRED", f"wrong_if holds ({j['pred']}) - broken by its own condition"
    unchecked = [d for d in j["deps"] if fields["snapshot"] and d not in j["seen"]]
    moves = [(d, o, n, s) for d, o, n, s in moved_deps(j, raw, ids) if not touched or d in touched]
    if any(s == "moved" for _, _, _, s in moves):
        d, o, n, _ = next(x for x in moves if x[3] == "moved")
        return "MOVED", (f"{d} moved {short(o)} -> {short(n)} since it was reviewed - "
                         f"if it still holds: review {name}")
    if unchecked:
        return "UNCHECKED", (f"never checked against {', '.join(unchecked)} - "
                             f"if it holds: review {name}")
    if any(s == "muted" for _, _, _, s in moves):
        d, o, n, _ = next(x for x in moves if x[3] == "muted")
        return "MUTED", (f"{d} moved {short(o)} -> {short(n)}, inside wrong_if ({j['pred']}) - "
                         f"nothing is asked")
    if not named_ and not blocked:
        reopened = _decided(j)
        if reopened:
            return "HOLDS", "decided; reopened by " + short(reopened, 80)
        return "HOLDS", "nothing evaluable would say otherwise"
    if not named_:
        return "HOLDS", "no predicate to evaluate; declared - " + short(blocked, 80)
    pred = short(j["pred"], 80)
    if evaluate(j["pred"], raw, ids) is None:
        if not CMP.match(j["pred"]):
            return "HOLDS", f"wrong_if is not a comparison this reader decides ({pred})"
        if any(t in PAGE for t in ID.findall(j["pred"])):
            return "HOLDS", f"wrong_if is counted when the page is built ({pred}) - page --verify decides it"
        return "HOLDS", f"wrong_if compares against something with no value to compare ({pred})"
    return "HOLDS", f"wrong_if does not hold ({pred})"


def _state_line(name, j, raw, ids, fields, touched=()):
    tag, why = _state(name, j, raw, ids, fields, touched)
    return f"{tag:<9} {name}: {why}"


def _texts_that_saw(paths, keys):
    """Sections of the brief whose text or seen names one of `keys` -> [(title, seen)]."""
    brief = _brief_beside(paths[0])
    if not brief:
        return []
    b = yaml.safe_load(io.open(brief, encoding="utf-8").read()) or {}
    secs = [s for s in (b.get("sections") or []) if isinstance(s, dict)]
    for tab in (b.get("tabs") or []):
        if isinstance(tab, dict):
            secs += [s for s in (tab.get("sections") or []) if isinstance(s, dict)]
    out = []
    for s in secs:
        if not s.get("text"):
            continue
        saw = dict(s.get("seen") or {})
        hit = [k for k in keys if k in saw or k in refs_in(s["text"])]
        if hit:
            out.append((str(s.get("title") or "?"), {k: saw.get(k) for k in hit}))
    return out


# ── the one validation step ──────────────────────────────────────────────────
def _own_ids(a, doc, ids):
    """The ids the file this write goes into already holds: a hypothesis's own, else the
    base's - an id the base holds and a hypothesis does not is what the hypothesis proposes."""
    h = a.get("hypothesis")
    hyps = getattr(doc, "hypotheses", None) or {}
    return hyps[h]["ids"] if h and h in hyps else ids


def _held_by(doc, k):
    """The hypotheses that hold `k`, in name order."""
    return [n for n, h in sorted((getattr(doc, "hypotheses", None) or {}).items())
            if not h["error"] and k in h["ids"]]


def _judgment_shaped(body, fields):
    """Whether a body would read as a judgment: it carries the dependency field, a list of ids."""
    if not isinstance(body, dict) or not fields["deps"]:
        return False
    deps = body.get(fields["deps"])
    return isinstance(deps, list) and bool(deps) and all(isinstance(x, str) for x in deps)


def _known_key(a, doc, ids, jud, fields, raw):
    out, k = [], a["id"]
    if a["kind"] == "set":
        if k not in ids:
            if a.get("hypothesis") or not _held_by(doc, k):     # else the fork rule says where it is
                out.append(f"{k} is not an entry. A new entry is added, with its source: add {k} v=... from=...")
        elif k in jud:
            out.append(f"{k} is a judgment: it is reviewed, not set")
        elif is_builtin(k):
            out.append(f"{k} is counted by the reader, never set")
        else:
            b = raw.get(k)
            if isinstance(b, dict) and b.get("v") is None and b.get("quoted") is None:
                out.append(f"{k} holds no value of its own"
                           + (" - it is worked out from a rule; change the rule, not the result"
                              if b.get("rule") else ""))
            elif isinstance(b, dict) and value_of(raw, ids, k) is None and b.get("v") is not None:
                out.append(f"{k} is worked out from a rule; change the rule, not the result")
    elif a["kind"] == "add":
        if not re.match(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*$", k):
            out.append(f"{k} is not an id: letters, digits, underscores, dots")
        elif k in jud and not _judgment_shaped(a["body"], fields):
            # what stands under that id is a judgment; a body without dependencies would
            # replace it with an entry, and the judgment would be gone without a trace
            out.append(f"{k} is a judgment - what replaces it rests on something: give "
                       f"{fields['deps']}=[...] with the new verdict, or review it")
        elif k in _own_ids(a, doc, ids) or k in (doc.get("meta") or {}):
            if a.get("hypothesis"):
                out.append(f"{k} is already in hypothesis {a['hypothesis']} - set changes its value "
                           f"there, review its snapshot")
            elif _disagreement(a, raw.get(k), raw, ids, jud, fields,
                               getattr(doc, "page", None)) is None:   # else the fork rule speaks
                out.append(f"{k} is already an entry - set changes its value, review its snapshot")
        elif is_builtin(k):
            out.append(f"{k} is a name the reader computes; it cannot be written")
        if isinstance(a["body"], dict) and fields["snapshot"] and fields["snapshot"] in a["body"]:
            out.append(f"{fields['snapshot']} is written by this tool, from what the "
                       f"dependencies hold - leave it out")
    elif a["kind"] == "review":
        if k not in jud and not a.get("section"):
            out.append(f"{k} is not a judgment" + (" - an entry's value is set, not reviewed"
                                                     if k in ids else ""))
    return out


def _sound_dependencies(a, doc, ids, jud, fields, raw):
    if a["kind"] != "add" or not isinstance(a["body"], dict):
        return []
    out, body = [], a["body"]
    deps = body.get(fields["deps"]) if fields["deps"] in body else None
    if deps is None:
        return out
    if not (isinstance(deps, list) and deps and all(isinstance(x, str) for x in deps)):
        out.append(f"{fields['deps']} must be a list of entry ids")
        return out
    for d in deps:
        if d not in ids and not is_builtin(d) and not _blocked_text(body) \
                and not (a.get("hypothesis") is None and _held_by(doc, d)):
            out.append(f"rests on {d}, which is not an entry - add it first, or declare it "
                       f"missing with blocked_on")
    pred = str(body.get(fields["predicate"]) or "") if fields["predicate"] else ""
    for tok in sorted(set(ID.findall(pred))):
        if (tok in ids or is_builtin(tok)) and tok not in deps:
            out.append(f"wrong_if reads {tok}, which the judgment does not rest on")
    return out


def _sound_references(a, doc, ids, jud, fields, raw):
    if a["kind"] != "add" or not isinstance(a["body"], dict):
        return []
    out, body = [], a["body"]
    deps = body.get(fields["deps"]) if fields["deps"] in body else None
    for f, val in body.items():
        if not isinstance(val, str):
            continue
        for r in refs_in(val):
            if r not in ids and not is_builtin(r):
                if a.get("hypothesis") is None and _held_by(doc, r):
                    continue                    # the fork rule says which hypothesis holds it
                out.append(f"{f} references {r}, which is not an entry")
            elif isinstance(deps, list) and r not in deps and r != a["id"]:
                out.append(f"{f} references {r}, which the judgment does not rest on - a change "
                           f"to it would never reach this")
    return out


def _reopener_is_prose(a, doc, ids, jud, fields, raw):
    """A re-opener is the sign a person reads; one written as a comparison over an entry is
    a predicate in the wrong field, and it is refused before it can look evaluated."""
    if a["kind"] != "add" or not isinstance(a["body"], dict):
        return []
    misfiled = _misfiled_reopener(a["body"], ids)
    if misfiled:
        return [f"reopened_by reads as a comparison ({short(misfiled, 60)}) - a predicate belongs "
                f"in wrong_if, where it is evaluated"]
    return []


def _arrangement_is_sound(a, doc, ids, jud, fields, raw):
    """An arrangement's sign is decided by the build, so it is one comparison that can hold;
    its `born` is the day it is written, stamped by this tool like `seen`; a person's
    request it was taken from is a session source carrying what was asked, rested on."""
    if a["kind"] != "add" or not isinstance(a["body"], dict):
        return []
    if not _arrangement_shaped(a["body"], fields, raw):
        # what replaces an arrangement is an arrangement: a body that drops the occasion or
        # the count would replace the decision with a judgment nothing holds a tab to, and
        # its born and what it replaced would go with it
        if a["id"] in jud and is_arrangement(jud[a["id"]], raw) and _judgment_shaped(a["body"], fields):
            return ["what replaces an arrangement is an arrangement - rest on the session sources "
                    "of the occasion it decides and give it a sign over a count; an occasion read "
                    "elsewhere is re-decided as that, never dropped"]
        return []
    out, body = [], a["body"]
    if "born" in body:
        out.append("born is written by this tool - the day the arrangement is decided - leave it out")
    if "replaced" in body:
        out.append("replaced is written by this tool, when a decision replaces another - leave it out")
    pred = str(body.get(fields["predicate"]) or "") if fields["predicate"] else ""
    bad = one_comparison(pred, raw, ids)
    if bad:
        out.append(f"wrong_if {bad} - an arrangement's sign is decided by the build, or it is "
                   f"decoration")
    req = body.get("request")
    if req is not None and not (isinstance(req, str) and is_intent(req, raw)
                                and req in body[fields["deps"]]):
        out.append(f"request: {req} is not a session source carrying what was asked, that the "
                   f"arrangement also rests on")
    return out


def _not_born_broken(a, doc, ids, jud, fields, raw):
    """A judgment whose own condition holds the moment it is written is a mistake caught
    here, not a record that fails check a second later. A re-decision of an arrangement is
    the one exception: it is recorded while the sign that ended the old decision still
    holds, and the next build decides it against the repaired brief."""
    if a["kind"] != "add" or not isinstance(a["body"], dict) or fields["deps"] not in a["body"]:
        return []
    if a["id"] in jud and is_arrangement(jud[a["id"]], raw):
        return []
    pred = str(a["body"].get(fields["predicate"]) or "") if fields["predicate"] else ""
    if pred and evaluate(pred, raw, ids) is True:
        return [f"wrong_if already holds ({pred}) - the judgment would be born broken"]
    return []


def _read_on(body, raw):
    """The day an entry's value was read: its own `of` or `read`, else its source's read date -
    a day is the finest clock the record keeps. None when nothing dates it."""
    if not isinstance(body, dict):
        return None
    for f in ("of", "read"):
        d = _as_day(body.get(f)) if body.get(f) is not None else None
        if d:
            return d
    src = body.get("from")
    if isinstance(src, str) and isinstance(raw.get(src), dict):
        for f in ("read", "of"):
            d = _as_day(raw[src].get(f)) if raw[src].get(f) is not None else None
            if d:
                return d
    return None


def may_supersede(nid, existing, new, raw, ids, jud, fields, as_of=None, page=None):
    """May a write under an id the base holds replace what it holds? -> (yes, why). The one
    door every same-id write goes through - `set` of a value, `add` of a judgment under a
    standing id - so the rules that open it live here and nowhere else. An entry: when the
    new reading is newer than the base's own - its `of:`, else its source's read date; a day
    is the finest clock the record keeps, and a value nothing dates is superseded by one
    something does. A judgment: when the standing one's wrong_if holds now - it is broken,
    and the new verdict is its repair - or when the new body names a person's request with
    `request: s.<date>_<slug>`, a session source whose asked: is that request verbatim, and
    rests on it too: a request in a person's words is a root nothing outranks. Resting on a
    session source alone says nothing - every judgment rests on the session that wrote it.
    Everything else contradicts, and a contradiction forks. `existing` is the base's body,
    `new` the value or the body being written.

    An arrangement - `page` carries what the page holds it to - has three rules of its own
    before those: never twice in a day, so two sessions cannot flip it and a second writer
    cannot re-decide it behind the first while the first's repair is still being made (a
    person's request is the exception, a root nothing outranks); only while the brief still
    carries what it decided, since a tab deleted or gutted makes the very sign it would
    cite - so a cut link is refused with "restore, then re-decide"; and its sign read with
    its tabs intact is what "its wrong_if holds" means for it."""
    if nid in jud:
        if not _judgment_shaped(new, fields):
            return False, "what replaces a judgment must rest on something, and this carries no " \
                          + str(fields["deps"] or "dependencies")
        facts = (page or {}).get(nid)
        req = new.get("request") if isinstance(new, dict) else None
        if facts is not None:
            stamp = _as_day(as_of) or datetime.date.today()
            born = _as_day(jud[nid]["body"].get("born"))
            if born and born >= stamp and not req:
                return False, (f"it was decided on {born} - a second decision on the same day is a "
                               f"contradiction, not a change")
            if not req and not facts["linked"]:
                return False, ("the brief no longer carries what it decided - " + facts["cut"]
                               + " - restore the tab, then re-decide")
            if not req and facts["fired"]:
                return True, (f"its sign holds ({short(jud[nid]['pred'], 60)}) with its tab intact"
                              + (f" - {facts['reading']}" if facts.get("reading") else ""))
        if evaluate(jud[nid]["pred"], raw, ids) is True:
            return True, f"its wrong_if holds ({short(jud[nid]['pred'], 60)})"
        deps = new.get(fields["deps"])
        req = new.get("request") if isinstance(new, dict) else None
        if isinstance(req, str) and req:
            b = raw.get(req)
            if isinstance(b, dict) and b.get("asked") and req in deps:
                return True, f"a person asked - it carries request: {req} and rests on it"
            return False, (f"request: {req} is not a session source carrying what was asked, that "
                           f"the new judgment also rests on")
        return False, ("the standing judgment holds, and no request: names a person's asking for "
                       "the change")
    when = _read_on(existing, raw)
    stamp = _as_day(as_of) or datetime.date.today()
    if when is None:
        return True, "nothing dates the reading the base holds"
    if stamp > when:
        return True, f"a reading from {stamp} that is newer than the base's"
    return False, ("a reading of the same day" if stamp == when
                   else f"a reading from {stamp} that is older than the base's")


def _disagreement(a, body, raw, ids, jud, fields, page=None):
    """What the write says against what the base holds under that id -> None when they agree,
    or when the id claims nothing comparable; else (kind, old, new, may, why, when): the two
    claims, whether the write may supersede the base and why, and the base's day. An
    arrangement written again with its verdict kept is a re-decision all the same - its
    occasion or its sign changed - so for one, any difference in what the session writes
    counts."""
    k = a["id"]
    if k in jud:
        if a["kind"] != "add" or not _judgment_shaped(a["body"], fields):
            return None
        old, new = _verdict_of(jud[k]["body"]), _verdict_of(a["body"])
        if old is None or new is None:
            return None
        if _same(old, new):
            if not is_arrangement(jud[k], raw):
                return None
            skip = ("born", "replaced")
            was = {f: v for f, v in jud[k]["body"].items()
                   if f not in skip and f != fields["snapshot"]}
            now = {f: v for f, v in a["body"].items() if f not in skip and f != fields["snapshot"]}
            if was == now:
                return None
        may, why = may_supersede(k, jud[k]["body"], a["body"], raw, ids, jud, fields,
                                 a.get("as_of"), page)
        return "verdict", old, new, may, why, None
    if not isinstance(body, dict):
        return None
    old = value_of(raw, ids, k)
    if old is None or isinstance(old, (list, dict)):
        return None
    if a["kind"] == "set":
        new = a["value"]
    else:
        b = a["body"] if isinstance(a["body"], dict) else {}
        new = b.get("v") if b.get("v") is not None else b.get("quoted")
        if new is None or isinstance(new, (list, dict)):
            return None
    if _same(old, new):
        return None
    may, why = may_supersede(k, body, new, raw, ids, jud, fields, a.get("as_of"))
    return "value", old, new, may, why, _read_on(body, raw)


def _command_of(a, name):
    """The write as it was asked, aimed at a hypothesis instead: the exact command."""
    parts = [a["kind"], a["id"]]
    if a["kind"] == "set":
        parts.append(scalar(a["value"], "'", fold=False) if isinstance(a["value"], str)
                     and not _bare_ok(a["value"]) else str(a["value"]).lower()
                     if isinstance(a["value"], bool) else str(a["value"]))
    elif a["kind"] == "add":
        body = a["body"]
        if isinstance(body, dict):
            for f, v in body.items():
                if isinstance(v, list):
                    s = "[" + ", ".join(scalar(x, fold=False) for x in v) + "]"
                elif isinstance(v, dict):
                    s = "{" + ", ".join(f"{k2}: {scalar(x, fold=False)}" for k2, x in v.items()) + "}"
                elif isinstance(v, bool):
                    s = str(v).lower()
                else:
                    s = str(v)
                parts.append(f"{f}={s}")
        else:
            parts.append(str(body))
    for opt, key in (("--in", "into"), ("--as-of", "as_of"), ("--why", "why")):
        if a.get(key):
            parts += [opt, str(a[key])]
    parts += ["--hypothesis", name]
    return " ".join(p if p.startswith("--") else shlex.quote(p) for p in parts)


def _hypothesis_name(doc, k, claim):
    """A name for the hypothesis a refused write would open: the id's, unless a hypothesis
    of that name already holds the id with another claim."""
    base = re.sub(r"[^A-Za-z0-9_\-]", "_", k)
    hyps = getattr(doc, "hypotheses", None) or {}
    name, n = base, 1
    while name in hyps and not (k in hyps[name]["ids"]
                                and _same_claim(claim_of(hyps[name]["raw"].get(k)), claim)):
        n += 1
        name = f"{base}_{n}"
    return name


def _forks_on_contradiction(a, doc, ids, jud, fields, raw):
    """The record forks on a contradiction, told by the id and the day. A value the base
    already holds under that id, read no later than the base's own reading and differing
    from it, is refused into the base - two readings of one day that disagree are two
    writers, not the world moving - and so is a verdict that differs under a judgment's id;
    each refusal names the command that writes it into a hypothesis instead. A write that
    rests on what only a hypothesis holds belongs in that hypothesis. A write aimed at a
    hypothesis is never refused for disagreeing: the file is where disagreement goes."""
    if a.get("hypothesis"):
        return []
    out, k = [], a["id"]
    if a["kind"] == "set":
        if k not in ids:
            held = _held_by(doc, k)
            if held:
                out.append(f"{k} is held only by hypothes{'is' if len(held) == 1 else 'es'} "
                           f"{', '.join(held)} - it is set there: {_command_of(a, held[0])}")
            return out
        d = _disagreement(a, raw.get(k), raw, ids, jud, fields)
        if d and not d[3]:
            _, old, new, _, why, when = d
            out.append(f"{k} holds {short(old)} as of {when}, and {why} says {short(new)} - the base "
                       f"keeps what it holds and a hypothesis holds the other: "
                       f"{_command_of(a, _hypothesis_name(doc, k, new))}")
        return out
    if a["kind"] != "add":
        return out
    if k in ids:
        d = _disagreement(a, raw.get(k), raw, ids, jud, fields, getattr(doc, "page", None))
        if d and not (d[0] == "verdict" and d[3]):        # a verdict that may supersede replaces
            kind, old, new, newer, why, when = d
            name = _hypothesis_name(doc, k, new)
            if kind == "verdict":
                out.append(f"{k} is already a judgment, concluding {short(old, 60)!r} - {why}, so a "
                           f"different verdict under the same id contradicts it, and a hypothesis "
                           f"holds the other: {_command_of(a, name)}")
            elif newer:
                out.append(f"{k} is already an entry, holding {short(old)}"
                           + (f" as of {when}" if when else "")
                           + f" - a newer reading updates it: set {k} {shlex.quote(str(new))}; one "
                           f"that disagrees opens a hypothesis: {_command_of(a, name)}")
            else:
                out.append(f"{k} is already an entry, holding {short(old)} as of {when}, and {why} "
                           f"says {short(new)} - the base keeps what it holds and a hypothesis holds "
                           f"the other: {_command_of(a, name)}")
    # the cone: whatever rests on a hypothesis goes into it, so that it folds - or is
    # refuted - together with what it rests on
    body = a["body"] if isinstance(a["body"], dict) else {}
    deps = body.get(fields["deps"]) if fields["deps"] and fields["deps"] in body else None
    deps = [d for d in deps if isinstance(d, str)] if isinstance(deps, list) else []
    refs = []
    for f, val in body.items():
        if isinstance(val, str):
            refs += [r for r in refs_in(val) if r not in refs]
    for dep, how in [(d, "rests on") for d in deps] + [(r, "references") for r in refs if r not in deps]:
        if dep in ids or is_builtin(dep):
            continue
        held = _held_by(doc, dep)
        if held:
            out.append(f"{how} {dep}, which only hypothes{'is' if len(held) == 1 else 'es'} "
                       f"{', '.join(held)} hold{'s' if len(held) == 1 else ''} - what rests on a "
                       f"hypothesis goes into it: {_command_of(a, held[0])}")
    return out


def _nearest_existing(a, doc, ids, jud, fields, raw):
    """The entries nearest a new one, said in the reply - a note, never a refusal - and a
    write under an id that was retired into another, refused and pointed at it. Both live in
    sameness.py, beside the commands that record the answer."""
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import sameness
    return sameness.nearest_existing(a, doc, ids, jud, fields, raw)


# Every refusal a write can meet, in one place. The fork on a contradiction is the last of
# them; the entries nearest a new one are said just before it.
VALIDATORS = [_known_key, _sound_dependencies, _sound_references, _reopener_is_prose,
              _arrangement_is_sound, _not_born_broken, _nearest_existing, _forks_on_contradiction]


def validate(action, doc, ids, jud, fields, raw):
    out = []
    for check_ in VALIDATORS:
        out += check_(action, doc, ids, jud, fields, raw)
    return out


# ── the edits, on text ───────────────────────────────────────────────────────
def _set_in(lines, key, value, stamp, why):
    """-> (old value as written, field). The value field of `key` rewritten in place, in
    the style it already had; `of:` stamped; the reason, if any, as a comment beneath."""
    loc = _locate(lines, key)
    name, ind, s, e = loc
    if _inline(lines[s]).startswith("{"):
        block = "\n".join(lines[s:e])
        m = re.search(r"\b(v|quoted):\s*" + FLOW_VALUE, block)
        if not m:
            raise Refused(f"{key} carries no v: this reader can rewrite")
        old = m.group(2).strip()
        new = scalar(value, _style(old), fold=False)
        block = block[:m.start(2)] + new + block[m.end(2):]
        mo = re.search(r"\bof:\s*" + FLOW_VALUE, block)
        if mo:
            block = block[:mo.start(1)] + scalar(stamp, _style(mo.group(1)), fold=False) + block[mo.end(1):]
        else:
            at = m.start(2) + len(new)
            block = block[:at] + f', of: "{stamp}"' + block[at:]
        new_lines = block.split("\n")
        if why:
            new_lines.append(" " * (ind + 2) + f"# set {stamp}: {why}")
        lines[s:e] = new_lines
        return old, m.group(1)
    span = _field_span(lines, s, e, "v") or _field_span(lines, s, e, "quoted")
    if not span:
        raise Refused(f"{key} carries no v: this reader can rewrite")
    find, i, j = span
    field = lines[i].strip().split(":", 1)[0]
    old = " ".join(_inline(lines[i]).split() + [l.strip() for l in lines[i + 1:j]])
    key_text = " " * find + field + ": "
    new_lines = (key_text + scalar(value, _style(old), len(key_text))).split("\n")
    lines[i:j] = new_lines
    e += len(new_lines) - (j - i)
    of = _field_span(lines, s, e, "of")
    if of:
        _, oi, oj = of
        lines[oi:oj] = [" " * find + "of: " + scalar(stamp, _style(_inline(lines[oi])), fold=False)]
        e += 1 - (oj - oi)
    else:
        at = i + len(new_lines)
        lines[at:at] = [" " * find + f'of: "{stamp}"']
        e += 1
    if why:
        at = i + len(new_lines) + (0 if of else 1)
        lines[at:at] = [" " * find + f"# set {stamp}: {why}"]
    return old, field


def _entry_lines(nid, body, ind, find, flow):
    if not isinstance(body, dict):
        return _field_lines(nid, body, ind)
    if flow:
        one = (" " * ind + nid + ": {"
               + ", ".join(f"{k}: {scalar(v, fold=False)}" if not isinstance(v, list)
                           else f"{k}: [" + ", ".join(scalar(x, fold=False) for x in v) + "]"
                           for k, v in body.items()) + "}")
        if len(one) <= 100 and not any(isinstance(v, dict) for v in body.values()):
            return [one]
    out = [" " * ind + nid + ":"]
    for k, v in body.items():
        out += _field_lines(k, v, find)
    return out


def _replace_in(lines, nid, body):
    """The entry's lines replaced whole by `body`, where it stands and in its style."""
    _, ind, s, e = _locate(lines, nid)
    flow = _inline(lines[s]).startswith("{")
    find = _members_of(lines, s, e)[0] or ind + 2
    lines[s:e] = _entry_lines(nid, body, ind, find, flow)


def _place(members, nid):
    """Where a new id goes among the members of its collection, in file order: beside
    the entries it shares the longest dotted prefix with, in id order among them - never
    at the tail for being new. -> (anchor id, before) or None for an empty collection."""
    if not members:
        return None

    def common(a, b):
        x, y, n = a.split("."), b.split("."), 0
        while n < min(len(x), len(y)) and x[n] == y[n]:
            n += 1
        return n
    best = max(common(m, nid) for m in members)
    sib = [m for m in members if common(m, nid) == best] if best else list(members)
    later = [m for m in sib if m > nid]
    return (later[0], True) if later else (sib[-1], False)


def _add_in(lines, nid, body, collection):
    """-> a line saying where it went. The entry inserted into `collection`, in id order
    beside its nearest siblings, in the style of the entry it lands beside."""
    cols = {n: (s, e) for n, s, e in _collections_in(lines)}
    if collection not in cols:
        raise Refused(f"no collection {collection} in this file")
    s, e = cols[collection]
    ind, members = _members_of(lines, s, e)
    if ind is None or not members:
        ind = ind or 2
        new = _entry_lines(nid, body, ind, ind + 2, False)
        lines[s + 1:s + 1] = new
        return f"{nid} into {collection}, its first entry"
    at = _place([m for m, _ in members], nid)
    anchor, before = at
    ai = next(i for m, i in members if m == anchor)
    ae = _block_end(lines, ai, ind, e)
    flow = _inline(lines[ai]).startswith("{")
    find = _members_of(lines, ai, ae)[0] or ind + 2
    new = _entry_lines(nid, body, ind, find, flow)
    if before:
        pos = ai
        if pos > 0 and not lines[pos - 1].strip() and pos - 1 > s:
            new = new + [""]
    else:
        pos = ae
        if pos < e and not lines[pos].strip():
            pos += 1
            new = new + [""]
    lines[pos:pos] = new
    return f"{nid} into {collection}, {'before' if before else 'after'} {anchor}"


def _seen_lines(lines, s, e, snapshot_field, seen):
    """The judgment's snapshot rewritten in place, in the style it had."""
    span = _field_span(lines, s, e, snapshot_field)
    find = span[0] if span else (_members_of(lines, s, e)[0] or 4)
    return _replace_field(lines, s, e, snapshot_field, _field_lines(snapshot_field, seen, find))


def _stamp_field(lines, s, e, field, stamp, after):
    """A date field rewritten if present, in its own style, keeping every comment on or
    under it; added after `after` if not."""
    span = _field_span(lines, s, e, field)
    if span:
        find, i, j = span
        m = re.match(r"^(\s+" + re.escape(field) + r":\s*)(\S+)(\s*(?:#.*)?)$", lines[i])
        head = (m.group(1) + scalar(stamp, _style(m.group(2)), fold=False) + m.group(3)) if m \
            else " " * find + f"{field}: " + scalar(stamp, "bare", fold=False)
        kept = [l for l in lines[i + 1:j] if l.strip().startswith("#")]
        lines[i:j] = [head] + kept
        return e + 1 + len(kept) - (j - i)
    find = _members_of(lines, s, e)[0] or 4
    return _replace_field(lines, s, e, field, [" " * find + f'{field}: "{stamp}"'], after=after)


def _section_span(lines, title, having=None):
    """-> (field indent, start, end) of the brief item titled `title` - a section or a tab -
    and, with `having`, the first such item that carries that field, so a tab and a section
    with one title are told apart by what they hold. None when there is none."""
    for i, l in enumerate(lines):
        m = ITEM.match(l)
        if not m:
            continue
        got = m.group(2).strip()
        if got[:1] in ('"', "'"):
            got = got[1:-1]
        if got != title:
            continue
        ind = len(m.group(1))
        j = i + 1
        while j < len(lines) and (not lines[j].strip() or _indent(lines[j]) > ind):
            j += 1
        while j > i + 1 and not lines[j - 1].strip():
            j -= 1
        if having and not _field_span(lines, i, j, having):
            continue
        return ind + 2, i, j
    return None


def _review_section_in(lines, title, seen, stamp):
    """The section's seen and reviewed rewritten - `- title:` puts its fields two columns
    in from the dash, and a field is replaced where it stands or added after the text."""
    span = _section_span(lines, title, having="text")
    if not span:
        raise Refused(f"no section titled {title!r} with text in the brief")
    find, s, e = span
    e = _replace_field(lines, s, e, "seen", _field_lines("seen", seen, find), after="text")
    _replace_field(lines, s, e, "reviewed", [" " * find + f'reviewed: "{stamp}"'], after="text")


def _tabs_for(brief, nid, facts):
    """The tabs whose shape an arrangement's review rewrites: the brief's one shape when it
    has no tabs; else every tab whose picks earn a session source the arrangement rests on,
    as the page links them - an arrangement that decides several occasions at once governs
    every one of them. -> [(title, is_tab)], empty when no tab earns any of its sources."""
    tabs = [t for t in (brief.get("tabs") or []) if isinstance(t, dict)]
    if not tabs:
        return [(None, False)]
    f = (facts or {}).get(nid) or {}
    return [(t, True) for t in (f.get("tabs") or [])]


def _shape_in(lines, where, shape):
    """The shape line rewritten in place: the brief's own, or one tab's."""
    title, is_tab = where
    if not is_tab:
        cols = {n: (s, e) for n, s, e in _collections_in(lines)}
        if "shape" not in cols:
            lines.append("shape: {" + ", ".join(f"{k}: {v}" for k, v in shape.items()) + "}")
            return
        s, e = cols["shape"]
        block = _inline(lines[s]).startswith("{")
        lines[s:e] = _field_lines("shape", shape, 0) if block else \
            ["shape:"] + [f"  {k}: {v}" for k, v in shape.items()]
        return
    span = _section_span(lines, title, having="sections")
    if not span:
        raise Refused(f"no tab titled {title!r} in the brief")
    find, s, e = span
    old = _field_span(lines, s, e, "shape")
    flow = not old or _inline(lines[old[1]]).startswith("{")
    new = _field_lines("shape", shape, find) if flow else \
        [" " * find + "shape:"] + [" " * (find + 2) + f"{k}: {v}" for k, v in shape.items()]
    _replace_field(lines, s, e, "shape", new)


# ── the one write ────────────────────────────────────────────────────────────
def _write_text(path, text):
    """The whole file replaced in one step, so a reader never meets half a record; the
    temporary file is this writer's own, and the record keeps its permissions."""
    d = os.path.dirname(os.path.abspath(path))
    fd, tmp = tempfile.mkstemp(prefix="." + os.path.basename(path) + ".", suffix=".tmp", dir=d)
    with io.open(fd, "w", encoding="utf-8") as f:
        f.write(text)
    if os.path.exists(path):
        os.chmod(tmp, os.stat(path).st_mode & 0o7777)
    os.replace(tmp, path)


@contextlib.contextmanager
def _locked(path):
    """One writer at a time on a record: the whole write - load, validate, edit, replace -
    runs under an exclusive lock on the record's directory, so two sessions on one file
    take turns instead of the last one silently discarding the first. Where the platform
    offers no such lock the write is unguarded."""
    try:
        import fcntl
    except ImportError:
        yield
        return
    fd = os.open(os.path.dirname(os.path.abspath(path)), os.O_RDONLY)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)


def _files_of(paths):
    """The files `load` reads, in order - every pointer followed the same way, as deep as
    it goes, each file once."""
    out, seen = [], set()

    def follow(f):
        if os.path.abspath(f) in seen or not os.path.exists(f):
            return
        seen.add(os.path.abspath(f))
        out.append(f)
        d = yaml.safe_load(io.open(f, encoding="utf-8").read()) or {}
        for key in ("record", "also"):
            v = d.get(key)
            v = [v] if isinstance(v, str) else v if isinstance(v, list) else \
                list(v.values()) if isinstance(v, dict) else []
            for c in v:
                if isinstance(c, str) and c.endswith((".yaml", ".yml")):
                    follow(os.path.join(os.path.dirname(f), c))
    for p in paths:
        for f in sorted(glob.glob(p)) or [p]:
            follow(f)
    return out


def _collection_for(doc, ids, jud, fields, nid, body, explicit):
    """The collection a new entry goes into: the one named; else the one holding the
    entries it shares a prefix with; else, by shape - a judgment with the judgments, a
    source-shaped body with the sources, anything else with the values."""
    cols = collections_of(doc)
    if explicit:
        if explicit not in cols and explicit not in doc:
            raise Refused(f"no collection {explicit} in this record")
        return explicit
    head = nid.split(".")[0]
    homes = [c for c, m in cols.items() if any(k.split(".")[0] == head for k in m)]
    if len(homes) == 1:
        return homes[0]
    if isinstance(body, dict) and fields["deps"] in body:
        by_jud = sorted(cols, key=lambda c: -sum(1 for k in cols[c] if k in jud))
        return by_jud[0]
    if not isinstance(body, dict):
        opens = [c for c in cols if c in OPEN]
        return opens[0] if opens else sorted(cols)[0]
    sourceish = not any(k in body for k in ("v", "rule", "quoted")) and \
        any(k in body for k in ("asked", "url", "file", "read"))
    cited = {}
    for c, m in cols.items():
        for k, b in m.items():
            if isinstance(b, dict) and isinstance(b.get("from"), str):
                for c2, m2 in cols.items():
                    if b["from"] in m2:
                        cited[c2] = cited.get(c2, 0) + 1
    if sourceish and cited:
        return sorted(cited, key=lambda c: -cited[c])[0]
    valued = sorted(cols, key=lambda c: -sum(1 for k, b in cols[c].items()
                                             if isinstance(b, dict) and k not in jud
                                             and ("v" in b or "rule" in b or "quoted" in b)))
    return valued[0]


def apply(paths, action):
    """The one write. `action` says what kind (set, add, review) and what that kind needs;
    it is validated whole before a byte is touched, applied to the file's text without
    reformatting anything else, read back, and answered with the reach. All of it under
    one lock, so a second writer waits rather than overwrites. Aimed at a hypothesis, the
    same write lands in the file beside the record and the base is not touched."""
    with _locked(paths[0]):
        if action.get("hypothesis"):
            return _fork(paths, action)
        return _apply(paths, action)


def _ensure_collection(lines, collection):
    """The collection opened at the end of the file when it lacks one; the file still ends
    in a newline."""
    if collection in {n for n, _, _ in _collections_in(lines)}:
        return
    while lines and not lines[-1].strip():
        lines.pop()
    lines += ([""] if lines else []) + [f"{collection}:", ""]


def _carry(files, nid):
    """-> (collection, lines): an entry's own lines in the base, whole - comments, style and
    all - and the collection they sit in; None when no file holds it."""
    for f in files:
        with io.open(f, encoding="utf-8") as fh:
            lines = fh.read().split("\n")
        loc = _locate(lines, nid)
        if loc:
            name, _, s, e = loc
            block = lines[s:e]
            while block and not block[-1].strip():
                block.pop()
            return name, block
    return None


def _insert_block(lines, collection, nid, block):
    """A block of lines - an entry carried over whole - placed in `collection` in id order,
    at the indent of the members it lands beside; the collection is opened if the file lacks
    it. -> a line saying where it went."""
    _ensure_collection(lines, collection)
    cols = {n: (s, e) for n, s, e in _collections_in(lines)}
    s, e = cols[collection]
    ind, members = _members_of(lines, s, e)
    shift = (ind if ind is not None else 2) - _indent(block[0])
    block = [(" " * max(0, _indent(l) + shift) + l.lstrip(" ")) if l.strip() else l for l in block]
    if not members:
        lines[s + 1:s + 1] = block
        return f"{nid} into {collection}, its first entry"
    anchor, before = _place([m for m, _ in members], nid)
    ai = next(i for m, i in members if m == anchor)
    pos = ai if before else _block_end(lines, ai, ind, e)
    lines[pos:pos] = block
    return f"{nid} into {collection}, {'before' if before else 'after'} {anchor}"


def _fork(paths, action):
    """A write into a hypothesis beside the record: the same validation and the same edits,
    on `PROVENANCE.d/<name>.yaml` - the base is not touched. The record is read as it stands
    under the hypothesis, so a judgment written there rests on what it proposes and its
    snapshot says so. A hypothesis that does not exist yet is opened by its first write, with
    the day it was born in its head; an entry the base holds is carried over whole and set
    there."""
    doc = load(paths)
    name, kind, nid = action["hypothesis"], action["kind"], action["id"]
    hyp = doc.hypotheses.get(name)
    if hyp and hyp["error"]:
        raise Refused(f"refused - hypothesis {name} could not be read: {hyp['error']}")
    fresh = hyp is None
    if fresh:
        hyp = _hypothesis(name, hypothesis_path(paths, name))
        doc.hypotheses = dict(doc.hypotheses, **{name: hyp})
    under = layered(doc, hyp)
    ids, jud, fields = infer(under)
    raw = with_builtins(under, ids, jud, fields)
    for k, v in counts(under, ids, jud, fields, bodies(under)).items():
        raw.setdefault(k, {"name": COMPUTED[k], "v": v})
    stamp = action.get("as_of") or datetime.date.today().isoformat()
    refusals = validate(action, under, ids, jud, fields, raw)
    if refusals:
        raise Refused("refused - " + "\n          ".join(refusals))
    if kind == "review" and nid not in hyp["ids"]:
        raise Refused(f"refused - {nid} is not in hypothesis {name}"
                      + (" - review it in the base, or propose it here with add" if nid in jud else ""))
    snapshot_field = fields["snapshot"] or "seen"
    brief = _brief_beside(paths[0])          # a page count is the base page's, the one drawn
    if fresh:
        original = None
    else:
        with io.open(hyp["path"], encoding="utf-8") as fh:
            original = fh.read()
    lines = (original if original is not None else f'hypothesis: {{born: "{stamp}"}}\n').split("\n")
    out, seen = [], {}
    if kind == "set":
        now = value_of(raw, ids, nid)
        if nid in hyp["ids"] and now is not None and _same(now, action["value"]) \
                and not action.get("as_of"):
            print(f"{nid} is already {scalar(action['value'], fold=False)} in hypothesis {name}; "
                  f"nothing written")
            return 0
        if nid not in hyp["ids"]:
            got = _carry(_files_of(paths), nid)
            if not got:
                raise Refused(f"refused - no file of the record holds {nid}")
            collection, block = got
            out.append("carry " + _insert_block(lines, collection, nid, block) + f" of hypothesis {name}")
        old, field = _set_in(lines, nid, action["value"], stamp, action.get("why"))
        out.append(f"set {nid} in hypothesis {name}: {old} -> {scalar(action['value'], fold=False)} "
                   f"(as of {stamp})")
    elif kind == "add":
        body = action["body"]
        if isinstance(body, dict) and fields["deps"] in body:
            seen = _snapshot(list(body[fields["deps"]]), raw, ids, jud, paths, brief)
            body[snapshot_field] = seen
        collection = _collection_for(under, ids, jud, fields, nid, body, action.get("into"))
        _ensure_collection(lines, collection)
        out.append("add " + _add_in(lines, nid, body, collection) + f" of hypothesis {name}")
    else:
        j = jud[nid]
        was = dict(j["snap"])
        seen = _snapshot(j["deps"], raw, ids, jud, paths, brief)
        _, ind, s, e = _locate(lines, nid)
        changed = [d for d in seen if d not in was or not _same(was[d], seen[d])] + \
            [d for d in was if d not in seen]
        if changed:
            e = _seen_lines(lines, s, e, snapshot_field, seen)
        if _field_span(lines, s, e, "reviewed"):
            _stamp_field(lines, s, e, "reviewed", stamp, None)
        out.append(f"review {nid} in hypothesis {name}: "
                   + (f"seen rewritten from what the record holds under it ({stamp})" if changed
                      else f"what it saw is what the record holds under it ({stamp})"))
        for d in j["deps"]:
            if d in was and d in seen and not _same(was[d], seen[d]):
                out.append(f"  {d}: {short(was[d])} -> {short(seen[d])}")
            elif d not in was and d in seen:
                out.append(f"  {d}: {short(seen[d])} (never checked against it before)")
    made_dir = False
    if fresh and not os.path.isdir(os.path.dirname(hyp["path"])):
        os.makedirs(os.path.dirname(hyp["path"]))
        made_dir = True
    _write_text(hyp["path"], "\n".join(lines))
    # read it back: the hypothesis must still load, and hold what was written
    try:
        doc2 = load(paths)
        h2 = doc2.hypotheses.get(name)
        if h2 is None or h2["error"]:
            raise ValueError(h2["error"] if h2 else "the file is not found after the write")
        if nid not in h2["ids"]:
            raise ValueError(f"{nid} is not in the hypothesis after the write")
        under2 = layered(doc2, h2)
        ids2, jud2, fields2 = infer(under2)
        raw2 = with_builtins(under2, ids2, jud2, fields2)
        if kind == "set":
            now = value_of(raw2, ids2, nid)
            if now is None or not _same(now, action["value"]):
                raise ValueError(f"{nid} reads back as {now!r}")
    except (Exception, SystemExit) as e:
        if original is None:
            os.remove(hyp["path"])
            if made_dir:
                os.rmdir(os.path.dirname(hyp["path"]))
        else:
            _write_text(hyp["path"], original)
        raise Refused(f"the write broke the hypothesis and was undone: {e}")
    for l in out:
        print(l)
    # the reach, read with the hypothesis laid over the base: what would move if it folded.
    # Nothing in the base has.
    if kind == "review":
        tag, why = _state(nid, jud2[nid], raw2, ids2, fields2)
        print(f"  {nid} {tag.lower()}: {why}")
    else:
        hit, moved, derived = reach_of(ids2, jud2, raw2, [nid])
        if derived:
            print("worked out from it: " + ", ".join(derived))
        if kind == "add" and nid in jud2:
            tag, why = _state(nid, jud2[nid], raw2, ids2, fields2)
            print(f"the new judgment {tag.lower()}: {why}")
            hit = {n: v for n, v in hit.items() if n != nid}
        if hit:
            print(f"rests on it, under {name}:")
            for n2 in sorted(hit):
                print("  " + _state_line(n2, jud2[n2], raw2, ids2, fields2, touched=moved))
        elif kind == "set":
            print("nothing rests on it")
    nj = sum(1 for k in h2["ids"] if k in jud2)
    ne = len(h2["ids"]) - nj
    print(f"\nthe base is untouched; {name} holds {ne} entr{'y' if ne == 1 else 'ies'} and "
          f"{nj} judgment{'' if nj == 1 else 's'}"
          + (", and never folds" if str(h2["head"].get("folds") or "") == "never" else ""))
    return 0


def _snapshot(deps, raw, ids, jud, paths, brief, first_born=False):
    """`seen` for these dependencies, from what each holds now. A page count needs the
    brief beside the record; a dependency declared missing has nothing to snapshot. A
    record's first arrangement may rest on `page.drift` before anything dates it: with
    `first_born` the share is taken as nothing-yet, and settled against its own `born`
    once it is written."""
    page = _page_side(paths)[0] if brief and any(d in PAGE for d in deps) else {}
    seen = {}
    for d in deps:
        if d not in ids and not is_builtin(d):
            continue
        v = snapshot_value(d, raw, ids, jud, page)
        if v is None and d == "page.drift" and first_born and brief:
            v = 0.0
        if v is None:
            raise Refused(f"refused - {d} has no value to snapshot: "
                          + ("the page counts it, and no brief sits beside the record" if not brief
                             else "the page could not count it"
                             + (" - no arrangement carries born, so nothing dates what was added"
                                if d == "page.drift" else "")))
        seen[d] = v
    return seen


def _review_section(paths, brief, title, stamp, raw, ids, jud):
    """A section of the brief reviewed by its title: the references of its text
    snapshotted into `seen`, and `reviewed` moved. The record is not touched."""
    btext = io.open(brief, encoding="utf-8").read()
    b = yaml.safe_load(btext) or {}
    secs = [s for s in (b.get("sections") or []) if isinstance(s, dict)]
    for tab in (b.get("tabs") or []):
        if isinstance(tab, dict):
            secs += [s for s in (tab.get("sections") or []) if isinstance(s, dict)]
    sec = next((s for s in secs if str(s.get("title")) == title and s.get("text")), None)
    if not sec:
        raise Refused(f"refused - {title} is not a judgment, and no section of the brief with "
                      f"that title carries text")
    refs = [r for r in refs_in(sec["text"]) if r in ids]
    was = dict(sec.get("seen") or {})
    seen = _snapshot(refs, raw, ids, jud, paths, brief)
    blines = btext.split("\n")
    _review_section_in(blines, title, seen, stamp)
    _write_text(brief, "\n".join(blines))
    print(f"review text '{title}': reviewed {stamp}")
    for k in refs:
        if k in was and k in seen and not _same(was[k], seen[k]):
            print(f"  seen {k}: {short(was[k])} -> {short(seen[k])}")
        elif k not in was and k in seen:
            print(f"  seen {k}: {short(seen[k])} (never read against it before)")
    return 0


def _apply(paths, action):
    doc = load(paths)
    ids, jud, fields = infer(doc)
    raw = with_builtins(doc, ids, jud, fields)
    # every count the reader can take, whether or not the record mentions it yet: a new
    # judgment may be the first to rest on one
    for k, v in counts(doc, ids, jud, fields, bodies(doc)).items():
        raw.setdefault(k, {"name": COMPUTED[k], "v": v})
    files = _files_of(paths)
    stamp = action.get("as_of") or datetime.date.today().isoformat()
    kind, nid = action["kind"], action["id"]
    brief = _brief_beside(paths[0])
    if kind == "review" and nid not in jud and brief:
        action["section"] = nid
    # an arrangement's write is held against the page - its counts, and what the brief
    # carries of every decision that stands - taken now, before anything is written, by the
    # same build a snapshot runs; so the birth check and the supersede door decide against
    # the numbers --verify decides. A brief that cannot be built leaves the page out of it.
    facts = {}
    if brief and kind == "add" and isinstance(action["body"], dict) and \
            (_arrangement_shaped(action["body"], fields, raw)
             or (nid in jud and is_arrangement(jud[nid], raw))):
        try:
            page0, _, facts = _page_side(paths)
        except (Exception, SystemExit):
            page0, facts = {}, {}
        for k, v in page0.items():
            raw.setdefault(k, {"name": COMPUTED[k]})["v"] = v
        doc.page = facts
    refusals = validate(action, doc, ids, jud, fields, raw)
    if refusals:
        raise Refused("refused - " + "\n          ".join(refusals))
    if kind == "review" and action.get("section"):
        return _review_section(paths, brief, nid, stamp, raw, ids, jud)

    snapshot_field = fields["snapshot"] or "seen"
    # which file holds the entry: the one it is found in; a new one goes where its
    # collection is, else into the first file
    target = files[0]
    if kind in ("set", "review"):
        for f in files:
            if _locate(io.open(f, encoding="utf-8").read().split("\n"), nid):
                target = f
                break
    out, seen, arrangement, supersede, arranged = [], {}, False, False, False
    if kind == "add":
        body = action["body"]
        # a judgment under a standing judgment's id got past validation only because it may
        # supersede it - its wrong_if holds, or a person asked - so it replaces it in place
        supersede = nid in jud and isinstance(body, dict)
        arranged = isinstance(body, dict) and _arrangement_shaped(body, fields, raw)
        if isinstance(body, dict) and fields["deps"] in body:
            seen = _snapshot(list(body[fields["deps"]]), raw, ids, jud, paths, brief,
                             first_born=arranged)
            body[snapshot_field] = seen
        if arranged:
            # an arrangement is born when it is written; a re-decision renews its born and
            # keeps, in one line, the decision it replaces - born, how long it stood, and
            # what ended it - so the sequence of decisions reads from the record alone
            extra = {"born": stamp}
            if supersede and is_arrangement(jud[nid], raw):
                old = jud[nid]["body"]
                prior = old.get("replaced") or []
                prior = [prior] if isinstance(prior, str) else list(prior)
                f = facts.get(nid) or {}
                _, why = may_supersede(nid, old, body, raw, ids, jud, fields, action.get("as_of"),
                                       facts)
                ended = f"on the word of {body['request']}" if body.get("request") else why
                stood = f.get("stood")
                extra["replaced"] = prior + [
                    f"born {old.get('born') or 'undated'}"
                    + (f", stood {stood} session{'' if stood == 1 else 's'}" if stood is not None else "")
                    + f"; {ended} on {stamp}"]
            body = {k: v for k, v in body.items() if k not in extra and k != snapshot_field}
            body.update(extra)
            body[snapshot_field] = seen
            action["body"] = body
        if supersede:
            for f in files:
                if _locate(io.open(f, encoding="utf-8").read().split("\n"), nid):
                    target = f
                    break
        else:
            collection = _collection_for(doc, ids, jud, fields, nid, body, action.get("into"))
            for f in files:
                if collection in {n for n, _, _ in
                                  _collections_in(io.open(f, encoding="utf-8").read().split("\n"))}:
                    target = f
                    break
    original = io.open(target, encoding="utf-8").read()
    lines = original.split("\n")
    if kind == "set":
        now = value_of(raw, ids, nid)
        if now is not None and _same(now, action["value"]) and not action.get("as_of"):
            print(f"{nid} is already {scalar(action['value'], fold=False)}; nothing written")
            return 0
        old, field = _set_in(lines, nid, action["value"], stamp, action.get("why"))
        out.append(f"set {nid}: {old} -> {scalar(action['value'], fold=False)} (as of {stamp})")
    elif kind == "add" and supersede:
        _, why = may_supersede(nid, jud[nid]["body"], body, raw, ids, jud, fields, action.get("as_of"),
                               facts)
        was = str(_verdict_of(jud[nid]["body"]) or nid)
        _replace_in(lines, nid, body)
        out.append(f"supersede {nid}: {short(was, 60)} -> {short(_verdict_of(body), 60)} - {why}")
    elif kind == "add":
        out.append("add " + _add_in(lines, nid, body, collection))
    else:
        j = jud[nid]
        arrangement = is_arrangement(j, raw)
        was = dict(j["snap"])
        seen = _snapshot(j["deps"], raw, ids, jud, paths, brief)
        _, ind, s, e = _locate(lines, nid)
        changed = [d for d in seen if d not in was or not _same(was[d], seen[d])] + \
            [d for d in was if d not in seen]
        if changed:
            e = _seen_lines(lines, s, e, snapshot_field, seen)
        if _field_span(lines, s, e, "reviewed"):
            _stamp_field(lines, s, e, "reviewed", stamp, None)
        out.append(f"review {nid}: " + (f"seen rewritten from what the record holds ({stamp})"
                                        if changed else f"what it saw is what the record holds ({stamp})"))
        for d in j["deps"]:
            if d in was and d in seen and not _same(was[d], seen[d]):
                out.append(f"  {d}: {short(was[d])} -> {short(seen[d])}")
            elif d not in was and d in seen:
                out.append(f"  {d}: {short(seen[d])} (never checked against it before)")
    _bump_updated(lines, stamp)
    _write_text(target, "\n".join(lines))

    def read_back():
        doc2 = load(paths)
        ids2, jud2, fields2 = infer(doc2)
        raw2 = with_builtins(doc2, ids2, jud2, fields2)
        if kind == "set":
            now = value_of(raw2, ids2, nid)
            if now is None or not _same(now, action["value"]):
                raise ValueError(f"{nid} reads back as {now!r}")
        elif nid not in ids2 and nid not in (doc2.get("meta") or {}):
            raise ValueError(f"{nid} is not in the record after the write")
        return doc2, ids2, jud2, fields2, raw2

    # read it back: the record must still load, and hold what was written. Then a page
    # count in the snapshot is settled against the record as it now stands - the write
    # itself changes what the page counts, an arrangement no longer moved or a new
    # judgment that spills, and a count taken a moment earlier would be flagged by the
    # very next build. The shape an arrangement stood on is taken last, for the same reason.
    shape, facts2 = None, {}
    try:
        doc2, ids2, jud2, fields2, raw2 = read_back()
        if seen and brief and any(d in PAGE for d in seen):
            page2, shape, facts2 = _page_side(paths)
            drift = {d: page2[d] for d in seen if d in PAGE and d in page2 and not _same(seen[d], page2[d])}
            if drift:
                seen.update(drift)
                lines2 = io.open(target, encoding="utf-8").read().split("\n")
                _, ind, s, e = _locate(lines2, nid)
                _seen_lines(lines2, s, e, snapshot_field, seen)
                _write_text(target, "\n".join(lines2))
                doc2, ids2, jud2, fields2, raw2 = read_back()
                shape, facts2 = _page_side(paths)[1:]
            # a new arrangement's sign is decided once more against the counts as they stand
            # with it written - its own born may date what was added, and a first drift was
            # taken as nothing-yet - so a decision born broken is undone, never left green
            if arranged and not supersede and nid in jud2:
                with_page = dict(raw2)
                for k, v in page2.items():
                    with_page[k] = dict(with_page.get(k) or {"name": COMPUTED[k]})
                    with_page[k]["v"] = v
                pred = jud2[nid]["pred"]
                if pred and evaluate(pred, with_page, ids2) is True:
                    raise ValueError(f"wrong_if already holds ({pred}) once the page counts it - the "
                                     f"arrangement would be born broken")
        elif arrangement and brief:
            shape, facts2 = _page_side(paths)[1:]
    except (Exception, SystemExit) as e:
        _write_text(target, original)
        raise Refused(f"the write broke the record and was undone: {e}")
    # an arrangement's review also rewrites the shape it stood on - every tab it governs -
    # once the record is safely written, so a failed write leaves the brief as it was
    if kind == "review" and arrangement and shape is not None:
        b = yaml.safe_load(io.open(brief, encoding="utf-8").read()) or {}
        wheres = _tabs_for(b, nid, facts2)
        if not wheres:
            out.append("  no tab's sections earn a session source this rests on - no shape "
                       "rewritten; serve it on the tab that reads what it wrote")
        else:
            blines = io.open(brief, encoding="utf-8").read().split("\n")
            for where in wheres:
                _shape_in(blines, where, shape)
            _write_text(brief, "\n".join(blines))
            for where in wheres:
                out.append(f"  shape of {'tab ' + repr(where[0]) if where[1] else 'the brief'}: "
                           + ", ".join(f"{k}: {v}" for k, v in shape.items()))
    for l in out:
        print(l)
    _report(paths, kind, nid, doc2, ids2, jud2, fields2, raw2)
    return 0


def _same(a, b):
    if " ".join(str(a).split()) == " ".join(str(b).split()):
        return True
    try:
        return Decimal(str(a).replace(",", "")) == Decimal(str(b).replace(",", ""))
    except InvalidOperation:
        return False


def _report(paths, kind, nid, doc, ids, jud, fields, raw):
    """The reach, as the write's return value: worked-out entries that read it, every
    judgment reached and its state now, the texts of the brief that saw it."""
    if kind == "review":
        tag, why = _state(nid, jud[nid], raw, ids, fields)
        print(f"  {nid} {tag.lower()}: {why}")
        return
    hit, moved, derived = reach_of(ids, jud, raw, [nid])
    if derived:
        print("worked out from it: " + ", ".join(derived))
    if kind == "add" and nid in jud:
        tag, why = _state(nid, jud[nid], raw, ids, fields)
        print(f"the new judgment {tag.lower()}: {why}")
        hit = {n: v for n, v in hit.items() if n != nid}
    if hit:
        print("rests on it:")
        for name in sorted(hit):
            print("  " + _state_line(name, jud[name], raw, ids, fields, touched=moved))
    elif kind == "set":
        print("nothing rests on it")
    texts = _texts_that_saw(paths, [nid] + derived)
    if texts:
        print("text that saw it:")
        for title, saw in texts:
            what = ", ".join(f"{k} = {short(v)}" if v is not None else f"{k} (never read against it)"
                             for k, v in saw.items())
            print(f"  '{title}' saw {what} - read it again, then: review \"{title}\"")
    c = counts(doc, ids, jud, fields, raw)
    n = c["graph.flagged"]
    print(f"\nthe record needs a person on {n} judgment{'s' if n != 1 else ''}"
          + (f" ({c['graph.moved']} moved, {c['graph.falsified']} falsified)" if n else "")
          + " - check says the rest")


# ── the hooks' own two commands ──────────────────────────────────────────────
# The opener marks where a session began; the stop gate holds the end against that mark.
# What already failed, or was already unserved, when the session opened is never the
# session's doing - so the gate reminds about what the session itself left, once.
def mark(state_path, paths):
    """Written at session start: how many problems check finds, which intents no tab of the
    page serves, and the ids the record holds."""
    doc = load(paths)
    ids, jud, fields = infer(doc)
    fail, _, _, _, _ = check_lines(paths)
    state = {"fails": len(fail), "unserved": _unserved(paths),
             "ids": sorted(_every_id(doc, ids))}
    with io.open(state_path, "w", encoding="utf-8") as f:
        json.dump(state, f)
    return 0


def _marked(state_path):
    """The state the opener wrote - or, from an opener that wrote only the count, that."""
    text = io.open(state_path, encoding="utf-8").read().strip()
    try:
        state = json.loads(text)
    except ValueError:
        state = text
    if not isinstance(state, dict):
        try:
            state = {"fails": int(str(state).strip() or 0)}
        except ValueError:
            state = {"fails": 0}
    return {"fails": int(state.get("fails") or 0), "unserved": list(state.get("unserved") or []),
            "ids": state.get("ids")}


def gate(state_path, paths):
    """What a session hears before it can finish, against the mark its opener left: the
    record failing worse than it found it; an intent the session left unserved; entries it
    wrote with no intent recorded. Printed, and 2 when there is anything - the hook bounces
    once and yields."""
    base = _marked(state_path)
    fail, _, _, _, _ = check_lines(paths)
    out = []
    if len(fail) > base["fails"]:
        out.append(f"{paths[0]} fails check with {len(fail)} problems ({base['fails']} at "
                   f"session start).")
        out.append("Fix the record - or declare the hole with blocked_on - before finishing:")
        out += ["FAIL " + f for f in fail[:12]]
    for s in _unserved(paths):
        if s not in base["unserved"]:
            out.append(f"{s} is served by no tab of the page - serve it in a tab whose sections "
                       f"pick what it wrote, or leave it outside and say why")
    if base["ids"] is not None:
        doc = load(paths)
        ids, jud, fields = infer(doc)
        raw = bodies(doc)
        # what a hypothesis holds is the session's writing too, attributed the same way
        rests = {}
        for h in doc.hypotheses.values():
            for k, b in h["raw"].items():
                raw.setdefault(k, b)
                if isinstance(b, dict) and fields["deps"] in b and isinstance(b[fields["deps"]], list):
                    rests[k] = [d for d in b[fields["deps"]] if isinstance(d, str)]
        every = _every_id(doc, ids)
        was = set(base["ids"])
        new = sorted(k for k in every if k not in was)
        intents = {k for k in every if k not in jud and isinstance(raw.get(k), dict)
                   and raw[k].get("asked")}

        def attributed(k):
            b = raw.get(k)
            if isinstance(b, dict) and str(b.get("from") or "") in intents:
                return True
            deps = jud[k]["deps"] if k in jud else rests.get(k, [])
            return bool(set(deps) & intents)
        if new and not (set(new) & intents) and not any(attributed(k) for k in new):
            named_ = ", ".join(new[:4]) + (f" and {len(new) - 4} more" if len(new) > 4 else "")
            out.append(f"this session wrote {len(new)} {'entry' if len(new) == 1 else 'entries'} "
                       f"({named_}) and recorded no intent: add s.<date>_<slug> asked=\"...\" "
                       f"name=\"...\", and from: it on what it wrote")
    for l in out:
        print(l)
    return 2 if out else 0


HELP = {
    "set": """  set <key> <value> [--why "..."] [--as-of YYYY-MM-DD] [--hypothesis NAME] [file]

Change one value. The entry's `v:` (or `quoted:`) is rewritten where it stands, `of:` is
stamped with the date, and the reason - if given - is kept as a comment beneath. A number
is written as a number, `true`/`false` as booleans, anything else as text. A worked-out
entry is refused: change its rule, not its result. A judgment is refused: review it. The
reply is the reach: what is worked out from it, every judgment resting on it and its state
now, and the texts of the brief that saw the old value.

A reading newer than the one the base holds - its `of:`, else its source's read date -
updates it. One of the same day or earlier that differs is a contradiction: refused into
the base, and the refusal names the command that writes it into a hypothesis instead.
`--hypothesis NAME` writes into `PROVENANCE.d/NAME.yaml` beside the record, opened by its
first write, and the base is not touched: the entry is carried over whole and set there.""",
    "add": """  add <id> field=value ... [--in COLLECTION] [--as-of YYYY-MM-DD] [--hypothesis NAME] [file]
  add <id> '{field: value, ...}'
  add <id> "an open question"

A new entry or judgment, inserted in id order beside the entries it shares a prefix with -
never at the tail - in the style of the entry it lands beside; meta.updated moves. A list
is written as `rests_on='[a, b]'`; a judgment's `seen` is filled by this tool from what its
dependencies hold now and must not be given. Refused: an id already in the record, a
dependency that is not an entry (add it first, or declare it with blocked_on), a reference
to nothing, a judgment whose wrong_if already holds. The reply is the reach.

A contradiction is refused into the base and named a hypothesis: the same id with a
different value or verdict, or a judgment resting on what only a hypothesis holds - the
refusal names the `--hypothesis NAME` command that writes it into `PROVENANCE.d/NAME.yaml`
beside the record instead, where its `seen` is taken from the record as it stands under
that hypothesis, and the base is not touched.

An arrangement - a judgment resting on a session source, with a sign over a count the build
takes - is born when it is written: `born` is stamped like `seen`, its sign is one comparison
that can hold, and `request: s.<date>_<slug>` names the person's request it was taken from.
Written again under its own id it is re-decided: admitted when its sign holds with its tabs
intact, or on a request; refused twice in a day, or once the brief no longer carries what it
decided; `replaced:` keeps each decision it replaced, one line.""",
    "review": """  review <id> [--as-of YYYY-MM-DD] [--hypothesis NAME] [file]
  review "<section title>"

"I read it, and it still holds." A judgment's `seen` is rewritten from what its
dependencies hold now, and `reviewed:` moves when the judgment carries one; an
arrangement's review also rewrites the brief's shape line - the one tab's, or of every tab
whose sections earn a session source it rests on. A section of the brief is reviewed by its title:
its text's references are snapshotted into `seen` and `reviewed:` moves. Never automatic:
a snapshot that refreshed itself could not show a difference. `--hypothesis NAME` reviews
a judgment the hypothesis holds, against the record as it stands under it.""",
}


def write_command(cmd, rest):
    if "--help" in rest or "-h" in rest:
        print(HELP[cmd].strip("\n"))
        return 0
    opts, args = {}, []
    i = 0
    while i < len(rest):
        a = rest[i]
        if a in ("--why", "--as-of", "--in", "--hypothesis"):
            if i + 1 >= len(rest):
                raise Refused(f"{a} needs a value")
            opts[a[2:].replace("-", "_")] = rest[i + 1]
            i += 2
            continue
        args.append(a)
        i += 1
    if not args:
        raise Refused(HELP[cmd].strip("\n"))
    nid, args = args[0], args[1:]
    as_of = opts.get("as_of")
    if as_of and not re.match(r"^\d{4}-\d{2}-\d{2}$", as_of):
        raise Refused("--as-of takes a date, YYYY-MM-DD")
    if opts.get("why") and "\n" in opts["why"]:
        raise Refused("--why is one line: a second line would be a line of the record")
    if opts.get("hypothesis") and not HYPOTHESIS_NAME.match(opts["hypothesis"]):
        raise Refused("--hypothesis takes a name - letters, digits, underscores, dashes - that "
                      f"becomes {HYPOTHESES}/<name>.yaml beside the record")
    action = {"kind": cmd, "id": nid, "as_of": as_of, "why": opts.get("why"), "into": opts.get("in"),
              "hypothesis": opts.get("hypothesis")}
    if cmd == "set":
        if not args:
            raise Refused("set needs a value: set <key> <value>")
        action["value"] = typed(args[0])
        files = [x for x in args[1:] if x.endswith((".yaml", ".yml"))]
    elif cmd == "add":
        files = [x for x in args if x.endswith((".yaml", ".yml")) and "=" not in x]
        given = [x for x in args if x not in files]
        if len(given) == 1 and given[0].lstrip().startswith("{"):
            try:
                body = yaml.safe_load(given[0])
            except yaml.YAMLError as e:
                raise Refused(f"the fields do not read as a mapping: {e}")
            if not isinstance(body, dict):
                raise Refused("the fields must be a mapping: '{v: 1, from: x}'")
        elif len(given) == 1 and "=" not in given[0]:
            body = given[0]                                  # a bare line: an open question
        else:
            body = {}
            for g in given:
                if "=" not in g:
                    raise Refused(f"{g!r}: fields are written field=value")
                f, v = g.split("=", 1)
                # a list or a mapping is read as one; `{{` opens a reference, not a mapping
                if v[:1] == "[" or (v[:1] == "{" and v[:2] != "{{"):
                    try:
                        v = yaml.safe_load(v)
                    except yaml.YAMLError as e:
                        raise Refused(f"{f}: {e}")
                else:
                    v = typed(v)
                body[f.strip()] = v
            if not body:
                raise Refused("add needs fields: add <id> v=... from=...")
        action["body"] = body
    else:
        files = [x for x in args if x.endswith((".yaml", ".yml"))]
    return apply(files or default_paths(), action)


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
    if cmd in ("mark", "gate"):
        # the hooks' own: the state file the opener writes, then the record
        if not rest:
            sys.exit(f"{cmd} needs the state file the opener writes")
        files = [x for x in rest[1:] if x.endswith((".yaml", ".yml"))] or default_paths()
        sys.exit((mark if cmd == "mark" else gate)(rest[0], files))
    if cmd in ("set", "add", "review"):
        sys.exit(write_command(cmd, rest))
    if cmd in ("same", "distinct"):
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import sameness
        sys.exit(sameness.command(cmd, rest))
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
