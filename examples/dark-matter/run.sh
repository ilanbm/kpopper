#!/bin/sh
# Replay a six-paper research record, with real CLI writes and dependency checks.
#
# Run from any directory: sh examples/dark-matter/run.sh [--output DIR]
# No model calls or network. Requires a native `kpop` on PATH and POSIX file
# locks (macOS/Linux), and a ready local reasoning core for the exact density
# calculation.
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
KPOP=${KPOP:-kpop}
ENTRIES="$HERE/entries.tsv"

OUTPUT=""
if [ "${1:-}" = "--output" ]; then
    OUTPUT=$2
fi

fail() {
    echo "$1" >&2
    exit 1
}

# record_sha FILE -- test the hashing command itself, not a pipeline: `cmd |
# awk` exits on awk's status, so a missing cmd with its stderr silenced would
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

# exact_ratio A B -- the reduced integer numerator/denominator of decimal
# string A divided by decimal string B, computed on scaled integers so no
# floating-point rounding enters the comparison against the record's own
# exact rational result.
exact_ratio() {
    awk -v a="$1" -v b="$2" '
    function scale(s, r,    dot, digits) {
        dot = index(s, ".")
        digits = (dot == 0) ? 0 : length(s) - dot
        gsub(/\./, "", s)
        r["n"] = s + 0
        r["d"] = digits
    }
    function gcd(x, y,   t) { while (y != 0) { t = x % y; x = y; y = t }; return x }
    BEGIN {
        scale(a, A); scale(b, B)
        num = A["n"]; den = B["n"]
        for (i = 0; i < B["d"]; i++) num = num * 10
        for (i = 0; i < A["d"]; i++) den = den * 10
        g = gcd(num, den); if (g == 0) g = 1
        printf "%d/%d", num / g, den / g
    }'
}

# add ROOT COLLECTION SUBJECT JSON
# A concurrent preparation can observe a newer committed record. Only retry
# this explicit pre-write refusal; semantic failures must remain failures.
add() {
    _root=$1
    _collection=$2
    _subject=$3
    _json=$4
    _attempt=0
    while true; do
        _attempt=$((_attempt + 1))
        if _out=$("$KPOP" --workspace "$_root" add "$_subject" "$_json" --in "$_collection" 2>&1); then
            return 0
        fi
        if [ "$_out" = "refused - record changed while preparing the write; retry" ] && [ "$_attempt" -lt 64 ]; then
            sleep 0.1
            continue
        fi
        fail "add $_subject: $_out"
    done
}

# add_phase ROOT PHASE -- sequential adds for one phase of entries.tsv, in file order.
add_phase() {
    _root=$1
    _phase=$2
    awk -F'\t' -v p="$_phase" '$1 == p' "$ENTRIES" | while IFS='	' read -r _phase _collection _subject _json; do
        add "$_root" "$_collection" "$_subject" "$_json"
    done
}

# add_findings_worker ROOT INDEX MOD -- every Nth finding, for concurrent writers.
add_findings_worker() {
    _root=$1
    _index=$2
    _mod=$3
    _n=0
    awk -F'\t' '$1 == "findings"' "$ENTRIES" | while IFS='	' read -r _phase _collection _subject _json; do
        if [ $((_n % _mod)) -eq "$_index" ]; then
            add "$_root" "$_collection" "$_subject" "$_json"
        fi
        _n=$((_n + 1))
    done
}

seed() {
    # The seven sources are the "sources" phase of entries.tsv, in file order;
    # each entry's payload is its rendered sources: sub-block (name/url|file/read).
    _root=$1
    {
        echo "meta:"
        echo "  scope: Six-paper worked example; selected source readings and an authored synthesis, not a complete review or live research-agent evaluation."
        echo "sources:"
        awk -F'\t' '$1 == "sources"' "$ENTRIES" | while IFS='	' read -r _phase _collection _subject _body; do
            printf '  %s:\n' "$_subject"
            printf '%b\n' "$_body" | sed 's/^/    /'
        done
        echo "known:"
        echo "judgments:"
        echo "open:"
    } > "$_root/GROUNDING.yaml"
    cp "$HERE/README.md" "$_root/README.md"
}

demonstrate() {
    _root=$1
    seed "$_root"

    # Overlap three separate CLI writers on the same file. The supported
    # writer lock serializes each mutation; the research tasks run concurrently.
    add_findings_worker "$_root" 0 3 &
    add_findings_worker "$_root" 1 3 &
    add_findings_worker "$_root" 2 3 &
    wait

    add_phase "$_root" frame
    add_phase "$_root" derived
    add_phase "$_root" judgments
    add_phase "$_root" open

    _record="$_root/GROUNDING.yaml"

    # Verify every prepared finding and frame reading against the record kpop
    # actually wrote, not against entries.tsv's own row count: a concurrent
    # write that silently dropped an unreferenced reading must fail here.
    _expected_known=$(awk -F'\t' '$1 == "findings" || $1 == "frame" || $1 == "derived" {print $3}' "$ENTRIES" | sort)
    _actual_known=$(awk '
        /^known:$/ { inknown = 1; next }
        /^judgments:$/ { inknown = 0 }
        inknown && /^  [^ :]+:$/ { line = $0; sub(/:$/, "", line); sub(/^  /, "", line); print line }
    ' "$_record" | sort)
    [ "$_expected_known" = "$_actual_known" ] \
        || fail "the record's known entries do not match the prepared findings and frame readings"

    # Exact value match for every numeric reading: an unambiguous, single-line
    # comparison between what was prepared and what the record now holds.
    awk -F'\t' '$1 == "findings" || $1 == "frame"' "$ENTRIES" | while IFS='	' read -r _phase _collection _subject _json; do
        case "$_json" in
            *'"v":'\ [-0-9]*)
                _expected_v=$(printf '%s' "$_json" | sed -E 's/.*"v": *(-?[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?).*/\1/')
                _actual_v=$(awk -v s="$_subject" '
                    /^known:$/ { inknown = 1; next }
                    /^judgments:$/ { inknown = 0 }
                    inknown && /^  [^ :]+:$/ { line = $0; sub(/:$/, "", line); sub(/^  /, "", line); cur = line; next }
                    inknown && cur == s && /^    v: / { v = $0; sub(/^    v: /, "", v); print v; exit }
                ' "$_record")
                [ "$_actual_v" = "$_expected_v" ] \
                    || fail "recorded value for $_subject: got [$_actual_v], expected [$_expected_v]"
                ;;
        esac
    done

    # Verify every judgment's seen snapshot against the record: the set of
    # premises it actually captured must equal the set it was declared to
    # rest on, read back from the record, not assumed from entries.tsv.
    awk '
        /^judgments:$/ { injudg = 1; next }
        /^open:$/ { injudg = 0 }
        injudg && /^  [^ :]+:$/ {
            if (subj != "") print subj "\t" rests_on "\t" seenkeys
            line = $0; sub(/:$/, "", line); sub(/^  /, "", line)
            subj = line; rests_on = ""; seenkeys = ""; inseen = 0
            next
        }
        injudg && /^    rests_on: \[/ {
            line = $0
            sub(/^    rests_on: \[/, "", line); sub(/\]$/, "", line); gsub(/ /, "", line)
            rests_on = line
            next
        }
        injudg && /^    seen:$/ { inseen = 1; next }
        injudg && inseen && /^      [^ :]+:/ {
            key = $0
            sub(/^      /, "", key); sub(/:.*/, "", key)
            seenkeys = seenkeys (seenkeys == "" ? "" : ",") key
            next
        }
        END { if (subj != "") print subj "\t" rests_on "\t" seenkeys }
    ' "$_record" | while IFS='	' read -r _judgment _rests_on _seen; do
        _sorted_rests_on=$(printf '%s' "$_rests_on" | tr ',' '\n' | sort | tr '\n' ',')
        _sorted_seen=$(printf '%s' "$_seen" | tr ',' '\n' | sort | tr '\n' ',')
        [ "$_sorted_rests_on" = "$_sorted_seen" ] \
            || fail "$_judgment: seen does not snapshot exactly its declared premises
rests_on: $_rests_on
seen:     $_seen"
    done

    _findings=$(awk -F'\t' '$1 == "findings"' "$ENTRIES" | wc -l | tr -d ' ')
    _judgments=$(awk -F'\t' '$1 == "judgments"' "$ENTRIES" | wc -l | tr -d ' ')
    echo "Three concurrent writers: all $_findings findings from six papers retained."
    echo "$_judgments judgments: every exact premise captured in seen."

    _omega_c=$(awk -F'\t' '$1 == "findings" && $3 == "cmb.omega_c_h2"' "$ENTRIES" | sed -E 's/.*"v": *([0-9.]+).*/\1/')
    _omega_b=$(awk -F'\t' '$1 == "findings" && $3 == "cmb.omega_b_h2"' "$ENTRIES" | sed -E 's/.*"v": *([0-9.]+).*/\1/')
    _rule_expr=$(awk -F'\t' '$1 == "derived" && $3 == "cmb.dark_to_baryon_density"' "$ENTRIES" | sed -E 's/.*"expr": *"([^"]*)".*/\1/')
    _exact_ratio=$(exact_ratio "$_omega_c" "$_omega_b")

    # Re-assert the density computation against what the record's own rule
    # engine computed, not a value derived from entries.tsv alone: the exact
    # reduced fraction and the formula that produced it must both appear.
    _density_pull=$("$KPOP" --workspace "$_root" pull cmb.dark_to_baryon_density --budget 400)
    case "$_density_pull" in
        *"cmb.dark_to_baryon_density: $_exact_ratio = $_rule_expr"*) ;;
        *) fail "the recorded density computation does not match the prepared rule or its exact result (expected $_exact_ratio = $_rule_expr):
$_density_pull" ;;
    esac

    awk -v c="$_omega_c" -v b="$_omega_b" \
        'BEGIN{printf "Density ratio from rounded central values: %.6f; not independent evidence.\n", c/b}'

    "$KPOP" --workspace "$_root" check

    echo
    echo "Read the connected argument:"
    "$KPOP" --workspace "$_root" pull synthesis.dark_matter --budget 2000

    # A separate copy asks a different review question. No published result is
    # altered and the assembled record remains available as the original example.
    _trial="$_root/scope-change"
    mkdir "$_trial"
    cp "$_root/GROUNDING.yaml" "$_trial/GROUNDING.yaml"
    cp "$_root/README.md" "$_trial/README.md"
    _original=$(record_sha "$_trial/GROUNDING.yaml")
    _changed=$("$KPOP" --workspace "$_trial" set research.framework \
        "Assess explanations without adopting base Lambda-CDM for the CMB result." \
        --why "Hypothetical change in review scope; no paper finding has changed." \
        --hypothesis scope_change)
    case "$_changed" in
        *"MOVED"*"synthesis.dark_matter"*) ;;
        *) fail "the changed review scope did not flag the synthesis
$_changed" ;;
    esac
    [ "$(record_sha "$_trial/GROUNDING.yaml")" = "$_original" ] \
        || fail "a hypothetical scope change rewrote the base record"
    echo
    echo "Hypothetical scope change: stop adopting base Lambda-CDM."
    echo "$_changed"
    echo "MOVED under the hypothesis asks for review; the base and paper findings are untouched."
}

if [ -n "$OUTPUT" ]; then
    [ -e "$OUTPUT" ] && fail "$OUTPUT already exists"
    mkdir -p "$OUTPUT"
    demonstrate "$(CDPATH= cd -- "$OUTPUT" && pwd)"
    echo
    echo "Saved example: $OUTPUT/GROUNDING.yaml"
else
    TMP=$(mktemp -d "${TMPDIR:-/tmp}/kpopper-dark-matter-XXXXXX")
    trap 'rm -rf "$TMP"' EXIT INT TERM
    demonstrate "$TMP"
fi
