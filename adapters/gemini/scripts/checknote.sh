#!/bin/sh
# the plugin's stop gate cannot exist here: gemini's SessionEnd "will not wait for this
# hook to complete and ignores all flow-control fields" - there is no exit code that
# blocks a session from ending. so this only ever notes, never gates: run check, print
# what it finds, exit 0 no matter what. GEMINI.md says as much, in words a person reads.
# no `set -e`: check exits 1 on finding problems, which is the normal, expected case
# here, not a crash - the same reason session_gate.sh never sets it either.

[ -f PROVENANCE.yaml ] || exit 0

# invoked via the extension's own ${extensionPath}-substituted absolute path, so $0 is
# already real - no symlink-following needed the way the cursor wrappers require.
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$HERE/../../.." && pwd) 2>/dev/null || ROOT=""
PROVENANCE_PY="$ROOT/scripts/provenance.py"
if [ ! -f "$PROVENANCE_PY" ]; then
  echo "checknote.sh: can't find scripts/provenance.py under '$ROOT'" >&2
  exit 0
fi

# check exiting 1 means it found problems, not that it crashed - it still printed a
# full report to stdout in that case, so the only real crash signal is empty output
# (missing python3/PyYAML, before a single line gets written).
OUT=$(python3 "$PROVENANCE_PY" check 2>/dev/null)
if [ -n "$OUT" ]; then
  printf '%s\n' "$OUT"
else
  echo "PROVENANCE.yaml is here but the reader could not run (python3 + PyYAML): read the record before relying on it."
fi
exit 0
