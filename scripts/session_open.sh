#!/bin/sh
# The session opener, run by the plugin's SessionStart hook. Prints nothing when
# the project keeps no record, so projects without one stay untouched.
#
# Where the record lives: at the project root - or, for a project whose tree cannot
# hold it, at the path registered in the git common dir (shared by every worktree,
# never tracked). provenance.py `where` resolves the same two places.
REC=PROVENANCE.yaml
if [ ! -f "$REC" ]; then
  ROOT=$(git rev-parse --show-toplevel 2>/dev/null)
  if [ -n "$ROOT" ] && [ -f "$ROOT/PROVENANCE.yaml" ]; then
    REC="$ROOT/PROVENANCE.yaml"
  fi
fi
if [ ! -f "$REC" ]; then
  G=$(git rev-parse --git-common-dir 2>/dev/null) || exit 0
  [ -f "$G/kpopper-record" ] || exit 0
  REC=$(head -n 1 "$G/kpopper-record")
  case "$REC" in "~/"*) REC="$HOME/${REC#"~/"}" ;; esac
  [ -n "$REC" ] && [ -f "$REC" ] || exit 0
fi
SID=$(python3 -c 'import json,sys; print(json.load(sys.stdin).get("session_id",""))' 2>/dev/null)
python3 "$(dirname "$0")/session_hook.py" "$REC"
MODE=$?
if [ "$MODE" -eq 3 ]; then
  python3 "$(dirname "$0")/provenance.py" open --chars 2000 "$REC" 2>/dev/null \
    || echo "$REC is here but the reader could not run (python3 + PyYAML): read the record before relying on it."
fi
# the stop gate compares against this mark: what already failed, or was already left
# unserved, when the session began is not the session's doing, and must not block its end.
if [ -n "$SID" ]; then
  python3 "$(dirname "$0")/provenance.py" mark "${TMPDIR:-/tmp}/kpopper-base-$SID" "$REC" \
    >/dev/null 2>&1 || true
fi
exit 0
