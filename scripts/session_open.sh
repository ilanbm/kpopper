#!/bin/sh
# The session opener, run by the plugin's SessionStart hook. Prints nothing when
# the project keeps no record, so projects without one stay untouched.
#
# Where the record lives: at the project root - or, for a project whose tree cannot
# hold it, at the path registered in the git common dir (shared by every worktree,
# never tracked). provenance.py `where` resolves the same two places.
REC=PROVENANCE.yaml
if [ ! -f "$REC" ]; then
  G=$(git rev-parse --git-common-dir 2>/dev/null) || exit 0
  [ -f "$G/kpopper-record" ] || exit 0
  REC=$(head -n 1 "$G/kpopper-record")
  case "$REC" in "~/"*) REC="$HOME/${REC#"~/"}" ;; esac
  [ -n "$REC" ] && [ -f "$REC" ] || exit 0
fi
SID=$(python3 -c 'import json,sys; print(json.load(sys.stdin).get("session_id",""))' 2>/dev/null)
python3 "$(dirname "$0")/provenance.py" open --chars 2000 "$REC" 2>/dev/null \
  || echo "$REC is here but the reader could not run (python3 + PyYAML): read the record before relying on it."
# the stop gate compares against this count: what already failed when the session
# began is not the session's doing, and must not block its end.
if [ -n "$SID" ]; then
  python3 "$(dirname "$0")/provenance.py" check "$REC" 2>/dev/null | grep -c '^FAIL' \
    > "${TMPDIR:-/tmp}/kpopper-base-$SID" 2>/dev/null || true
fi
exit 0
