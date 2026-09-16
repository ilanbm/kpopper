# Experimental deterministic core

`core/v1` is an explicit assessment and authoring profile. Existing ordinary
readers keep their legacy interpretation until their consumers are integrated.

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
module registry. A read never activates authoring or migrates stored content.

The new assessment envelope has `schema_version: 2` and the schema at
`scripts/reasoning/assessment.schema.json`. Existing ordinary assessments retain
their original v1 schema. `checked-reader/v1` names the existing checked-session
semantics for compatibility documentation; older checked-session responses did
not carry that identifier. Missing metadata keeps the existing surface's legacy
interpretation. An assessment's explicit `--profile core/v1` override does not
rewrite a record.

## Explicit authoring and review history

`add`, `set` and `review` accept `--profile core/v1`. A declared core record also
selects this writer when the flag is omitted. Missing metadata without the flag
retains legacy authoring. Supported newly authored formulas are stored as
`{expr: "original text"}` based on syntax and resolved intent, independently of
legacy checked-session setup. A broken packaged runtime refuses a computation
or required review before changing the record; it cannot turn the formula into prose.

```sh
kpopper add m.double 'rule=m.total * 2' --profile core/v1 example.yaml
kpopper review d.order --as-of 2026-09-17 example.yaml
```

New core writes declare record metadata version 2, with the unchanged semantic
profile `core/v1` and required module `arithmetic/v1`. Metadata version 1 remains
readable. Existing executable legacy fields require explicit migration before
record-wide promotion; a flag does not reinterpret them. New core history uses
`seen.<dependency>.computed` with `version: 2`, a canonical typed `value`, the
complete actual `basis`, and the original `rule` when applicable. The record's
mapped snapshot field is respected. Unrelated writes preserve old history, and
missing historical basis is never backfilled automatically.

The writer's `--as-of` is a recording date. It does not bind an ambient date into
computations: normal review basis uses semantic `as_of: null`, matching default
assessment on later days. Successful null, false, zero and exact fractions retain
their distinct typed values. Scope history records member count, sorted member
IDs, all projected fingerprints and the full scope witness under basis version 1,
recipe `scope-inputs/v2`. Equal member counts do not hide changed membership.
Complete generated history is subject to the output budget; it is never truncated.

Atomic report ingestion accepts `profile: core/v1` in its envelope and selects
core automatically for declared records. It normalizes and validates all changes
in the final staged world, including forward references, before publishing one
record replacement. Its internal assessment gate and recovery evidence distinguish
introduced integrity failures from an unchanged judgment whose falsifier newly
fires after an input update. Existing single-file, source-permission and primary
review restrictions still apply.

Older readers that only support metadata version 1 refuse the newer record
format. Upgrade readers before sharing a newly authored core record; metadata
cannot protect it from already released writers that ignore capabilities.
Ordinary check/page readers remain unavailable for declared core records. Use
`assess --profile core/v1` to inspect their current findings.

Computed reader/page builtins are not core inputs. New core calculations or
reviews requiring them refuse with `unsupported_core_builtin`; arrangement and
brief-section reviews retain that restriction. Specialized legacy mutations
(`same`, `distinct`, consolidation, refutation and shared-watch apply) still
refuse declared core records before changing them. Explicit migration remains a
separate operation; a profile flag does not convert existing executable fields.

Project contribution capture uses manifest version 2. It binds the selected
document's profile, required modules, complete dependency and scope closure, and
portable evidence hashes. Legacy content has an explicit `ordinary-reader/v1`
identity in its manifest and no new record declaration. Existing version 1
bundles and receipts keep their original identities and stored closures.

Materialization and publication validate the declaration again. Supported core
metadata versions 1 and 2 share meaning when bodies, schema, scopes and required
modules match; legacy and core profiles always remain distinct. A target merge
retains core metadata and validates previously accepted active contributions.
Unknown requirements can remain in immutable archival evidence, but cannot be
interpreted, materialized as a supported record or published.

An accepted contribution still participates in the live overlay until explicitly
retired. A profile change refuses incompatible active revisions with
`pending_profile_reconciliation_required`. Explicit withdrawal names the exact
revision and a reason; it preserves its bundle and prior acceptance receipts.
Resuming it checks compatibility with the current record before reactivating it.

Numeric constants keep their decimal lexemes and evaluate as exact rationals.
Quoted numbers remain text; booleans, numeric zero, null and missing inputs remain
distinct. A YAML float has already lost its original lexeme: the adapter uses its
round-trip decimal representation, not a claim of recovered precision.

The default `resources/v2` profile bounds 20,000 nodes, 100,000 potential edges,
1,000,000 expression steps in each of type preflight and evaluation separately,
evaluator depth 128 and 256-digit rational parts. `cost.preflight_steps` counts
expressions entered during type checking, including references to cached types;
`cost.steps` and `executed_reads` count only actual evaluation. If preflight
exhausts the step budget, evaluation has zero steps and no executed reads.
Potential witnesses remain complete on that failure. The resource version
binds this accounting change in computation identity.
Limits are explicit results. These differ from the legacy checked executor's
64-level expression parser and 1024-digit numeric bound. Selecting a profile can
therefore change a limit result; installing an executor does not migrate meaning.

Operational limits are separate from arithmetic meaning and appear in results
and assessment reports: 30 seconds per native batch, 1,000 requests (and at most
1,000 selected assessment entries), 16 MiB aggregate native input and 64 MiB
aggregate native output, including stderr. Evaluation envelopes and assessment
reports also have a 64 MiB compact ASCII JSON budget, counting every occurrence
of shared evidence. Requests are checked as they are prepared and report entries
as they are assembled, before retaining more closures. Output is never clipped.
An oversized batch/report raises `OperationalLimit` (`batch_request_limit`,
`batch_input_limit`, or `output_limit`); the CLI reports the refusal. Select fewer
entries to continue. A native timeout returns `operational_error` with diagnostic
`runtime_timeout`; loading or process failures use `runtime_unavailable`.
Neither failure is a semantic false value or a cached arithmetic negative.

Python callers can lower these bounds with `operational_limits={...}` on
`evaluate`, `Evaluator`, `assess` or `Runtime`; the keys are `timeout_seconds`,
`batch_requests`, `input_bytes` and `output_bytes`. Operational settings do not
change `computation_id`. Actual reads carry the same `{kind, id, fingerprint}`
witness as the corresponding potential dependency, binding transitive input
identity even when changed inputs produce the same value.

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

Comparison targets retain their declared reasoning version, profile and required
modules. Before using target entries, the reader checks the target and all its
hypotheses. Unsupported or malformed requirements make the target unavailable.
The ordinary reader still refuses declared core targets; missing metadata keeps
ordinary-reader/v1 semantics.

Supplied snapshots and live pending contributions retain their existing
contention policies. In a supplied snapshot, two disagreeing hypothesis claims
are contested; a single alternative remains a proposal. Live pending reads also
compare proposals against the checkout and locally observed target, so one
proposal differing from the checkout can be contested. Neither path adopts an
alternative as a base fact.

Generated basis fingerprints bind rules and their transitive inputs, including
changes whose numeric effects cancel. Missing old basis is `not_recorded`, not
an automatic request to review. New history is not written by assessment.

An entry can define `collection_scope: {collection: parameters, fields: [v]}`.
Its members are derived from the captured collection. `snapshot.capture_scope(id)`
returns member count, complete membership/projected-field witnesses and a limited
view. `SnapshotView` grants exact nodes and fields and refuses undeclared reads.
The separate `scope` field retains its existing contribution-provenance meaning.
Scopes cannot grant the mapped historical snapshot field, `assessment`, or
`current_assessment`, including when the collection is empty.

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
