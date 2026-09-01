#!/bin/sh
# every adapter file that embeds method-summary.md wraps the embed in
#   <!-- kpopper:method --> ... <!-- /kpopper:method -->
# this walks the adapters tree, extracts each wrapped block, and diffs it against
# method-summary.md - the one place the text is allowed to be authored. a check that
# cannot fail is decoration, so a missing embed anywhere is also a failure, not a skip.
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SRC="$HERE/method-summary.md"
ROOT=$(CDPATH= cd -- "$HERE/.." && pwd)
OPEN='<!-- kpopper:method -->'
CLOSE='<!-- /kpopper:method -->'

# the source of truth, this script, and the top-level README all legitimately contain
# the marker text as a string (authoring it, matching it, or documenting it) without
# embedding a block - named as absolute paths, not by pattern, so relocating or
# renaming this tree (as the verification below does, on purpose) can't fool the match.
SELF=$(CDPATH= cd -- "$HERE" && pwd)/check-drift.sh
TOPREADME="$ROOT/README.md"

[ -f "$SRC" ] || { echo "check-drift: missing $SRC" >&2; exit 1; }

TMP=$(mktemp -d "${TMPDIR:-/tmp}/kpopper-drift.XXXXXX")
trap 'rm -rf "$TMP"' EXIT INT TERM

status=0
found=0

files=$(grep -rl 'kpopper:method' "$ROOT" 2>/dev/null | sort)

if [ -z "$files" ]; then
  echo "check-drift: no files under $ROOT reference the marker" >&2
  exit 1
fi

is_skipped() {
  [ "$1" = "$SRC" ] || [ "$1" = "$SELF" ] || [ "$1" = "$TOPREADME" ]
}

for f in $files; do
  is_skipped "$f" && continue
  block="$TMP/block"
  awk -v mopen="$OPEN" -v mclose="$CLOSE" '
    index($0, mopen) { on=1; next }
    index($0, mclose) { on=0 }
    on { print }
  ' "$f" > "$block"

  if [ ! -s "$block" ]; then
    echo "DRIFT $f (marker present but no complete open/close pair - nothing extracted)"
    status=1
    continue
  fi

  found=$((found + 1))
  if ! diff -q "$SRC" "$block" >/dev/null 2>&1; then
    echo "DRIFT $f"
    status=1
  fi
done

if [ "$status" -eq 0 ]; then
  echo "check-drift: $found embed(s) match $SRC"
fi
exit $status
