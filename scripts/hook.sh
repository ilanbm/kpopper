#!/bin/sh
# Hooks never install a runtime or forward child failures as flow control.
export PYTHONIOENCODING=utf-8
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 0
case "${KPOPPER_RUNTIME:-rust}" in
  python) python3 "$HERE/plugin_runtime.py" hook "$@"; exit 0 ;;
  rust) ;;
  *) printf 'kpopper: KPOPPER_RUNTIME must be rust or python\n' >&2; exit 0 ;;
esac
BINARY=$(sh "$HERE/native_runtime.sh" --path) || exit 0
SCRIPT=${1:-}
[ $# -gt 0 ] && shift
case "$SCRIPT" in
  session_start.py) set -- session-start "$@" ;;
  ingestion_hooks.py) set -- ingestion-hook "$@" ;;
  followups_hook.py) set -- _hook followups "$@" ;;
  watch_hook.py) set -- _hook watch "$@" ;;
  ground_hook.py) set -- _hook ground "$@" ;;
  edit_hook.py) set -- _hook edit "$@" ;;
  *) printf 'kpopper: unsupported native hook: %s\n' "$SCRIPT" >&2; exit 0 ;;
esac
"$BINARY" "$@"
exit 0
