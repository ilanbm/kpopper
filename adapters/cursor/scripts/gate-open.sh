#!/bin/sh
# cursor's sessionStart translated to the plugin's own protocol. cursor sends
# conversation_id, never session_id - so this keeps its own baseline file, keyed by
# conversation_id, instead of reusing scripts/session_open.sh's baseline convention.
# Existing records and first-use guidance use the shared opener.
# no `set -e`, to match session_open.sh/session_gate.sh: `check` returning 1 because it
# found problems is the normal case this script exists to handle, not a crash to abort on.

IN=$(cat)

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

printf '%s' "$IN" | python3 "$ROOT/scripts/session_start.py" --cursor
exit 0
