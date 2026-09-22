#!/bin/sh
set -eu

REPOSITORY=https://github.com/ilanbm/kpopper
VERSION=
ARCHIVE=
EXPECTED=
PREFIX=
PLUGIN_ROOT=

usage() {
  cat <<'EOF'
Usage: install.sh [--version VERSION] [--archive FILE --sha256 HEX] [--prefix DIR]
                  [--plugin-root DIR]

Without --archive, downloads the requested GitHub release. If --version is
omitted, the official Latest release is selected. --prefix installs into a
disposable/self-contained prefix. --plugin-root stages a plugin runtime.
EOF
}

die() { printf 'kpopper installer: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version|--archive|--sha256|--prefix|--plugin-root)
      [ "$#" -ge 2 ] || die "$1 requires a value"
      case "$1" in
        --version) VERSION=$2 ;;
        --archive) ARCHIVE=$2 ;;
        --sha256) EXPECTED=$2 ;;
        --prefix) PREFIX=$2 ;;
        --plugin-root) PLUGIN_ROOT=$2 ;;
      esac
      shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

[ -z "$PLUGIN_ROOT" ] || [ -z "$PREFIX" ] || die "--prefix and --plugin-root cannot be combined"
case "$VERSION" in *[!A-Za-z0-9._-]*|.|..) die "invalid version: $VERSION" ;; esac

SYSTEM=$(uname -s) || die "cannot determine operating system"
MACHINE=$(uname -m) || die "cannot determine architecture"
case "$SYSTEM:$MACHINE" in
  Darwin:arm64|Darwin:aarch64) TARGET=darwin-arm64 ;;
  Darwin:x86_64) TARGET=darwin-x86_64 ;;
  Linux:aarch64|Linux:arm64) TARGET=linux-aarch64 ;;
  Linux:x86_64) TARGET=linux-x86_64 ;;
  *) die "unsupported native platform: $SYSTEM $MACHINE" ;;
esac

TMP_ROOT=${TMPDIR:-/tmp}
WORK=$(mktemp -d "$TMP_ROOT/kpopper-install.XXXXXX") || die "cannot create temporary directory"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT HUP INT TERM

if [ -n "$ARCHIVE" ]; then
  [ -n "$VERSION" ] || die "--version is required with --archive"
  [ -n "$EXPECTED" ] || die "--sha256 is required with --archive"
  [ -f "$ARCHIVE" ] || die "archive does not exist: $ARCHIVE"
else
  [ -z "$EXPECTED" ] || die "--sha256 is only valid with --archive"
  command -v curl >/dev/null 2>&1 || die "curl is required for online installation"
  if [ -z "$VERSION" ]; then
    LATEST=$(curl -fsSL --proto '=https' --tlsv1.2 -o /dev/null -w '%{url_effective}' "$REPOSITORY/releases/latest") || die "cannot resolve Latest release"
    VERSION=${LATEST##*/tag/}
    VERSION=${VERSION#v}
    [ -n "$VERSION" ] || die "Latest release did not identify a version"
  fi
  case "$VERSION" in *[!A-Za-z0-9._-]*|.|..) die "invalid version: $VERSION" ;; esac
  NAME=kpopper-$VERSION-$TARGET.tar.gz
  ARCHIVE=$WORK/$NAME
  curl -fL --proto '=https' --tlsv1.2 -o "$ARCHIVE" "$REPOSITORY/releases/download/v$VERSION/$NAME" || die "release download failed"
  curl -fL --proto '=https' --tlsv1.2 -o "$WORK/SHA256SUMS" "$REPOSITORY/releases/download/v$VERSION/SHA256SUMS" || die "checksum download failed"
  EXPECTED=$(awk -v name="$NAME" '$2 == name || $2 == "*" name {print $1; found=1; exit} END {if (!found) exit 1}' "$WORK/SHA256SUMS") || die "archive is absent from SHA256SUMS"
fi

case "$EXPECTED" in *[!0-9A-Fa-f]*|'') die "invalid SHA256" ;; esac
[ "${#EXPECTED}" -eq 64 ] || die "invalid SHA256"
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL=$(sha256sum "$ARCHIVE" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL=$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')
else
  die "sha256sum or shasum is required"
fi
[ "$(printf '%s' "$ACTUAL" | tr 'A-F' 'a-f')" = "$(printf '%s' "$EXPECTED" | tr 'A-F' 'a-f')" ] || die "archive SHA256 does not match"

TOP=kpopper-$VERSION-$TARGET
LIST=$WORK/members
tar -tzf "$ARCHIVE" > "$LIST" || die "archive is not a readable tar.gz"
[ -s "$LIST" ] || die "archive is empty"
while IFS= read -r MEMBER; do
  case "$MEMBER" in
    ''|/*|../*|*/../*|*/..|*\\*) die "unsafe archive path: $MEMBER" ;;
    "$TOP"|"$TOP/"|"$TOP/"*) ;;
    *) die "unexpected archive member: $MEMBER" ;;
  esac
done < "$LIST"
tar -tvzf "$ARCHIVE" > "$WORK/verbose" || die "cannot inspect archive members"
while IFS= read -r LINE; do
  TYPE=$(printf '%s' "$LINE" | cut -c1)
  case "$TYPE" in -|d) ;; *) die "archive contains a link or special file" ;; esac
done < "$WORK/verbose"
grep -Fx "$TOP/manifest.json" "$LIST" >/dev/null || die "archive manifest is missing"
grep -Fx "$TOP/bin/kpop" "$LIST" >/dev/null || die "kpop executable is missing"
grep -Fx "$TOP/bin/kpopper" "$LIST" >/dev/null || die "kpopper executable is missing"
grep -Fx "$TOP/bin/resources/reasoning/" "$LIST" >/dev/null || die "reasoning resources are missing"
grep -Fx "$TOP/bin/resources/ordinary/" "$LIST" >/dev/null || die "ordinary resources are missing"
mkdir "$WORK/unpacked"
(umask 077; tar -xzf "$ARCHIVE" -C "$WORK/unpacked") || die "archive extraction failed"
SOURCE=$WORK/unpacked/$TOP/bin
PACKAGE_ROOT=$WORK/unpacked/$TOP
[ -x "$SOURCE/kpop" ] && [ -x "$SOURCE/kpopper" ] || die "archive executables do not have executable mode"

if [ -n "$PLUGIN_ROOT" ]; then
  DEST=$PLUGIN_ROOT/scripts/runtime/$TARGET
  if [ -e "$DEST" ] && [ ! -f "$DEST/.kpopper-managed" ]; then
    die "refusing to replace unmanaged plugin runtime: $DEST"
  fi
  STAGE=$PLUGIN_ROOT/scripts/runtime/.install-$TARGET-$$
  [ ! -e "$STAGE" ] || die "temporary plugin path already exists: $STAGE"
  mkdir -p "$STAGE"
  cp -R "$SOURCE/." "$STAGE/"
  printf '%s\n' "$VERSION" > "$STAGE/.kpopper-managed"
  [ ! -e "$DEST" ] || rm -rf "$DEST"
  mv "$STAGE" "$DEST"
  printf 'Installed kpopper %s plugin runtime at %s\n' "$VERSION" "$DEST"
  exit 0
fi

BASE=${PREFIX:-"$HOME/.local"}
PUBLIC_BIN=$BASE/bin
DEST=$BASE/lib/kpopper/$VERSION/$TARGET
for NAME in kpop kpopper; do
  LINK=$PUBLIC_BIN/$NAME
  if [ -e "$LINK" ] || [ -L "$LINK" ]; then
    [ -L "$LINK" ] || die "refusing to replace unmanaged path: $LINK"
    OLD=$(readlink "$LINK") || die "cannot inspect existing link: $LINK"
    case "$OLD" in ../lib/kpopper/*/*/bin/$NAME|"$BASE"/lib/kpopper/*/*/bin/$NAME) ;; *) die "refusing to replace unmanaged link: $LINK" ;; esac
  fi
done
if [ -e "$DEST" ] && [ ! -f "$DEST/.kpopper-managed" ]; then
  die "refusing to replace unmanaged version directory: $DEST"
fi
mkdir -p "$BASE/lib/kpopper/$VERSION" "$PUBLIC_BIN"
STAGE=$BASE/lib/kpopper/$VERSION/.install-$TARGET-$$
[ ! -e "$STAGE" ] || die "temporary install path already exists: $STAGE"
mkdir "$STAGE"
cp -R "$PACKAGE_ROOT/." "$STAGE/"
printf '%s\n' "$VERSION" > "$STAGE/.kpopper-managed"
[ ! -e "$DEST" ] || rm -rf "$DEST"
mv "$STAGE" "$DEST"
for NAME in kpop kpopper; do
  LINK=$PUBLIC_BIN/$NAME
  rm -f "$LINK"
  ln -s "../lib/kpopper/$VERSION/$TARGET/bin/$NAME" "$LINK"
done
printf 'Installed kpopper %s at %s\nPublic commands: %s/kpop and %s/kpopper\n' "$VERSION" "$DEST" "$PUBLIC_BIN" "$PUBLIC_BIN"
