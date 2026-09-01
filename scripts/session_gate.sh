#!/bin/sh
# The stop gate, run by the plugin's Stop hook: a session that leaves the record
# failing worse than it found it is bounced once, with the failures, before it
# can finish. The comparison is against the count taken at session start, so a
# record that was already red never blocks a session that did not touch it - and
# the gate yields after one bounce, so it reminds rather than imprisons.
IN=$(cat)
[ -f PROVENANCE.yaml ] || exit 0
echo "$IN" | python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("stop_hook_active") else 1)' 2>/dev/null && exit 0
SID=$(echo "$IN" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("session_id",""))' 2>/dev/null)
BASE_FILE="${TMPDIR:-/tmp}/kpopper-base-$SID"
[ -n "$SID" ] && [ -f "$BASE_FILE" ] || exit 0
OUT=$(python3 "$(dirname "$0")/provenance.py" check 2>/dev/null)
NOW=$(printf '%s\n' "$OUT" | grep -c '^FAIL')
BASE=$(cat "$BASE_FILE" 2>/dev/null || echo 0)
[ "$NOW" -gt "$BASE" ] 2>/dev/null || exit 0
{
  echo "PROVENANCE.yaml fails check with $NOW problems ($BASE at session start)."
  echo "Fix the record - or declare the hole with blocked_on - before finishing:"
  printf '%s\n' "$OUT" | grep '^FAIL' | head -12
} >&2
exit 2
