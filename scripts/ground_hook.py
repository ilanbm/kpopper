"""The grounding line: at every prompt, the entries of the record that the user's words touch,
named once so the session reads them before answering from memory.

The record reaches a session once, at the opener; after that nothing connects a question to
what the record holds unless the session thinks to look. This hook looks: it matches the
prompt's words against ids, names and verdicts, weighted by how rare each word is across the
record, and prints at most three entries with the skill that reads them. It carries no
value - values are pulled by subject when the work asks - and it is silent on a greeting, on
a prompt that touches nothing, and where no record exists.

An entry is named until it is read. A read leg on shell output marks the ids that passed
through `pull`, `affects` or `open`; a read entry is not named again until its recorded body
changes or the session compacts, and an unread one is repeated only after a cooldown.
"""
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import workspace as W  # noqa: E402

COOLDOWN = 10        # turns before an unread entry is named again
NAMED_AT_MOST = 3    # entries per line
SHARED_WORDS = 2     # rare words a prompt and an entry must share
RARE_FLOOR = 6       # a word in at most this many entries, or a fifth of them, is rare
SKILL_FORMS = {"claude": "/kpopper:{}", "codex": "${}"}
STOP = set("""the and that this with from what when does only before after into over same more than
then where which while will would could should have has had been being about also each every some
such their there these those they them its are was were not but for you your our one two how why who
any all can may get got use used using make made take said say says just like still very much many
does did done being here now new old out off per via""".split())
LATIN = re.compile(r"[a-z][a-z0-9']{3,}")
HEBREW = re.compile(r"[א-ת]{3,}")
HEBREW_PREFIXES = "הובלמשכ"
ID = re.compile(r"\b[a-z][a-z0-9]*(?:\.[a-z0-9_]+)+\b")


def state_path(sid):
    if not isinstance(sid, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,200}", sid):
        return None
    return Path(tempfile.gettempdir()) / ("kpopper-ground-" + sid + ".json")


def load_state(path):
    try:
        state = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(state, dict):
            return state
    except (OSError, ValueError):
        pass
    return {}


def save_state(path, state):
    # a private temporary name: two hooks of one session can write at once
    fd, tmp = tempfile.mkstemp(prefix=path.name + ".", dir=str(path.parent))
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write(json.dumps(state, ensure_ascii=False))
    os.replace(tmp, path)


def words(text):
    """The words of a text worth matching: Latin words of four letters or more that are not
    function words, and Hebrew words of three letters or more. A Hebrew word also yields the
    form without one leading prefix letter (ה ו ב ל מ ש כ), so the definite and the
    conjunctive forms meet the record's bare name."""
    text = text.lower()
    out = {w for w in LATIN.findall(text) if w not in STOP}
    for w in HEBREW.findall(text):
        out.add(w)
        if len(w) >= 4 and w[0] in HEBREW_PREFIXES:
            out.add(w[1:])
    return out


def kind_and_value(x):
    """A value the record holds that JSON has no form for - a date, most often - written with
    its kind beside it, so a date and the same day written as a string are not one thing."""
    return [type(x).__name__, str(x)]


def digest(body):
    """A short fingerprint of an entry as the record holds it: it changes when the record
    changes the entry, and not when the same fields are written in another order. Taken for
    every entry at every prompt, so it is taken the cheapest way that says that."""
    try:
        text = json.dumps(body, sort_keys=True, default=kind_and_value, ensure_ascii=False)
    except (TypeError, ValueError):
        import yaml                      # a body with keys of mixed kinds: the parser's own way
        text = yaml.safe_dump(body, sort_keys=True, allow_unicode=True)
    return hashlib.sha1(text.encode("utf-8")).hexdigest()[:12]


def entries(record):
    """-> {id: (words, digest)} over sources, known, judgments and open questions: the id's own
    segments, its name and its verdict - never its value."""
    import provenance as P
    doc = P.load([record])
    ids, _, _ = P.infer(doc)
    raw = P.bodies(doc)
    head = set((doc.get("meta") or {}) if isinstance(doc.get("meta"), dict) else {})
    out = {}
    for k in P._every_id(doc, ids):
        body = raw.get(k)
        if not isinstance(k, str) or k in head or body is None:
            continue
        if isinstance(body, dict):
            text = " ".join(str(body.get(f) or "") for f in ("name", "verdict", "asked", "title", "label", "what"))
        else:
            text = str(body)
        key = " ".join(re.split(r"[._]", k))
        out[k] = (words(key + " " + text), digest(body))
    return out


def hits(prompt, index):
    """The entries the prompt touches, best first: an id written in the prompt outright, then
    entries sharing enough rare words. -> [(score, id, shared)]"""
    pw = words(prompt)
    mentioned = {m for m in ID.findall(prompt.lower()) if m in index}
    n = max(1, len(index))
    df = {}
    for ws, _ in index.values():
        for w in ws:
            df[w] = df.get(w, 0) + 1
    # a word most entries carry says nothing about which one is meant; rarity is relative to
    # the record, so a small record's every word counts and a large one's common words do not
    rare = max(RARE_FLOOR, n / 5)
    out = []
    for k, (ws, _) in index.items():
        if k in mentioned:
            out.append((float("inf"), k, [k]))
            continue
        shared = sorted(w for w in pw & ws if df[w] <= rare)
        if len(shared) < SHARED_WORDS:
            continue
        out.append((sum(math.log(n / df[w]) for w in shared), k, shared))
    out.sort(key=lambda t: (-t[0], t[1]))
    return out


def moves(host):
    form = SKILL_FORMS.get(host or "")
    return form.format("ground") if form else "kpop pull"


def line(named, host):
    ids = ", ".join(named)
    seeds = " ".join(named)
    return ("kpopper: the record holds %s on this - %s %s before answering from memory."
            % (ids, moves(host), seeds))


def choose(found, state, turn):
    """Which of the hits are named this turn: an id read since its body last changed is not;
    one named within the cooldown and still unread is not; the rest, best first, up to the cap."""
    named = state.setdefault("named", {})
    read = state.setdefault("read", {})
    digests = state.get("digests", {})
    out = []
    for _, k, _ in found:
        if read.get(k) and read[k] == digests.get(k):
            continue
        last = named.get(k)
        if last and turn - int(last.get("turn", 0)) < COOLDOWN and not read.get(k):
            continue
        out.append(k)
        if len(out) == NAMED_AT_MOST:
            break
    for k in out:
        named[k] = {"turn": turn}
        read.pop(k, None)
    return out


def prompt_context(payload, host, path):
    location = W.locate(payload.get("cwd"))
    if location["status"] != "found":
        return ""
    prompt = payload.get("prompt")
    if not isinstance(prompt, str) or not prompt.strip():
        return ""
    state = load_state(path)
    turn = int(state.get("turns", 0)) + 1
    state["turns"] = turn
    index = entries(location["record"])
    state["digests"] = {k: d for k, (_, d) in index.items()}
    # a read entry whose body changed since is unread again: the session saw an older reading
    for k, d in list(state.get("read", {}).items()):
        if state["digests"].get(k) != d:
            state["read"].pop(k)
    named = choose(hits(prompt[:4000], index), state, turn)
    soft = reminder(location["record"], payload.get("session_id"), state, turn, host, location["workspace"])
    save_state(path, state)
    parts = [line(named, host)] if named else []
    if soft:
        parts.append(soft)
    return "\n".join(parts)


def reminder(record, sid, state, turn, host, workspace=None):
    """Advisory recording context after work, repeated only after a prompt cooldown."""
    import provenance as P
    mark = Path(tempfile.gettempdir()) / ("kpopper-base-" + sid)
    try:
        base = json.loads(mark.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return ""
    if not isinstance(base, dict) or not base.get("digest"):
        return ""
    last = max(int(state.get("nudged_turn", -P.NUDGE_COOLDOWN)), int(base.get("nudged_turn", -P.NUDGE_COOLDOWN)))
    if turn - last < P.NUDGE_COOLDOWN:
        return ""
    asked = P.untouched(dict(base, nudged=False), [record], turn, host, workspace=workspace)
    if not asked:
        return ""
    state["nudged_turn"] = turn
    return asked


def read_context(payload, path):
    """The read leg: ids that passed through a tool's output are read, at the body the
    record held when they were last named or seen."""
    state = load_state(path)
    digests = state.get("digests")
    if not digests:
        return
    response = payload.get("tool_response")
    if response is None:
        response = payload.get("tool_output")
    # whatever shape the host gives a tool's result, the ids in it are what count
    text = response if isinstance(response, str) else json.dumps(response, ensure_ascii=False, default=str)
    seen = {m for m in ID.findall(text.lower()) if m in digests}
    if not seen:
        return
    read = state.setdefault("read", {})
    for k in seen:
        read[k] = digests[k]
    save_state(path, state)


def start(payload, path):
    """A fresh session starts with nothing named; a compaction forgets what was named and read,
    since the model did; a resume keeps its state."""
    source = payload.get("source")
    if source == "resume" and path.exists():
        return
    state = load_state(path) if source == "compact" else {}
    state.pop("named", None)
    state.pop("read", None)
    save_state(path, state)


def handle(payload, host, mode):
    # agent_type also names a main session started with --agent.
    if not isinstance(payload, dict) or payload.get("agent_id"):
        return ""
    path = state_path(payload.get("session_id"))
    if path is None:
        return ""
    if mode == "start":
        start(payload, path)
        return ""
    if mode == "read":
        read_context(payload, path)
        return ""
    if mode == "prompt":
        return prompt_context(payload, host, path)
    return ""


def main():
    host, mode = (sys.argv[1:3] + [None, None])[:2]
    try:
        text = handle(json.load(sys.stdin), host, mode)
    except (Exception, SystemExit) as error:
        # a line that cannot be produced must never hold the prompt - a record that does not
        # parse, a state file that does not read, a host field of the wrong shape
        print("kpopper grounding unavailable: " + " ".join(str(error).split())[:200], file=sys.stderr)
        return 0
    if text:
        event = "UserPromptSubmit"
        print(json.dumps({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}},
                         ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
