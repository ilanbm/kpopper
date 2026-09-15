# Experimental deterministic core

`core/v1` is an explicit, read-only assessment profile. Existing commands keep
their legacy interpretation until the remaining consumers are integrated.

```sh
kpopper assess m.total d.order --profile core/v1 --record example.yaml
```

```yaml
meta:
  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}
parameters:
  p.price: {v: 10}
  p.quantity: {v: 3}
calculations:
  m.total: {rule: {expr: 'p.price * p.quantity'}}
decisions:
  d.order:
    rests_on: [m.total]
    wrong_if: {expr: 'm.total > 100'}
    seen: {m.total: 30}
```

The total is 30 and the falsifier does not hold. The result retains the original
body, field mapping, historical review, potential dependencies, actual reads and
independent integrity/contention findings. An unavailable executor produces an
operational error, never `false`. Prose remains a declared unknown; it is not
silently promoted to an executable condition.

This profile currently supports explicit scalar literals, references, exact
arithmetic and comparisons. Boolean composition, conditional expressions and
queries are separate planned capabilities. Unknown required modules refuse
dependent interpretation. No record text can load code or change the installed
module registry. Authoring and migration are not activated by this reader slice.

The new assessment envelope has `schema_version: 2` and the schema at
`scripts/reasoning/assessment.schema.json`. Existing ordinary assessments retain
their original v1 schema. `checked-reader/v1` names the existing checked-session
semantics for compatibility documentation; older checked-session responses did
not carry that identifier. Missing metadata keeps the existing surface's legacy
interpretation. An explicit `--profile core/v1` override does not rewrite a record.

Numeric constants keep their decimal lexemes and evaluate as exact rationals.
Quoted numbers remain text; booleans, numeric zero, null and missing inputs remain
distinct. A YAML float has already lost its original lexeme: the adapter uses its
round-trip decimal representation, not a claim of recovered precision.

The default resource profile bounds 20,000 nodes, 100,000 potential edges,
1,000,000 expression steps, evaluator depth 128 and 256-digit rational parts.
Limits are explicit results. These differ from the legacy checked executor's
64-level expression parser and 1024-digit numeric bound. Selecting a profile can
therefore change a limit result; installing an executor does not migrate meaning.

## Snapshot and extension API

```python
from kpopper.reasoning.snapshot import Snapshot
from kpopper.reasoning.evaluate import evaluate

snapshot = Snapshot.capture(['example.yaml'], as_of='2026-09-15')
result = evaluate(snapshot, {'expr': 'm.total / 3'}, declared=['m.total'])
```

Capture binds source closure, project routing/generation, live/frozen mode,
pinned pending contributions, typed conflicts, target observations and an
explicit `as_of`. No clock or network read occurs inside evaluation. The
normalized snapshot identity is separate from the authored file-hash revision.
Returned data is detached. Historical `seen` is evidence, never current input.

Generated basis fingerprints bind rules and their transitive inputs, including
changes whose numeric effects cancel. Missing old basis is `not_recorded`, not
an automatic request to review. New history is not written by assessment.

An entry can define `collection_scope: {collection: parameters, fields: [v]}`.
Its members are derived from the captured collection. `snapshot.capture_scope(id)`
returns member count, complete membership/projected-field witnesses and a limited
view. `SnapshotView` grants exact nodes and fields and refuses undeclared reads.
The separate `scope` field retains its existing contribution-provenance meaning.

First-party module preparation receives a limited snapshot view and normalized
IR; the compiled registry computes the result. Arithmetic consumes this boundary
today. Module/version definitions live in `reasoning.contract` and
`reasoning.modules`. Rendering depends on value/witness types, not operator names.
This API is experimental until the finite-query extension and consumer gates pass.

## Runtime, licensing and assurance

The package and plugin carry the same platform archives. Extraction is automatic
and offline; ordinary computation needs no Lean compiler or session setup. The
maintainer builder compiles pinned sources, checks proofs, audits native linkage
and builds replaceable GMP shared libraries. Supported target evidence and build
instructions are in `scripts/reasoning/native/README.md`.

The executable and original archive are hash-bound to runtime sources. GMP can be
replaced with an ABI-compatible modified library; its actual hash is disclosed in
implementation metadata. Full notices and the exact GMP source/build archive ship
alongside the binaries. See the included LGPLv3/GPLv3 and other license texts.

`Kpopper.Proof.evaluate_closedRat_sound` proves conditional successful denotation
and unchanged memo/read sets for closed rational arithmetic in the actual total
Lean evaluator, including nonzero division. `evaluate_literal_success` supplies a
non-vacuous successful case under stated limits. The audited dependencies are
`propext`, `Classical.choice` and `Quot.sound`.

These theorems do not prove parsing, references, adapters, snapshots, native code
generation, source-world truth or arbitrary prose. Reference-bearing results are
computed findings, not proof certificates. The Python/native transport and the
installed executable have behavioral tests in addition to the named proofs.
