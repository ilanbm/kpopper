#!/bin/sh
# Hooks never install a runtime or forward child failures as flow control.
export PYTHONIOENCODING=utf-8
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 0
case "${KPOPPER_RUNTIME:-rust}" in
  python) python3 "$HERE/plugin_runtime.py" hook "$@"; exit 0 ;;
  rust) ;;
  *) printf 'kpopper: KPOPPER_RUNTIME must be rust or python\n' >&2; exit 0 ;;
esac
SCRIPT=${1:-}
if [ "$SCRIPT" != session_start.py ]; then
  BINARY=$(sh "$HERE/native_runtime.sh" --path) || exit 0
elif ! BINARY=$(sh "$HERE/native_runtime.sh" --path 2>/dev/null); then
  # Claude Code and Codex add a SessionStart hook's standard output to the model's context
  # and never show the model its standard error. So the opener alone reports a missing
  # runtime on standard output, with the command that installs it; the other hooks stay
  # silent. The diagnostic is collected apart from the path, so no warning joins the path.
  REPORT=$(sh "$HERE/native_runtime.sh" --path 2>&1 >/dev/null)
  printf '%s\n' "$REPORT" \
    'kpopper did not open this session, and its hooks never download or install the runtime.'
  case "$REPORT" in
    *'Install this active copy: '*)
      printf '%s %s\n' "Offer to run the command above (it downloads this version's runtime from" \
        'the kpopper GitHub release and checks its SHA-256), then ask the user to start a new session.' ;;
  esac
  exit 0
fi
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
