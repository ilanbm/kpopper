#!/bin/sh
# Compile the packaged ordinary Lean source into the versioned local program cache
# and print the directory that holds it.
#
# The cache layout is the one the native reader expects:
#
#   <cache root>/<target>/<sha256 of Main.lean>/{build.json,epistemic-core}
#
# `build.json` is compact JSON with sorted keys, and carries the SHA-256 of the Lean
# source and of the compiled executable. The reader validates both before it runs the
# program, and accepts no other file in the directory, so nothing else is published
# there. `lean_version` and `platform` describe the machine that compiled the program
# and are informational.
#
# Usage: build_ordinary_program.sh [--rebuild] [LEAN_TOOLCHAIN_PREFIX]
#
# The toolchain prefix may also come from KPOPPER_LEAN_ROOT; without either, `lean` is
# taken from PATH and asked for its own prefix. KPOPPER_CORE_CACHE selects the cache
# root, otherwise XDG_CACHE_HOME (or ~/.cache) plus `kpopper/lean`.
#
# Nothing is downloaded and no record command is executed: only the reviewed Lean
# source in this repository is compiled.
set -eu

LEAN_VERSION=4.33.1

fail() {
    echo "build_ordinary_program: $*" >&2
    exit 1
}

if command -v sha256sum >/dev/null 2>&1; then
    sha256_of() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256_of() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
    fail "no SHA-256 tool found (sha256sum or shasum)"
fi

# Quote a string for a JSON document.
json_string() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# Read one top-level string field out of the compact manifest.
manifest_field() {
    sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p" "$2"
}

# Hosts whose shell and native tools disagree about path syntax carry a translator.
if command -v cygpath >/dev/null 2>&1; then
    shell_path() { cygpath -u "$1"; }
    host_path() { cygpath -m "$1"; }
else
    shell_path() { printf '%s\n' "$1"; }
    host_path() { printf '%s\n' "$1"; }
fi

rebuild=""
lean_root=""
while [ $# -gt 0 ]; do
    case "$1" in
        --rebuild) rebuild=yes ;;
        -*) fail "unknown option: $1" ;;
        *) [ -z "$lean_root" ] || fail "unexpected argument: $1"; lean_root="$1" ;;
    esac
    shift
done
[ -n "$lean_root" ] || lean_root="${KPOPPER_LEAN_ROOT:-}"
[ -z "$lean_root" ] || lean_root=$(shell_path "$lean_root")

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
source_file="$here/../shared/session/lean/Main.lean"
[ -f "$source_file" ] || fail "ordinary Lean source not found at $source_file"
source_sha256=$(sha256_of "$source_file")

# The platform names the reader resolves when it looks for this program in the local cache.
machine=$(uname -m)
case "$(uname -s)" in
    Darwin) system=darwin ;;
    Linux) system=linux ;;
    MINGW* | MSYS* | CYGWIN* | Windows_NT) system=windows ;;
    *) fail "unsupported operating system: $(uname -s)" ;;
esac
case "$system:$machine" in
    darwin:arm64 | darwin:aarch64) target=darwin-arm64 ;;
    darwin:x86_64) target=darwin-x86_64 ;;
    linux:aarch64 | linux:arm64) target=linux-aarch64 ;;
    linux:x86_64) target=linux-x86_64 ;;
    windows:x86_64 | windows:amd64) target=windows-amd64 ;;
    *) fail "unsupported platform: $system-$machine" ;;
esac

if [ "$system" = windows ]; then
    suffix=.exe
    program=epistemic-core.exe
else
    suffix=""
    program=epistemic-core
fi

cache_root="${KPOPPER_CORE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/kpopper/lean}"
case "$cache_root" in
    "~/"*) cache_root="$HOME/${cache_root#~/}" ;;
esac
cache_root=$(shell_path "$cache_root")
case "$cache_root" in
    /*) ;;
    *) cache_root="$PWD/$cache_root" ;;
esac
parent="$cache_root/$target"
destination="$parent/$source_sha256"

# Print the cache directory in the form the host's other tools accept.
publish() {
    host_path "$1"
}

if [ -d "$destination" ] && [ -z "$rebuild" ]; then
    [ -f "$destination/build.json" ] && [ -f "$destination/$program" ] ||
        fail "the cached program at $destination is incomplete; rerun with --rebuild"
    [ "$(manifest_field source_sha256 "$destination/build.json")" = "$source_sha256" ] &&
        [ "$(manifest_field binary_sha256 "$destination/build.json")" = "$(sha256_of "$destination/$program")" ] ||
        fail "the cached program at $destination does not match its manifest; rerun with --rebuild"
    publish "$destination"
    exit 0
fi

if [ -n "$lean_root" ]; then
    lean="$lean_root/bin/lean$suffix"
    [ -x "$lean" ] || fail "no Lean compiler at $lean"
else
    lean=$(command -v lean || true)
    [ -n "$lean" ] || fail "Lean $LEAN_VERSION is required; pass its toolchain prefix"
    # Resolve the toolchain a proxy selects, under its own invocation name.
    lean_root=$(shell_path "$("$lean" --print-prefix)")
    lean="$lean_root/bin/lean$suffix"
fi
leanc="$lean_root/bin/leanc$suffix"
[ -x "$leanc" ] || fail "no Lean linker at $leanc"

lean_version=$("$lean" --version)
case "$lean_version" in
    *"version $LEAN_VERSION,"*) ;;
    *) fail "expected Lean $LEAN_VERSION; found $lean_version" ;;
esac

mkdir -p "$parent"
work=$(mktemp -d "$parent/.build-XXXXXX")
cleanup() { [ -z "${work:-}" ] || rm -rf "$work"; }
trap cleanup EXIT
trap 'cleanup; exit 1' HUP INT TERM

cp "$source_file" "$work/Main.lean"
(cd "$work" && "$lean" -o Main.olean -c Main.c Main.lean) >&2
(cd "$work" && "$leanc" -o "$program" Main.c) >&2
[ "$(sha256_of "$source_file")" = "$source_sha256" ] ||
    fail "the Lean source changed while it was being compiled; retry on a settled checkout"

printf '{"binary_sha256":"%s","lean_version":"%s","platform":"%s","source_sha256":"%s"}\n' \
    "$(sha256_of "$work/$program")" \
    "$(json_string "$lean_version")" \
    "$(json_string "$(uname -s)-$(uname -r)-$machine")" \
    "$source_sha256" > "$work/build.json"
rm -f "$work/Main.lean" "$work/Main.c" "$work/Main.olean"

# Renaming the directory publishes a matching executable and manifest at once.
backup=""
if [ -d "$destination" ]; then
    backup="$destination.replaced-$$"
    mv "$destination" "$backup"
fi
if mv "$work" "$destination"; then
    work=""
    [ -z "$backup" ] || rm -rf "$backup"
else
    [ -z "$backup" ] || mv "$backup" "$destination"
    fail "could not publish the compiled program at $destination"
fi

publish "$destination"
