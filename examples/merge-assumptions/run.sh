#!/bin/sh
# Reproduce the two README merge stories in disposable local Git repositories.
#
# Run from any directory: sh examples/merge-assumptions/run.sh
# Requires Git, a native `kpop` on PATH, and Python 3.9+ with the repository's
# Python dependencies installed (the example's own fictional project is
# Python). No network is used.
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
KPOP=${KPOP:-kpop}

# Clear every inherited Git repository-selection and injected-configuration
# variable for the whole run. git -C only sets a working directory; an
# inherited GIT_DIR/GIT_WORK_TREE (or GIT_INDEX_FILE, GIT_CONFIG_*, ...) still
# wins over it, so without this a caller's own checkout could be written to.
for _var in $(env | sed -n 's/^\(GIT_[A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p'); do
    unset "$_var"
done
unset _var

GIT_CONFIG_GLOBAL=/dev/null
GIT_CONFIG_SYSTEM=/dev/null
GIT_TERMINAL_PROMPT=0
export GIT_CONFIG_GLOBAL GIT_CONFIG_SYSTEM GIT_TERMINAL_PROMPT

# Temporary directories are tracked one per line, never word-split or glob-
# expanded on cleanup: a space or a glob character in TMPDIR must not turn
# `rm -rf` loose on an unrelated path.
TMP_DIRS=""
add_tmp_dir() {
    TMP_DIRS="$TMP_DIRS$1
"
}
cleanup() {
    _ifs=$IFS
    IFS='
'
    set -f
    for _d in $TMP_DIRS; do
        [ -n "$_d" ] && rm -rf "$_d"
    done
    set +f
    IFS=$_ifs
}
trap cleanup EXIT INT TERM

fail() {
    echo "$1" >&2
    exit 1
}

run_git() {
    # Fixed identity and a checkout untouched by the host's own git config.
    _root=$1
    shift
    git -C "$_root" -c user.name=Example -c user.email=example@example.invalid \
        -c commit.gpgsign=false -c core.autocrlf=false "$@"
}

overlay() {
    # Copy every file from an overlay directory onto the assembled repository.
    _source=$1
    _root=$2
    ( cd "$_source" && find . -type f ) | while IFS= read -r _rel; do
        mkdir -p "$_root/$(dirname "$_rel")"
        cp "$_source/$_rel" "$_root/$_rel"
    done
}

commit_all() {
    _root=$1
    _message=$2
    run_git "$_root" add -A
    run_git "$_root" commit -q -m "$_message"
}

branch_tests() {
    # The example's own fictional project is Python; run its unit tests the
    # way its own contributors would.
    _root=$1
    _out=$(cd "$_root" && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s . 2>&1) \
        || fail "branch tests failed:
$_out"
    case "$_out" in
        *"Ran 0 tests"*) fail "the example's branch tests were not discovered" ;;
    esac
}

# record_sha FILE -- test the hashing command itself, not a pipeline: `cmd | awk`
# exits on awk's status, so a missing cmd with its stderr silenced would
# otherwise print nothing and still report success.
record_sha() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        cksum "$1"
    fi
}

# A fresh interpreter, one call each, cwd=root: same isolation the original
# Python runner gave this probe, distinct from the branch tests. An ordinary
# integration test can catch these problems too.
CACHE_PROBE='
from cache import lookup
lookup("private", "alice")
print(any(p["private"] and p["owner"] == "alice" for p in lookup("private", "bob")))
'
DOWNLOAD_PROBE='
from pathlib import Path
import os
import re
import tempfile
from storage import purge_exports
days = int(re.search(r"Download available for (\d+) days",
                     Path("download-email.html").read_text()).group(1))
created = 1735689600
with tempfile.TemporaryDirectory() as folder:
    exported = Path(folder) / "export.csv"
    exported.write_text("fictional export")
    os.utime(exported, (created, created))
    purge_exports(folder, created + (days - 1) * 86400)
    print(not exported.exists())
'

# probe CASE ROOT MERGED -- a fresh-process integration probe distinct from
# the branch tests; an ordinary integration test can catch these problems too.
probe() {
    _case=$1
    _root=$2
    _merged=$3
    if [ "$_case" = cache ]; then
        _outcome=$(cd "$_root" && python3 -c "$CACHE_PROBE")
    else
        _outcome=$(cd "$_root" && python3 -c "$DOWNLOAD_PROBE")
    fi
    _outcome=$(printf '%s' "$_outcome" | tr '[:upper:]' '[:lower:]')
    [ "$_outcome" = "$_merged" ] || fail "unexpected cross-component behavior for $_case: $_outcome"
}

verify() {
    _case=$1
    _root=$2
    _stage=$3
    _merged=${4:-false}

    branch_tests "$_root"

    _record="$_root/GROUNDING.yaml"
    _before=$(record_sha "$_record")

    # check alone cannot see a code/config change that has not updated the record.
    "$KPOP" check "$_record" >/dev/null

    _expected_status=0
    [ "$_merged" = true ] && _expected_status=1
    set +e
    _measured=$("$KPOP" remeasure --run "$_record" 2>&1)
    _status=$?
    set -e
    [ "$_status" -eq "$_expected_status" ] || fail "remeasure --run: expected exit $_expected_status, got $_status
$_measured"

    if [ "$_case" = cache ]; then _decision=search.shared_cache; else _decision=downloads.availability; fi
    if [ "$_merged" = true ]; then
        case "$_measured" in
            *"$_decision"*"wrong_if holds"*) ;;
            *) fail "expected the documented failed condition
$_measured" ;;
        esac
    fi

    [ "$(record_sha "$_record")" = "$_before" ] || fail "measurement changed the canonical record"

    probe "$_case" "$_root" "$_merged"

    [ -z "$(run_git "$_root" status --porcelain)" ] || fail "the checks changed the example checkout"

    if [ "$_merged" = true ]; then
        _status_line="declared condition fails"
    else
        _status_line="measurement passes"
    fi
    echo "$_case / $_stage: branch tests pass; $_status_line"
    if [ "$_merged" = true ]; then
        printf '%s\n' "$_measured" | grep -m 1 "$_decision" | grep "wrong_if holds" | sed 's/^/  /'
        echo "  The separate integration probe also detects the $_case problem."
    fi
}

story() {
    _case=$1
    _root=$(mktemp -d "${TMPDIR:-/tmp}/kpopper-$_case-XXXXXX")
    add_tmp_dir "$_root"

    overlay "$HERE/$_case/base" "$_root"
    run_git "$_root" init -q -b base
    commit_all "$_root" "Shared base"

    run_git "$_root" checkout -q -b pr-a
    overlay "$HERE/$_case/pr-a" "$_root"
    commit_all "$_root" "PR A"
    verify "$_case" "$_root" "PR A"

    run_git "$_root" checkout -q base
    run_git "$_root" checkout -q -b pr-b
    overlay "$HERE/$_case/pr-b" "$_root"
    commit_all "$_root" "PR B"
    verify "$_case" "$_root" "PR B"

    run_git "$_root" merge --no-edit pr-a >/dev/null
    _parents=$(run_git "$_root" rev-list --parents -n 1 HEAD | wc -w | tr -d ' ')
    [ "$_parents" = 3 ] || fail "expected a real merge of the two branches"
    echo "$_case: Git merged PR A and PR B without a text conflict"
    verify "$_case" "$_root" merged true
}

for _example in cache downloads; do
    story "$_example"
done
