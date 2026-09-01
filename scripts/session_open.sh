#!/bin/sh
# The session opener, run by the plugin's SessionStart hook. Prints nothing when
# the directory keeps no record, so projects without one stay untouched.
[ -f PROVENANCE.yaml ] || exit 0
SID=$(python3 -c 'import json,sys; print(json.load(sys.stdin).get("session_id",""))' 2>/dev/null)
python3 "$(dirname "$0")/provenance.py" open --chars 2000 2>/dev/null \
  || echo "PROVENANCE.yaml is here but the reader could not run (python3 + PyYAML): read the record before relying on it."
# the stop gate compares against this count: what already failed when the session
# began is not the session's doing, and must not block its end.
if [ -n "$SID" ]; then
  python3 "$(dirname "$0")/provenance.py" check 2>/dev/null | grep -c '^FAIL' \
    > "${TMPDIR:-/tmp}/kpopper-base-$SID" 2>/dev/null || true
fi
exit 0
