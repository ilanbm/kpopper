#!/bin/sh
# The stop gate, run by the plugin's Stop hook: a session that leaves the record
# with new failures, leaves an intent no tab of the page serves, or wrote
# entries and recorded no intent, is bounced once - with the reasons - before it can
# finish. Everything is compared against the mark taken at session start, so a record
# that was already red or already unserved never blocks a session that did not touch
# it. An unchanged existing judgment falsified by updated readings remains flagged but
# does not block recording. The gate yields after one bounce, so it reminds rather than imprisons.
IN=$(cat)
# Resolve the same project as the opener, even when the host runs a hook elsewhere.
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REC=$(printf '%s' "$IN" | python3 "$HERE/workspace.py" --hook --path 2>/dev/null) || exit 0
echo "$IN" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("stop_hook_active") else 1)' 2>/dev/null && exit 0
SID=$(printf '%s' "$IN" | python3 -c 'import json,re,sys; s=json.load(sys.stdin).get("session_id",""); print(s if isinstance(s,str) and re.fullmatch(r"[A-Za-z0-9_-]{1,200}",s) else "")' 2>/dev/null)
BASE_FILE="${TMPDIR:-/tmp}/kpopper-base-$SID"
[ -n "$SID" ] && [ -f "$BASE_FILE" ] || exit 0
# 2 is the gate's own answer: something to say. Any other failure is the reader's, and a
# reader that cannot run must not hold a session at its end.
OUT=$(python3 "$HERE/provenance.py" gate "$BASE_FILE" "$REC" 2>/dev/null)
RC=$?
if [ "$RC" -eq 2 ]; then
  printf '%s\n' "$OUT" >&2
  exit 2
fi
exit 0
