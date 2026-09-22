#!/bin/sh
# Resolve the native executable shipped with this exact plugin/package copy.
HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd) || exit 1
SYSTEM=$(uname -s) || exit 1
MACHINE=$(uname -m) || exit 1
case "$SYSTEM:$MACHINE" in
  Darwin:arm64|Darwin:aarch64) TARGET=darwin-arm64 ;;
  Darwin:x86_64) TARGET=darwin-x86_64 ;;
  Linux:aarch64|Linux:arm64) TARGET=linux-aarch64 ;;
  Linux:x86_64) TARGET=linux-x86_64 ;;
  MINGW*:x86_64|MSYS*:x86_64|CYGWIN*:x86_64) TARGET=windows-x86_64 ;;
  *) printf 'kpopper: unsupported native platform: %s %s\n' "$SYSTEM" "$MACHINE" >&2; exit 1 ;;
esac
BINARY="$HERE/runtime/$TARGET/kpop"
[ "$TARGET" = windows-x86_64 ] && BINARY="$BINARY.exe"
if [ ! -x "$BINARY" ]; then
  printf 'kpopper: native runtime is not installed for %s in this package.\n' "$TARGET" >&2
  if [ "$TARGET" = windows-x86_64 ]; then
    PACKAGE=$(cygpath -m "$HERE/..") || exit 1
    VERSION=$(cat "$HERE/../VERSION") || exit 1
    printf 'Install this active copy: pwsh -NoProfile -File "%s/install.ps1" -Version "%s" -PluginRoot "%s"\n' "$PACKAGE" "$VERSION" "$PACKAGE" >&2
  else
    printf 'Install this active copy: sh "%s/install_native.sh"\n' "$HERE" >&2
  fi
  exit 1
fi
if [ "${1:-}" = --path ]; then
  printf '%s\n' "$BINARY"
  exit 0
fi
[ "${1:-}" = --exec ] && shift
exec "$BINARY" "$@"
