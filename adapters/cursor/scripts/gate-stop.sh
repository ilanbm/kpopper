#!/bin/sh
# cursor's stop translated to the plugin's own protocol. mirrors scripts/session_gate.sh
# exactly, except: keyed by conversation_id (cursor sends no session_id), and cursor's
# stop hook has no block/deny field to return - the only lever is "followup_message",
# which cursor resubmits as the next turn. loop_count is cursor's analogue of
# stop_hook_active: 0 on the first stop, incremented each time a hook's followup_message
# triggers another turn - so a bounce is only offered once per conversation, same as the
# plugin's own gate yields after one bounce rather than imprisoning the session.
# no `set -e`, to match session_open.sh/session_gate.sh: `check` returning 1 because it
# found problems is the normal case this script exists to handle, not a crash to abort on.

IN=$(cat)
# the record is at the root, or where the checkout registered it - same as session_gate.sh
REC=PROVENANCE.yaml
if [ ! -f "$REC" ]; then
  G=$(git rev-parse --git-common-dir 2>/dev/null) || exit 0
  [ -f "$G/kpopper-record" ] || exit 0
  REC=$(head -n 1 "$G/kpopper-record")
  case "$REC" in "~/"*) REC="$HOME/${REC#"~/"}" ;; esac
  [ -n "$REC" ] && [ -f "$REC" ] || exit 0
fi

resolve_self() {
  p=$0
  while [ -L "$p" ]; do
    link=$(readlink "$p")
    case "$link" in
      /*) p=$link ;;
      *) p=$(dirname "$p")/$link ;;
    esac
  done
  printf '%s\n' "$p"
}
if [ -n "${KPOPPER_ROOT:-}" ]; then
  ROOT=$KPOPPER_ROOT
else
  SELF=$(resolve_self)
  SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$SELF")" && pwd -P)
  ROOT=$(CDPATH= cd -- "$SELF_DIR/../../.." && pwd -P) 2>/dev/null || ROOT=""
fi
PROVENANCE_PY="$ROOT/scripts/provenance.py"
if [ ! -f "$PROVENANCE_PY" ]; then
  echo "gate-stop.sh: can't find scripts/provenance.py under '$ROOT' - set KPOPPER_ROOT to the kpopper checkout" >&2
  exit 0
fi

CID=$(printf '%s' "$IN" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("conversation_id",""))' 2>/dev/null || true)
BASE_FILE="${TMPDIR:-/tmp}/kpopper-base-cursor-$CID"
[ -n "$CID" ] && [ -f "$BASE_FILE" ] || exit 0

LOOP=$(printf '%s' "$IN" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("loop_count",0))' 2>/dev/null || echo 0)

OUT=$(python3 "$PROVENANCE_PY" check "$REC" 2>/dev/null)
NOW=$(printf '%s\n' "$OUT" | grep -c '^FAIL')
BASE=$(cat "$BASE_FILE" 2>/dev/null || echo 0)
[ "$NOW" -gt "$BASE" ] 2>/dev/null || exit 0

MSG=$(printf '%s\n%s\n%s\n' \
  "$REC fails check with $NOW problems ($BASE at session start)." \
  "Fix the record - or declare the hole with blocked_on - before finishing:" \
  "$(printf '%s\n' "$OUT" | grep '^FAIL' | head -12)")

if [ "$LOOP" = "0" ]; then
  # first stop this conversation: offer one bounce, same as stop_hook_active=false.
  printf '%s' "$MSG" | python3 -c 'import json,sys; print(json.dumps({"followup_message": sys.stdin.read()}))'
else
  # already bounced once (or another hook already spent this conversation's followup):
  # yield rather than fight cursor's own loop_limit - print the note and let it end.
  printf '%s\n' "$MSG" >&2
fi
exit 0
