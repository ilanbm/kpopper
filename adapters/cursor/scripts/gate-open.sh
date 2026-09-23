#!/bin/sh
# cursor's sessionStart translated to the plugin's own protocol. use the common
# conversation_id across both hooks (sessionStart also documents session_id), with
# a cursor-prefixed baseline to keep it separate from other hosts.
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
RUNTIME="$ROOT/scripts/native_runtime.sh"
if [ ! -f "$RUNTIME" ]; then
  echo "gate-open.sh: can't find scripts/native_runtime.sh under '$ROOT' - set KPOPPER_ROOT to the kpopper checkout" >&2
  exit 0
fi

case "${KPOPPER_RUNTIME:-rust}" in
  python) printf '%s' "$IN" | python3 "$ROOT/scripts/session_start.py" --cursor; exit 0 ;;
  rust) ;;
  *) printf 'gate-open.sh: KPOPPER_RUNTIME must be rust or python\n' >&2; exit 0 ;;
esac

# Cursor gives the agent additional_context and never shows it standard error, so a
# missing runtime is reported there, with the command that installs it.
json_text() {
  sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/	/\\t/g' | awk 'NR > 1 { printf "\\n" } { printf "%s", $0 }'
}
if ! REPORT=$(sh "$RUNTIME" --path 2>&1 >/dev/null); then
  TEXT=$(printf '%s\n%s' "$REPORT" \
    'kpopper did not open this session, and its hooks never download or install the runtime.')
  case "$REPORT" in
    *'Install this active copy: '*)
      TEXT=$(printf '%s\n%s %s' "$TEXT" "Offer to run the command above (it downloads this version's runtime from" \
        'the kpopper GitHub release and checks its SHA-256), then ask the user to start a new session.') ;;
  esac
  printf '{"additional_context": "%s"}\n' "$(printf '%s' "$TEXT" | json_text)"
  exit 0
fi
printf '%s' "$IN" | sh "$RUNTIME" --exec session-start --cursor
exit 0
