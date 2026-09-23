#!/bin/sh
# Legacy Stop invocations are silent. Diagnostics are delivered only as context
# attached to a real prompt/tool event, never as a continuation or tool rejection.
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
  rust) ;;
  *) printf 'kpopper: KPOPPER_RUNTIME must be rust\n' >&2; exit 0 ;;
esac
BINARY=$(sh "$HERE/native_runtime.sh" --path) || exit 0
set -- session-context --event "$EVENT"
[ -n "$HOST" ] && set -- "$@" --host "$HOST"
"$BINARY" "$@"
exit 0
