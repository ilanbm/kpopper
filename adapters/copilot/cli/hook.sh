#!/bin/sh
# Copilot CLI hooks. `config` prints the hook configuration, `start` opens the record at
# sessionStart, and `stop` answers configurations that still register agentStop. Each
# prints one JSON object and exits 0; the hooks never install a runtime.
HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P) || { printf '{}\n'; exit 0; }
ROOT=$(CDPATH='' cd -- "$HERE/../../.." && pwd -P) || { printf '{}\n'; exit 0; }

# Standard input as the body of a JSON string, its lines joined with \n.
json_text() {
  LC_ALL=C tr -d '\001-\010\013\014\016-\037' |
    LC_ALL=C sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e "s/$(printf '\t')/\\\\t/g" -e "s/$(printf '\r')/\\\\r/g" |
    LC_ALL=C awk 'NR > 1 { printf "%s", "\\n" } { printf "%s", $0 }'
}

case "${1:-}" in
  config)
    printf '{"version": 1, "hooks": {"sessionStart": [{"type": "command", "exec": "sh", "args": ["%s", "start"], "timeoutSec": 65}]}}\n' \
      "$(printf '%s\n' "$HERE/hook.sh" | json_text)"
    exit 0 ;;
  stop) printf '{}\n'; exit 0 ;;
  start) ;;
  *) printf 'usage: hook.sh config|start|stop\n' >&2; exit 2 ;;
esac
case "${KPOPPER_RUNTIME:-rust}" in
  python) python3 "$HERE/hook.py" start; exit 0 ;;
  rust) ;;
  *) printf 'kpopper: KPOPPER_RUNTIME must be rust or python\n' >&2; printf '{}\n'; exit 0 ;;
esac
RUNTIME="$ROOT/scripts/native_runtime.sh"
if REPORT=$(sh "$RUNTIME" --path 2>&1 >/dev/null); then
  if sh "$RUNTIME" --exec session-start --help 2>/dev/null | grep -q -- '--copilot'; then
    OUT=$(sh "$RUNTIME" --exec session-start --copilot)
  else
    # A runtime released before --copilot: its plain opening, shaped here.
    TEXT=$(sh "$RUNTIME" --exec session-start)
    OUT=
    [ -z "$TEXT" ] || OUT=$(printf '{"additionalContext": "%s"}' "$(printf '%s\n' "$TEXT" | json_text)")
  fi
  [ -n "$OUT" ] || OUT='{}'
  printf '%s\n' "$OUT"
  exit 0
fi
# Without its runtime the opening says so, with the command that installs it. The payload
# is read all the same, as the opener would.
cat >/dev/null 2>&1
case "$REPORT" in
  *'Install this active copy: '*)
    OFFER="Offer to run the command above (it downloads this version's runtime from"
    OFFER="$OFFER the kpopper GitHub release and checks its SHA-256), then ask the user to start a new session." ;;
  *) OFFER= ;;
esac
printf '{"additionalContext": "%s"}\n' "$(printf '%s\n' "$REPORT" \
  'kpopper did not open this session, and its hooks never download or install the runtime.' \
  ${OFFER:+"$OFFER"} | json_text)"
exit 0
