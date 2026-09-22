#!/bin/sh
# Legacy Stop invocations are silent. Diagnostics are delivered only as context
# attached to a real prompt/tool event, never as a continuation or tool rejection.
export PYTHONIOENCODING=utf-8
HOST=
EVENT=
while [ $# -gt 0 ]; do
  case "$1" in
    --host) [ $# -ge 2 ] || exit 0; HOST=$2; shift 2 ;;
    --context) [ $# -ge 2 ] || exit 0; EVENT=$2; shift 2 ;;
    *) shift ;;
  esac
done
case "$EVENT" in UserPromptSubmit|PostToolUse) ;; *) exit 0 ;; esac
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
case "${KPOPPER_RUNTIME:-rust}" in
  rust)
    BINARY=$(sh "$HERE/native_runtime.sh" --path) || exit 0
    set -- session-context --event "$EVENT"
    [ -n "$HOST" ] && set -- "$@" --host "$HOST"
    "$BINARY" "$@"
    exit 0 ;;
  python) ;;
  *) printf 'kpopper: KPOPPER_RUNTIME must be rust or python\n' >&2; exit 0 ;;
esac
PYTHON=$(python3 "$HERE/plugin_runtime.py" python) || exit 0
"$PYTHON" "$HERE/session_context.py" "$EVENT" "$HOST"
exit 0
