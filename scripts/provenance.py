#!/usr/bin/env python3
"""Read a knowledge record: check its invariants, and report what a change reaches.

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

Without a file argument the record is GROUNDING.yaml here - or PROVENANCE.yaml, the name
records were born under before - else the path this checkout registered in
`<git common dir>/kpopper-record`, for a project whose tree cannot hold it.
A file is parsed when it changes, not at every command: its parsed form is kept under the
user's state directory, keyed by what the file is. `--no-cache`, or KPOPPER_NO_CACHE=1,
parses every time.

The record forks on a contradiction, never on a session: a write that contradicts the base
is refused into it and goes into a hypothesis - `.kpopper/hypotheses/<name>.yaml` beside the
record, the record's own shape - with `--hypothesis <name>`. Every command reads the hypotheses over
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
import io, os, re, sys, glob, json, time, shlex, stat, pickle, hashlib, subprocess, datetime, \
    textwrap, tempfile, contextlib, yaml
from decimal import Decimal, InvalidOperation
import importlib.util

with io.open(__file__, 'rb') as _source_file:
    LOADED_SOURCE_HASH = hashlib.sha256(_source_file.read()).hexdigest()

_expression_name = "_kpopper_expressions_" + hashlib.sha256(os.path.dirname(__file__).encode()).hexdigest()[:12]
if _expression_name not in sys.modules:
    _spec = importlib.util.spec_from_file_location(_expression_name, os.path.join(os.path.dirname(__file__), "expressions.py"))
    sys.modules[_expression_name] = importlib.util.module_from_spec(_spec)
    _spec.loader.exec_module(sys.modules[_expression_name])
E = sys.modules[_expression_name]


def predicate_text(value):
    rendered = E.text(value) if isinstance(value, dict) else str(value or "")
    # Operators keep their inner grouping; the surrounding sentence supplies its
    # own parentheses, so do not double-wrap the outermost expression.
    return rendered[1:-1] if isinstance(value, dict) and 'expr' not in value and rendered.startswith('(') and rendered.endswith(')') else rendered


def predicate_refs(value):
    return E.refs(value) if isinstance(value, dict) else ID.findall(str(value or ""))


def predicate_of(body, fields):
    value = body.get(fields["predicate"]) if fields["predicate"] else None
    return value if isinstance(value, dict) else str(value or "")


def rule_refs(body, ids):
    if not isinstance(body, dict):
        return []
    if isinstance(body.get("rule"), dict):
        return E.refs(body["rule"])
    refs = set()
    for field in ("rule", "v"):
        value = body.get(field)
        if field == "rule" and isinstance(value, dict):
            refs.update(E.refs(value))
        elif isinstance(value, str) and EXPR.search(value):
            refs.update(ref for ref in ID.findall(value) if ref in ids)
    return sorted(refs)

def _peer(name):
    """Load sibling modules even when a configured reader was imported by file path."""
    import importlib
    import importlib.util
    package = __package__
    if not package:
        package = '_kpopper_runtime'
        if package not in sys.modules:
            spec = importlib.util.spec_from_file_location(package,
                os.path.join(os.path.dirname(__file__), '__init__.py'),
                submodule_search_locations=[os.path.dirname(__file__)])
            module = importlib.util.module_from_spec(spec)
            sys.modules[package] = module
            spec.loader.exec_module(module)
    return importlib.import_module(package + '.' + name)


import contextvars
import copy
_RAW_READS = contextvars.ContextVar('raw_record_reads', default=False)
# Only the common core loader may consume a declared reasoning profile.
_CORE_READS = contextvars.ContextVar('core_record_reads', default=False)
# Strict capture observes actual reads, including cache hits; ordinary reads are inert.
_CAPTURE_READS = contextvars.ContextVar('record_capture_reads', default=None)


def _capture_event(kind, path, value):
    observer = _CAPTURE_READS.get()
    if observer is not None:
        observer(kind, os.path.abspath(path), value)


def _capture_glob(pattern):
    found = glob.glob(pattern)
    _capture_event('glob', pattern, sorted(map(os.path.abspath, found)))
    return found



ID = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+")
EXPR = re.compile(r"[<>=!+\-*/()]|\bor\b|\band\b|\bnot\b")
# A reference: an entry named inside prose, `{{heat.loss_kw}}`, resolved wherever the text is
# shown and never retyped. A judgment id placed this way, `{{c.boiler_short}}`, asks for that
# judgment's reasoning at that spot.
REF = re.compile(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}")
ENTRY = "GROUNDING.yaml"            # the record's entry point, born by the first add
LEGACY_ENTRY = "PROVENANCE.yaml"    # the name records were born under before: still read, never created
ENTRY_NAMES = (ENTRY, LEGACY_ENTRY)
HOME = ".kpopper"                   # beside the entry file: everything a record keeps beside itself
DEFAULT = [ENTRY]

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
            if set(val) in ({"ref"}, {"num"}, {"text"}, {"bool"}, {"op", "args"}, {"expr"}):
                yield from E.refs(val)
                continue
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
    """Use the same bounded discovery as the opener, including non-Git workspaces."""
    try:
        from .workspace import locate
    except ImportError:
        # Configured readers are also loaded by file path (ingestion and checked
        # sessions), without a package or their directory on sys.path.
        import importlib.util
        spec = importlib.util.spec_from_file_location(
            "_kpopper_workspace", os.path.join(os.path.dirname(__file__), "workspace.py"))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        locate = module.locate
    location = locate()
    if location["status"] == "unavailable":
        raise SystemExit(location["reason"] + " " + location["record"])
    return [location["record"]] if location["status"] in ("found", "pending") else DEFAULT


HYPOTHESES = "PROVENANCE.d"      # the hypotheses of a record under the old name, beside it
HYPOTHESES_HOME = "hypotheses"   # under HOME for a record under the new: one file per hypothesis
HYPOTHESIS_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_\-]*$")


def is_legacy(entry):
    """Every record except one born under the new canonical name keeps the adjacent layout
    older releases supported. That includes explicitly named and registered records such as
    notes.yaml; renaming one must not silently move where its hypotheses and metadata live."""
    return os.path.basename(str(entry)) != ENTRY


def _first_of(paths):
    return (sorted(glob.glob(paths[0])) or [paths[0]])[0]


def hypotheses_rel(entry):
    """The hypotheses directory relative to the entry file's own - for a tree read through
    git, where nothing is absolute."""
    return HYPOTHESES if is_legacy(entry) else HOME + "/" + HYPOTHESES_HOME


def layout(first):
    """Where a record keeps what sits beside it, decided by the name of the file the reader
    opens first. GROUNDING.yaml keeps everything under .kpopper/ beside it - hypotheses/,
    view.yaml, measure.yaml, session.json, replaced.yaml, and build/ for what is rebuilt.
    Records under the earlier default name or a custom name keep PROVENANCE.d/,
    <record>.view.yaml, PROVENANCE.measure.yaml, PROVENANCE.session.json and
    PROVENANCE.replaced.yaml beside them, as they always did. One home per record: the reader
    never looks in both."""
    first = os.path.abspath(str(first))
    d = os.path.dirname(first)
    if is_legacy(first):
        return {"legacy": True, "entry": first, "home": d,
                "hypotheses": os.path.join(d, HYPOTHESES), "hypotheses_name": HYPOTHESES,
                "view": re.sub(r"\.ya?ml$", "", first) + ".view.yaml",
                "measure": os.path.join(d, "PROVENANCE.measure.yaml"),
                "measure_name": "PROVENANCE.measure.yaml",
                "session": os.path.join(d, "PROVENANCE.session.json"),
                "replaced": os.path.join(d, "PROVENANCE.replaced.yaml"),
                "history": os.path.join(d, "PROVENANCE.history"),
                "history_commits": os.path.join(d, "PROVENANCE.history-commits"),
                "history_authority": os.path.join(d, "PROVENANCE.history.yaml"),
                "build": None, "page": "record.html"}
    home = os.path.join(d, HOME)
    return {"legacy": False, "entry": first, "home": home,
            "hypotheses": os.path.join(home, HYPOTHESES_HOME),
            "hypotheses_name": HOME + "/" + HYPOTHESES_HOME,
            "view": os.path.join(home, "view.yaml"),
            "measure": os.path.join(home, "measure.yaml"), "measure_name": HOME + "/measure.yaml",
            "session": os.path.join(home, "session.json"),
            "replaced": os.path.join(home, "replaced.yaml"),
            "history": os.path.join(home, "history"),
            "history_commits": os.path.join(home, "history-commits"),
            "history_authority": os.path.join(home, "history.yaml"),
            "build": os.path.join(home, "build"), "page": os.path.join(home, "build", "page.html")}


def layout_of(paths):
    return layout(_first_of(paths))


def leftovers(paths):
    """Files of the record's other layout beside the entry file -> [(what, where it belongs)]:
    a record moved by half, renamed with its files left under the earlier names, or a
    .kpopper/ opened beside a record still under the earlier name. The reader looks in one
    home, so nothing reads these - and a hypothesis or a brief nobody reads is worse than
    none, which is why check fails on them and the opener names them. An empty hypotheses
    directory holds nothing to lose and is passed over."""
    lay = layout_of(paths)
    d = os.path.dirname(lay["entry"])
    other = layout(os.path.join(d, ENTRY if lay["legacy"] else LEGACY_ENTRY))
    out = []
    for role in ("hypotheses", "view", "measure", "session", "replaced",
                 "history", "history_commits", "history_authority"):
        there = other[role]
        if not os.path.exists(there):
            continue
        if role == "hypotheses" and not glob.glob(os.path.join(there, "*.y*ml")):
            continue
        tail = "/" if role == "hypotheses" else ""
        out.append((os.path.relpath(there, d) + tail, os.path.relpath(lay[role], d) + tail))
    return out


def leftover_lines(paths):
    """What check says about each file left under the other layout."""
    lay = layout_of(paths)
    if lay["legacy"]:
        return [f"{what} is not read beside {LEGACY_ENTRY} - rename the record to {ENTRY} and move "
                f"its files into {HOME}/, or move this to {where}" for what, where in leftovers(paths)]
    return [f"{what} is not read - left under the earlier name; move it to {where}"
            for what, where in leftovers(paths)]


def leftover_head(paths):
    """The opener's one line about a record moved by half, or None."""
    left = leftovers(paths)
    if not left:
        return None
    names = ", ".join(what for what, _ in left)
    if layout_of(paths)["legacy"]:
        return (f"{names} not read beside {LEGACY_ENTRY} - rename the record to {ENTRY} and move its "
                f"files into {HOME}/")
    return f"left under the earlier name, not read: {names} - move into {HOME}/"


def brief_for(paths, explicit=None):
    """The brief the page is built from: the one given, else the record's own - under .kpopper/
    beside a record under the new name; beside the record or any file it points at, then in
    the working directory, for one kept under the old."""
    if explicit:
        return explicit
    lay = layout_of(paths)
    if not lay["legacy"]:
        return lay["view"] if os.path.exists(lay["view"]) else None
    for p in list(paths) + [LEGACY_ENTRY]:
        c = re.sub(r"\.ya?ml$", "", p) + ".view.yaml"
        if os.path.exists(c):
            return c
    return None


class Record(dict):
    """The base record's document. The hypotheses beside it ride along as an attribute rather
    than a key, so nothing that walks the document's collections mistakes one for an entry:
    every reader sees the base, and asks for the layer by name."""
    def __init__(self, *a, **k):
        super().__init__(*a, **k)
        self.hypotheses = {}
        self.origins = {}  # collection -> member -> file; reader metadata, never YAML


def hypothesis_dir(paths):
    """Where a record's hypotheses live: .kpopper/hypotheses beside the file the reader opens
    first - the root of a pointer record, the registered file of a project whose tree holds
    none - and PROVENANCE.d beside a record kept under the old name. One place per record."""
    return layout_of(paths)["hypotheses"]


def hypothesis_path(paths, name):
    return os.path.join(hypothesis_dir(paths), name + ".yaml")


def latest_today():
    """Today, as the later of the local day and the UTC day: a reading is dated no later than
    this. A host behind UTC in its evening already has readings dated tomorrow by UTC - the
    pull request's re-measurement stamps its day in UTC - and those are not from the future."""
    return max(datetime.date.today(), datetime.datetime.now(datetime.timezone.utc).date())


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


# ── reading a file of the record ─────────────────────────────────────────────
# Every command reads the whole record, and the hooks read it at every prompt and every edit.
# Reading the bytes is a millisecond and parsing them is the whole cost, so a file's parsed
# document is kept under the user's own state directory, keyed by the file's absolute path, its
# size, the nanosecond it was last written, and the digest of the bytes that were parsed. Any of
# the four differing is a parse. The digest is what makes the key honest: a length and a
# nanosecond cannot tell apart a rewrite of the same length inside one tick, and a tick is a
# whole second on filesystems that keep timestamps coarsely. It costs a thousandth of the parse.
#
# The files are the record; the cache is only a copy of what one of them said. An entry that is
# missing, truncated, foreign, stale or of the wrong shape is a parse, never an error, and every
# write drops the entry for the file it wrote. Set KPOPPER_NO_CACHE=1 to parse every time - for
# a test, or to tell the cache out of a problem.
NO_CACHE = "KPOPPER_NO_CACHE"
CACHE_FORM = 1              # the entry's own shape: a newer reader ignores what an older wrote
CACHE_DAYS = 14             # an entry no read has renewed for this long is nobody's
CACHE_ENTRIES = 64          # and this many, newest kept: more records than anyone reads at once
SWEEP = ".swept"            # in the directory: when it was last looked over
_PARSED = {}                # this process's own: absolute path -> (identity, the entry's bytes)
_SWEPT = False              # the directory is looked over once in a run, and once in a day
# What a parsed record can hold beyond mappings, lists and scalars, which carry themselves
VALUES = {("datetime", "date"), ("datetime", "datetime"), ("datetime", "time"),
          ("datetime", "timedelta"), ("datetime", "timezone"), ("builtins", "set"),
          ("builtins", "frozenset"), ("builtins", "complex")}


class _OnlyValues(pickle.Unpickler):
    """A cache entry holds what the parser returned - mappings, lists, scalars, dates - and
    nothing else is built from it, so a file under the state directory can never become code
    this process runs."""

    def find_class(self, module, name):
        if (module, name) not in VALUES:
            raise pickle.UnpicklingError(f"{module}.{name} is not a value a record holds")
        return super().find_class(module, name)


def cache_dir():
    """Where the parsed documents are kept: the user's state directory, where this tool keeps
    what belongs to a person rather than to a project."""
    configured = os.environ.get("XDG_STATE_HOME", "")
    base = configured if configured and os.path.isabs(configured) else \
        os.path.join(os.path.expanduser("~"), ".local", "state")
    return os.path.join(base, "kpopper", "cache")


def _cache_file(path):
    """One entry per file, named for the whole absolute path: two records never share an entry -
    two checkouts of one project, or two hypotheses of the same name beside different records."""
    return os.path.join(cache_dir(), hashlib.sha256(path.encode("utf-8")).hexdigest() + ".parse")


def _cache_ready():
    """The directory, private to this user. Nothing is read from or written to one that anybody
    else can write to, and a directory that cannot be made leaves the cache out of this run."""
    d = cache_dir()
    try:
        os.makedirs(d, mode=0o700, exist_ok=True)
        if os.path.islink(d):
            return None       # a link is somebody else's directory wearing this one's name
        st = os.stat(d)
        uid = getattr(os, "getuid", None)
        if not stat.S_ISDIR(st.st_mode):
            return None
        # the mode bits are read where the platform keeps them; where it does not, they are
        # made up, and a made-up answer is no reason to refuse the directory
        if uid and (stat.S_IMODE(st.st_mode) & 0o022 or st.st_uid != uid()):
            return None
    except OSError:
        return None
    if not _SWEPT:
        _sweep(d)
    return d


def _sweep(d):
    """A record that was moved, renamed or deleted - a throwaway checkout, a test's own file -
    leaves behind an entry nobody will ask for again. So the directory is looked over once a
    day: what a parse has not rewritten for a fortnight goes, and so does everything past the
    newest few dozen. A record still in use is read from an entry that outlives that or parsed
    once more, and either way the directory stays the size of what is actually read."""
    global _SWEPT
    _SWEPT = True
    now = time.time()
    try:
        names = [n for n in os.listdir(d) if n.endswith(".parse")]
    except OSError:
        return
    if len(names) <= CACHE_ENTRIES:
        try:                  # within its bounds and looked over today: nothing to do
            if now - os.stat(os.path.join(d, SWEEP)).st_mtime < 86400:
                return
        except OSError:
            pass
    aged = []
    for name in names:
        entry = os.path.join(d, name)
        try:
            aged.append((os.stat(entry).st_mtime, entry))
        except OSError:
            continue          # somebody else's sweep got there first
    aged.sort(reverse=True)
    for i, (when, entry) in enumerate(aged):
        if i < CACHE_ENTRIES and now - when <= CACHE_DAYS * 86400:
            continue
        try:
            os.remove(entry)
        except OSError:
            pass              # one entry that will not go is no reason to leave the rest
    try:                      # the mark of a sweep is written when there has been one
        fd = os.open(os.path.join(d, SWEEP), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        os.close(fd)
    except OSError:
        pass


def _identity(path, data):
    """What tells one state of a file from another: how long it is, when it was written, and
    what it holds - the last of which is the one of the three nothing can fake by accident."""
    st = os.stat(path)
    return [st.st_size, st.st_mtime_ns, hashlib.sha256(data).hexdigest()]


def _cached(path, identity):
    """What this file parsed to last time, if the file is still the one that parsed. Anything
    unreadable, of another shape, or about another file reads as nothing at all."""
    raw, memo = None, _PARSED.get(path)
    if memo is not None and memo[0] == identity:
        raw = memo[1]
    elif _cache_ready():
        try:
            with io.open(_cache_file(path), "rb") as f:
                raw = f.read()
        except OSError:
            return None
    if raw is None:
        return None
    try:
        entry = _OnlyValues(io.BytesIO(raw)).load()
    except Exception:
        return None
    if not isinstance(entry, dict) or entry.get("form") != CACHE_FORM \
            or entry.get("path") != path or entry.get("identity") != identity \
            or not isinstance(entry.get("doc"), dict):
        return None
    _PARSED[path] = (identity, raw)
    return entry["doc"]


def _keep(path, identity, doc):
    """The parse kept for the next reader: in this process, and in a file of its own - written
    where only this user can read it, and put in place in one step, so nobody meets half an
    entry. A cache that cannot be written changes nothing about the answer. Only a document of
    the shape a record has is kept, so nothing that reads one has to ask what it got."""
    if not isinstance(doc, dict):
        return
    try:
        raw = pickle.dumps({"form": CACHE_FORM, "path": path, "identity": identity, "doc": doc},
                           protocol=pickle.HIGHEST_PROTOCOL)
    except Exception:
        return                       # a document that will not pickle is simply not kept
    _PARSED[path] = (identity, raw)
    if not _cache_ready():
        return
    tmp = None
    try:
        # the temporary name ends in .parse too, so one left by a killed writer is swept
        fd, tmp = tempfile.mkstemp(prefix=".writing.", suffix=".parse", dir=cache_dir())
        with io.open(fd, "wb") as f:
            f.write(raw)
        os.replace(tmp, _cache_file(path))
    except OSError:
        if tmp and os.path.exists(tmp):
            try:
                os.remove(tmp)
            except OSError:
                pass


def forget(path):
    """What a write leaves behind: the entry for the file it wrote, gone from this process and
    from the state directory. The key would catch the change anyway; dropping the entry is what
    keeps the directory one entry per file rather than one per version of one."""
    p = os.path.abspath(path)
    _PARSED.pop(p, None)
    try:
        os.remove(_cache_file(p))
    except OSError:
        pass


def parse(path=None, *, text=None):
    """One file of the record, as the parser reads it - the record, a file a pointer names, a
    hypothesis beside it. Every reader of a record file comes through here, so none of them can
    be served the parse of a file as it no longer is: what comes back is the parse of the bytes
    that are there now, or those bytes parsed again. Staged text uses the same parser
    with no file identity or cache entry."""
    if text is not None:
        if path is not None:
            raise ValueError('parse takes a file or staged text, not both')
        return yaml.safe_load(text)
    path = os.path.abspath(path)
    try:
        with io.open(path, "rb") as f:
            data = f.read()
    except OSError as error:
        _capture_event('unreadable', path, type(error).__name__)
        raise
    _capture_event('bytes', path, data)
    if os.environ.get(NO_CACHE, "") not in ("", "0"):
        return yaml.safe_load(data.decode("utf-8"))
    try:
        identity = _identity(path, data)
    except OSError:
        return yaml.safe_load(data.decode("utf-8"))
    got = _cached(path, identity)
    if got is not None:
        return got
    doc = yaml.safe_load(data.decode("utf-8"))
    _keep(path, identity, doc)
    return doc


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
    _capture_event('directory', d, os.path.isdir(d))
    if not os.path.isdir(d):
        return out
    for f in sorted(_capture_glob(os.path.join(d, "*.yaml")) + _capture_glob(os.path.join(d, "*.yml"))):
        name = re.sub(r"\.ya?ml$", "", os.path.basename(f))
        hyp = _hypothesis(name, f)
        try:
            body = parse(f) or {}
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
        except OSError as e:
            if _CAPTURE_READS.get() is None:
                raise
            hyp["error"] = type(e).__name__
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
            if f == 'rule' and isinstance(body[f], dict):
                try:
                    return E.lower(body[f])
                except ValueError:
                    pass
            return body[f]
    return body


def _same_claim(a, b):
    if isinstance(a, (dict, list)) or isinstance(b, (dict, list)):
        return a == b
    return _same(a, b)


def _claim_key(claim):
    """One claim written the way `_same_claim` reads it: two claims this record calls the same
    are written the same, no two others are written alike, and every claim is written at all.
    Each is written by its kind, so that nothing reads as anything else - a structure by its
    parts in a fixed order, whatever kinds they are, each part with its length in front so a
    delimiter cannot fall two ways; a number by its value alone, since trailing zeros, a signed
    zero and an exponent are not the number and no working precision may round it; and any
    other text by itself, with whitespace collapsed."""
    if isinstance(claim, dict):
        parts = sorted((_claim_key(k), _claim_key(v)) for k, v in claim.items())
        return "{" + "".join(f"{len(p)}:{p}" for kv in parts for p in kv) + "}"
    if isinstance(claim, (list, tuple)):
        return "[" + "".join(f"{len(p)}:{p}" for p in map(_claim_key, claim)) + "]"
    text = " ".join(str(claim).split())
    try:
        d = Decimal(text.replace(",", ""))
    except InvalidOperation:
        return "t" + text
    if not d.is_finite():
        return "n" + str(d)                     # a NaN or an infinity, by the one name for it
    sign, digits, exp = d.as_tuple()
    while len(digits) > 1 and digits[-1] == 0:  # by hand: normalize() would round to the context
        digits, exp = digits[:-1], exp + 1
    if digits == (0,):
        return "n0"                             # 0, -0 and 0.00 are the one number
    return "n" + ("-" if sign else "") + "".join(map(str, digits)) + "e" + str(exp)


def _claim_mark(claim, n=6):
    """Six characters standing for one claim: the same six on every machine and every day,
    and none of the sentence itself. A digest of the whole claim, never a slug of its opening
    words - two claims that read alike until their last word are exactly the pair a name has
    to tell apart."""
    return hashlib.sha256(_claim_key(claim).encode("utf-8")).hexdigest()[:n]


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
    for nid, variants in getattr(doc, 'knowledge_conflicts', {}).items():
        out[nid] = [(name, claim_of(body)) for name, body in variants]
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
    hyps = {name: hyp for name, hyp in (getattr(doc, 'hypotheses', None) or {}).items()
            if hyp.get('kind') != 'contribution'}
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
    return line if len(line) <= width else line[:width] + " ..."


def _every_id(doc, ids):
    """The ids the record holds anywhere: the base's, and every hypothesis's."""
    out = {k for k in ids if not is_builtin(k)}
    for h in (getattr(doc, "hypotheses", None) or {}).values():
        out |= h["ids"]
    return out


def load(paths, *, read_mode=None):
    mode = read_mode or ('frozen' if _RAW_READS.get() else os.environ.get('KPOPPER_READ_MODE', 'live'))
    if mode == 'live':
        paths = _peer('knowledge_views').write_paths(paths)
    doc, seen = Record(), set()

    def merge(f, d):
        # Validate each physical declaration before a later shard can mask it.
        try:
            _peer('reasoning.contract').capabilities(d)
        except _peer('reasoning.contract').CapabilityError as error:
            raise Refused(error.code + ': ' + str(error)) from None
        seen.add(os.path.abspath(f))
        for k, v in d.items():
            if isinstance(v, dict):
                if not isinstance(doc.get(k), dict):
                    doc.origins[k] = {}
                doc.origins.setdefault(k, {}).update(dict.fromkeys(v, os.path.abspath(f)))
            else:
                doc.origins.pop(k, None)
            if isinstance(v, dict) and isinstance(doc.get(k), dict):
                # a legend is declared per file, and a record split across files keeps
                # every file's letters rather than the last file's
                mine, theirs = doc[k].get("prefixes"), v.get("prefixes")
                if k == "meta" and isinstance(mine, dict) and isinstance(theirs, dict):
                    v = dict(v, prefixes={**mine, **theirs})
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
                _capture_event('exists', cf, os.path.exists(cf))
                if os.path.abspath(cf) in seen or not os.path.exists(cf):
                    continue
                merge(cf, parse(cf) or {})

    for p in paths:
        for f in sorted(_capture_glob(p)) or [p]:
            _capture_event('exists', f, os.path.exists(f))
            if not os.path.exists(f):
                if not _RAW_READS.get() and (read_mode or os.environ.get('KPOPPER_READ_MODE', 'live')) == 'live' and _peer('knowledge_views').has_pending(paths):
                    doc['meta'] = {}
                    continue
                sys.exit(f"{f}: no record here. Run this from the directory the record sits "
                         "in, or name the record file as an argument.")
            merge(f, parse(f) or {})
    doc.hypotheses = load_hypotheses(paths)
    mode = read_mode or ('frozen' if _RAW_READS.get() else os.environ.get('KPOPPER_READ_MODE', 'live'))
    doc = _peer('knowledge_views').overlay(paths, doc, read_mode=mode)
    # A dormant profile must not be interpreted by legacy check/page/session paths.
    # Attached proposals retain their own declared semantics as well as the base.
    for document in [doc, *(hyp['doc'] for hyp in doc.hypotheses.values())]:
        meta = document.get('meta')
        if isinstance(meta, dict) and 'reasoning' in meta:
            contract = _peer('reasoning.contract')
            try:
                contract.capabilities(document)
            except contract.CapabilityError as error:
                raise Refused(error.code + ': ' + str(error)) from None
            if not _CORE_READS.get():
                raise Refused('unsupported_capability: use core/v1 consumer')
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
    if isinstance(doc.get('meta'), dict) and 'reasoning' in doc['meta']:
        collections = dict(collections)
        collections['meta'] = {key: value for key, value in collections.get('meta', {}).items()
                               if key != 'reasoning'}
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
                if f in IDENTITIES:
                    continue  # other identities of this subject, never what it rests on
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
                source_collections = {k: v for k, v in collections.items() if k != 'meta'}
                judgment_fields = {'rests_on', 'wrong_if', 'seen', 'verdict', 'reopened_by', 'blocked_on'} | \
                                  {sch[k] for k in ('deps', 'snapshot', 'predicate') if sch.get(k)}
                if source_collections and set(source_collections) <= {'known', 'sources', 'open', 'questions'} \
                        and all(sch.get(k) for k in ('deps', 'snapshot', 'predicate')) \
                        and not cand['deps'] and set(unresolved) <= {'labels', 'tags', 'v', 'quoted'} \
                        and not any(judgment_fields.intersection(body)
                            for group in source_collections.values() for body in group.values()
                            if isinstance(body, dict)):
                    # A portable source/fact closure may retain the explicit parent
                    # schema without retaining a downstream judgment. An actual
                    # dependency declaration or judgment shape still cannot vanish.
                    return sch[role]
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
        # The checked-session transport already accepts this narrow starting shape.
        # Let the reader/writer do so too: a first source and finding need not invent
        # a judgment. Unknown collections or judgment-shaped fields still require
        # role inference; this must never hide a misspelled dependency declaration.
        source_collections = {k: v for k, v in collections.items() if k != "meta"}
        judgment_fields = {"rests_on", "wrong_if", "seen", "verdict", "reopened_by", "blocked_on"}
        # A record born with only its head - meta, and nothing yet - is the moment before
        # the first entry, not a record that lost its graph.
        newborn = bool(doc) and set(doc) <= {"meta"}
        if (newborn or source_collections and set(source_collections) <= {"known", "sources", "open", "questions"}) \
                and not unresolved and not any(
                    judgment_fields.intersection(body) for group in source_collections.values()
                    for body in group.values() if isinstance(body, dict)):
            return {nid for group in source_collections.values() for nid in group} | (ids & set(COMPUTED)), {}, \
                {"deps": "rests_on", "snapshot": "seen", "predicate": "wrong_if"}
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
                if f in REOPENED or f in PROSE_BY_NAME or f in IDENTITIES:
                    continue           # read by name: it never reads as a predicate or a snapshot
                if isinstance(val, dict) and val and all(k in ids for k in val):
                    cand["snapshot"][f] = cand["snapshot"].get(f, 0) + 1
                elif isinstance(val, dict) and ("op" in val or "expr" in val or f == "wrong_if"):
                    cand["predicate"][f] = cand["predicate"].get(f, 0) + 1
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
                    "pred": predicate_of(body, fields),
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
# Two fields the reader reads by name and never by shape: whose asking a judgment was taken
# from (`request:`, a session source - provenance, and nothing a door reads), and the
# decisions an arrangement replaced (`replaced:`, one line each, naming a count and the sign
# that ended them - which would otherwise read as a predicate).
PROSE_BY_NAME = ("request", "replaced")
# `also:` names other identities of the subject - the ids retired into it, or the siblings a
# record declares - and never what it rests on. It is read by name for that reason: a retired
# id written again, by a merge that brings back the branch it came from, turns the field into
# a list of ids that are all entries, which is the shape the dependency role is voted on by -
# and in a small record it ties with the real one and nothing can be read at all. A record
# that does keep its dependencies there says so: `schema: deps: also`.
IDENTITIES = ("also",)
OPEN = ("open", "questions")
# An entry whose value is a fact about the tree names the recipe that takes it again:
# `measure: <name>`, a bare name that the allowlist beside the record resolves to an argument
# list. The reader reads the name and runs nothing - only `remeasure`, which the pull request
# runs, does - so the record never carries a command, and a name here is refused when it is
# not a name, or stands on anything but a stored scalar reading.
MEASURE = "measure"
MEASURE_NAME = re.compile(r"^[A-Za-z][A-Za-z0-9_\-]*$")

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
    if isinstance(v, dict) and E.number(v) is not None:
        return E.display_value(v)
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
    if v is not None and (not isinstance(v, (list, dict)) or E.number(v) is not None):
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
BOOL = re.compile(r"(?i)^(true|false)$")
# Quoted text is a literal: whatever it holds, it is one value and none of it is read as an
# operator. Taken out of a right-hand side before that side is looked at for a second
# comparison - the thing the comparison shape would otherwise swallow as the value.
QUOTED = re.compile(r"(['\"]).*?\1", re.S)
SECOND = re.compile(r"(<=|>=|==|!=|<|>)|\bor\b|\band\b")


def comparison_parts(pred):
    """Expose a simple comparison for validation/display without parsing rendered text."""
    if isinstance(pred, dict):
        try:
            pred = E.lower(pred, predicate=True)
        except (ValueError, TypeError):
            return None
        left, right = pred["args"]
        if set(left) != {"ref"} or "op" in right:
            return None
        return left["ref"], E.COMPARISONS[pred["op"]], E.text(right)
    match = CMP.match(str(pred or ""))
    return match.groups() if match else None


def why_undecided(pred):
    """Why this reader cannot decide `pred` as one comparison - '' when it can.

    The comparison shape alone is not enough. It takes everything after the operator as
    the value, so `a > 0 or b > 0` compares `a` against the text "0 or b > 0" and is false
    in every state of the record - a falsifier that reads as evaluable and can never fire,
    which is decoration. One reading of the shape, so what an arrangement refuses when it
    is written, what check reports and what evaluate declines are the same thing.

    What it asks of the right-hand side is only that it carry no second comparison. Any
    narrower rule refuses a value the reader can compare perfectly well - a date and a
    thousands separator both carry characters an expression would use - and refusing a
    falsifier that works is this same failure from the other side.
    """
    if isinstance(pred, dict):
        try:
            pred = E.lower(pred, predicate=True)
        except (ValueError, TypeError) as error:
            return str(error)
        parts = comparison_parts(pred)
        if parts is None:
            return ""
        name, op, rhs = parts
        boolean_literal = "bool" in pred["args"][1]
    else:
        m = CMP.match(str(pred or ""))
        if not m or SECOND.search(QUOTED.sub("", m.group(3).strip())):
            return "is not one comparison this reader decides (a name, an operator, one value)"
        name, op, rhs = m.group(1), m.group(2), m.group(3).strip()
        boolean_literal = bool(BOOL.match(rhs))
    if boolean_literal:
        # a truth value is matched, and only against another truth value. A computed name
        # is a count whatever the record holds, so this one is decided here and not left
        # to a reading that would come back None and be taken for green.
        if op not in ("==", "!="):
            return f"orders a truth value ({op} {rhs}), which is matched, never ordered"
        if is_builtin(name):
            return f"holds a count against a truth value ({name} {op} {rhs}), which never matches"
    return ""


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
    if getattr(raw, 'world', None) is not None:
        return raw.world.value(k)
    b = raw.get(k)
    if not isinstance(b, dict):
        return b if k in ids else None
    if isinstance(b.get("rule"), dict):
        return E.current(raw, ids, k)["value"]
    v = b.get("v")
    if v is None:                      # absent or null: the quoted text is the value
        v = b.get("quoted")
    if isinstance(v, str) and EXPR.search(v) and any(t in ids for t in ID.findall(v)):
        return None
    return v


def evaluate(pred, raw, ids):
    """-> True when the falsifier holds, False when it does not, None when it is not a
    single comparison this reader can decide. Richer predicates are surfaced, never
    guessed at - a compound would be decided against the text after its first operator,
    so it is declined here and said where the record is checked.

    A truth value is matched as one: a record writes `false` and the fact holds Python's
    False, and comparing them as text matches in neither state of the fact.
    """
    if getattr(raw, 'world', None) is not None:
        return raw.world.predicate(pred)
    if isinstance(pred, dict):
        try:
            E.validate(pred, predicate=True)
        except (ValueError, TypeError):
            return None
        return E.compute(raw, ids, pred)["predicate"]["holds_on_current_values"]
    m = CMP.match(str(pred or ""))
    if not m or why_undecided(pred):
        return None
    if any(isinstance(raw.get(k), dict) and isinstance(raw[k].get("rule"), dict)
           for k in (m.group(1), m.group(3).strip())):
        try:
            expression = E.convert(pred, predicate=True)
        except (ValueError, SyntaxError, RecursionError):
            return None
        return E.compute(raw, ids, expression)["predicate"]["holds_on_current_values"]
    a = value_of(raw, ids, m.group(1))
    if a is None:
        return None
    rhs, op = m.group(3).strip(), m.group(2)
    if BOOL.match(rhs):
        if not isinstance(a, bool):
            return None        # a truth value and a value that is not one never compare
        same = a is (rhs.lower() == "true")
        return same if op == "==" else not same
    b = value_of(raw, ids, rhs) if ID.fullmatch(rhs) else rhs.strip("\"'")
    if b is None or isinstance(a, bool) != isinstance(b, bool):
        return None

    def num(x):
        try:
            return float(str(x).replace(",", ""))
        except ValueError:
            return None
    na, nb = num(a), num(b)
    a, b = (na, nb) if na is not None and nb is not None else (str(a), str(b))
    return {"<": a < b, ">": a > b, "<=": a <= b, ">=": a >= b,
            "==": a == b, "!=": a != b}[op]


def computation_error(pred, raw, ids):
    """A required evaluator failure is different from an unavailable operand."""
    if not isinstance(pred, dict):
        match = CMP.match(str(pred or ""))
        if not match or not any(isinstance(raw.get(key), dict) and isinstance(raw[key].get("rule"), dict)
                                for key in (match.group(1), match.group(3).strip())):
            return ""
        try:
            pred = E.convert(pred, predicate=True)
        except (ValueError, SyntaxError, RecursionError):
            return ""
    return E.compute(raw, ids, pred).get("error", "")


def short(v, n=40):
    s = " ".join((predicate_text(v) if isinstance(v, dict) and ('op' in v or 'expr' in v) else str(v)).split())
    return s if len(s) <= n else s[:n - 1] + "…"


def apart(a, b, n=40):
    """Two readings being compared, clipped to where they differ rather than to where they
    agree -> (a, b). `short` cuts at the head, and when one value is written over another
    the head is exactly the part that did not change: two long readings that part at the
    end come back as the same clipped string twice, which tells a reader something moved
    and shows them nothing to judge it by. So the window opens at the last whole word
    before the two part, and what is dropped in front of it is marked.

    Where they part early enough for the head to show it, this is `short`."""
    sa, sb = " ".join(str(a).split()), " ".join(str(b).split())
    if len(sa) <= n and len(sb) <= n:
        return sa, sb
    same = 0
    while same < min(len(sa), len(sb)) and sa[same] == sb[same]:
        same += 1
    if same < n // 2:                  # they part inside what the head would have shown
        return short(sa, n), short(sb, n)
    at = sa.rfind(" ", 0, same) + 1    # open on a whole word, not mid-word
    if at == 0 or same - at > n:
        # no word to open on, or the nearest one too far back to be worth the room: a url,
        # a hash, a long number. Cut into the token instead - a window that shows the
        # difference is worth more here than one that starts where a reader would.
        at = max(0, same - n // 4)
    return "…" + short(sa[at:], n - 1), "…" + short(sb[at:], n - 1)


def legacy_snapshot_rule(old, current_rule):
    """A legacy formula is historical syntax, never a historical numeric result."""
    if not isinstance(old, str) or not isinstance(current_rule, dict):
        return None
    try:
        rule = E.convert(old)
        return rule if E.refs(rule) else None
    except (ValueError, SyntaxError, RecursionError):
        return None


def formula_only_snapshots(j, raw):
    return [dep for dep, old in j["snap"].items()
            if isinstance(raw.get(dep), dict) and isinstance(raw[dep].get("rule"), dict)
            and E.same(legacy_snapshot_rule(old, raw[dep]["rule"]), raw[dep]["rule"])]


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
    refs = {t for t in predicate_refs(j["pred"]) if t in ids}
    verdict = evaluate(j["pred"], raw, ids) if refs else None
    for dep, old in j["snap"].items():
        snapshot = old.get("computed") if isinstance(old, dict) else None
        current_rule = raw[dep].get("rule") if isinstance(raw.get(dep), dict) else None
        legacy_rule = legacy_snapshot_rule(old, current_rule)
        if legacy_rule is not None:
            if not E.same(legacy_rule, current_rule):
                out.append((dep, old, predicate_text(current_rule), "moved"))
            continue
        if dep in ids and (isinstance(current_rule, dict) or isinstance(snapshot, dict)):
            now = value_of(raw, ids, dep)
            previous = snapshot.get("value") if isinstance(snapshot, dict) else old
            rule_moved = isinstance(snapshot, dict) and not E.same(snapshot.get("rule"), current_rule)
            a, b = E.number(previous), E.number(now)
            equal = a == b if a is not None and b is not None else previous == now
            if rule_moved or (now is not None and previous is not None and not equal):
                state = "crossed" if dep in refs and verdict is True else "muted" if dep in refs and verdict is False and not rule_moved else "moved"
                out.append((dep, previous, now, state))
            continue
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


def measure_problem(nid, body, ids, jud, fields, raw):
    """Why `measure:` cannot stand on this body, or '' - a recipe is named by a stored scalar
    reading alone: an entry holding v: or quoted:, not worked out from a rule, not a judgment,
    not a name the reader computes; and the name is a bare name, never a command."""
    if not isinstance(body, dict) or MEASURE not in body:
        return ""
    name = body[MEASURE]
    if not isinstance(name, str) or not MEASURE_NAME.match(name):
        return (f"measure: {short(name)!r} is not a recipe name - letters, digits, underscores and "
                f"dashes, opening with a letter; what runs lives in the allowlist beside the record, "
                f"never here")
    if nid in jud or (fields.get("deps") and fields["deps"] in body):
        return f"measure: {name} on a judgment - a judgment is not measured; the entries it rests on are"
    if is_builtin(nid):
        return f"measure: {name} on a name the reader computes"
    v = body.get("v") if body.get("v") is not None else body.get("quoted")
    if body.get("rule") is not None or (isinstance(v, str) and EXPR.search(v)
                                        and any(t in ids for t in ID.findall(v))):
        return f"measure: {name} on an entry worked out from a rule - measure what it is worked out from"
    if v is None:
        return f"measure: {name} on an entry with no value of its own - a recipe's line replaces a stored reading"
    if isinstance(v, (list, dict)):
        return f"measure: {name} on a value that is a list or a mapping - a recipe prints one scalar"
    return ""


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


# ── the legend ───────────────────────────────────────────────────────────────
# A prefix is meant to be a word a question carries. A record kept under letters - born
# before that was asked for, and kept because a rename reaches every reference on every
# branch - says once in its head what they stand for, `meta.prefixes: {d: decision}`, and
# every surface that shows a prefix to a person shows the word beside it: the opener's head
# and the page's namespace bar. Nothing else reads the field.
LEGEND_WORD = 40            # a legend entry is a word; a sentence there would eat the head


def legend_of(meta, ids):
    """{prefix: word} for the prefixes the record holds, judgments included - a letter the
    record no longer holds is not repeated, and what is not a word is not a legend."""
    declared = (meta or {}).get("prefixes")
    if not isinstance(declared, dict):
        return {}
    held = {k.split(".")[0] for k in ids if isinstance(k, str) and not is_builtin(k)}
    out = {}
    for g, w in declared.items():
        if isinstance(g, str) and isinstance(w, str) and g in held:
            w = " ".join(w.split())
            if w:
                out[g] = w if len(w) <= LEGEND_WORD else w[:LEGEND_WORD - 1] + "…"
    return out


def legend_notes(meta, ids):
    """What check says about a legend nothing can print: a key YAML read as something other
    than a word (`on:` and `yes:` are booleans unless quoted), a value that is not one, a
    prefix the record does not hold - each silent in the opener, and said here instead."""
    declared = (meta or {}).get("prefixes")
    if declared is None:
        return []
    if not isinstance(declared, dict):
        return ["meta.prefixes is not a mapping of prefix to word - nothing is printed from it"]
    held = {k.split(".")[0] for k in ids if isinstance(k, str) and not is_builtin(k)}
    notes = []
    for g, w in declared.items():
        if not isinstance(g, str):
            notes.append(f"meta.prefixes: the key {g!r} is not a word - yes, no, on, off, true "
                         "and false are booleans to YAML unless quoted")
        elif g not in held:
            notes.append(f"meta.prefixes: {g} is held by nothing in the record")
        elif not isinstance(w, str) or not w.strip():
            notes.append(f"meta.prefixes: {g} stands for {w!r}, which is not a word - quote it")
    return notes


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
    return bool(intents_of(j, raw)) and (any(is_builtin(t) for t in predicate_refs(j["pred"]))
                                          or any(is_builtin(d) for d in j["deps"]))


def _arrangement_shaped(body, fields, raw):
    """Whether a body being written would read as an arrangement."""
    if not _judgment_shaped(body, fields):
        return False
    deps = body[fields["deps"]]
    pred = predicate_of(body, fields)
    return any(is_intent(d, raw) for d in deps) and (any(is_builtin(t) for t in predicate_refs(pred))
                                                     or any(is_builtin(d) for d in deps))


def one_comparison(pred, raw=None, ids=None):
    """An arrangement's sign, as the build can decide it: one comparison - a name, an
    operator, a number or a text or a truth value or another entry - that can hold. -> ''
    when it is, else what is wrong with it. The shape is the reader's own, so a sign an
    arrangement may carry is exactly a falsifier the record can decide; on top of that a
    count below zero or a share above one never happens, and an entry with no value to
    compare would leave the sign green forever."""
    bad = why_undecided(pred)
    if bad:
        return bad
    parts = comparison_parts(pred)
    if parts is None:  # a valid structured comparison containing arithmetic
        return ""
    name, op, rhs = parts
    rhs = rhs.strip()
    typed = isinstance(pred, dict)
    reference_rhs = "ref" in E.lower(pred, predicate=True)["args"][1] if typed else bool(ID.fullmatch(rhs))
    # On top of the shape, a sign names its value outright. This is the arrangement's own
    # rule and not a second reading of the shape: what the reader can decide is settled in
    # why_undecided, and this asks the narrower thing a sign over a count is held to.
    if not (typed or NUM_VALUE.match(rhs) or reference_rhs or BOOL.match(rhs)
            or QUOTED.fullmatch(rhs)):
        return ("does not name one value a sign carries (a number, a truth value, a text in "
                "quotes, or another entry)")
    if reference_rhs and raw is not None and not is_builtin(rhs) \
            and (rhs not in (ids or ()) or value_of(raw, ids, rhs) is None):
        return f"compares against {rhs}, which holds no value the build can compare"
    # and a count held against a truth value never matches, whether the truth value is
    # written into the sign or reached through an entry - the shape alone cannot see the
    # second one, and here the value is in hand
    if is_builtin(name) and reference_rhs and raw is not None \
            and isinstance(value_of(raw, ids, rhs), bool):
        return f"holds a count against a truth value ({name} {op} {rhs}), which never matches"
    if reference_rhs:
        return ""
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


def pending_counts(pred, raw, ids):
    """Do not feed judgments about not-yet-counted builtins back into those counts."""
    pending, seen = list(predicate_refs(pred)), set()
    while pending:
        key = pending.pop()
        if key in seen:
            continue
        seen.add(key)
        body = raw.get(key)
        if is_builtin(key) and (not isinstance(body, dict) or body.get("v") is None):
            return True
        if isinstance(body, dict):
            pending.extend(rule_refs(body, ids))
    return False


def assessment_module():
    # A configured reader can be loaded directly by file path, outside sys.path.
    try:
        from . import assessment
    except ImportError:
        path = os.path.join(os.path.dirname(__file__), 'assessment.py')
        existing = sys.modules.get('assessment')
        if existing is not None and os.path.abspath(existing.__file__) == os.path.abspath(path):
            return existing
        name = '_kpopper_assessment_' + hashlib.sha256(
            (os.path.abspath(path) + LOADED_SOURCE_HASH).encode()).hexdigest()[:12]
        if name not in sys.modules:
            from types import SimpleNamespace
            spec = importlib.util.spec_from_file_location(name, path)
            module = importlib.util.module_from_spec(spec)
            module.P = SimpleNamespace(**globals())
            sys.modules[name] = module
            spec.loader.exec_module(module)
        assessment = sys.modules[name]
    return assessment


def flags(ids, jud, fields, raw, defer_counts=False):
    """Existing flags are a compatibility policy over shared assessment findings."""
    assessment = assessment_module()
    return {name: assessment.reader_flags(
                assessment.judgment_state(j, raw, ids, fields, jud),
                j, raw, ids, fields, defer_counts)
            for name, j in jud.items()}


def _legacy_computation(doc):
    """A supplied document needs the same semantic guard as a loaded one."""
    meta = doc.get('meta')
    if isinstance(meta, dict) and 'reasoning' in meta:
        contract = _peer('reasoning.contract')
        try:
            contract.capabilities(doc)
        except contract.CapabilityError as error:
            raise Refused(error.code + ': ' + str(error)) from None
        raise Refused('unsupported_capability: use core/v1 consumer')


def counts(doc, ids, jud, fields, raw):
    """Every graph.* value, counted from the record alone - and taken before any judgment
    that reads a count is decided. A line drawn against "how many are flagged" could
    otherwise be crossed by the drawing of it and uncrossed by the crossing, forever; so a
    count never includes what reading it decided, and every surface that then decides
    those judgments - check, the opener, the page - decides them against the same numbers.
    `raw` here is the record's own bodies, without the counts."""
    _legacy_computation(doc)
    open_ids = {k for g in OPEN for k in (doc.get(g) or {})}
    held = [k for k in ids if k not in jud and not is_builtin(k)]
    fl = flags(ids, jud, fields, {k: v for k, v in raw.items() if not is_builtin(k)}, defer_counts=True)

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
        "graph.hypotheses": sum(h.get('kind') != 'contribution' for h in (getattr(doc, "hypotheses", None) or {}).values()),
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
    _legacy_computation(doc)
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


def _fired_failure(name, judgment):
    return f"{name}: wrong_if holds ({predicate_text(judgment['pred'])}) - broken by its own condition"


def check_lines(paths):
    """What check finds -> (fail, note, moved, contested, summary), unprinted: the gate reads
    the same lines the command prints."""
    doc = load(paths)
    ids, jud, fields = infer(doc)
    raw = with_builtins(doc, ids, jud, fields)
    open_ids = {k for g in OPEN for k in (doc.get(g) or {})}
    fail, note, moved = [], _peer('knowledge_views').lines(doc), []
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
    # A recipe is named by a stored scalar reading, and the record's own bodies are where that
    # is read: the reader's world replaces a computed name's body with the count it took, so a
    # measure hand-written on one would never be seen through it.
    for nid, body in sorted(bodies(doc).items()):
        problems = expression_problems(body, raw, ids, fields)
        fail.extend(f"{nid}: {problem}" for problem in problems)
        if not problems and isinstance(body, dict) and isinstance(body.get("rule"), dict):
            result = E.current(raw, ids, nid)
            if result["value"] is None:
                (note if _blocked_text(body) else fail).append(f"{nid}: rule cannot be computed: {result['reason']}")
        bad = measure_problem(nid, body, ids, jud, fields, raw)
        if bad:
            fail.append(f"{nid}: {bad}")
    if not fields["snapshot"]:
        note.append("no snapshot field anywhere: dependencies are declared but never "
                    "captured, so drift can never be detected")
    for name, j in sorted(jud.items()):
        if name in open_ids:
            continue
        state = assessment_module().judgment_state(j, raw, ids, fields, jud)
        condition = {'holds': True, 'does_not_hold': False}.get(state['falsifier']['status'])
        blocked = _blocked_text(j["body"])
        for d in j["deps"]:
            if d not in ids:
                (note if blocked else fail).append(
                    f"{name}: rests on {d}, which is not an entry"
                    + (f" - declared: {blocked[:90]}" if blocked else ""))
            elif fields["snapshot"] and d not in j["seen"]:
                fail.append(f"{name}: no snapshot for {d} - never checked against it")
        for tok in sorted(set(predicate_refs(j["pred"]))):
            if tok in ids and tok not in j["deps"]:
                fail.append(f"{name}: predicate reads {tok}, which it does not declare as a "
                            f"dependency - a change to it would never reach this")
        # A predicate field holding prose is not a predicate. It reads like one,
        # which is worse than an empty field: nothing evaluates it and nobody notices.
        # So does one this reader cannot decide - a compound is read to its first operator
        # and false ever after - and it is said here rather than passed over.
        undecided = why_undecided(j["pred"]) if j["pred"] else ""
        evaluable = bool([t for t in predicate_refs(j["pred"]) if t in ids]) and not undecided
        misfiled = _misfiled_reopener(j["body"], ids)
        if misfiled:
            fail.append(f"{name}: reopened_by reads as a comparison ({short(misfiled, 60)}) - a "
                        f"predicate belongs in wrong_if, where it is evaluated; a re-opener is the "
                        f"sign a person reads")
        # An arrangement's sign is held to the same shape a few lines down, and said
        # there in the terms an arrangement is decided by; it is not said twice.
        if not evaluable and not (undecided and is_arrangement(j, raw)):
            what = ("no predicate at all" if not j["pred"] else
                    "prose, not an evaluable predicate"
                    if not [t for t in predicate_refs(j["pred"]) if t in ids] else
                    f"wrong_if {undecided}")
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
        elif (error := computation_error(j["pred"], raw, ids)):
            (note if blocked else fail).append(f"{name}: condition cannot be computed: " + error)
        elif condition is True:
            fail.append(_fired_failure(name, j))
        elif [t for t in predicate_refs(j["pred"]) if t in PAGE]:
            named_page = sorted({t for t in predicate_refs(j["pred"]) if t in PAGE})
            note.append(f"{name}: wrong_if reads {', '.join(named_page)}, which is counted "
                        f"when the page is built - `page --verify` decides it")
        elif condition is None:
            # one comparison, and the reading it needs is not there: a side that holds no
            # value yet, or a truth value held against something that is not one. The shape
            # is right and only the reading is missing, so it is noted rather than failed
            note.append(f"{name}: nothing decides wrong_if ({short(j['pred'], 60)}) - a side "
                        f"holds no value to compare, or a truth value is held against a "
                        f"value that is not one")
        # An arrangement's sign is decided by the build - by check for a count of the record,
        # by the page for a count of the page - so it is one comparison that can hold. A
        # re-opener may stand beside it, never in its place: an unevaluable sign on an
        # arrangement is decoration, and decoration is freeze in disguise.
        if is_arrangement(j, raw):
            bad = one_comparison(j["pred"], raw, ids)
            if bad:
                fail.append(f"{name}: wrong_if {bad} - an arrangement's sign is decided by the "
                            f"build, or it is decoration")
        # Whose asking a judgment was taken from is provenance a reader holds against the
        # asked: it points at, so it is the same claim wherever it appears.
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
                was, is_ = apart(old, now)
                moved.append(f"{name}: {dep} differs from its snapshot "
                             f"({was} -> {is_}) - re-review, or refresh seen")
        # A verdict replaced under its id is a question for a person, not a failure: the
        # replacement passed the door, and the trail says so until someone reviews it
        day = reversal_pending(j["body"]) if not is_arrangement(j, raw) else None
        if day:
            note.append(f"{name}: reversed on {day} - the verdict under this id changed; review it "
                        f"once read, or pull {name} --history")
    # What the record stands on: one line, printed and never failed on - the confidences are
    # the record's to defend, and a count of them is not a problem with it.
    priors = priors_line(ids, jud, raw)
    if priors:
        note.append(priors)
    # A legend the opener could not print, said here since the opener says nothing.
    note += legend_notes(doc.get("meta"), ids)
    # Coverage, when a brief sits beside the record: which intents no tab of the page serves,
    # and where what each of them wrote falls - facts the page counted, said here so a session
    # that never builds the page still hears them. The page decides its own falsifiers; the
    # brief held against the arrangements that stand is the record's own claim, so a brief
    # that no longer carries what an arrangement decided fails here too.
    info = _page_or_error(paths, read_mode=getattr(doc, 'read_mode', None))
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
    # `also:` where the named id is an entry after all: the retirement reading does not hold
    # there, and the sibling reading is a record's own business - so this is said, and decided
    # by nobody but a person.
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import sameness
    note += sameness.also_lines(doc, fields["deps"])
    # Hypotheses beside the record: a file the reader cannot read fails; an id two of them hold
    # with different claims needs a person and fails nothing - which of them folds, or neither,
    # is decided at consolidation, where the union is tested.
    cont = []
    for name, h in sorted(doc.hypotheses.items()):
        label = 'contribution' if h.get('kind') == 'contribution' else 'hypothesis'
        if h["error"]:
            fail.append(f"{label} {name} could not be read: {h['error']}")
            continue
        got, why = _layer_view(doc, h)
        if got is None:
            fail.append(f"{label} {name} cannot be read over the base: {why}")
    for k, hs in contested(doc).items():
        cont.append(f"{k}: " + ", ".join(f"{n} says {short(c)}" for n, c in hs)
                    + (" - contribution versions need explicit reconciliation" if k in getattr(doc, 'knowledge_conflicts', {})
                       else " - one of them folds, or neither; a person decides"))
    # Files of the other layout beside the entry file: a record moved by half. Nothing reads
    # them, and a checker that passes over what it cannot read is the failure this method
    # exists to refuse.
    fail += leftover_lines(paths)
    held = sum(1 for k in ids if not is_builtin(k))
    summary = (f"{len(jud)} judgments, {held} entries, {len(fail)} problems"
               + (f", {len(moved)} moved" if moved else "")
               + (f", {len(cont)} contested" if cont else "")
               + (f", {len(note)} declared" if note else ""))
    return fail, note, moved, cont, summary


SKILL_FORMS = {"claude": "/kpopper:{}", "codex": "${}"}


def next_moves(host):
    """The session's next moves, named the way the host invokes a skill - `/kpopper:ground`
    in Claude Code, `$ground` in Codex - or nothing, for a host that runs the reader's verbs
    directly and is told those instead."""
    form = SKILL_FORMS.get(host or "")
    if not form:
        return None
    return {"ground": form.format("ground") + " <entry|prefix>", "record": form.format("record"),
            "consolidate": form.format("consolidate")}


def opening(paths, budget=25, chars=None, host=None):
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
        for tok in sorted(set(predicate_refs(j["pred"]))):
            if tok in ids and tok not in j["deps"]:
                items.append((100, name, f"predicate reads {tok}, which it does not declare - "
                                         f"a change to it never reaches this"))
        evaluable = bool([t for t in predicate_refs(j["pred"]) if t in ids]) \
            and not why_undecided(j["pred"])
        if _misfiled_reopener(j["body"], ids):
            items.append((100, name, "reopened_by reads as a comparison - a predicate belongs in "
                                     "wrong_if"))
        if not evaluable and not blocked and not _decided(j):
            items.append((40, name, "nothing evaluable would falsify it"))
        elif evaluable and evaluate(j["pred"], raw, ids) is True:
            items.append((95, name, f"wrong_if holds ({predicate_text(j['pred'])[:60]}) - broken by its own "
                                    f"condition"))
        for dep, old, now, state in moved_deps(j, raw, ids):
            if state == "moved":
                was, is_ = apart(old, now, 28)
                items.append((70, name, f"{dep} differs from what it last saw: {was} -> {is_}"))
        day = reversal_pending(j["body"]) if not is_arrangement(j, raw) else None
        if day:
            items.append((75, name, f"reversed on {day} - the verdict under this id changed; review "
                                    f"it once read, or pull {name} --history"))
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
    # What a letter stands for, beside the namespace - judgments' prefixes included, since
    # a grounding line names them by id (see legend_of).
    legend = legend_of(meta, ids)
    if legend:
        head.append("prefixes: " + " · ".join(f"{g}={w}" for g, w in legend.items()))
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
    head.extend(_peer('knowledge_views').lines(doc))
    waiting = hypothesis_line(doc)
    if waiting:
        head.append(waiting)
    # readings that moved while only a replaced judgment listened: nothing standing will
    # ever flag them, so the opener says how many and names the first
    lost = lost_ears(paths, ids, jud, raw)
    if lost:
        k, jid, day = lost[0]
        head.append(f"{len(lost)} reading{'s' if len(lost) != 1 else ''} moved that only a replaced "
                    f"judgment listened to: {k} ({jid} until {day})"
                    + (f" and {len(lost) - 1} more" if len(lost) > 1 else "")
                    + f" - pull {jid} --history")
    # a record moved by half: what sits beside the entry file under the other layout is
    # read by nothing, and this is where a session would otherwise never learn it
    left = leftover_head(paths)
    if left:
        head.append(left)
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

    # The next moves are named as the host invokes them: a skill where the host has skills
    # (`/kpopper:ground` in Claude Code, `$ground` in Codex), the reader's own verbs elsewhere.
    moves = next_moves(host)
    if moves:
        footer = (f"next: {moves['ground']} (values with sources, what a change reaches) · "
                  f"{moves['record']} (what this session found) · check")
        rest = (f" · {moves['ground']} (values with sources, what a change reaches) · "
                f"{moves['record']}")
    else:
        footer = ("next: pull <entry|prefix> (values with sources) · affects <entry> "
                  "(what a change reaches) · check")
        rest = (" · pull <entry|prefix> (values with sources) · affects <entry> "
                "(what a change reaches)")
    # An intent no tab of the page serves is said at every open, in the one line that is
    # already about what to do next: the newest first and how many more, never the list -
    # the slot is for what needs a person, and check names the rest with a hint each. An
    # arrangement whose sign appeared comes first: it is the gap read by a decision.
    info = _page_or_error(paths, read_mode=getattr(doc, 'read_mode', None))
    facts = (info.get("arrangements") or {}) if info and "error" not in info else {}
    fired = sorted(k for k, f in facts.items() if f["fired"])
    cov = info.get("coverage") if info and "error" not in info else None
    unserved = [r["id"] for r in cov["rows"] if r["unserved"]] if cov else []
    if fired:
        footer = (f"next: check - {fired[0]} fired ({short(jud[fired[0]]['pred'], 40)})"
                  + (f" (and {len(fired) - 1} more)" if len(fired) > 1 else "") + rest)
    elif unserved:
        footer = (f"next: check - {unserved[0]} is served by no tab"
                  + (f" (and {len(unserved) - 1} more)" if len(unserved) > 1 else "") + rest)
    if moves and doc.hypotheses:
        # hypotheses beside the record are a move of their own on a host that has the skill
        n = len(doc.hypotheses)
        footer += f" · {moves['consolidate']} ({n} hypothes{'is waits' if n == 1 else 'es wait'})"

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
        standing.append("standing:" + (f"  ({len(flagged)} above need{'s' if len(flagged) == 1 else ''} "
                                        f"a person)" if flagged else ""))
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
        for t in rule_refs(b, ids):
            if t in ids and t != k:
                feeds.setdefault(t, []).append(k)
    # Index judgment readers once, in the same order used by the reach report. Rebuild
    # for this record so edits and hypothesis worlds never share stale dependencies.
    judgment_feeds = {}
    for name, j in sorted(jud.items()):
        for dep in set(j["deps"]):
            judgment_feeds.setdefault(dep, []).append(name)
    for readers in feeds.values():
        readers.sort()
    hit, seen_e, frontier, derived = {}, set(changed), list(changed), []
    while frontier:
        m = frontier.pop()
        for e in feeds.get(m, ()):
            if e not in seen_e:
                seen_e.add(e)
                derived.append(e)
                frontier.append(e)
        for name in judgment_feeds.get(m, ()):
            if name not in hit:
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
            named = [d for d in j["deps"] if d in moved and d in predicate_refs(j["pred"])]
            why = ("evaluate the predicate against " + ", ".join(named)) if named else "flagged only"
            label = ('contribution ' if n and doc.hypotheses[n].get('kind') == 'contribution' else 'hypothesis ') + str(n)
            print(f"{name}" + (f" (in {label})" if n else "")
                  + f"\n    via {via} -> {why}"
                  + (f"\n    predicate: {predicate_text(j['pred'])}" if j["pred"] else ""))
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

    def layer_label(name):
        return ('contribution ' if doc.hypotheses[name].get('kind') == 'contribution' else 'hypothesis ') + name

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

    def describe(k, b, ids_, raw_):
        """-> (line, shown): an entry's line, and the value it shows."""
        v, rule = resolved(b, ids_)
        if isinstance(rule, dict):
            v = value_of(raw_, ids_, k)
        if v is not None:
            shown = E.display_value(v) + (" = " + predicate_text(rule) if isinstance(rule, dict) else "")
        elif rule:
            shown = "= " + predicate_text(rule)
        elif b.get("quoted"):
            shown = '"' + str(b["quoted"]) + '"'
        else:
            shown = ""
        nm = named(b)
        line = f"{k}: {shown}" + (f" ({nm})" if nm else "")
        if b.get(MEASURE):
            line += f" measured by {b[MEASURE]}"
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
            state = f"broken: wrong_if holds ({predicate_text(j['pred'])})"
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
            out.append(cut(f"    wrong_if: {predicate_text(j['pred'])}", 110))
        reopened = _reopened_text(body)
        if reopened:
            out.append(cut(f"    reopened by: {reopened}", 110))
        # whose asking a decision was taken from is read wherever the decision is: it is
        # provenance a reader weighs, never what admitted the write
        if body.get("request"):
            out.append(cut(f"    on the word of: {body['request']}", 110))

        # Every move since the snapshot, with what the predicate made of it - pull is
        # the grounding surface, so here even a muted move is worth a line.
        for dep, old, now, state in sorted(moved_deps(j, raw_, ids_)):
            mark_ = {"muted": " - within wrong_if", "crossed": " - across wrong_if"}.get(state, "")
            # the two readings are clipped to what the line has left for them, and the
            # line itself is never cut: a budget spent on the first value would take the
            # arrow and the second reading with it, which is the whole failure here.
            head = f"    moved since review: {dep} "
            room = 110 - len(head) - len(mark_) - len(" -> ")
            # a floor under each side, so an id long enough to eat the line leaves a
            # comparison a reader can still use rather than two stubs
            was, is_ = apart(old, now, max(24, room // 2))
            out.append(f"{head}{was} -> {is_}{mark_}")
        return out

    def dispute(k):
        return cut("    CONTESTED: " + ", ".join(f"{n} says {short(c)}" for n, c in disputed[k]), 110)

    lines = list(unread)
    for k in sorted(entries):
        holders = in_hypothesis(k)
        if k in raw:
            line, shown = describe(k, raw.get(k) or {}, ids, raw)
            lines.append(cut(line, 110))
            # what each hypothesis proposes for it - read from the record as it stands under
            # that hypothesis, so a rule or a reference resolves there; a source read again,
            # or a value re-sourced, is a proposal too
            for n, l in holders:
                line_h, now = describe(k, l[4].get(k) or {}, l[1], l[4])
                if now != shown:
                    lines.append(cut(f"    proposes {shown} -> {now}, from {n}", 110))
                elif line_h != line:
                    lines.append(cut(f"    proposes instead, from {n}: {line_h[len(k) + 2:].strip()}", 110))
        elif holders:
            n, l = holders[0]
            line, _ = describe(k, l[4].get(k) or {}, l[1], l[4])
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
                                    tag=f" (in {layer_label(n)})")
        if name in disputed:
            lines.append(dispute(name))

    lines = _peer('knowledge_views').lines(doc) + lines
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
NUM_VALUE = re.compile(r"^-?\d[\d,]*(\.\d+)?$")   # as a record writes one: 1,000 is 1000
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
    key = " " * ind + scalar(field, fold=False) + ": "
    children = v.values() if isinstance(v, dict) else v if isinstance(v, list) else []
    if any(isinstance(child, (dict, list)) for child in children):
        dumped = yaml.safe_dump({field: v}, allow_unicode=True, sort_keys=False, width=width - ind)
        return [" " * ind + line for line in dumped.rstrip().splitlines()]
    if isinstance(v, list):
        return [key + "[" + ", ".join(scalar(x, fold=False) for x in v) + "]"]
    if isinstance(v, dict):
        flow = key + "{" + ", ".join(f"{scalar(k, fold=False)}: {scalar(x, fold=False)}" for k, x in v.items()) + "}"
        if len(flow) <= width:
            return [flow]
        out = [" " * ind + scalar(field, fold=False) + ":"]
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
        if _inline(lines[s]).startswith('{'):
            block = '\n'.join(lines[s:e])
            metadata = yaml.compose(block).value[0][1]
            current = next((value for key, value in metadata.value if key.value == 'updated'), None)
            if current is not None:
                start, end = current.start_mark.index, current.end_mark.index
                block = block[:start] + scalar(date, _style(block[start:end]), fold=False) + block[end:]
            else:
                at = metadata.start_mark.index + 1
                block = block[:at] + 'updated: ' + scalar(date, fold=False) + (', ' if metadata.value else '') + block[at:]
            lines[s:e] = block.split('\n')
            return True
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
    b = layout(path)["view"]
    return b if os.path.exists(b) else None


def _page_info(paths, read_mode=None):
    """What the page knows when it is built beside this record - its counts, its shape, the
    coverage report - or None when no brief sits beside the record."""
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import render_page as R
    mode = read_mode or ('frozen' if _RAW_READS.get() else os.environ.get('KPOPPER_READ_MODE', 'live'))
    paths = R.record_paths(paths, read_mode=mode)
    brief = R.find_brief(paths, read_mode=mode)
    if not brief:
        return None
    return R.build(paths, brief, read_mode=mode)[4]


def _page_side(paths, read_mode=None):
    """page.* as the page counts them - every name, mentioned or not, since a new judgment
    may be the first to rest on one - the record's shape, and every arrangement held against
    the brief: -> (values, shape, arrangements). ({}, None, {}) without a brief; a count the
    page could not take is left out."""
    info = _page_info(paths) if read_mode is None else _page_info(paths, read_mode=read_mode)
    if info is None:
        return {}, None, {}
    return ({k: v for k, v in info["page"].items() if v is not None}, info["shape"],
            info.get("arrangements") or {})


def _page_or_error(paths, read_mode=None):
    """What the page knows, or None without a brief, or {"error": why} when the brief cannot
    be built - the reader never fails on the page's account, it says so in a line."""
    try:
        return _page_info(paths) if read_mode is None else _page_info(paths, read_mode=read_mode)
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
    if getattr(raw, 'world', None) is not None:
        return raw.world.history(dep, jud, page)
    if dep in jud:
        b = jud[dep]["body"]
        return str(b.get("verdict") or b.get("title") or dep)
    if dep in PAGE:
        return page.get(dep)
    b = raw.get(dep)
    if not isinstance(b, dict):
        return b
    if isinstance(b.get("rule"), dict):
        result = E.current(raw, ids, dep)
        if result["value"] is None:
            raise Refused("cannot snapshot " + dep + ": " + result["reason"])
        return {"computed": {"value": result["value"], "rule": b["rule"]}}
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


def reasoning_of(body):
    """The argument a judgment carries, under either of the names the page draws."""
    return str(body.get("because") or body.get("breaks_if") or "")


def shown_value(dep, raw, ids, jud, page):
    """What a section's text records for a reference it draws. A judgment placed in prose
    puts both halves of itself on the page - the conclusion the sentence around it was
    written against, and the argument the sentence actually shows - so the text is held to
    both, and a rewrite of either is a move under it. They are recorded as two fields rather
    than as one line, because a delimiter is not a representation: prose contains every
    separator anyone might pick, and a snapshot that has to be parsed back is a snapshot
    that can be read wrongly. A judgment's *own* snapshot keeps only the verdict, because
    what rests on a judgment rests on its conclusion and should survive three rewrites of
    the prose; a text is the one place that prose is itself on the surface."""
    if dep in jud:
        b = jud[dep]["body"]
        seen = {"verdict": str(b.get("verdict") or b.get("title") or dep)}
        because = reasoning_of(b)
        if because:
            seen["because"] = because
        return seen
    return snapshot_value(dep, raw, ids, jud, page)


def same_seen(old, now):
    """Whether a snapshot still holds: text after whitespace, then number - and, for the two
    fields a placed judgment records, field by field."""
    if isinstance(old, dict) or isinstance(now, dict):
        if not (isinstance(old, dict) and isinstance(now, dict)):
            return False
        return set(old) == set(now) and all(same_seen(old[k], now[k]) for k in old)
    if " ".join(str(old).split()) == " ".join(str(now).split()):
        return True
    try:
        return Decimal(str(old).replace(",", "")) == Decimal(str(now).replace(",", ""))
    except InvalidOperation:
        return False


HALVES = {"because": "the reasoning it places",
          "verdict": "the verdict over the reasoning it places"}


def which_moved(old, now):
    """-> (what to call it, was, now). A placed judgment records its conclusion and its
    argument as two fields, so quoting the whole snapshot would print the same clipped
    verdict twice when only the argument was rewritten. Name the field that moved and quote
    that; every surface reporting a text's move reads the same reading. Anything that is
    not a placement is quoted whole, under no name."""
    if isinstance(old, dict) and isinstance(now, dict):
        moved = [f for f in ("verdict", "because")
                 if not same_seen(old.get(f, ""), now.get(f, ""))]
        if len(moved) == 1:
            f = moved[0]
            return HALVES[f], old.get(f, "never read against it"), now.get(f, "")
        return "", old.get("verdict", ""), now.get("verdict", "")
    # a text written before a placement was held to its argument saw the verdict alone:
    # nothing moved under it, it was never read against the sentence it shows
    if not isinstance(old, dict) and isinstance(now, dict):
        if same_seen(old, now.get("verdict", "")):
            return HALVES["because"], "never read against it", now.get("because", "")
        return HALVES["verdict"], old, now.get("verdict", "")
    return "", old, now


def _state(name, j, raw, ids, fields, touched=()):
    """-> (tag, reason): a judgment's state after a write - the reading `check` gives it,
    said in terms of what just moved."""
    if getattr(raw, 'world', None) is not None:
        return raw.world.state(name)
    blocked = _blocked_text(j["body"])
    missing = [d for d in j["deps"] if d not in ids]
    if missing:
        return (("BLOCKED", f"waiting on {', '.join(missing)} - {blocked[:70]}") if blocked
                else ("BROKEN", f"rests on {', '.join(missing)}, which is not an entry"))
    named_ = [t for t in predicate_refs(j["pred"]) if t in ids]
    if named_ and evaluate(j["pred"], raw, ids) is True:
        return "FIRED", f"wrong_if holds ({predicate_text(j['pred'])}) - broken by its own condition"
    unchecked = [d for d in j["deps"] if fields["snapshot"] and d not in j["seen"]]
    formula_only = formula_only_snapshots(j, raw)
    moves = [(d, o, n, s) for d, o, n, s in moved_deps(j, raw, ids) if not touched or d in touched]
    if any(s == "moved" for _, _, _, s in moves):
        d, o, n, _ = next(x for x in moves if x[3] == "moved")
        snapshot = j["snap"].get(d)
        if isinstance(snapshot, dict) and isinstance(snapshot.get("computed"), dict):
            body = raw.get(d)
            if not E.same(snapshot["computed"].get("rule"), body.get("rule") if isinstance(body, dict) else None):
                return "MOVED", f"{d}: formula changed since review"
        current_body = raw.get(d)
        if legacy_snapshot_rule(snapshot, current_body.get("rule") if isinstance(current_body, dict) else None) is not None:
            return "MOVED", f"{d}: formula changed since legacy review; no historical result was recorded"
        was, is_ = apart(o, n)
        return "MOVED", (f"{d} moved {was} -> {is_} since it was reviewed - "
                         f"if it still holds: review {name}")
    if unchecked:
        return "UNCHECKED", (f"never checked against {', '.join(unchecked)} - "
                             f"if it holds: review {name}")
    if formula_only:
        return "UNCHECKED", (f"legacy snapshot records only the formula for {', '.join(formula_only)}, "
                             f"not a historical result; review {name} to record the current calculation")
    if any(s == "muted" for _, _, _, s in moves):
        d, o, n, _ = next(x for x in moves if x[3] == "muted")
        was, is_ = apart(o, n)
        return "MUTED", (f"{d} moved {was} -> {is_}, inside wrong_if ({predicate_text(j['pred'])}) - "
                         f"nothing is asked")
    if not named_ and not blocked:
        reopened = _decided(j)
        if reopened:
            return "HOLDS", "decided; reopened by " + short(reopened, 80)
        return "NO_PREDICATE", "nothing evaluable would say otherwise"
    if not named_:
        return "DECLARED", "no predicate to evaluate; declared - " + short(blocked, 80)
    pred = short(j["pred"], 80)
    if evaluate(j["pred"], raw, ids) is None:
        if why_undecided(j["pred"]):
            return "UNKNOWN", f"wrong_if is not a comparison this reader decides ({pred})"
        if any(t in PAGE for t in predicate_refs(j["pred"])):
            return "UNKNOWN", f"wrong_if is counted when the page is built ({pred}) - page --verify decides it"
        return "UNKNOWN", f"wrong_if cannot currently be evaluated ({pred}); a value or the Lean core is unavailable, or types differ"
    return "HOLDS", f"wrong_if does not hold ({pred})"


def _state_line(name, j, raw, ids, fields, touched=()):
    tag, why = _state(name, j, raw, ids, fields, touched)
    return f"{tag:<9} {name}: {why}"


def _texts_that_saw(paths, keys):
    """Sections of the brief whose text or seen names one of `keys` -> [(title, seen)]."""
    brief = _brief_beside(paths[0])
    if not brief:
        return []
    b = parse(brief) or {}
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
            if isinstance(b, dict) and b.get("v") is None and b.get("quoted") is None and not (
                    getattr(raw, 'world', None) is not None and any(key in b for key in ('v', 'quoted'))):
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
    pred = predicate_of(body, fields)
    for tok in sorted(set(predicate_refs(pred))):
        if (isinstance(pred, dict) or tok in ids or is_builtin(tok)) and tok not in deps:
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


def _sound_citation(a, doc, ids, jud, fields, raw):
    """An explicit replacement citation names a recorded source and its new location."""
    if a.get("source") is None and a.get("at") is None:
        return []
    if a["kind"] != "set":
        return ["--source and --at apply to set; add carries from= and at= in its fields"]
    if not isinstance(a.get("source"), str) or not a["source"].strip() \
            or not isinstance(a.get("at"), str) or not a["at"].strip():
        return ["set requires --source and --at together, both nonempty"]
    src = a["source"]
    body = raw.get(src)
    if src not in ids or src == a["id"] or src in jud or is_builtin(src) \
            or not isinstance(body, dict) or any(f in body for f in ("v", "quoted", "rule")) \
            or not any(body.get(f) for f in ("asked", "file", "url", "of", "read")):
        return [f"{src} is not a recorded source; add the source before citing it"]
    current = raw.get(a["id"])
    if isinstance(current, dict) and any(f in current for f in ("src", "source")):
        return ["set --source writes from/at; reconcile the entry's src/source fields first"]
    if isinstance(current, dict) and any(isinstance(current.get(f), (dict, list)) for f in ("from", "at")):
        return ["set --source needs scalar from/at fields"]
    return []


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
    """An arrangement's sign is decided by the build, so it is one comparison that can hold,
    and its `born` is the day it is written, stamped by this tool like `seen`."""
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
    pred = predicate_of(body, fields)
    bad = one_comparison(pred, raw, ids)
    if bad:
        out.append(f"wrong_if {bad} - an arrangement's sign is decided by the build, or it is "
                   f"decoration")
    return out


def _request_names_the_asking(a, doc, ids, jud, fields, raw):
    """`request:` names whose asking the judgment was taken from - a session source carrying
    that request verbatim, which the judgment also rests on. It opens no door; it is the
    claim a reader holds against the `asked:` it points at, so it is asked of every judgment
    that carries one, and refused before it can look like anyone's word."""
    if a["kind"] != "add" or not isinstance(a["body"], dict):
        return []
    body = a["body"]
    req = body.get("request")
    if req is None:
        return []
    deps = body.get(fields["deps"]) if fields["deps"] else None
    deps = deps if isinstance(deps, list) else []
    if isinstance(req, str) and is_intent(req, raw) and req in deps:
        return []
    return [f"request: {req} is not a session source carrying what was asked, that the judgment "
            f"also rests on"]


def _not_born_broken(a, doc, ids, jud, fields, raw):
    """A judgment whose own condition holds the moment it is written is a mistake caught
    here, not a record that fails check a second later. A re-decision of an arrangement is
    the one exception: it is recorded while the sign that ended the old decision still
    holds, and the next build decides it against the repaired brief."""
    if a["kind"] != "add" or not isinstance(a["body"], dict) or fields["deps"] not in a["body"]:
        return []
    if a["id"] in jud and is_arrangement(jud[a["id"]], raw):
        return []
    pred = predicate_of(a["body"], fields)
    if pred and evaluate(pred, raw, ids) is True:
        return [f"wrong_if already holds ({predicate_text(pred)}) - the judgment would be born broken"]
    return []


def arrangement_renewal(old, ended, stamp, stood=None):
    """What a build writes when an arrangement is decided again under its own id -> the two
    fields: `born`, renewed to the day of the decision, and one line appended to `replaced:`
    keeping the born of what it replaced, how long it stood, and what ended it, so the
    sequence of decisions reads from the record alone. Written by `add` when the door admits
    the re-decision in place, and by the fold when a person lays one over it - a re-decision
    that reached the record either way leaves the same trail."""
    prior = old.get("replaced") or []
    prior = [prior] if isinstance(prior, str) else list(prior)
    return {"born": stamp,
            "replaced": prior + [f"born {old.get('born') or 'undated'}"
                                 + (f", stood {stood} session{'' if stood == 1 else 's'}"
                                    if stood is not None else "")
                                 + f"; {ended} on {stamp}"]}


def judgment_renewal(old, ended, stamp):
    """What a write leaves on a judgment it replaces in place -> the one field: one line
    appended to `replaced:` naming what ended the decision it replaces and the day, so the
    sequence of decisions reads from the record alone; the body it replaced is kept whole
    beside the record (`keep_replaced`), one version per line. The replacing body's own
    lines - a branch's history - are not carried: the base's trail is the base's, and the
    branch keeps its own. Written by `add` when the door admits the replacement, and by
    the fold."""
    prior = old.get("replaced") or []
    prior = [prior] if isinstance(prior, str) else list(prior)
    return {"replaced": prior + [f"{ended} on {stamp}"]}


TRAIL_FIELDS = ("replaced",)          # written by this tool on the judgment that replaced


def replaced_path(paths):
    """Where the record keeps the judgments its writes replaced: `.kpopper/replaced.yaml`
    beside a record under the new name, `PROVENANCE.replaced.yaml` beside one under the old."""
    return layout_of(paths)["replaced"]


def read_replaced(paths):
    """The kept versions of every replaced judgment -> {id: [version, ...]}, oldest first;
    {} when nothing was ever replaced. A version is the body a replacement removed, whole,
    with `ended` (what admitted the replacement) and `day`; one equal to an earlier version
    is kept as {same_as: <version number, from 1>, ended, day} - a return, not a copy."""
    p = replaced_path(paths)
    if not os.path.isfile(p):
        return {}
    try:
        data = parse(p) or {}          # the reader's one parser: kept against the file's identity
    except yaml.YAMLError:
        return {}
    return {k: v for k, v in data.items() if isinstance(v, list)} if isinstance(data, dict) else {}


def _version_core(v):
    """What tells two kept versions apart: the decision itself, not what it saw or when."""
    return {k: x for k, x in v.items()
            if k not in ("seen", "reviewed", "born", "replaced", "ended", "day", "same_as", "dropped")}


def version_at(versions, n):
    """Kept version n, counted from 1, with a pointer followed -> the body it stands for. A
    pointer that leads nowhere - out of range, or round in a circle - stands for itself."""
    v = versions[n - 1]
    hops = 0
    while isinstance(v, dict) and "same_as" in v and hops < len(versions):
        try:
            v = versions[int(v["same_as"]) - 1]
        except (ValueError, TypeError, IndexError):
            break
        hops += 1
    return v


def keep_replaced(paths, nid, old, ended, stamp, dropped=None):
    """One version kept beside the record: the body a replacement removed - its verdict, its
    why, whose asking it answered, what it rested on, its condition, what it saw - with what
    ended it and the day; the dependencies the replacement dropped, each with its reason
    when one was given. A body equal to a version already kept is kept as a pointer to it.
    Written at the moment of replacement and read only when asked, so the file grows with
    reversals and never with the record. -> the version's index."""
    kept = read_replaced(paths)
    versions = kept.setdefault(nid, [])
    body = {k: copy.deepcopy(v) for k, v in old.items() if k not in TRAIL_FIELDS}
    if isinstance(body.get("wrong_if"), dict):
        body["wrong_if"] = predicate_text(body["wrong_if"])
    core = _version_core(body)
    version = None
    for n in range(1, len(versions) + 1):
        if _version_core(version_at(versions, n)) == core:
            version = {"same_as": n}
            break
    if version is None:
        version = body
    version["ended"] = ended
    version["day"] = stamp
    if dropped:
        version["dropped"] = dict(dropped)
    versions.append(version)
    p = replaced_path(paths)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    text = yaml.safe_dump(kept, allow_unicode=True, sort_keys=False, width=100)
    _write_text(p, "# Judgments this record's writes replaced, kept whole - read with "
                   "`kpopper pull <id> --history`.\n" + text)
    return len(versions)


def returns_to(paths, nid, body):
    """A kept version the body being written returns to -> (number from 1, version, exact) or
    None: exact when the decision is the one kept - verdict, why, dependencies, condition -
    else the verdict alone, on other grounds. A decision that stood before and fell is not a
    fresh decision, and the write that brings it back is told so."""
    versions = read_replaced(paths).get(nid) or []
    want = _verdict_of(body)
    if want is None:
        return None
    core = _version_core({k: v for k, v in body.items() if k not in TRAIL_FIELDS})
    if isinstance(core.get("wrong_if"), dict):
        core["wrong_if"] = predicate_text(core["wrong_if"])
    found = None
    for n in range(1, len(versions) + 1):
        v = version_at(versions, n)
        if not isinstance(v, dict) or _verdict_of(v) is None or not _same(_verdict_of(v), want):
            continue
        if _version_core(v) == core:
            return n, versions[n - 1], True
        found = found or (n, versions[n - 1], False)
    return found


def _text_of_or_none(path):
    if not os.path.isfile(path):
        return None
    with io.open(path, encoding="utf-8") as f:
        return f.read()


REPLACED_DAY = re.compile(r" on (\d{4}-\d{2}-\d{2})$")


def reversal_pending(body):
    """The day a judgment's verdict was last replaced under its id, when nobody has reviewed
    it since -> "YYYY-MM-DD", else None. The trail line carries the day; `reviewed:` is
    what clears it, and review writes that field on a judgment that carries a trail."""
    if not isinstance(body, dict):
        return None
    lines = body.get("replaced") or []
    lines = [lines] if isinstance(lines, str) else list(lines)
    if not lines:
        return None
    m = REPLACED_DAY.search(str(lines[-1]))
    if not m:
        return None
    day = m.group(1)
    seen = _as_day(body.get("reviewed"))
    return None if seen and seen.isoformat() >= day else day


def listened_until(kept, dep):
    """The replaced judgments that rested on `dep` where nothing standing does now ->
    [(judgment id, day it stopped listening, what it saw)], latest version per judgment;
    `kept` is what `read_replaced` returned, read once by the caller."""
    out = []
    for jid, versions in sorted(kept.items()):
        last = None
        for n in range(1, len(versions) + 1):
            v = version_at(versions, n)
            deps = v.get("rests_on") if isinstance(v, dict) else None
            if isinstance(deps, list) and dep in deps:
                seen = v.get("seen") if isinstance(v.get("seen"), dict) else {}
                last = (jid, versions[n - 1].get("day"), seen.get(dep))
        if last:
            out.append(last)
    return out


def lost_ears(paths, ids, jud, raw):
    """Readings that moved while only a replaced judgment listened to them -> [(id, judgment,
    day)]: no standing judgment rests on the reading, a kept version did, and the value now
    differs from what that version saw. Said once in the opener, in one line."""
    kept = read_replaced(paths)
    if not kept:
        return []
    listened = {d for j in jud.values() for d in j["deps"]}
    out = []
    for k in sorted(ids):
        if k in jud or k in listened or is_builtin(k):
            continue
        now = value_of(raw, ids, k)
        for jid, day, saw in listened_until(kept, k):
            if saw is not None and now is not None and not _same(saw, now):
                out.append((k, jid, day))
                break
    return out


def history_lines(paths, names):
    """The kept versions of the judgments named, oldest first, for `pull --history`."""
    kept = read_replaced(paths)
    where = os.path.relpath(replaced_path(paths), os.path.dirname(_first_of(paths)))
    out = []
    for nid in names:
        versions = kept.get(nid) or []
        if not versions:
            continue
        out.append(f"history of {nid}: {len(versions)} version{'s' if len(versions) != 1 else ''} "
                   f"kept in {where}")
        for n, v in enumerate(versions, 1):
            head = f"  {n}. until {v.get('day')} - {v.get('ended')}"
            if "same_as" in v:
                out.append(head + f" (the same decision as version {v['same_as']})")
                continue
            out.append(head)
            for f in ("verdict", "because"):
                if v.get(f):
                    out.append(f"     {f}: {short(str(v[f]), 100)}")
            deps = v.get("rests_on")
            if isinstance(deps, list):
                out.append("     rests_on: [" + ", ".join(str(d) for d in deps) + "]")
            if v.get("wrong_if"):
                out.append(f"     wrong_if: {predicate_text(v['wrong_if'])}")
            if v.get("request"):
                out.append(f"     request: {v['request']}")
            for d, why in (v.get("dropped") or {}).items():
                out.append(f"     no longer rested on {d}: {why}")
    return out


def dropped_deps(old, new, fields):
    """The dependencies the replacing body no longer rests on -> [id]."""
    was = old.get(fields["deps"]) if fields["deps"] else None
    now = new.get(fields["deps"]) if fields["deps"] else None
    was = [d for d in was if isinstance(d, str)] if isinstance(was, list) else []
    now = [d for d in now if isinstance(d, str)] if isinstance(now, list) else []
    return [d for d in was if d not in now]


def trail_lines(paths, nid, old, new, fields, drops=None, index=None):
    """What the reply says about a replacement: what was kept, what the new decision no
    longer rests on and why, and whether it returns to a decision that stood before."""
    out = []
    gone = [f for f in ("verdict", "because", "request") if f in old]
    where = os.path.relpath(replaced_path(paths), os.path.dirname(_first_of(paths)))
    out.append(f"  kept: the replaced {', '.join(gone) if gone else 'body'}, in {where}"
               + (f" (version {index})" if index is not None else ""))
    dropped = dropped_deps(old, new, fields)
    if dropped:
        drops = drops or {}
        out.append("  no longer rests on " + "; ".join(
            f"{d}: {drops[d]}" if drops.get(d) else d for d in dropped))
    return out


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


def may_supersede(nid, existing, new, raw, ids, jud, fields, as_of=None, page=None,
                  by_hand=False):
    """May a write under an id the base holds replace what it holds? -> (yes, why). The one
    door every same-id write goes through - `set` of a value, `add` of a judgment under a
    standing id, and the fold of a hypothesis - so the rules that open it live here and
    nowhere else. An entry: when the new reading is newer than the base's own - its `of:`,
    else its source's read date; a day is the finest clock the record keeps, and a value
    nothing dates is superseded by one something does. A judgment: when the standing one's
    wrong_if holds now - it is broken, and the new verdict is its repair - or when
    `by_hand`, a fold in which a person named the id (`--take`). Nothing inside the write itself opens it: every
    session's first write is a source carrying what it was asked, so a field that read a
    person's authority off one would hand every session the key to every standing judgment.
    Everything else contradicts, and a contradiction forks - into a hypothesis a person
    folds. `existing` is the base's body, `new` the value or the body being written.

    An arrangement - `page` carries what the page holds it to, which only the write path
    reads - has three rules of its own before those: never twice in a day, so two sessions
    cannot flip it and a second writer cannot re-decide it behind the first while the
    first's repair is still being made; only while the brief still carries what it decided,
    since a tab deleted or gutted makes the very sign it would cite - so a cut link is
    refused with "restore, then re-decide"; and its sign read with its tabs intact is what
    "its wrong_if holds" means for it."""
    if nid in jud:
        if not _judgment_shaped(new, fields):
            return False, "what replaces a judgment must rest on something, and this carries no " \
                          + str(fields["deps"] or "dependencies")
        facts = (page or {}).get(nid)
        if facts is not None:
            stamp = _as_day(as_of) or datetime.date.today()
            born = _as_day(jud[nid]["body"].get("born"))
            if born and born >= stamp:
                return False, (f"it was decided on {born} - a second decision on the same day is a "
                               f"contradiction, not a change")
            if not facts["linked"]:
                return False, ("the brief no longer carries what it decided - " + facts["cut"]
                               + " - restore the tab, then re-decide")
            if facts["fired"]:
                return True, (f"its sign holds ({short(jud[nid]['pred'], 60)}) with its tab intact"
                              + (f" - {facts['reading']}" if facts.get("reading") else ""))
        if evaluate(jud[nid]["pred"], raw, ids) is True:
            return True, f"its wrong_if holds ({short(jud[nid]['pred'], 60)})"
        if by_hand:
            return True, "the standing judgment holds, and a person takes this over it by name"
        return False, "the standing judgment holds, and its wrong_if has not fired"
    when = _read_on(existing, raw)
    stamp = _as_day(as_of) or datetime.date.today()
    if when is None:
        return True, "nothing dates the reading the base holds"
    if stamp > when:
        return True, f"a reading from {stamp} that is newer than the base's"
    return False, ("a reading of the same day" if stamp == when
                   else f"a reading from {stamp} that is older than the base's")


JUDGMENT_KINDS = ("verdict", "grounds", "arrangement")   # what _disagreement says of a judgment


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
        kind = "verdict"
        if _writer_same(raw, old, new):
            skip = ("born", "replaced", "reviewed")
            was = {f: v for f, v in jud[k]["body"].items()
                   if f not in skip and f != fields["snapshot"]}
            now = {f: v for f, v in a["body"].items() if f not in skip and f != fields["snapshot"]}
            if was == now:
                return None
            # the same verdict on other grounds - another why, other dependencies, another
            # condition - is a decision written again, and the same door decides it
            kind = "arrangement" if is_arrangement(jud[k], raw) else "grounds"
        may, why = may_supersede(k, jud[k]["body"], a["body"], raw, ids, jud, fields,
                                 a.get("as_of"), page)
        return kind, old, new, may, why, None
    if not isinstance(body, dict):
        return None
    old = value_of(raw, ids, k)
    if (old is None and not (getattr(raw, 'world', None) is not None and raw.world.result(k)['status'] == 'ok')) or isinstance(old, (list, dict)):
        return None
    if a["kind"] == "set":
        new = a["value"]
    else:
        b = a["body"] if isinstance(a["body"], dict) else {}
        new = b.get("v") if b.get("v") is not None else b.get("quoted")
        if new is None or isinstance(new, (list, dict)):
            return None
    if _writer_same(raw, old, new):
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
                    s = yaml.safe_dump(v, default_flow_style=True, allow_unicode=True,
                                       sort_keys=False, width=1000000).strip()
                elif isinstance(v, bool):
                    s = str(v).lower()
                else:
                    s = str(v)
                parts.append(f"{f}={s}")
        else:
            parts.append(str(body))
    for opt, key in (("--in", "into"), ("--as-of", "as_of"), ("--why", "why"),
                     ("--source", "source"), ("--at", "at")):
        if a.get(key):
            parts += [opt, str(a[key])]
    for d, why in (a.get("drops") or {}).items():
        parts += ["--drop", f"{d}: {why}"]
    parts += ["--hypothesis", name]
    return " ".join(p if p.startswith("--") else shlex.quote(p) for p in parts)


def _hypothesis_name(doc, k, claim):
    """A name for the hypothesis a refused write would open: the id contradicted and a mark
    of the claim written. Two branches refused on one id open one file only where they claim
    the same thing - and that meeting is worth having, since the two heads say one thing and
    whoever merges them keeps either. Two that disagree, which is what the fork exists for,
    open two files and merge. The count beside the name separates two claims that mark alike
    in one checkout; it counts only what this checkout holds, so it never crosses a branch."""
    base = re.sub(r"[^A-Za-z0-9_\-]", "_", k) + "_" + _claim_mark(claim)
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
            was, is_ = apart(old, new)
            out.append(f"{k} holds {was} as of {when}, and {why} says {is_} - the base "
                       f"keeps what it holds and a hypothesis holds the other: "
                       f"{_command_of(a, _hypothesis_name(doc, k, new))}")
        return out
    if a["kind"] != "add":
        return out
    if k in ids:
        d = _disagreement(a, raw.get(k), raw, ids, jud, fields, getattr(doc, "page", None))
        if d and not (d[0] in JUDGMENT_KINDS and d[3]):   # a judgment that may supersede replaces
            kind, old, new, newer, why, when = d
            name = _hypothesis_name(doc, k, str(new) + " (regrounded)" if kind == "grounds" else new)
            if kind == "verdict":
                out.append(f"{k} is already a judgment, concluding {short(old, 60)!r} - {why}, so a "
                           f"different verdict under the same id contradicts it, and a hypothesis "
                           f"holds the other: {_command_of(a, name)}")
            elif kind == "grounds":
                out.append(f"{k} is already a judgment concluding the same, on other grounds - {why}, "
                           f"so the same verdict on other grounds is a decision written again, and a "
                           f"hypothesis holds it until a person takes it: {_command_of(a, name)}")
            elif kind == "arrangement":
                out.append(f"{k} is already this arrangement - {why}, so a hypothesis holds the "
                           f"re-decision: {_command_of(a, name)}")
            elif newer:
                out.append(f"{k} is already an entry, holding {short(old)}"
                           + (f" as of {when}" if when else "")
                           + f" - a newer reading updates it: set {k} {shlex.quote(str(new))}; one "
                           f"that disagrees opens a hypothesis: {_command_of(a, name)}")
            else:
                was, is_ = apart(old, new)
                out.append(f"{k} is already an entry, holding {was} as of {when}, and {why} "
                           f"says {is_} - the base keeps what it holds and a hypothesis holds "
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


def _not_from_the_future(a, doc, ids, jud, fields, raw):
    """An entry's own day - of:, read: - is a reading's place in the record's clock, and a
    day ahead of today would outrank every reading of today: refused, dated when read."""
    if a["kind"] != "add" or not isinstance(a.get("body"), dict):
        return []
    today = latest_today()
    out = []
    for f in ("of", "read"):
        day = _as_day(a["body"].get(f))
        if day and day > today:
            out.append(f"{f}: {a['body'][f]} is after today ({today.isoformat()}) - a reading is dated "
                       f"the day it was read, never ahead")
    return out


def _trail_is_tool_written(a, doc, ids, jud, fields, raw):
    """`replaced:` is the trail this tool leaves on a judgment that replaced another - on
    every judgment, not only an arrangement - and a session cannot write a history."""
    if a["kind"] != "add" or not isinstance(a.get("body"), dict):
        return []
    if _arrangement_shaped(a["body"], fields, raw):
        return []                                    # _arrangement_is_sound says it
    if "replaced" in a["body"]:
        return ["replaced is written by this tool, when a decision replaces another - leave it out"]
    return []


def _drops_are_named(a, doc, ids, jud, fields, raw):
    """A replacement that rests on less than the judgment it replaces stops listening to
    something - a decision, not a side effect - so each dependency dropped is named with
    its reason (--drop "<id>: <why>"), and the reason is kept with the replaced version."""
    if a["kind"] != "add" or not isinstance(a.get("body"), dict) or a.get("hypothesis"):
        return []
    k = a["id"]
    if k not in jud or not _judgment_shaped(a["body"], fields):
        return []
    if _arrangement_shaped(a["body"], fields, raw) or is_arrangement(jud[k], raw):
        return []          # an arrangement re-decided changes the count its sign is over
    d = _disagreement(a, raw.get(k), raw, ids, jud, fields, getattr(doc, "page", None))
    if not d or not (d[0] in JUDGMENT_KINDS and d[3]):
        return []                                    # refused anyway, or nothing replaces
    drops = a.get("drops") or {}
    old = jud[k]["body"]
    gone = dropped_deps(old, a["body"], fields)
    out = []
    unnamed = [x for x in gone if x not in drops]
    if unnamed:
        cmd = " ".join(["add", k] + [p for p in _command_of(a, "_").split(" --hypothesis ")[0].split(" ")[2:]]
                       + [f"--drop {shlex.quote(x + ': <why>')}" for x in unnamed])
        out.append(f"the new judgment no longer rests on {', '.join(unnamed)} - a dependency "
                   f"dropped is a decision with a reason: {cmd}")
    stray = [x for x in drops if x not in gone]
    if stray:
        out.append(f"--drop names {', '.join(stray)}, which the new judgment "
                   f"{'still rests on' if any(x in (a['body'].get(fields['deps']) or []) for x in stray) else 'never rested on here'}")
    return out


def _measure_is_a_name(a, doc, ids, jud, fields, raw):
    """A recipe is named by a stored scalar reading, with a bare name: the same rule check
    holds the record to, asked before the write."""
    if a["kind"] != "add" or not isinstance(a.get("body"), dict):
        return []
    bad = measure_problem(a["id"], a["body"], ids, jud, fields, raw)
    return [bad] if bad else []


def expression_problems(body, raw, ids, fields):
    if not isinstance(body, dict):
        return []
    problems = []
    for field, predicate in (("rule", False), (fields["predicate"], True)):
        value = body.get(field)
        if not isinstance(value, dict):
            continue
        try:
            E.validate(value, predicate=predicate)
        except (ValueError, TypeError) as error:
            problems.append(f"{field}: {error}")
            continue
        refs = E.refs(value)
        missing = [key for key in refs if key not in ids and not is_builtin(key)]
        if missing and not _blocked_text(body):
            problems.append(f"{field}: unknown references: " + ", ".join(missing))
        if predicate:
            undeclared = set(refs) - set(body.get(fields["deps"]) or [])
            if undeclared:
                problems.append("predicate reads undeclared references: " + ", ".join(sorted(undeclared)))
        elif any(key in body for key in ("v", "quoted")):
            problems.append("a structured rule cannot also store v or quoted")
    return problems


def _structured_is_sound(a, doc, ids, jud, fields, raw):
    if a["kind"] != "add":
        return []
    body = a["body"]
    out = expression_problems(body, raw, ids | {a["id"]}, fields)
    if not out and isinstance(body, dict) and isinstance(body.get("rule"), dict):
        candidate = dict(raw)
        candidate[a["id"]] = body
        candidate_ids = ids | {a["id"]} | {key for key in E.refs(body["rule"]) if is_builtin(key)}
        result = E.current(candidate, candidate_ids, a["id"])
        if result["value"] is None and not _blocked_text(body):
            out.append("rule cannot be computed: " + result["reason"])
    return out


def _nearest_existing(a, doc, ids, jud, fields, raw):
    """The entries nearest a new one, said in the reply - a note, never a refusal - and a
    write under an id that was retired into another, refused and pointed at it. Both live in
    sameness.py, beside the commands that record the answer."""
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import sameness
    return sameness.nearest_existing(a, doc, ids, jud, fields, raw)


def authored_fields(action, fields):
    """A first conventional falsifier has no existing predicate role to infer from."""
    body = action.get('body')
    if action['kind'] == 'add' and isinstance(body, dict) and fields['deps'] in body \
            and fields['predicate'] is None and 'wrong_if' in body:
        return dict(fields, predicate='wrong_if')
    return fields


def normalize_authored(action, ids, fields, raw):
    """Normalize only new/changed expression fields, in their actual writing context."""
    if getattr(raw, 'world', None) is not None:
        return raw.world.normalize(action)
    if action['kind'] != 'add' or not isinstance(action.get('body'), dict):
        return action, []
    import copy
    action = copy.deepcopy(action)
    body, nid = action['body'], action['id']
    predicate = fields['deps'] in body
    field = fields['predicate'] if predicate else 'rule'
    source = body.get(field)
    previous = raw.get(nid)
    if not isinstance(source, str) or (isinstance(previous, dict) and previous.get(field) == source):
        return action, []

    def keep(reason):
        return action, [f"NOTE {nid}.{field} kept as text: {reason}"]

    if not predicate and any(key in body for key in ('v', 'quoted')):
        return keep('a stored reading and a calculation need distinct fields/entries')
    try:
        comparison = CMP.match(source) if predicate else None
        tree = E.convert_authored(source, predicate=predicate,
            legacy_rhs=comparison.group(3) if comparison else None)
    except (ValueError, SyntaxError, RecursionError) as error:
        return keep(str(error))
    # Plain words joined by operators may describe qualitative work. Only explicit
    # expressions assert that unknown names are intended graph dependencies.
    if not predicate and any(ref not in ids and ref != nid and not is_builtin(ref)
                             for ref in E.refs(tree)):
        return keep('unknown or ambiguous reference; use rule={expr: "..."} for an intended calculation')
    candidate = dict(raw)
    candidate[nid] = dict(body, **{field: tree})
    candidate_ids = ids | {nid} | {key for key in E.refs(tree) if is_builtin(key)}
    result = E.compute(candidate, candidate_ids, tree if predicate else None)
    if result.get('error'):
        return keep(result['error'])
    if predicate:
        before = evaluate(source, raw, ids)
        after = result['predicate']['holds_on_current_values']
        # Arithmetic operands were never understood by the legacy single-value
        # comparison reader. A new, decidable formula uses the typed semantics.
        arithmetic = any('op' in node for node in tree['args'])
        if after is None or (before is not None and before is not after and not arithmetic):
            return keep('typed comparison is unavailable or changes the existing interpretation')
        left, right = tree['args']
        # Legacy comparisons may coerce two text readings to numbers later.
        if 'ref' in left and 'ref' in right and any(isinstance(value_of(raw, ids, node['ref']), str) for node in (left, right)):
            return keep('comparisons between text readings need an explicit choice of typed semantics')
    else:
        calculated = result['values'].get(nid, {})
        if calculated.get('value') is None:
            reason = calculated.get('reason', 'unavailable calculation')
            if reason in {'division_by_zero', 'cyclic_reference', 'missing_reference'} and not _blocked_text(body):
                raise Refused(f"refused - {nid}: rule cannot be computed: {reason}")
            return keep(reason)
    body[field] = {'expr': source}
    return action, [f"{nid}.{field}: stored as a readable expression"]


# Every refusal a write can meet, in one place. The fork on a contradiction is the last of
# them; the entries nearest a new one are said just before it.
VALIDATORS = [_known_key, _sound_dependencies, _sound_references, _sound_citation, _reopener_is_prose,
              _structured_is_sound,
              _arrangement_is_sound, _not_from_the_future, _trail_is_tool_written,
              _request_names_the_asking,
              _not_born_broken, _measure_is_a_name, _nearest_existing, _forks_on_contradiction,
              _drops_are_named]


def validate(action, doc, ids, jud, fields, raw):
    if getattr(raw, 'world', None) is not None:
        return raw.world.validate(action, doc, ids, jud, fields)
    out = []
    for check_ in VALIDATORS:
        out += check_(action, doc, ids, jud, fields, raw)
    return out


# ── the edits, on text ───────────────────────────────────────────────────────
def _citation_field_in(lines, key, field, value):
    """Replace a scalar citation token, preserving other fields, comments and quoting.

    Tokens distinguish a direct from/at field from those words inside a quoted value
    or a nested mapping. Scanning also works when a citation uses an external alias.
    """
    _, ind, s, e = _locate(lines, key)
    block = "\n".join(lines[s:e])
    tokens = list(yaml.scan(block))
    depth = 0
    for i, token in enumerate(tokens):
        if isinstance(token, (yaml.BlockMappingStartToken, yaml.FlowMappingStartToken,
                              yaml.BlockSequenceStartToken, yaml.FlowSequenceStartToken)):
            depth += 1
        elif isinstance(token, (yaml.BlockEndToken, yaml.FlowMappingEndToken, yaml.FlowSequenceEndToken)):
            depth -= 1
        elif isinstance(token, yaml.KeyToken) and depth == 2 \
                and isinstance(tokens[i + 1], yaml.ScalarToken) and tokens[i + 1].value == field:
            colon = tokens[i + 2]
            old = tokens[i + 3]
            if isinstance(old, (yaml.ScalarToken, yaml.AliasToken)):
                start, end = old.start_mark.index, old.end_mark.index
                replacement = scalar(value, _style(block[start:end]), fold=False)
            elif isinstance(old, (yaml.KeyToken, yaml.BlockEndToken, yaml.FlowEntryToken, yaml.FlowMappingEndToken)):
                start = end = colon.end_mark.index
                replacement = " " + scalar(value, fold=False)
            else:
                raise Refused(f"{key}: {field} is not a scalar citation")
            block = block[:start] + replacement + block[end:]
            lines[s:e] = block.split("\n")
            return
    if _inline(lines[s]).startswith("{"):
        opening = next(t for t in tokens if isinstance(t, yaml.FlowMappingStartToken))
        at = opening.end_mark.index
        block = block[:at] + f"{field}: {scalar(value, fold=False)}, " + block[at:]
        lines[s:e] = block.split("\n")
    else:
        _replace_field(lines, s, e, field,
                       [" " * (ind + 2) + f"{field}: {scalar(value, fold=False)}"], after="of")


def _set_in(lines, key, value, stamp, why, source=None, at=None):
    """-> (old value as written, field). The value field of `key` rewritten in place, in
    the style it already had; `of:` stamped; the reason, if any, as a comment beneath."""
    if source is not None:
        _citation_field_in(lines, key, "from", source)
        _citation_field_in(lines, key, "at", at)
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
    forget(path)


@contextlib.contextmanager
def _locked(path, *, project=None):
    """One writer at a time on a record: the whole write - load, validate, edit, replace -
    runs under an exclusive lock on the record's directory, so two sessions on one file
    take turns instead of the last one silently discarding the first. Where the platform
    offers no such lock the write refuses."""
    project = project or _peer('knowledge_views').project_for([path])
    # All direct writers share this boundary. Policy precedes the directory lock;
    # capture/configuration own their locks and are never called inside this scope.
    with project.lock():
        with _directory_locked(path):
            yield


@contextlib.contextmanager
def _directory_locked(path):
    """Directory-only lock for policy owners; never captures or configures."""
    token = _RAW_READS.set(True)
    try:
        with _peer('history_transaction').writer_guard(os.path.dirname(os.path.abspath(path))):
            yield
    finally:
        _RAW_READS.reset(token)


def _files_of(paths):
    """The files `load` reads, in order - every pointer followed the same way, as deep as
    it goes, each file once."""
    out, seen = [], set()

    def follow(f):
        if os.path.abspath(f) in seen or not os.path.exists(f):
            return
        seen.add(os.path.abspath(f))
        out.append(f)
        d = parse(f) or {}
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


def _file_for(files, nid, collection=None, texts=None):
    """Which file of the record a write lands in. A record is one file until it outgrows one,
    and then a pointer index over a file per domain - so the choice has to keep a subject
    together instead of piling every new entry into whichever file comes first:

      the file that already holds the entry - what a set, a review and a superseded judgment
        are looking for;
      else the file whose collection already holds the entry's neighbours: the ids sharing the
        head of its id, which is what a record is divided by when it is divided at all;
      else the file that is that subject's own, everything in it sharing the head - which opens
        the collection there, rather than filing a room's first source among the meetings;
      else the first file that holds the collection;
      else the first file that holds anything at all, a pointer index being a list of files and
        not a place to keep entries.

    A record in one file answers that file to all of it, as it always did. `texts` gives the
    files' lines where the caller is already holding them."""
    head, opened, own, held = nid.split(".")[0], None, None, None
    for f in files:
        if texts is not None and f in texts:
            lines = texts[f]
        else:
            with io.open(f, encoding="utf-8") as fh:
                lines = fh.read().split("\n")
        if _locate(lines, nid):
            return f
        if collection is None:
            continue
        cols, members = {}, {}
        for name, s, e in _collections_in(lines):
            cols[name] = (s, e)
            if name != "meta":          # the head of the file is not a collection of entries
                members[name] = [m for m, _ in _members_of(lines, s, e)[1] if "." in m]
        theirs = [m for ms in members.values() for m in ms]
        if held is None and theirs:
            held = f
        if collection in cols:
            if any(m.split(".")[0] == head for m in members.get(collection, [])):
                return f
            if opened is None:
                opened = f
        if own is None and theirs and all(m.split(".")[0] == head for m in theirs):
            own = f
    return own or opened or held or files[0]


def _collection_for(doc, ids, jud, fields, nid, body, explicit):
    """The collection a new entry goes into: the one named; else the one holding the
    entries it shares a prefix with; else, by shape - a judgment with the judgments, a
    source-shaped body with the sources, anything else with the values."""
    cols = collections_of(doc)
    if explicit:
        if explicit not in cols and explicit not in doc:
            raise Refused(f"no collection {explicit} in this record")
        return explicit
    # the head is never a home: meta reads as a collection when every value in it is a
    # plain scalar, and an entry filed there is one no reader can see
    cols = {c: m for c, m in cols.items() if c != "meta"}
    if not cols:
        # a newborn record: the three sections, named as the method recommends, by shape
        if isinstance(body, dict) and fields["deps"] in body:
            return "judgments"
        if not isinstance(body, dict):
            return OPEN[0]
        if not any(k in body for k in ("v", "rule", "quoted")) and \
                any(k in body for k in ("asked", "url", "file", "read")):
            return "sources"
        return "known"
    head = nid.split(".")[0]
    homes = [c for c, m in cols.items() if any(k.split(".")[0] == head for k in m)]
    if len(homes) == 1:
        return homes[0]
    if isinstance(body, dict) and fields["deps"] in body:
        if not jud:
            return "judgments"
        by_jud = sorted(cols, key=lambda c: -sum(1 for k in cols[c] if k in jud))
        return by_jud[0]
    if not isinstance(body, dict):
        opens = [c for c in cols if c in OPEN]
        return opens[0] if opens else OPEN[0]
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

    def counted(c):
        return sum(1 for k, b in cols[c].items() if isinstance(b, dict) and k not in jud
                   and ("v" in b or "rule" in b or "quoted" in b))
    if sourceish:
        # nothing cites a source yet: a section whose every member is source-shaped is the
        # sources', whatever it is called; a young record without one opens it under the
        # method's own name
        def source_shaped(b):
            return isinstance(b, dict) and not any(k in b for k in ("v", "rule", "quoted")) \
                and any(k in b for k in ("asked", "url", "file", "read"))
        homes = [c for c in cols if c not in OPEN and c != "judgments" and cols[c]
                 and all(source_shaped(b) for b in cols[c].values())]
        if "sources" in cols:
            return "sources"
        return sorted(homes)[0] if homes else "sources"
    valued = sorted(cols, key=lambda c: -counted(c))
    # a value lands where values are; a young record holding none yet opens the section
    return valued[0] if counted(valued[0]) else "known"


class _Reader:
    # File-path imports need the same writer callbacks and context variables as
    # package imports, without borrowing another module instance's read permission.
    def __getattr__(self, name):
        if name in globals():
            return globals()[name]
        raise AttributeError(name)

    load = staticmethod(load)
    Record = Record
    collections_of = staticmethod(collections_of)


def apply(paths, action, diagnostics=None):
    """The one write. `action` says what kind (set, add, review) and what that kind needs;
    it is validated whole before a byte is touched, applied to the file's text without
    reformatting anything else, read back, and answered with the reach. All of it under
    one lock, so a second writer waits rather than overwrites. Aimed at a hypothesis, the
    same write lands in the file beside the record and the base is not touched."""
    project = _peer('knowledge_views').project_for(paths)
    policy = project.config()
    original_paths = list(paths)
    paths = _peer('knowledge_views').write_paths(paths)
    try:
        receipt = _peer('recording').route(paths, action, sys.modules.get(__name__) or _Reader(),
                                          project=project, expected_policy=policy)
    except ValueError as error:
        raise Refused('refused - ' + str(error))
    if receipt is not None:
        print(json.dumps(receipt, ensure_ascii=False))
        return 0
    with _locked(paths[0], project=project):
        if project.config() != policy or list(_peer('knowledge_views').write_paths(original_paths)) != list(paths):
            raise Refused('refused - project mode or record destination changed; retry the write')
        return _apply_unlocked(paths, action, diagnostics, project=project)


def _apply_unlocked(paths, action, diagnostics=None, *, project=None):
    """The mutation body for a caller already holding the record-directory lock."""
    # Recheck privacy inside the write lock: a source can change while a writer waits.
    private = _peer('recording').private_route(paths, action, sys.modules.get(__name__) or _Reader(), project=project)
    if private is not None:
        print(json.dumps(private, ensure_ascii=False))
        return 0
    if action.get("hypothesis"):
        return _fork(paths, action, diagnostics)
    return _apply(paths, action, diagnostics)


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


def _fork(paths, action, diagnostics=None):
    """A write into a hypothesis beside the record: the same validation and the same edits,
    on `.kpopper/hypotheses/<name>.yaml` - the base is not touched. The record is read as it stands
    under the hypothesis, so a judgment written there rests on what it proposes and its
    snapshot says so. A hypothesis that does not exist yet is opened by its first write, with
    the day it was born in its head; an entry the base holds is carried over whole and set
    there."""
    doc, base_world = _peer('reasoning.authoring').prepare((sys.modules.get(__name__) or _Reader()), paths, action)
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
    fields = authored_fields(action, fields)
    world = _peer('reasoning.authoring').World((sys.modules.get(__name__) or _Reader()), under,
        original=base_world.snapshot) if base_world is not None else None
    if world is not None:
        fields = {**fields, **world.fields}
        raw = world.raw
    else:
        raw = with_builtins(under, ids, jud, fields)
        for k, v in counts(under, ids, jud, fields, bodies(under)).items():
            raw.setdefault(k, {"name": COMPUTED[k], "v": v})
    action, expression_notes = normalize_authored(action, ids, fields, raw)
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
    out, seen = list(expression_notes), {}
    if kind == "set":
        now = value_of(raw, ids, nid)
        if nid in hyp["ids"] and now is not None and _writer_same(raw, now, action["value"]) \
                and not action.get("as_of") and action.get("source") is None:
            print(f"{nid} is already {scalar(action['value'], fold=False)} in hypothesis {name}; "
                  f"nothing written")
            return 0
        if nid not in hyp["ids"]:
            got = _carry(_files_of(paths), nid)
            if not got:
                raise Refused(f"refused - no file of the record holds {nid}")
            collection, block = got
            out.append("carry " + _insert_block(lines, collection, nid, block) + f" of hypothesis {name}")
        old, field = _set_in(lines, nid, action["value"], stamp, action.get("why"),
                             action.get("source"), action.get("at"))
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
        changed = [d for d in seen if d not in was or not _writer_same(raw, was[d], seen[d])] + \
            [d for d in was if d not in seen]
        if changed:
            e = _seen_lines(lines, s, e, snapshot_field, seen)
        if _field_span(lines, s, e, "reviewed"):
            _stamp_field(lines, s, e, "reviewed", stamp, None)
        out.append(f"review {nid} in hypothesis {name}: "
                   + (f"seen rewritten from what the record holds under it ({stamp})" if changed
                      else f"what it saw is what the record holds under it ({stamp})"))
        for d in j["deps"]:
            if d in was and d in seen and not _writer_same(raw, was[d], seen[d]):
                out.append("  {}: {} -> {}".format(d, *apart(was[d], seen[d])))
            elif d not in was and d in seen:
                out.append(f"  {d}: {short(seen[d])} (never checked against it before)")
    if world is not None:
        _peer('reasoning.authoring').declare(lines, (sys.modules.get(__name__) or _Reader()))
        staged = parse(text="\n".join(lines)) or {}
        staged.pop('hypothesis', None)
        _peer('reasoning.contract').capabilities(staged)
        candidate = layered(doc, dict(hyp, doc=staged))
        _peer('reasoning.authoring').World((sys.modules.get(__name__) or _Reader()), candidate,
            original=world.snapshot).assessment()
    made_dir = False
    if fresh and not os.path.isdir(os.path.dirname(hyp["path"])):
        os.makedirs(os.path.dirname(hyp["path"]))
        made_dir = True
    _write_text(hyp["path"], "\n".join(lines))
    # read it back: the hypothesis must still load, and hold what was written
    try:
        doc2 = _peer('reasoning.authoring').load((sys.modules.get(__name__) or _Reader()), paths) if world is not None else load(paths)
        h2 = doc2.hypotheses.get(name)
        if h2 is None or h2["error"]:
            raise ValueError(h2["error"] if h2 else "the file is not found after the write")
        if nid not in h2["ids"]:
            raise ValueError(f"{nid} is not in the hypothesis after the write")
        under2 = layered(doc2, h2)
        ids2, jud2, fields2 = infer(under2)
        raw2 = _peer('reasoning.authoring').World((sys.modules.get(__name__) or _Reader()), under2,
            original=world.snapshot).raw if world is not None else with_builtins(under2, ids2, jud2, fields2)
        if world is not None:
            raw2.world.assessment()
        if kind == "set":
            now = value_of(raw2, ids2, nid)
            if (now is None and not (getattr(raw2, 'world', None) and raw2.world.result(nid)['status'] == 'ok')) or not _writer_same(raw2, now, action["value"]):
                raise ValueError(f"{nid} reads back as {now!r}")
            _check_citation_readback(action, raw2[nid])
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
    if diagnostics is not None:
        diagnostics.extend(expression_notes)
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


def _snapshot(deps, raw, ids, jud, paths, brief, first_born=False, value=None):
    """`seen` for these dependencies, from what each holds now. A page count needs the
    brief beside the record; a dependency declared missing has nothing to snapshot. A
    record's first arrangement may rest on `page.drift` before anything dates it: with
    `first_born` the share is taken as nothing-yet, and settled against its own `born`
    once it is written. `value` is the reading each is recorded by - what rests on a
    judgment keeps its verdict, what a text draws keeps the sentence it draws."""
    value = value or snapshot_value
    page = _page_side(paths)[0] if brief and any(d in PAGE for d in deps) else {}
    seen = {}
    for d in deps:
        if d not in ids and not is_builtin(d):
            continue
        v = value(d, raw, ids, jud, page)
        if v is None and d == "page.drift" and first_born and brief:
            v = 0.0
        if v is None:
            raise Refused(f"refused - {d} has no value to snapshot: "
                          + ("the page counts it, and no brief sits beside the record" if not brief
                             else "the page could not count it"
                             + (" - no arrangement carries born, so nothing dates what was added"
                                if d == "page.drift" else "")))
        seen[d] = v
    if getattr(raw, 'world', None) is not None:
        _peer('reasoning.contract').OutputBudget(raw.world.bounds['output_bytes']).add(seen)
    return seen


def _review_section(paths, brief, title, stamp, raw, ids, jud):
    """A section of the brief reviewed by its title: the references of its text
    snapshotted into `seen`, and `reviewed` moved. The record is not touched."""
    btext = io.open(brief, encoding="utf-8").read()
    b = parse(brief) or {}
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
    seen = _snapshot(refs, raw, ids, jud, paths, brief, value=shown_value)
    blines = btext.split("\n")
    _review_section_in(blines, title, seen, stamp)
    _write_text(brief, "\n".join(blines))
    print(f"review text '{title}': reviewed {stamp}")
    for k in refs:
        if k in was and k in seen and not same_seen(was[k], seen[k]):
            half, a, b = which_moved(was[k], seen[k])
            was, is_ = apart(a, b)
            print(f"  seen {k}{' - ' + half if half else ''}: {was} -> {is_}")
        elif k not in was and k in seen:
            print(f"  seen {k}: {short(seen[k])} (never read against it before)")
    return 0


def _check_citation_readback(action, body):
    if action.get("source") is not None and (body.get("from") != action["source"]
                                             or body.get("at") != action["at"]):
        raise ValueError(f"{action['id']} did not retain the requested from/at citation")


def _apply(paths, action, diagnostics=None):
    doc, world = _peer('reasoning.authoring').prepare((sys.modules.get(__name__) or _Reader()), paths, action)
    ids, jud, fields = infer(doc)
    fields = authored_fields(action, fields)
    if world is not None:
        fields = {**fields, **world.fields}
        raw = world.raw
    else:
        raw = with_builtins(doc, ids, jud, fields)
        # Legacy counts remain available only to legacy computations.
        for k, v in counts(doc, ids, jud, fields, bodies(doc)).items():
            raw.setdefault(k, {"name": COMPUTED[k], "v": v})
    files = _files_of(paths)
    stamp = action.get("as_of") or datetime.date.today().isoformat()
    kind, nid = action["kind"], action["id"]
    brief = _brief_beside(paths[0])
    side, side_before = replaced_path(paths), None     # the kept versions, restored with the record
    if kind == "review" and nid not in jud and brief:
        action["section"] = nid
    # an arrangement's write is held against the page - its counts, and what the brief
    # carries of every decision that stands - taken now, before anything is written, by the
    # same build a snapshot runs; so the birth check and the supersede door decide against
    # the numbers --verify decides. A brief that cannot be built leaves the page out of it.
    facts = {}
    if world is None and brief and kind == "add" and isinstance(action["body"], dict) and \
            (_arrangement_shaped(action["body"], fields, raw)
             or (nid in jud and is_arrangement(jud[nid], raw))):
        try:
            page0, _, facts = _page_side(paths)
        except (Exception, SystemExit):
            page0, facts = {}, {}
        for k, v in page0.items():
            raw.setdefault(k, {"name": COMPUTED[k]})["v"] = v
        doc.page = facts
    action, expression_notes = normalize_authored(action, ids, fields, raw)
    refusals = validate(action, doc, ids, jud, fields, raw)
    if refusals:
        raise Refused("refused - " + "\n          ".join(refusals))
    if kind == "review" and action.get("section"):
        return _review_section(paths, brief, nid, stamp, raw, ids, jud)

    snapshot_field = fields["snapshot"] or "seen"
    # which file of the record holds the write: the one that holds the entry, and for a new
    # one the file its neighbours are in (_file_for)
    target = files[0] if kind == "add" else _file_for(files, nid)
    out, seen, arrangement, supersede, arranged = list(expression_notes), {}, False, False, False
    if kind == "add":
        body = action["body"]
        # a judgment under a standing judgment's id got past validation only because it may
        # supersede it - it is broken by its own sign - so it replaces it in place
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
                _, ended = may_supersede(nid, old, body, raw, ids, jud, fields,
                                         action.get("as_of"), facts)
                extra = arrangement_renewal(old, ended, stamp, (facts.get(nid) or {}).get("stood"))
            body = {k: v for k, v in body.items() if k not in extra and k != snapshot_field}
            body.update(extra)
            body[snapshot_field] = seen
            action["body"] = body
        if supersede:
            target = _file_for(files, nid)
        else:
            collection = _collection_for(doc, ids, jud, fields, nid, body, action.get("into"))
            target = _file_for(files, nid, collection)
    with io.open(target, encoding="utf-8") as source:
        original = source.read()
    lines = original.split("\n")
    if kind == "set":
        now = value_of(raw, ids, nid)
        if now is not None and _writer_same(raw, now, action["value"]) and not action.get("as_of") \
                and action.get("source") is None and ('_record_scope' not in action or
                    _writer_same(raw, action['_record_scope'], raw[nid].get('scope') if isinstance(raw[nid], dict) else None)):
            print(f"{nid} is already {scalar(action['value'], fold=False)}; nothing written")
            return 0
        old, field = _set_in(lines, nid, action["value"], stamp, action.get("why"),
                             action.get("source"), action.get("at"))
        out.append(f"set {nid}: {old} -> {scalar(action['value'], fold=False)} (as of {stamp})")
        if action.get("source") is not None:
            out.append(f"source: {action['source']}, at {action['at']}")
    elif kind == "add" and supersede:
        old = jud[nid]["body"]
        _, why = may_supersede(nid, old, body, raw, ids, jud, fields, action.get("as_of"), facts)
        was = str(_verdict_of(old) or nid)
        if not arranged:
            # the trail an arrangement already carries, on every judgment: one line on the
            # judgment, the body it replaced kept whole beside the record
            extra = judgment_renewal(old, why, stamp)
            body = {k: v for k, v in body.items() if k not in extra and k != snapshot_field}
            body.update(extra)
            body[snapshot_field] = seen
            action["body"] = body
        back = returns_to(paths, nid, body)
        side_before = _text_of_or_none(side)
        index = keep_replaced(paths, nid, old, why, stamp, action.get("drops"))
        _replace_in(lines, nid, body)
        if _same(was, str(_verdict_of(body) or nid)):
            out.append(f"supersede {nid}: the same verdict on other grounds - {why}")
        else:
            out.append("supersede {}: {} -> {} - {}".format(
                nid, *apart(was, _verdict_of(body), 60), why))
        out += trail_lines(paths, nid, old, body, fields, action.get("drops"), index)
        if back:
            n, v, exact = back
            out.append(f"  returns to {'version' if exact else 'the verdict of version'} {n}"
                       + ("" if exact else ", on other grounds")
                       + f" - it stood until {v.get('day')} and fell because {v.get('ended')}")
    elif kind == "add":
        # a collection the file lacks - the first judgment, a newborn record's first
        # section - is opened at the end, and the entry is its first member
        _ensure_collection(lines, collection)
        out.append("add " + _add_in(lines, nid, body, collection))
    else:
        j = jud[nid]
        arrangement = is_arrangement(j, raw)
        was = dict(j["snap"])
        seen = _snapshot(j["deps"], raw, ids, jud, paths, brief)
        _, ind, s, e = _locate(lines, nid)
        changed = [d for d in seen if d not in was or not _writer_same(raw, was[d], seen[d])] + \
            [d for d in was if d not in seen]
        if changed:
            e = _seen_lines(lines, s, e, snapshot_field, seen)
        if _field_span(lines, s, e, "reviewed"):
            _stamp_field(lines, s, e, "reviewed", stamp, None)
        elif "replaced" in j["body"] and not arrangement:
            # a judgment that carries a trail keeps the day it was last read, so the
            # reversal the trail records stops asking once someone has reviewed it; an
            # arrangement's day is its born, renewed by the re-decision itself
            _stamp_field(lines, s, e, "reviewed", stamp, "replaced")
        out.append(f"review {nid}: " + (f"seen rewritten from what the record holds ({stamp})"
                                        if changed else f"what it saw is what the record holds ({stamp})"))
        for d in j["deps"]:
            if d in was and d in seen and not _writer_same(raw, was[d], seen[d]):
                out.append("  {}: {} -> {}".format(d, *apart(was[d], seen[d])))
            elif d not in was and d in seen:
                out.append(f"  {d}: {short(seen[d])} (never checked against it before)")
    if '_record_scope' in action and kind != 'add':
        entry = copy.deepcopy(raw[nid])
        if not isinstance(entry, dict):
            raise Refused('refused - scoped writes need a complete entry body')
        if kind == 'set':
            entry = _peer('recording').set_body(entry, action)
        elif kind == 'review':
            entry[snapshot_field] = seen
            if 'reviewed' in entry:
                entry['reviewed'] = stamp
        entry['scope'] = copy.deepcopy(action['_record_scope'])
        _replace_in(lines, nid, entry)
    if world is not None:
        _peer('reasoning.authoring').declare(lines, (sys.modules.get(__name__) or _Reader()))
    _bump_updated(lines, stamp)
    if world is not None:
        # Validate the exact candidate bytes before publication, including history.
        staged = parse(text="\n".join(lines)) or {}
        _peer('reasoning.contract').capabilities(staged)
        final_document = copy.deepcopy(doc)
        for collection, members in staged.items():
            if isinstance(members, dict) and isinstance(final_document.get(collection), dict):
                final_document[collection].update(members)
            else:
                final_document[collection] = members
        _peer('reasoning.authoring').World((sys.modules.get(__name__) or _Reader()), final_document,
            original=world.snapshot).assessment()
    _write_text(target, "\n".join(lines))

    def read_back():
        doc2, world2 = _peer('reasoning.authoring').prepare((sys.modules.get(__name__) or _Reader()), paths, action)
        ids2, jud2, fields2 = infer(doc2)
        raw2 = world2.raw if world2 is not None else with_builtins(doc2, ids2, jud2, fields2)
        if kind == "set":
            now = value_of(raw2, ids2, nid)
            if (now is None and not (getattr(raw2, 'world', None) and raw2.world.result(nid)['status'] == 'ok')) or not _writer_same(raw2, now, action["value"]):
                raise ValueError(f"{nid} reads back as {now!r}")
            _check_citation_readback(action, raw2[nid])
        elif nid not in ids2 and nid not in (doc2.get("meta") or {}):
            raise ValueError(f"{nid} is not in the record after the write")
        if world2 is not None:
            world2.assessment()
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
            drift = {d: page2[d] for d in seen if d in PAGE and d in page2 and not _writer_same(raw, seen[d], page2[d])}
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
                    raise ValueError(f"wrong_if already holds ({predicate_text(pred)}) once the page counts it - the "
                                     f"arrangement would be born broken")
        elif arrangement and brief:
            shape, facts2 = _page_side(paths)[1:]
    except (Exception, SystemExit) as e:
        _write_text(target, original)
        if side_before is not None or os.path.isfile(side):
            if side_before is None:
                os.remove(side)
                forget(side)
            else:
                _write_text(side, side_before)
        raise Refused(f"the write broke the record and was undone: {e}")
    # an arrangement's review also rewrites the shape it stood on - every tab it governs -
    # once the record is safely written, so a failed write leaves the brief as it was
    if kind == "review" and arrangement and shape is not None:
        b = parse(brief) or {}
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
    if diagnostics is not None:
        diagnostics.extend(expression_notes)
    return 0


def _writer_same(raw, a, b):
    if getattr(raw, 'world', None) is not None:
        return raw.world.same(a, b)
    return _same(a, b)


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
    if getattr(raw, 'world', None) is not None:
        report = raw.world.assessment()
        for name, node in report['nodes'].items():
            if fields['deps'] in (node['body'] if isinstance(node['body'], dict) else {}):
                tag, why = raw.world.state(name)
                print(f"  {name} {tag.lower()}: {why}")
        return
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
        for jid, day, _ in listened_until(read_replaced(paths), nid):
            print(f"  listened to by nothing standing - {jid} listened until {day}: pull {jid} --history")
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
# Existing judgments can become false when evidence changes. That is a finding to
# retain, not a structural error to erase before the session can finish.
def _gate_judgments(ids, jud, raw):
    """Mark both the judgment and the readings it was evaluated against, not its seen alone."""
    return {
        name: {
            "shape": hashlib.sha256(yaml.safe_dump({"body": j["body"], "deps": j["deps"],
                                                     "predicate": j["pred"]}, sort_keys=False).encode()).hexdigest(),
            "predicate": evaluate(j["pred"], raw, ids),
            "arrangement": is_arrangement(j, raw),
            "inputs": {d: yaml.safe_dump(value_of(raw, ids, d), sort_keys=False) for d in j["deps"]
                       if d in ids and d not in jud and not is_builtin(d)},
        } for name, j in jud.items()
    }


NUDGE_TURNS = 8      # prompts a session may run with the record untouched before it is asked once
TREE_FILES = 500     # working files the mark keeps, so a dirty tree cannot bloat it


def tree_state(paths, workspace=None):
    """What a session's end is held against beyond the record's own checks: a digest of the
    record with its hypotheses - any write moves it - and the git tree's state of the
    workspace the session works in (the current directory unless named): HEAD, each tracked
    file's changed line counts against it, and each untracked file with a digest of its
    content - so a file that was already dirty at the mark still counts when it changes
    again. None outside git."""
    h = hashlib.sha1()
    for f in list(_files_of(paths)) + sorted(glob.glob(os.path.join(hypothesis_dir(paths), "*.yaml"))):
        with io.open(f, "rb") as fh:
            h.update(fh.read())
    tree = None
    cwd = workspace or os.getcwd()

    def git(*args, timeout=5):
        return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, timeout=timeout)
    try:
        head = git("rev-parse", "HEAD")
        changed = git("diff", "HEAD", "--numstat")
        untracked = git("ls-files", "--others", "--exclude-standard")
        if head.returncode == 0 and changed.returncode == 0 and untracked.returncode == 0:
            files = sorted(l.strip() for l in changed.stdout.splitlines() if l.strip())
            for rel in sorted(l for l in untracked.stdout.splitlines() if l.strip())[:TREE_FILES]:
                full = os.path.join(cwd, rel)
                try:
                    size = os.path.getsize(full)
                    if size <= 1 << 20:
                        with io.open(full, "rb") as fh:
                            stamp = hashlib.sha1(fh.read()).hexdigest()[:12]
                    else:
                        stamp = f"size {size}"
                except OSError:
                    stamp = "unreadable"
                files.append(f"?? {rel} {stamp}")
            tree = {"head": head.stdout.strip(), "files": files[:TREE_FILES], "n": len(files)}
    except (OSError, subprocess.SubprocessError):
        pass
    return {"digest": h.hexdigest(), "tree": tree}


NUDGE_COOLDOWN = 10  # prompts between two askings, in either form


def untouched(base, paths, turns, host=None, nudged_at=None, workspace=None):
    """The one thing the gate asks of a session that never wrote: after real work - files
    of the tree changed since the mark, or enough prompts went by - was there nothing to
    keep? Asked once, and only while the record is exactly as the session found it. Not on
    the turn whose prompt already carried the softer form of the question: the turn after,
    when a line went unanswered, is when a stop is earned."""
    if not base.get("digest") or base.get("nudged"):
        return None
    if nudged_at is not None and turns <= nudged_at:
        return None
    now = tree_state(paths, workspace)
    if now["digest"] != base["digest"]:
        return None
    was, is_ = base.get("tree"), now["tree"]
    changed = 0
    if was and is_:
        changed = len(set(was.get("files") or []) ^ set(is_.get("files") or []))
        if was.get("head") != is_.get("head"):
            changed = max(changed, 1)
    if not changed and turns < NUDGE_TURNS:
        return None
    form = SKILL_FORMS.get(host or "")
    record = form.format("record") if form else "`kpopper add`"
    what = (f"{changed} file{'' if changed == 1 else 's'} of the tree changed" if changed
            else f"{turns} prompts in")
    return (f"kpopper: {what}, the record untouched. If a finding, decision or measurement came "
            f"out of this session, {record} keeps it now; if nothing will be revisited, finish.")


def mark(state_path, paths):
    """Written at session start: how many problems check finds, which intents no tab of the
    page serves, the ids the record holds, and the record and tree as the session found
    them."""
    doc = load(paths)
    ids, jud, fields = infer(doc)
    fail, _, _, _, _ = check_lines(paths)
    state = {"fails": len(fail), "failures": fail, "unserved": _unserved(paths),
             "ids": sorted(_every_id(doc, ids)),
             "judgments": _gate_judgments(ids, jud, with_builtins(doc, ids, jud, fields)),
             "nudged": False, **tree_state(paths)}
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
            "ids": state.get("ids"), "failures": state.get("failures"),
            "judgments": state.get("judgments") or {}, "digest": state.get("digest"),
            "tree": state.get("tree"), "nudged": bool(state.get("nudged"))}


def gate(state_path, paths, turns=0, host=None, nudged_at=None, *, _recording_context=None):
    """What a session hears before it can finish, against the mark its opener left: the
    record failing worse than it found it; an intent the session left unserved; entries it
    wrote with no intent recorded; and, once, real work that left the record untouched.
    Printed, and 2 when there is anything - the hook bounces once and yields."""
    base = _marked(state_path)
    fail, _, _, _, _ = check_lines(paths)
    doc = load(paths)
    ids, jud, fields = infer(doc)
    raw = with_builtins(doc, ids, jud, fields)
    now = _gate_judgments(ids, jud, raw)
    allowed = {}
    for name, old in base["judgments"].items():
        current = now.get(name)
        if not current or old.get("shape") != current["shape"] \
                or old.get("arrangement") or current["arrangement"] \
                or not isinstance(old.get("inputs"), dict) \
                or old.get("predicate") is True or current["predicate"] is not True:
            continue
        if any(d not in old.get("inputs", {}) or v != old["inputs"][d]
               for d, v in current["inputs"].items()):
            allowed[_fired_failure(name, jud[name])] = name
    if base["failures"] is not None:
        previous = set(base["failures"])
        added = [f for f in fail if f not in previous and f not in allowed]
    else:
        # An old mark has no evidence about why a judgment changed. Keep its conservative
        # count-based behavior until the session is opened with a new mark.
        added = fail if len(fail) > base["fails"] else []
    out = []
    if added:
        out.append(f"{paths[0]} fails check with {len(fail)} problems ({base['fails']} at "
                   f"session start).")
        out.append("Fix the record - or declare the hole with blocked_on - before finishing:")
        out += ["FAIL " + f for f in added[:12]]
    for s in _unserved(paths):
        if s not in base["unserved"]:
            out.append(f"{s} is served by no tab of the page - serve it in a tab whose sections "
                       f"pick what it wrote, or leave it outside and say why")
    if base["ids"] is not None:
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
        # A captured external report records why its evidence entered the graph;
        # it is not a new user request that a page must serve as a reading occasion.
        try:
            from .ingestion import recording_source
        except ImportError:
            from ingestion import recording_source
        # Match bodies(doc)'s collection order, including overrides reached through
        # pointers. Another checked file cannot lend ownership to an effective copy.
        origins = {nid: doc.origins.get(section, {}).get(nid)
                   for section, members in doc.items() if isinstance(members, dict) for nid in members}
        recordings = {k for k in every if k not in jud and isinstance(raw.get(k), dict)
                      and isinstance(raw[k].get('recorded_for'), str) and raw[k]['recorded_for'].strip()
                      and not any(field in raw[k] for field in ('v', 'quoted', 'rule', 'verdict'))
                      and recording_source(k, raw[k], origins.get(k), _preparing=_recording_context)}
        recording_sources = intents | recordings

        def attributed(k):
            b = raw.get(k)
            if isinstance(b, dict) and str(b.get("from") or "") in recording_sources:
                return True
            deps = jud[k]["deps"] if k in jud else rests.get(k, [])
            return bool(set(deps) & recording_sources)
        if new and not (set(new) & recording_sources) and not any(attributed(k) for k in new):
            named_ = ", ".join(new[:4]) + (f" and {len(new) - 4} more" if len(new) > 4 else "")
            out.append(f"this session wrote {len(new)} {'entry' if len(new) == 1 else 'entries'} "
                       f"({named_}) and recorded no intent: add s.<date>_<slug> asked=\"...\" "
                       f"name=\"...\", and from: it on what it wrote")
    if not out:
        nudge = untouched(base, paths, turns, host, nudged_at)
        if nudge:
            out.append(nudge)
            # once: the mark remembers that the question was asked, and at which prompt
            try:
                raw_state = json.loads(io.open(state_path, encoding="utf-8").read())
                if isinstance(raw_state, dict):
                    raw_state["nudged"], raw_state["nudged_turn"] = True, turns
                    with io.open(state_path, "w", encoding="utf-8") as f:
                        json.dump(raw_state, f)
            except (OSError, ValueError):
                pass
    for l in out:
        print(l)
    if allowed:
        print("Updated readings falsified unchanged judgments: " + ", ".join(sorted(allowed.values()))
              + ". They remain flagged for review; check still reports their failed conditions.")
    return 2 if out else 0


HELP = {
    "set": """  set <key> <value> [--source <id> --at "..."] [--why "..."] [--as-of YYYY-MM-DD] [--hypothesis NAME] [--profile core/v1] [file]

Change one value. The entry's `v:` (or `quoted:`) is rewritten where it stands, `of:` is
stamped with the date, and the reason - if given - is kept as a comment beneath. A number
is written as a number, `true`/`false` as booleans, anything else as text. A worked-out
entry is refused: change its rule, not its result. A judgment is refused: review it. The
reply is the reach: what is worked out from it, every judgment resting on it and its state
now, and the texts of the brief that saw the old value.

For a reading from a different source, supply --source and --at together. They replace
the entry's from/at citation in the same write as its value and date; --why remains a
comment, not a citation. The source must already be recorded (add it first): a mapping
with asked/file/url/of/read and no v/quoted/rule, not a judgment or computed value. This
checks the recorded source identity, not the contents or availability of an external source. Omitting
these options retains the existing citation. A citation-only change is written even
when the value is unchanged. This option uses from/at; entries with src/source fields
must reconcile those fields first. Judgment snapshots are never refreshed by set. A reading dated
after today is refused: a day is the record's clock, and a day ahead would outrank every reading
of today.

A reading newer than the one the base holds - its `of:`, else its source's read date -
updates it. One of the same day or earlier that differs is a contradiction: refused into
the base, and the refusal names the command that writes it into a hypothesis instead.
`--hypothesis NAME` writes into `.kpopper/hypotheses/NAME.yaml` beside the record, opened by its
first write, and the base is not touched: the entry is carried over whole and set there.""",
    "add": """  add <id> field=value ... [--in COLLECTION] [--as-of YYYY-MM-DD] [--hypothesis NAME] [--profile core/v1] [file]
  add <id> '{field: value, ...}'
  add <id> "an open question"

A new entry or judgment, inserted in id order beside the entries it shares a prefix with -
never at the tail - in the style of the entry it lands beside; meta.updated moves. A list
is written as `rests_on='[a, b]'`; a judgment's `seen` is filled by this tool from what its
dependencies hold now and must not be given. Refused: an id already in the record, a
dependency that is not an entry (add it first, or declare it with blocked_on), a reference
to nothing, a judgment whose wrong_if already holds. The reply is the reach. Where no record
resolves for the workspace, the first add creates GROUNDING.yaml at its root - a checkout's
root, or the working directory outside git - with that entry; a registered record that is
unavailable is a location problem, refused rather than replaced.

A contradiction is refused into the base and named a hypothesis: the same id with a
different value or verdict, or a judgment resting on what only a hypothesis holds - the
refusal names the `--hypothesis NAME` command that writes it into `.kpopper/hypotheses/NAME.yaml`
beside the record instead, where its `seen` is taken from the record as it stands under
that hypothesis, and the base is not touched.

An arrangement - a judgment resting on a session source, with a sign over a count the build
takes - is born when it is written: `born` is stamped like `seen`, and its sign is one
comparison that can hold. Written again under its own id it is re-decided: admitted when its
sign holds with its tabs intact; refused twice in a day, once the brief no longer carries
what it decided, or while the sign has not fired - and a refusal names the hypothesis a
person folds. `request: s.<date>_<slug>` names whose asking any judgment was taken from,
and admits nothing.

A judgment written under a standing judgment's id, once the standing one is broken by its
own condition, replaces it in place - and the replacement leaves a trail: one line appended
to `replaced:` on the judgment, saying what ended the decision it replaced and the day, and
the replaced body kept whole in `.kpopper/replaced.yaml` beside the record (its verdict,
because, request, rests_on, wrong_if, seen). The reply says what was kept and what the new
judgment no longer rests on. A replacement that drops a dependency is refused until each
dropped id is named with its reason: `--drop "<id>: <why>"`, kept with the version. One
whose verdict a kept version already held is told that it returns to it. `replaced:` is
written by this tool on every judgment; a write that carries one is refused.""",
    "review": """  review <id> [--as-of YYYY-MM-DD] [--hypothesis NAME] [--profile core/v1] [file]
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
    opts, args, drops = {}, [], {}
    i = 0
    while i < len(rest):
        a = rest[i]
        if a in ("--why", "--as-of", "--in", "--hypothesis", "--source", "--at", "--profile", "--shareability", "--scope", "--environment", "--commit", "--event-id", "--contribution-id", "--evidence-root"):
            if i + 1 >= len(rest):
                raise Refused(f"{a} needs a value")
            opts[a[2:].replace("-", "_")] = rest[i + 1]
            i += 2
            continue
        if a == "--drop":
            if i + 1 >= len(rest) or ":" not in rest[i + 1]:
                raise Refused('--drop takes "<id>: <why>" - the dependency the new judgment no '
                              'longer rests on, and the reason')
            d, why = rest[i + 1].split(":", 1)
            if not d.strip() or not why.strip():
                raise Refused('--drop takes "<id>: <why>" - both halves')
            drops[d.strip()] = why.strip()
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
    if as_of and _as_day(as_of) and _as_day(as_of) > latest_today():
        raise Refused(f"--as-of {as_of} is after today ({latest_today().isoformat()}) - a day is "
                      f"the record's clock, and a reading dated ahead would outrank every reading of "
                      f"today; date it the day it was read")
    if opts.get("why") and "\n" in opts["why"]:
        raise Refused("--why is one line: a second line would be a line of the record")
    if opts.get("hypothesis") and not HYPOTHESIS_NAME.match(opts["hypothesis"]):
        raise Refused("--hypothesis takes a name - letters, digits, underscores, dashes - that "
                      "becomes <name>.yaml in the hypotheses directory beside the record")
    action = {"kind": cmd, "id": nid, "as_of": as_of, "why": opts.get("why"), "into": opts.get("in"),
              "hypothesis": opts.get("hypothesis"), "source": opts.get("source"), "at": opts.get("at")}
    if drops:
        if cmd != "add":
            raise Refused("--drop goes with add, on a judgment that replaces a standing one")
        action["drops"] = drops
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
    if 'profile' in opts:
        action['profile'] = opts['profile']
    action.update({key: opts[key] for key in _peer('recording').ROUTING if key in opts})
    if cmd == "add" and not files:
        code, born = _apply_first_add(action)
    else:
        code, born = apply(files or default_paths(), action), []
    for f in born:
        print(f"created {f} - this workspace's record, born with its first entry")
    return code


HEAD_LINE = "# Kept with kpopper: read it with `kpopper open`, write it with `kpopper add`.\n"


def _newborn_only(path):
    """A record that holds nothing but what its birth wrote - the head line and the day."""
    try:
        with io.open(path, encoding="utf-8") as f:
            text = f.read()
    except OSError:
        return False
    return re.fullmatch(re.escape(HEAD_LINE) + r"meta:\n  updated: \d{4}-\d{2}-\d{2}\n", text) is not None


def _workspace_location():
    """The workspace locator, loaded in both package and file-path use."""
    try:
        from .workspace import locate
    except ImportError:
        import importlib.util
        spec = importlib.util.spec_from_file_location(
            "_kpopper_workspace", os.path.join(os.path.dirname(__file__), "workspace.py"))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        locate = module.locate
    return locate()


def _newborn(action, path):
    """Create the empty head for a first add at its resolved path. The caller holds the
    record-directory lock through this write, the entry mutation, and any cleanup."""
    stamp = action.get("as_of") or datetime.date.today().isoformat()
    try:
        _write_text(path, HEAD_LINE + f"meta:\n  updated: {stamp}\n")
    except OSError as e:
        raise Refused(f"refused - no record here, and none could be created at {path}: {e}")
    return path


def _apply_first_add(action):
    """Apply an add that names no record file, creating the workspace record if needed.

    Birth is part of the same critical section as the ordinary load/edit/replace mutation.
    A refused birth is removed before the lock is released, so a waiting writer either sees
    the completed record or creates a fresh one after cleanup. -> (exit code, born paths)."""
    location = _workspace_location()
    if location["status"] == "unavailable":
        raise SystemExit(location["reason"] + " " + location["record"])
    path = location["record"]
    project = _peer('project_modes').Project(location.get('workspace', os.path.dirname(path)))
    policy = project.config()
    try:
        receipt = _peer('recording').route([path], action, sys.modules.get(__name__) or _Reader(),
                                          project=project, expected_policy=policy)
    except ValueError as error:
        raise Refused('refused - ' + str(error))
    if receipt is not None:
        print(json.dumps(receipt, ensure_ascii=False))
        return 0, []
    while True:
        with _locked(path, project=project):
            if project.config() != policy:
                raise Refused('refused - project mode or record destination changed; retry the write')
            # Resolve again only after owning the directory. Another first writer may have
            # completed, or may have removed its refused newborn, while this writer waited.
            location = _workspace_location()
            if location["status"] == "unavailable":
                raise SystemExit(location["reason"] + " " + location["record"])
            current = location["record"]
            if os.path.dirname(os.path.abspath(current)) != os.path.dirname(os.path.abspath(path)):
                # Registration moved concurrently, or a registered file disappeared and the
                # workspace fell back to its own root. Release this lock and retry under the
                # directory that actually owns the current record.
                path = current
                continue
            if location["status"] == "found":
                return _apply_unlocked([current], action, project=project), []
            _newborn(action, current)
            try:
                return _apply_unlocked([current], action, project=project), [current]
            except BaseException:
                # Still inside the birth lock: no successful waiter can be removed between
                # this exact-content check and unlink.
                if _newborn_only(current):
                    os.remove(current)
                raise


if __name__ == "__main__":
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        if hasattr(stream, 'reconfigure'):
            stream.reconfigure(encoding='utf-8', newline='\n')
    a = [x for x in sys.argv[1:] if x != "--no-cache"]
    if len(a) != len(sys.argv[1:]):
        # the same switch as KPOPPER_NO_CACHE=1, set here so anything this run starts
        # parses the record too, and nothing reads a kept parse
        os.environ[NO_CACHE] = "1"
    if '--frozen' in a:
        a.remove('--frozen')
        os.environ['KPOPPER_READ_MODE'] = 'frozen'
    a = a or ["check"]
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
        # the hooks' own: the state file the opener writes, then the record; the gate also
        # hears how many prompts the session ran and which host asks, for its one question
        if not rest:
            sys.exit(f"{cmd} needs the state file the opener writes")
        files = [x for x in rest[1:] if x.endswith((".yaml", ".yml"))] or default_paths()
        if cmd == "mark":
            sys.exit(mark(rest[0], files))
        turns = int(rest[rest.index("--turns") + 1]) if "--turns" in rest else 0
        host = rest[rest.index("--host") + 1] if "--host" in rest else None
        at = int(rest[rest.index("--nudged-at") + 1]) if "--nudged-at" in rest else None
        sys.exit(gate(rest[0], files, turns, host, at))
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
        b, seeds, files, history = 40, [], [], False
        i = 0
        while i < len(rest):
            if rest[i] == "--budget":
                b = int(rest[i + 1]); i += 2; continue
            if rest[i] == "--history":
                history = True; i += 1; continue
            (files if rest[i].endswith((".yaml", ".yml")) else seeds).append(rest[i])
            i += 1
        code = pull(files or default_paths(), seeds, b)
        if history:
            lines = history_lines(files or default_paths(), seeds)
            print()
            for l in lines or ["no replaced version is kept for " + ", ".join(seeds)]:
                print(l)
        sys.exit(code)
    files = [x for x in rest if x.lower().endswith((".yaml", ".yml"))] or default_paths()
    if cmd == "open":
        b = int(rest[rest.index("--budget") + 1]) if "--budget" in rest else 25
        c = int(rest[rest.index("--chars") + 1]) if "--chars" in rest else None
        h = rest[rest.index("--host") + 1] if "--host" in rest else None
        sys.exit(opening(files, b, c, h))
    sys.exit(check(files))
