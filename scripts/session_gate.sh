#!/bin/sh
# The stop gate, run by the plugin's Stop hook: a session that leaves the record
# with new failures, leaves an intent no tab of the page serves, or wrote
# entries and recorded no intent, is bounced once - with the reasons - before it can
# finish. Everything is compared against the mark taken at session start, so a record
# that was already red or already unserved never blocks a session that did not touch
# it. An unchanged existing judgment falsified by updated readings remains flagged but
# does not block recording. A session that did real work - files of the tree changed, or
# enough prompts went by - and never touched the record is asked once whether there was
# nothing to keep. The gate yields after one bounce, so it reminds rather than imprisons.
# The host's hook may name itself (--host claude|codex) so the question names the host's
# record skill.
export PYTHONIOENCODING=utf-8
HOST=
while [ $# -gt 0 ]; do
  case "$1" in
    --host) HOST=$2; shift 2 ;;
    *) shift ;;
  esac
done
IN=$(cat)
# Resolve the same project as the opener, even when the host runs a hook elsewhere.
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REC=$(printf '%s' "$IN" | python3 "$HERE/workspace.py" --hook --path 2>/dev/null) || exit 0
echo "$IN" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("stop_hook_active") else 1)' 2>/dev/null && exit 0
SID=$(printf '%s' "$IN" | python3 -c 'import json,re,sys; s=json.load(sys.stdin).get("session_id",""); print(s if isinstance(s,str) and re.fullmatch(r"[A-Za-z0-9_-]{1,200}",s) else "")' 2>/dev/null)
BASE_FILE="${TMPDIR:-/tmp}/kpopper-base-$SID"
[ -n "$SID" ] && [ -f "$BASE_FILE" ] || exit 0
# how many prompts the session ran, and at which the grounding hook last asked its soft
# question: counted by that hook, 0 and none where it did not run
GROUND="${TMPDIR:-/tmp}/kpopper-ground-$SID.json"
TURNS=$(python3 -c 'import json,sys; print(int(json.load(open(sys.argv[1])).get("turns", 0)))' "$GROUND" 2>/dev/null || echo 0)
AT=$(python3 -c 'import json,sys; v=json.load(open(sys.argv[1])).get("nudged_turn"); print("" if v is None else int(v))' "$GROUND" 2>/dev/null || echo "")
set -- gate "$BASE_FILE" "$REC" --turns "$TURNS"
[ -n "$HOST" ] && set -- "$@" --host "$HOST"
[ -n "$AT" ] && set -- "$@" --nudged-at "$AT"
# 2 is the gate's own answer: something to say. Any other failure is the reader's, and a
# reader that cannot run must not hold a session at its end.
OUT=$(python3 "$HERE/provenance.py" "$@" 2>/dev/null)
RC=$?
if [ "$RC" -eq 2 ]; then
  printf '%s\n' "$OUT" >&2
  exit 2
fi
exit 0
