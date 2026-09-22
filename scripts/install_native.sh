#!/bin/sh
set -eu
ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd) || exit 1
[ -f "$ROOT/VERSION" ] || { printf 'kpopper installer: missing %s/VERSION\n' "$ROOT" >&2; exit 1; }
VERSION=$(sed -n '1p' "$ROOT/VERSION")
[ -n "$VERSION" ] || { printf 'kpopper installer: VERSION is empty\n' >&2; exit 1; }
exec sh "$ROOT/install.sh" "$@" --plugin-root "$ROOT" --version "$VERSION"
