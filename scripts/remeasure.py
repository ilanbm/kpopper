#!/usr/bin/env python3
"""The tree measured against the record. Every entry that names a recipe - `measure: <name>` -
is taken again from the tree by the argument list the allowlist beside the record holds for
that name, and what differs is laid over the record as one more hypothesis, tested by the
same dry run that tests any other.

  python3 remeasure.py [--run] [file]

Without --run it prints the plan - which entry names which recipe, the executable it resolves
to, and where it would run - and runs nothing. With --run it runs each cited recipe once,
without a shell, its streams capped and its time bounded, and compares the one line it prints
with what the record holds. Recipes run from the root of the checkout the record sits in - and,
for a record the tree cannot hold, from the record's own directory, beside the allowlist that
names them; the plan says which, every time. The readings that differ make the
hypothesis `tree/<commit>`, evaluated and never written; the exit code is the dry run's - 1 on a
falsifier that holds, a hole, or a contested reading; 0 when the tree agrees, or when a reading
older than the tree's moved without crossing a line - said, with the command that refreshes it.

The record never carries a command: `measure:` is a bare name, and the allowlist is the one
file whose content runs, read here and nowhere else. The pull request runs this with --run.
"""
import datetime, glob, io, os, re, shlex, shutil, signal, subprocess, sys, threading
import yaml

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import provenance as P    # noqa: E402
import consolidate as C   # noqa: E402

ALLOWLIST = "measure.yaml"              # under .kpopper beside the first file the reader opens, and
                                        # only there; PROVENANCE.measure.yaml beside a record under the old name
TIMEOUT = 60                            # seconds a recipe may take
CAP = 64 * 1024                         # bytes either stream may print before the recipe is killed
TREE = "tree"                           # the hypothesis the tree's readings make: tree/<commit> -
                                        # a slash, so no file beside the record can share the name


# ── the allowlist ────────────────────────────────────────────────────────────
def allowlist_path(paths):
    """Where the recipes live: beside the first record file, and nowhere else - no pointer,
    shard or hypothesis is followed for it."""
    return P.layout_of(paths)["measure"]


def allowlist_name(paths):
    """The allowlist as the record's directory names it: `.kpopper/measure.yaml`, or
    `PROVENANCE.measure.yaml` beside a record under the old name."""
    return P.layout_of(paths)["measure_name"]


def _name_of(path):
    base = os.path.basename(path)
    return P.HOME + "/" + base if os.path.basename(os.path.dirname(path)) == P.HOME else base


class _Strict(yaml.SafeLoader):
    """The safe loader, refusing a key that appears twice - after merge keys are expanded, so a
    recipe merged in and written again is caught too; the default keeps the last in silence."""
    def construct_mapping(self, node, deep=False):
        if isinstance(node, yaml.MappingNode):
            self.flatten_mapping(node)
        seen = set()
        for kn, _ in node.value:
            key = self.construct_object(kn, deep=deep)
            if isinstance(key, (str, int, float, bool)) or key is None:
                if key in seen:
                    raise yaml.constructor.ConstructorError(
                        None, None, f"the key {key!r} appears twice", kn.start_mark)
                seen.add(key)
        return super().construct_mapping(node, deep)


def read_allowlist(path):
    """-> {name: argv}: the whole file read and refused whole before anything runs. A mapping
    of names to argument lists: every key a bare name (a loader that read `on` as a boolean, or
    a merge key, fails that), every value a non-empty list of non-empty strings without NUL -
    the executable and its arguments, never a line for a shell."""
    with io.open(path, encoding="utf-8") as f:
        text = f.read()
    try:
        data = yaml.load(text, Loader=_Strict)
    except yaml.YAMLError as e:
        raise P.Refused(f"refused - {_name_of(path)} does not read: " + " ".join(str(e).split())[:200])
    if data is None:
        return {}
    if not isinstance(data, dict):
        raise P.Refused(f"refused - {_name_of(path)} is not a mapping of recipe names to argument lists")
    out, problems = {}, []
    for k, v in data.items():
        if not isinstance(k, str) or not P.MEASURE_NAME.match(k):
            problems.append(f"{k!r} is not a recipe name - letters, digits, underscores and dashes, "
                            f"opening with a letter; quote it if the loader read it as something else")
            continue
        if not isinstance(v, list) or not v or \
                not all(isinstance(x, str) and x and "\0" not in x for x in v):
            problems.append(f"{k}: a recipe is a non-empty list of non-empty strings - the executable "
                            f"and its arguments - never a line for a shell")
            continue
        out[k] = list(v)
    if problems:
        raise P.Refused(f"refused - {_name_of(path)}:\n  " + "\n  ".join(problems))
    return out


# ── what the record declares ─────────────────────────────────────────────────
def declared(doc, hyps):
    """Every entry that names a recipe -> ({id: {name: [holder, ...]}}, problems), read from the
    base's own bodies and from each hypothesis's own - before any layering, so a hypothesis that
    repeats an entry without the field cannot cancel what the base declares. A holder is the
    hypothesis's name, or None for the base."""
    names, problems = {}, []
    worlds = [(None, doc, P.bodies(doc))]
    for h in hyps:
        worlds.append((h["name"], P.layered(doc, h), h["raw"]))
    for holder, world, own in worlds:
        ids, jud, fields = P.infer(world)
        raw = P.with_builtins(world, ids, jud, fields)
        for nid, body in sorted(own.items()):
            if not isinstance(body, dict) or P.MEASURE not in body:
                continue
            where = f" (in hypothesis {holder})" if holder else ""
            bad = P.measure_problem(nid, body, ids, jud, fields, raw)
            if bad:
                problems.append(f"{nid}{where}: {bad}")
                continue
            names.setdefault(nid, {}).setdefault(str(body[P.MEASURE]), []).append(holder)
    for nid, by in sorted(names.items()):
        if len(by) > 1:
            said = "; ".join(f"{n} by {', '.join(h or 'the base' for h in hs)}" for n, hs in sorted(by.items()))
            problems.append(f"{nid}: two recipes named for one entry - {said} - a contested recipe; "
                            f"one of them, or neither")
    # a hypothesis that replaces a measured entry and says nothing about the recipe would drop
    # it at the fold - the block is carried over whole - and nothing would take that reading
    # again. The base's recipe is not inherited in silence: the hypothesis carries it, or says
    # it is gone by the base dropping it first.
    own = P.bodies(doc)
    for nid, by in sorted(names.items()):
        mine = next((n for n, hs in by.items() if None in hs), None)
        if not mine:
            continue
        for h in hyps:
            body = h["raw"].get(nid)
            if nid in h["ids"] and isinstance(body, dict) and P.MEASURE not in body:
                problems.append(f"{nid}: {h['name']} replaces it without measure: {mine} - the fold "
                                f"carries the block over whole, so the recipe would be dropped and "
                                f"nothing would take this reading again; carry measure: {mine} into "
                                f"{h['name']}, or drop it from the base first")
    return names, problems


# ── running a recipe ─────────────────────────────────────────────────────────
def resolve(argv, root):
    """The executable a recipe names, as a path - on the PATH, or relative to the root."""
    exe = argv[0]
    if os.path.dirname(exe):
        cand = exe if os.path.isabs(exe) else os.path.join(root, exe)
        return cand if os.path.isfile(cand) and os.access(cand, os.X_OK) else None
    return shutil.which(exe)


def _kill(proc):
    try:
        if os.name == "nt":
            proc.kill()
        else:
            os.killpg(proc.pid, signal.SIGKILL)
    except (OSError, ProcessLookupError):
        pass


def run_recipe(argv, root, timeout=None, cap=None):
    """One recipe, run once -> (stdout text, problem or None). No shell; the checkout's root as
    the working directory; nothing on stdin; both streams read as they come and capped, the
    process group killed on a cap crossed or the time spent. The problem, when there is one, is
    said with the tail of what the recipe printed on stderr."""
    timeout = TIMEOUT if timeout is None else timeout
    cap = CAP if cap is None else cap
    exe = resolve(argv, root)
    if not exe:
        return "", f"no executable {argv[0]!r} on the path" + (f" or under {root}" if os.path.dirname(argv[0]) else "")
    try:
        proc = subprocess.Popen([exe] + list(argv[1:]), cwd=root, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                start_new_session=(os.name != "nt"),
                                creationflags=(getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
                                               if os.name == "nt" else 0))
    except OSError as e:
        return "", f"could not start {exe}: {e}"
    got, size, over = {"out": [], "err": []}, {"out": 0, "err": 0}, []

    def drain(name, pipe):
        try:
            while True:
                b = pipe.read(4096)
                if not b:
                    break
                size[name] += len(b)
                if size[name] <= cap:
                    got[name].append(b)
                elif not over:
                    over.append(name)
                    _kill(proc)
        finally:
            pipe.close()
    threads = [threading.Thread(target=drain, args=("out", proc.stdout), daemon=True),
               threading.Thread(target=drain, args=("err", proc.stderr), daemon=True)]
    for t in threads:
        t.start()
    timed_out = False
    try:
        proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        _kill(proc)
        proc.wait()
    # whatever the recipe started and left running holds the pipes open and outlives the
    # bound this promises, so the group goes either way - a recipe prints its line and ends
    _kill(proc)
    for t in threads:
        t.join(timeout=5)
    out = b"".join(got["out"]).decode("utf-8", "replace")
    err = b"".join(got["err"]).decode("utf-8", "replace")
    tail = " ".join(err.split())[-200:]
    if over:
        return out, f"printed more than {cap // 1024} KiB on std{over[0]} and was stopped"
    if timed_out:
        return out, f"did not finish in {timeout} s and was stopped" + (f" - stderr: {tail}" if tail else "")
    if proc.returncode:
        return out, f"exited {proc.returncode}" + (f" - stderr: {tail}" if tail else "")
    return out, None


def reading(out, recorded):
    """The value a recipe printed, as the record holds its kind -> (value, problem or None): one
    non-empty line, a finite plain number where the record holds a number, true or false where
    it holds a boolean, the text itself otherwise - never a text compared as a number, never a
    number that is not one."""
    lines = [l.strip() for l in out.splitlines() if l.strip()]
    if not lines:
        return None, "printed nothing - the value is the one line a recipe prints"
    if len(lines) > 1:
        return None, (f"printed {len(lines)} lines - the value is the one line a recipe prints; "
                      f"diagnostics go to stderr")
    text = lines[0]
    if isinstance(recorded, bool):
        if text in ("true", "false"):
            return text == "true", None
        return None, f"printed {text!r} where the record holds true or false"
    if isinstance(recorded, (int, float)):
        if P.NUMBER.match(text):
            return (float(text) if "." in text else int(text)), None
        return None, f"printed {text!r} where the record holds a number"
    return text, None


def agrees(recorded, value):
    """Whether the recipe's line is the reading the record holds. A number is compared as a
    number, so 28 and 28.0 are one reading; text is compared exactly, since a text reading is
    text - "001" is not 1, and two spaces are not one."""
    if isinstance(recorded, bool) or isinstance(value, bool):
        return recorded is value
    if isinstance(recorded, (int, float)) and isinstance(value, (int, float)):
        return P._same(recorded, value)
    return str(recorded) == str(value)


# ── the tree as a hypothesis ─────────────────────────────────────────────────
def _root_of(paths):
    first = (sorted(glob.glob(paths[0])) or [paths[0]])[0]
    here = os.path.dirname(os.path.realpath(first))
    code, top, _ = C._git(here, "rev-parse", "--show-toplevel")
    return os.path.realpath(top.strip()) if not code and top.strip() else here


def _commit_of(root):
    """What the tree is -> (commit, said): the short commit, and how a durable reason should
    name it. A working tree with changes of its own is not that commit, and a reason that said
    it was would outlive the run and be wrong."""
    code, out, _ = C._git(root, "rev-parse", "--short=7", "HEAD")
    commit = out.strip() if not code and out.strip() else None
    if not commit:
        return None, None
    code, dirty, _ = C._git(root, "status", "--porcelain")
    return commit, (f"{commit} with the working tree changed" if not code and dirty.strip()
                    else commit)


def _by(name, said):
    return f"measured by {name}" + (f" at {said}" if said else "")


def _value_field(body):
    return "quoted" if body.get("v") is None and body.get("quoted") is not None else "v"


def _dated_by(body, raw):
    """How the base dates a reading -> text: its own of: or read:, else its source's."""
    for f in ("of", "read"):
        if body.get(f) is not None and P._as_day(body.get(f)):
            return f"its own {f}:"
    src = body.get("from")
    if isinstance(src, str) and isinstance(raw.get(src), dict):
        for f in ("read", "of"):
            if raw[src].get(f) is not None and P._as_day(raw[src].get(f)):
                return f"the {f}: of {src}"
    return "nothing"


def tree_hypothesis(udoc, differing, commit, said, today):
    """The tree's readings that differ from the record, as one hypothesis held in memory -
    each entry carried over whole with the measured value, dated the day it was taken and
    saying which recipe took it - never folded, never written."""
    doc = {}
    cols = P.collections_of(udoc)
    for nid, (name, body, value) in sorted(differing.items()):
        col = next(c for c, m in cols.items() if nid in m)
        new = dict(body)
        new[_value_field(body)] = value
        new["of"] = today.isoformat()
        new["at"] = _by(name, said)
        doc.setdefault(col, {})[nid] = new
    head = {"claim": f"what the tree{' at ' + commit if commit else ''} measures",
            "born": today.isoformat(), "folds": C.NEVER}
    return C.hypothesis(f"{TREE}/{commit or 'here'}", doc, head)


def refresh_command(nid, value, name, said, holder, paths, today):
    """The line that says how the tree's reading gets where the entry stands: the write, quoted
    for a shell and dated with the day the measurement was taken - or the advice, when no
    command carries the value as it was measured."""
    text = str(value).lower() if isinstance(value, bool) else str(value)
    why = None
    if text in ("-h", "--help") or text.startswith("-") or "\n" in text or "\r" in text:
        why = "a value shaped like an option or spanning lines is not carried by set"
    elif P.typed(text) != value:
        # the write path reads its argument the way a command line does, so text that looks
        # like a number or a boolean would be written as one - a different reading
        why = (f"set would read {text!r} as {P.scalar(P.typed(text), fold=False)}, which is not "
               f"what was measured")
    if why:
        return f"edit it by hand in this pull request - {nid}: v: {text!r} - {why}"
    parts = ["kpop", "set", nid, text, "--why", _by(name, said),
             "--as-of", today.isoformat()]
    if holder:
        parts += ["--hypothesis", holder]
    if list(paths) != P.DEFAULT:
        parts.append(paths[0])
    return "refresh: " + " ".join(shlex.quote(p) for p in parts)


# ── the command ──────────────────────────────────────────────────────────────
GENERIC = ("re-read against the merged tree: pull each id to see every reading beside the base's, "
           "then set what holds today - in the base with a later day, or in the hypothesis that read "
           "it - or refute one, and consolidate again",
           "  read again on a later day - set it in the base or in the hypothesis with --as-of - or "
           "refute the hypothesis")


def measure(paths, run=False, timeout=None, cap=None, today=None):
    """What `remeasure` prints -> (lines, exit code). The plan, always; the measurements and the
    union's report only with `run`."""
    today = today or datetime.datetime.now(datetime.timezone.utc).date()
    doc, hyps = C.read(paths)
    names, problems = declared(doc, hyps)
    apath = allowlist_path(paths)
    allow = read_allowlist(apath) if os.path.isfile(apath) else None
    out = []
    if problems:
        return ["refused - the record names recipes it cannot: "] + ["  " + p for p in problems], 1
    if not names:
        if allow:
            n = len(allow)
            out.append(f"{allowlist_name(paths)} holds {n} recipe{'s' if n != 1 else ''}, and no entry names one - "
                       f"nothing to re-measure")
        else:
            out.append("no measures beside the record - nothing to re-measure")
        return out, 0
    cited = sorted({n for by in names.values() for n in by})
    if allow is None:
        out.append(f"refused - {len(cited)} recipe{'s' if len(cited) != 1 else ''} named and no "
                   f"{allowlist_name(paths)} beside the record to hold {'them' if len(cited) != 1 else 'it'}: "
                   + ", ".join(cited))
        return out, 1
    missing = [n for n in cited if n not in allow]
    if missing:
        out.append(f"refused - the record names recipe{'s' if len(missing) != 1 else ''} "
                   f"{allowlist_name(paths)} does not hold: " + ", ".join(missing)
                   + " - a measurement nothing takes is a hole, and a falsifier reading it tests nothing")
        return out, 1
    unused = [n for n in sorted(allow) if n not in cited]
    root = _root_of(paths)
    commit, said = _commit_of(root)
    # the union's own bodies: what each measured entry holds with the hypotheses laid over
    udoc = doc
    for h in hyps:
        udoc = P.layered(udoc, h)
    uraw = P.bodies(udoc)
    serves = {}
    for nid, by in names.items():
        for n in by:
            serves.setdefault(n, []).append(nid)
    out.append(f"{len(cited)} recipe{'s' if len(cited) != 1 else ''} named by {len(names)} "
               f"entr{'ies' if len(names) != 1 else 'y'}, from {allowlist_name(paths)}, run from {root}:")
    for n in cited:
        exe = resolve(allow[n], root)
        shown = " ".join(shlex.quote(" ".join(a.split())) for a in allow[n][1:])
        out.append(f"  {', '.join(sorted(serves[n]))} <- {n}: {exe or allow[n][0] + ' (not found)'} "
                   + (shown if len(shown) < 90 else shown[:90] + " ..."))
    if unused:
        out.append(f"  named by no entry, never run: {', '.join(unused)}")
    if not run:
        out.append("")
        out.append("nothing ran - add --run to measure this tree")
        return out, 0
    out.append("")
    results, failed = {}, []
    for n in cited:
        text, problem = run_recipe(allow[n], root, timeout, cap)
        if problem:
            failed.append(f"  FAIL {n} ({', '.join(sorted(serves[n]))}): {problem}")
            continue
        results[n] = text
    differing, agreed = {}, []
    for nid, by in sorted(names.items()):
        n = next(iter(by))
        if n not in results:
            continue
        body = uraw.get(nid)
        if not isinstance(body, dict):
            failed.append(f"  FAIL {n} ({nid}): what holds this id now is not an entry with a "
                          f"reading of its own, so there is nothing to measure against")
            continue
        recorded = body.get(_value_field(body))
        value, problem = reading(results[n], recorded)
        if problem:
            failed.append(f"  FAIL {n} ({nid}): {problem}")
            continue
        if agrees(recorded, value):
            agreed.append(f"  {nid}: {P.scalar(value, fold=False)} - as recorded ({n})")
        else:
            differing[nid] = (n, body, value)
    out.append(f"measured on {today.isoformat()} (UTC){', at ' + said if said else ''}: {len(names)} "
               f"entr{'ies' if len(names) != 1 else 'y'} by {len(cited)} recipe{'s' if len(cited) != 1 else ''}")
    out += agreed
    if failed:
        out += failed
    if not differing:
        out.append("")
        if failed:
            out.append(f"not clean: a hole - {len(failed)} recipe{'s' if len(failed) != 1 else ''} failed, "
                       f"and a measurement nothing takes is a hole")
            return out, 1
        out.append(f"the record holds what this tree measures")
        return out, 0
    tree = tree_hypothesis(udoc, differing, commit, said, today)
    if tree["name"] in doc.hypotheses:
        raise P.Refused(f"refused - a hypothesis beside the record is named {tree['name']}, which is "
                        f"the tree's own; rename it")
    fail_b, _, moved_b, _, _ = P.check_lines(paths)
    laid = sorted(hyps + [tree], key=lambda h: h["name"])
    # the same day and the same page facts the fold would ask the door with, so this run and
    # the consolidation walk say the same thing about every hypothesis laid over the record
    c = C.union_of(doc, laid, (fail_b, moved_b), today.isoformat(), C.page_of(paths, laid))
    report = [l for l in C.report(c, today) if l not in GENERIC]
    if report and report[-1] and not c.contested:
        report = report[:-1]           # the fold's verdict: the tree never folds, and the last word is below
    while report and not report[-1]:
        report.pop()
    out.append("")
    out += report
    # a judgment whose sign the page decides cannot be decided here against what the tree
    # measured: the page does not see the tree, so the case is refused rather than left green
    page_bound = []
    if c.jud:
        for name_, j in sorted(c.jud.items()):
            toks = set(P.predicate_refs(j["pred"]))
            pages = sorted(t for t in toks if t in P.PAGE)
            mine = sorted(t for t in toks if t in differing)
            if pages and mine:
                page_bound.append(f"  FAIL {name_}: wrong_if reads {', '.join(pages)} beside "
                                  f"{', '.join(mine)}, which the tree measured differently - the page "
                                  f"decides it, and the page does not see the tree")
    if page_bound:
        out.append("")
        out += page_bound
    out.append("")
    refused = {k: why for k, _, why in c.refused}
    base_ids = c.base[1]
    for nid, (n, body, value) in sorted(differing.items()):
        was = P.scalar(body.get(_value_field(body)), fold=False)
        now = P.scalar(value, fold=False)
        day = P._read_on(body, uraw)
        dated = _dated_by(body, uraw)
        line = f"  {nid}: {was} recorded" + (f" ({day}, by {dated})" if day else " (undated)") \
            + f" -> {now} {_by(n, said)}"
        out.append(line)
        holder = None if nid in base_ids else next(
            (h["name"] for h in hyps if nid in h["ids"]), None)
        if nid in refused or nid in c.contested:
            out.append(f"    correct the recorded claim in this pull request, or run the measurement "
                       f"again on a later day; do not future-date this result")
        else:
            out.append("    " + refresh_command(nid, value, n, said, holder, paths, today))
    out.append("")
    if c.red or failed or page_bound:
        what = []
        if c.contested or c.refused:
            what.append("a reading the tree contests")
        if c.falsified or c.head_falsified:
            what.append("a falsifier that holds on what the tree measures")
        if c.holes or failed:
            what.append("a hole")
        if page_bound:
            what.append("a sign the page decides")
        if getattr(c, "untaken", None):
            what.append("a reversal a person has not taken by name")
        if getattr(c, "drops_needed", None):
            what.append("a dropped dependency to name at the fold")
        out.append("not clean: " + ", ".join(what) + " - red until the record and the tree agree")
        return out, 1
    n_moved = len(c.moved)
    if n_moved:
        out.append(f"the tree moved {len(differing)} reading{'s' if len(differing) != 1 else ''} under "
                   f"{n_moved} judgment{'s' if n_moved != 1 else ''} - refresh, then re-review; "
                   f"a move flags and fails nothing")
    else:
        out.append(f"the tree reads {len(differing)} entr{'ies' if len(differing) != 1 else 'y'} "
                   f"differently, none across a line - refresh them")
    return out, 0


HELP = """  remeasure [--run] [file]

Every entry that names a recipe - `measure: <name>` - taken again from the tree, by the argument
list .kpopper/measure.yaml beside the record holds for that name (PROVENANCE.measure.yaml beside
a record under the old name). Without --run: the plan,
and nothing runs. With --run: each cited recipe once, from the checkout's root, without a shell,
its streams capped and its time bounded; the one line it prints is the value. What differs from
the record is laid over it as the hypothesis tree/<commit> - with the hypotheses beside the
record, through the same dry run - and reported the same way. The exit code is the dry run's:
1 on a falsifier that holds, a hole, or a reading the tree contests; 0 when the tree agrees, or
when an older reading moved without crossing a line - said, with the command that refreshes it."""


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if "--help" in argv or "-h" in argv:
        print(HELP.strip("\n"))
        return 0
    run, files = False, []
    for a in argv:
        if a == "--run":
            run = True
        elif a.startswith("-"):
            raise P.Refused(f"{a} is not an option of remeasure\n\n" + HELP.strip("\n"))
        elif a.endswith((".yaml", ".yml")):
            files.append(a)
        else:
            raise P.Refused(f"{a}: remeasure takes --run and a record file, nothing else")
    lines, code = measure(files or P.default_paths(), run)
    for l in lines:
        print(l)
    return code


if __name__ == "__main__":
    sys.exit(main())
