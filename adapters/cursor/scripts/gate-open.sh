#!/bin/sh
# cursor's sessionStart translated to the plugin's own protocol. cursor sends
# conversation_id, never session_id - so this keeps its own baseline file, keyed by
# conversation_id, instead of reusing scripts/session_open.sh's baseline convention.
# prints nothing when the directory keeps no record, same guard as session_open.sh.
# no `set -e`, to match session_open.sh/session_gate.sh: `check` returning 1 because it
# found problems is the normal case this script exists to handle, not a crash to abort on.

IN=$(cat)
# the record is at the root, or where the checkout registered it - same as session_open.sh
REC=PROVENANCE.yaml
if [ ! -f "$REC" ]; then
  G=$(git rev-parse --git-common-dir 2>/dev/null) || exit 0
  [ -f "$G/kpopper-record" ] || exit 0
  REC=$(head -n 1 "$G/kpopper-record")
  case "$REC" in "~/"*) REC="$HOME/${REC#"~/"}" ;; esac
  [ -n "$REC" ] && [ -f "$REC" ] || exit 0
fi

# resolve the kpopper checkout: KPOPPER_ROOT wins if set (required when this script was
# copied rather than symlinked - a copy has no path back to where it came from). absent
# that, resolve our own location, following symlinks by hand (portable: macOS's
# built-in readlink has no -f). this only finds the checkout when gate-open.sh itself
# is a symlink to adapters/cursor/scripts/gate-open.sh - see README for the install step.
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
  echo "gate-open.sh: can't find scripts/provenance.py under '$ROOT' - set KPOPPER_ROOT to the kpopper checkout" >&2
  exit 0
fi

CID=$(printf '%s' "$IN" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("conversation_id",""))' 2>/dev/null || true)

OUT=$(python3 "$PROVENANCE_PY" open --chars 2000 "$REC" 2>/dev/null) || \
  OUT="$REC is here but the reader could not run (python3 + PyYAML): read the record before relying on it."

# cursor's sessionStart response is JSON, not bare stdout (unlike claude/codex) - wrap it.
printf '%s' "$OUT" | python3 -c 'import json,sys; print(json.dumps({"additional_context": sys.stdin.read()}))'

# same reasoning as session_open.sh: what already failed when the session began is not
# this conversation's doing, and must not block gate-stop.sh at the end.
if [ -n "$CID" ]; then
  python3 "$PROVENANCE_PY" check "$REC" 2>/dev/null | grep -c '^FAIL' \
    > "${TMPDIR:-/tmp}/kpopper-base-cursor-$CID" 2>/dev/null || true
fi
exit 0
