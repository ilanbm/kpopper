#!/bin/sh
# The stop gate, run by the plugin's Stop hook: a session that leaves the record
# with new failures, leaves an intent no tab of the page serves, or wrote
# entries and recorded no intent, is bounced once - with the reasons - before it can
# finish. Everything is compared against the mark taken at session start, so a record
# that was already red or already unserved never blocks a session that did not touch
# it. An unchanged existing judgment falsified by updated readings remains flagged but
# does not block recording. The gate yields after one bounce, so it reminds rather than imprisons.
IN=$(cat)
# the record is at the root, or where the checkout registered it - same as the opener
REC=PROVENANCE.yaml
if [ ! -f "$REC" ]; then
  G=$(git rev-parse --git-common-dir 2>/dev/null) || exit 0
  [ -f "$G/kpopper-record" ] || exit 0
  REC=$(head -n 1 "$G/kpopper-record")
  case "$REC" in "~/"*) REC="$HOME/${REC#"~/"}" ;; esac
  [ -n "$REC" ] && [ -f "$REC" ] || exit 0
fi
echo "$IN" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("stop_hook_active") else 1)' 2>/dev/null && exit 0
SID=$(echo "$IN" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("session_id",""))' 2>/dev/null)
BASE_FILE="${TMPDIR:-/tmp}/kpopper-base-$SID"
[ -n "$SID" ] && [ -f "$BASE_FILE" ] || exit 0
# 2 is the gate's own answer: something to say. Any other failure is the reader's, and a
# reader that cannot run must not hold a session at its end.
OUT=$(python3 "$(dirname "$0")/provenance.py" gate "$BASE_FILE" "$REC" 2>/dev/null)
RC=$?
if [ "$RC" -eq 2 ]; then
  printf '%s\n' "$OUT" >&2
  exit 2
fi
exit 0
