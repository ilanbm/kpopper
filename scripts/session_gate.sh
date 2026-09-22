#!/bin/sh
# The stop gate, run by the plugin's Stop hook: a session that leaves the record
# with new failures, leaves an intent no tab of the page serves, or wrote
# entries and recorded no intent, is bounced once - with the reasons - before it can
# finish. Everything is compared against the mark taken at session start, so a record
# that was already red or already unserved never blocks a session that did not touch
# it. An unchanged existing judgment falsified by updated readings remains flagged but
# does not block recording. Reminders about an untouched record are advisory prompt
# context only: a Stop block creates a new continuation request on some hosts.
# Private write receipts establish authorship; imported or manual
# changes are not attributed to the session. Delivery receipts suppress each repeated
# finding independently of the host's stop_hook_active flag. Validation still runs.
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
PYTHON=$(python3 "$HERE/plugin_runtime.py" python) || exit 0
REC=$(printf '%s' "$IN" | "$PYTHON" "$HERE/workspace.py" --hook --path 2>/dev/null) || exit 0
SID=$(printf '%s' "$IN" | "$PYTHON" -c 'import json,re,sys; s=json.load(sys.stdin).get("session_id",""); print(s if isinstance(s,str) and re.fullmatch(r"[A-Za-z0-9_-]{1,200}",s) else "")' 2>/dev/null)
BASE_FILE="${TMPDIR:-/tmp}/kpopper-base-$SID"
[ -n "$SID" ] && [ -f "$BASE_FILE" ] || exit 0
set -- gate "$BASE_FILE" "$REC" --session "$SID"
[ -n "$HOST" ] && set -- "$@" --host "$HOST"
# 2 is the gate's own answer: something to say. Any other failure is the reader's, and a
# reader that cannot run must not hold a session at its end.
OUT=$("$PYTHON" "$HERE/provenance.py" "$@" 2>/dev/null)
RC=$?
if [ "$RC" -eq 2 ]; then
  printf '%s\n' "$OUT" >&2
  exit 2
fi
exit 0
