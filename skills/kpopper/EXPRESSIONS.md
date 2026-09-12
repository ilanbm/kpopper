# Structured calculations and falsifiers

Store new calculations and executable conditions as structured data. The primary agent
chooses the formula and its meaning; the writer validates its structure and the packaged
Lean core computes it. No model call or arbitrary code execution is involved in evaluation.

```yaml
order.total:
  rule:
    op: mul
    args:
      - ref: order.price
      - ref: order.quantity

c.affordable:
  rests_on: [order.total]
  verdict: The order fits the budget
  wrong_if:
    op: gt
    args:
      - ref: order.total
      - num: "150"
```

This is an authoring example: `add` and batch ingestion fill `seen` from the record.
Do not manually assign a current value to a `rule` entry. The rule is stored once;
display text and dependency links are derived from it.

## Expression grammar, version 1

Each node has exactly the fields shown:

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

Build the optional local core once using `kpopper session setup`. This compiles the pinned
Lean source; reads never download or compile it. A missing or invalid core makes structured
calculations explicitly unavailable. Legacy literal reading still works without Lean.

`pull`, source search, the page and checked sessions use the same calculation semantics.
The Python layer derives references and display text and calls Lean for arithmetic. Results
are cached by record content, expression and core identity; changing an input invalidates
the result.

When a judgment is written or explicitly reviewed, a calculated premise gets a snapshot:

```yaml
seen:
  order.total:
    computed:
      value: 100
      rule: {op: mul, args: [{ref: order.price}, {ref: order.quantity}]}
```

The historical result is not substituted for the current calculation. A changed formula
also reopens review when it happens to produce the same number. Existing snapshots remain
intact until a person or agent actually reviews the judgment. Correct arithmetic does not
establish that the source readings or the chosen model describe the world correctly.
Snapshots are independent historical copies; do not make their formulas YAML aliases of
the current formula, since editing such an alias would also rewrite the supposed history.

## Existing expressions

Existing textual rules stay readable and unevaluated. Existing supported textual predicates
continue to work, including comparisons against newly computed structured rules. To get a
structured expression without writing anything:

```sh
kpopper expressions convert 'order.price * order.quantity'
kpopper expressions convert 'order.total > 150' --predicate
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
checked before one atomic replacement; new check failures prevent application. Pointer,
multi-file and hypothesis-backed records require explicit authoring instead.
