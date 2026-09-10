"""Before a file is edited: the entries of the record that cite it, said once per file.

A source recorded with `file:` and an entry measured by a recipe that reads a file are the
record's ties to the tree. Editing the file does not change the record, and nothing that reads
the record notices - `check` compares the record with itself. So the moment the edit is about
to happen is the moment to say what rests on that file, and which command re-reads it.
"""
import json
import os
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import ground_hook as G  # noqa: E402
import workspace as W  # noqa: E402

TOOLS = {"Edit", "Write", "MultiEdit", "NotebookEdit"}
SKILL_FORMS = {"claude": "/kpopper:{}", "codex": "${}"}


def cited(record, rel):
    """-> (sources whose file: is this path, [(entry, recipe)] measured from it)"""
    import provenance as P
    doc = P.load([record])
    ids, jud, fields = P.infer(doc)
    raw = P.bodies(doc)
    base = os.path.dirname(os.path.abspath(record))
    target = os.path.normpath(os.path.join(base, rel))
    sources = sorted(k for k in P._every_id(doc, ids)
                     if isinstance(raw.get(k), dict) and isinstance(raw[k].get("file"), str)
                     and os.path.normpath(os.path.join(base, raw[k]["file"])) == target)
    # the recipes are the remeasure module's to read: it owns the allowlist, its name and its
    # refusals, and this hook only asks which recipes read the file
    import remeasure as R
    measured = []
    allowlist = R.allowlist_path([record])
    if os.path.isfile(allowlist):
        try:
            table = R.read_allowlist(allowlist)
        except (OSError, SystemExit):
            table = {}
        reading = {name for name, args in table.items() if any(rel in a for a in args)}
        measured = sorted((k, raw[k][P.MEASURE]) for k in P._every_id(doc, ids)
                          if isinstance(raw.get(k), dict) and raw[k].get(P.MEASURE) in reading)
    return sources, measured


def context(payload, host):
    if not isinstance(payload, dict) or payload.get("agent_id") or payload.get("agent_type"):
        return ""
    if payload.get("tool_name") not in TOOLS:
        return ""
    given = payload.get("tool_input") or {}
    file = given.get("file_path") or given.get("notebook_path") if isinstance(given, dict) else None
    if not isinstance(file, str) or not file:
        return ""
    location = W.locate(payload.get("cwd"))
    if location["status"] != "found":
        return ""
    record = location["record"]
    base = os.path.dirname(os.path.abspath(record))
    path = os.path.abspath(os.path.join(payload.get("cwd") or os.getcwd(), file))
    if os.path.basename(path).startswith("PROVENANCE."):
        return ""              # the record and the files beside it are the writer's business
    rel = os.path.relpath(path, base)
    if rel.startswith(".."):
        return ""              # outside the project the record answers for
    state_file = G.state_path(payload.get("session_id"))
    state = G.load_state(state_file) if state_file else {}
    if rel in state.get("cited", {}):
        return ""
    sources, measured = cited(record, rel)
    if not sources and not measured:
        return ""
    form = SKILL_FORMS.get(host or "")
    ground = form.format("ground") if form else "kpopper affects"
    parts = []
    if sources:
        n = len(sources)
        parts.append("%d %s cite %s (%s) - after the edit, %s %s says what the change reaches"
                     % (n, "entry" if n == 1 else "entries", rel, ", ".join(sources), ground, sources[0]))
    if measured:
        parts.append("%s measured from it (recipe %s) - `kpopper remeasure --run` takes the reading again"
                     % (", ".join(k for k, _ in measured), ", ".join(sorted({r for _, r in measured}))))
    if state_file:
        state.setdefault("cited", {})[rel] = True
        G.save_state(state_file, state)
    return "kpopper: " + "; ".join(parts) + "."


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else None
    try:
        text = context(json.load(sys.stdin), host)
    except (OSError, ValueError, ImportError, SystemExit) as error:
        print("kpopper citation check unavailable: " + str(error), file=sys.stderr)
        return 0
    if text:
        print(json.dumps({"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": text}},
                         ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
