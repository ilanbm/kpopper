#!/bin/sh
# Gemini CLI's SessionStart hook. Gemini gives the model only the JSON field
# hookSpecificOutput.additionalContext, so standard output is one JSON object. The hook
# never installs a runtime and always exits 0.
HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P) || { printf '{}\n'; exit 0; }
ROOT=$(CDPATH='' cd -- "$HERE/../../.." && pwd -P) || { printf '{}\n'; exit 0; }
case "${KPOPPER_RUNTIME:-rust}" in
  python) python3 "$HERE/hook.py" SessionStart; exit 0 ;;
  rust) ;;
  *) printf 'kpopper: KPOPPER_RUNTIME must be rust or python\n' >&2; printf '{}\n'; exit 0 ;;
esac

# Standard input as the body of a JSON string, its lines joined with \n.
json_text() {
  LC_ALL=C tr -d '\001-\010\013\014\016-\037' |
    LC_ALL=C sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e "s/$(printf '\t')/\\\\t/g" -e "s/$(printf '\r')/\\\\r/g" |
    LC_ALL=C awk 'NR > 1 { printf "%s", "\\n" } { printf "%s", $0 }'
}
context() {
  printf '{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "%s"}}\n' "$1"
}

RUNTIME="$ROOT/scripts/native_runtime.sh"
if REPORT=$(sh "$RUNTIME" --path 2>&1 >/dev/null); then
  if sh "$RUNTIME" --exec session-start --help 2>/dev/null | grep -q -- '--gemini'; then
    OUT=$(sh "$RUNTIME" --exec session-start --gemini)
  else
    # A runtime released before --gemini: its plain opening, shaped here.
    TEXT=$(sh "$RUNTIME" --exec session-start)
    OUT='{}'
    [ -z "$TEXT" ] || OUT=$(context "$(printf '%s\n' "$TEXT" | json_text)")
  fi
  if [ -n "$OUT" ]; then
    printf '%s\n' "$OUT"
  else
    context 'kpopper could not open the record; check it before relying on it.'
  fi
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
context "$(printf '%s\n' "$REPORT" \
  'kpopper did not open this session, and its hooks never download or install the runtime.' \
  ${OFFER:+"$OFFER"} | json_text)"
exit 0
