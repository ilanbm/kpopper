#!/bin/sh
# Resolve the native executable for this exact plugin/package copy: the runtime installed in
# the copy itself, or else the machine-wide install of the same version that install.sh makes
# without --plugin-root. Nothing here downloads or installs a runtime.
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
EXE=
[ "$TARGET" = windows-x86_64 ] && EXE=.exe
BINARY="$HERE/runtime/$TARGET/kpop$EXE"
if [ ! -x "$BINARY" ]; then
  # One install serves every copy of its version: the exact version only, in a directory the
  # installer marked with that version.
  VERSION=$(sed -n '1p' "$HERE/../VERSION" 2>/dev/null | tr -d '\r')
  case "$VERSION" in ''|.|..|*[!A-Za-z0-9._-]*) VERSION= ;; esac
  INSTALLED="${HOME:-}/.local/lib/kpopper/$VERSION/$TARGET"
  if [ -n "$VERSION" ] && [ -n "${HOME:-}" ] && [ -f "$INSTALLED/.kpopper-managed" ] &&
    [ -f "$INSTALLED/bin/kpop$EXE" ] && [ -x "$INSTALLED/bin/kpop$EXE" ] &&
    [ "$(tr -d '\r' 2>/dev/null < "$INSTALLED/.kpopper-managed")" = "$VERSION" ]; then
    BINARY="$INSTALLED/bin/kpop$EXE"
  else
    printf 'kpopper: native runtime is not installed for %s in this package.\n' "$TARGET" >&2
    if [ "$TARGET" = windows-x86_64 ]; then
      PACKAGE=$(cygpath -m "$HERE/..") || exit 1
      [ -n "$VERSION" ] || exit 1
      printf 'Install this active copy: pwsh -NoProfile -File "%s/install.ps1" -Version "%s" -PluginRoot "%s"\n' "$PACKAGE" "$VERSION" "$PACKAGE" >&2
      printf 'Or install once for every copy of this version: pwsh -NoProfile -File "%s/install.ps1" -Version "%s"\n' "$PACKAGE" "$VERSION" >&2
    else
      printf 'Install this active copy: sh "%s/install_native.sh"\n' "$HERE" >&2
      [ -z "$VERSION" ] || printf 'Or install once for every copy of this version: sh "%s/install.sh" --version %s\n' "${HERE%/*}" "$VERSION" >&2
    fi
    exit 1
  fi
fi
if [ "${1:-}" = --path ]; then
  printf '%s\n' "$BINARY"
  exit 0
fi
[ "${1:-}" = --exec ] && shift
exec "$BINARY" "$@"
