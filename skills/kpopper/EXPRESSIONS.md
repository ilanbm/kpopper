# Readable calculations and falsifiers

Store a new calculation or executable condition as `{expr: "formula"}`. The primary agent
chooses its meaning; a bounded deterministic parser derives its structure and references,
and the packaged Lean core computes it. No model call or arbitrary code execution is
involved in evaluation. The stored expression remains the source of truth.

`add` and source-report `update` also accept a supported formula as text, such as
`rule='order.price * order.quantity'` or `wrong_if='order.price * order.quantity > 150'`.
The normal writer stores the corresponding `expr` expression when Lean can validate
its result. This includes exact decimal arithmetic and references in the record's original
language. Within `expr`, a bare name is a reference, a quoted string is literal text,
and `ref("an opaque id")` explicitly references a reserved name or an ID containing spaces
or punctuation. Calls other than this reference constructor are unsupported.

Unsupported syntax, date-like rules, numeric-looking quoted literals and comparisons
depending on legacy text coercion stay text with a diagnostic. Missing Lean also produces
an explicit fallback diagnostic; it never turns text into a calculated result. Report
receipts retain these diagnostics. Existing formulas and historical `seen` fields are
not migrated by an unrelated write. A newly authored calculation with a missing dependency,
cycle or division by zero is refused unless its missing computation is explicitly declared.

```yaml
order.total:
  rule: {expr: "order.price * order.quantity"}

c.affordable:
  rests_on: [order.total]
  verdict: The order fits the budget
  wrong_if: {expr: "order.total > 150"}
```

This is an authoring example: `add` and batch ingestion fill `seen` from the record.
Do not manually assign a current value to a `rule` entry. The rule is stored once;
display text and dependency links are derived from it.

## Expression grammar, version 1

Readable formulas support `+`, `-`, `*`, `/`, unary minus, parentheses, exact decimal and
exponent constants, quoted text, booleans, and references. Falsifiers have one comparison:
`==`, `!=`, `<`, `<=`, `>`, or `>=`, with arithmetic permitted on either side. Each `expr`
mapping has exactly one field and at most 4000 characters. There are no assignments,
arbitrary calls, indexing, comprehensions, or compound logical conditions.

The old tagged tree format remains supported. The parser lowers readable formulas to this
same format for Lean; it is not stored beside the source as a second editable formula.
These tagged nodes remain useful for constructing expressions programmatically:

| Form | Meaning |
|---|---|
| `{ref: order.price}` | Read this exact recorded ID. |
| `{num: "0.1"}` | An exact decimal constant, encoded as a string. |
| `{text: "order.price"}` | Literal text; this creates no reference. |
| `{bool: false}` | A boolean literal, distinct from numeric zero. |
| `{op: mul, args: [LEFT, RIGHT]}` | Apply a supported binary operator. |

Rules support `add`, `sub`, `mul`, `div` and nested expressions. A falsifier has a comparison
at its root: `eq`, `ne`, `lt`, `le`, `gt`, `ge`; its two arguments may contain arithmetic.
It must read at least one reference, and every directly read reference must be declared
in `rests_on`. A conclusion may declare additional premises that its falsifier does not
read. Calculation dependencies come from tagged `ref` leaves, including missing references,
so identifiers inside literal text never create links.

Arithmetic uses exact rational numbers. Integers appear as ordinary numbers; non-integral
results use `{rational: ["numerator", "denominator"]}` with reduced integer strings and a
positive denominator. For example, `0.1 + 0.2` returns `3/10`, and `1/3` stays `1/3`.
There is no implicit rounding, text-to-number conversion or unit conversion. Ordinary
recorded numbers retain their existing representation; use `num` strings for exact
constants rather than passing long decimals through floating-point tooling.

Unknown operators, extra fields, incompatible types, missing inputs, cycles and division
by zero never yield a fabricated value. Unknown results cannot satisfy a true or false
assertion. Expressions are bounded to 64 parse levels; evaluation has a depth bound and
memoizes shared references. Numeric tokens are limited to 512 characters and bounded
decimal exponents, and computed numerators/denominators to 1024 decimal characters.

## Computation and review snapshots

The local core is required for structured calculations. Before authoring the first one,
run `kpopper session status`. If it is not ready, make Lean **4.33.1** available and run
`kpopper session setup` (or pass `--lean-root /path/to/toolchain`). Both commands work with
the base Python package; session transport extras are needed only for checked session views.
Setup compiles the pinned source once. Reads never download or compile it, and there is no
second arithmetic evaluator. After a package update changes the core source, run setup again.

A missing, invalid or incompatible core makes structured calculations explicitly unavailable.
Conditions that need them read `UNKNOWN`; ordinary scalar reads, independent writes and
independent followups still work. Writing or reviewing a judgment that needs a calculated
snapshot remains refused until it can actually be computed. `check` reports an unavailable
core for rules or conditions as a failure unless they explicitly declare why they are
blocked; success is not a substitute for a missing computation.

`pull`, source search, the page and checked sessions use the same calculation semantics.
The Python layer derives references and display text and calls Lean for arithmetic. Results
are cached by record content, expression and core/parser identity; changing an input
invalidates the result. A bounded per-process cache holds up to 512 parsed formulas, keyed
by their source and grammar version. Changing values reuses the parsed formula. Changing
parser code invalidates a live checked session; restart it to use the new program.

When a judgment is written or explicitly reviewed, a calculated premise gets a snapshot:

```yaml
seen:
  order.total:
    computed:
      value: 100
      rule: {expr: "order.price * order.quantity"}
```

The historical result is not substituted for the current calculation. A changed formula
also reopens review when it happens to produce the same number. Whitespace, redundant
parentheses, and equivalent tagged/readable representations are not formula changes.
Existing snapshots remain
intact until a person or agent actually reviews the judgment. Correct arithmetic does not
establish that the source readings or the chosen model describe the world correctly.
Snapshots are independent historical copies; do not make their formulas YAML aliases of
the current formula, since editing such an alias would also rewrite the supposed history.

## Existing expressions

An undecidable condition is now flagged `unknown`, including a condition over an old
textual rule with no numeric result. This changes attention counts: `graph.flagged`,
the page's flagged shape and, when such a judgment is omitted, `page.spill` can increase
without a source edit. Existing layout judgments and saved page shapes may therefore
need review. `unknown` is not a falsified condition, and historical `seen` values are
never refreshed automatically. Explicitly migrate supported formulas or explain the
missing reading; verify the page before reviewing its layout conclusions.

Existing textual rules stay readable and unevaluated. Existing supported textual predicates
continue to work, including comparisons against newly computed structured rules. To get a
structured expression without writing anything:

```sh
kpopper expressions convert 'order.price * order.quantity'
kpopper expressions convert 'order.total > 150' --predicate
kpopper expressions convert 'order.price * order.quantity' --readable
```

To preview a record migration, then apply a clean result:

```sh
kpopper expressions migrate --record GROUNDING.yaml
kpopper expressions migrate --record GROUNDING.yaml --apply
```

Migration uses a deterministic grammar, never a model. It converts supported rules and
predicates, including legacy expressions in `v`, and reports unconverted fields. Unknown
names, unsupported syntax, ambiguous bare predicate operands and ambiguous escaped literals
stay unchanged for review. It does not refresh snapshots. The whole single-file result is
checked before one atomic replacement; new check failures prevent application. Every
previously decidable condition must keep its result, so numeric text that the legacy
reader coerced cannot silently become an unknown typed comparison. Record a typed numeric
reading with its source explicitly when that is what the source means.

A historical snapshot containing only a textual formula has no historical calculated value.
After an equivalent conversion it reads `UNCHECKED`, with the original snapshot preserved,
until an explicit review captures the current result. It is not reported as a numeric move.
Pointer, multi-file and hypothesis-backed records require explicit authoring instead.

To convert active tagged trees and supported legacy text to the readable format, preview
`kpopper expressions migrate --record GROUNDING.yaml --readable`, then add `--apply` to
apply a clean result. Existing readable formulas and historical snapshots stay untouched.
The conversion must round-trip to the same parsed structure; reserved or opaque reference
names use `ref("...")`. Existing source-value types and decidable conditions keep their
meaning. An unrelated write never migrates existing formulas.
