#!/bin/sh
# Hooks never install or download a runtime. Python is an explicit compatibility mode.
export PYTHONIOENCODING=utf-8
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 0
case "${KPOPPER_RUNTIME:-rust}" in
  python) exec python3 "$HERE/plugin_runtime.py" hook "$@" ;;
  rust) ;;
  *) printf 'kpopper: KPOPPER_RUNTIME must be rust or python\n' >&2; exit 0 ;;
esac
BINARY=$(sh "$HERE/native_runtime.sh" --path) || exit 0
SCRIPT=${1:-}
[ $# -gt 0 ] && shift
case "$SCRIPT" in
  session_start.py) exec "$BINARY" session-start "$@" ;;
  ingestion_hooks.py) exec "$BINARY" ingestion-hook "$@" ;;
  followups_hook.py) exec "$BINARY" _hook followups "$@" ;;
  watch_hook.py) exec "$BINARY" _hook watch "$@" ;;
  ground_hook.py) exec "$BINARY" _hook ground "$@" ;;
  edit_hook.py) exec "$BINARY" _hook edit "$@" ;;
  *) printf 'kpopper: unsupported native hook: %s\n' "$SCRIPT" >&2; exit 0 ;;
esac
