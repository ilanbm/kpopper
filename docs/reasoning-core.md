# Experimental deterministic core

`core/v1` is an explicit assessment and authoring profile. Existing ordinary
records keep their legacy interpretation. Select the profile explicitly for
the core readers described below.

```sh
kpop assess m.total d.order --profile core/v1 --record example.yaml
kpop assess m.total d.order --profile core/v1 --history --record example.yaml
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

The profile remains explicit and default-off. Its scalar fragment supports
literals, references, exact arithmetic and comparisons through `arithmetic/v1`.
Records may additionally declare `composition/v1` for finite typed values and
composable conditions. Finite-scope query and member selection are provided by
the additive `query/v1` extension described in [Query operations](query.md).
Unknown required modules refuse dependent interpretation. No record text can
load code or change the installed module registry. A read never activates
authoring or migrates stored content.

The new assessment envelope has `schema_version: 2` and the schema at
`scripts/reasoning/assessment.schema.json`. Existing ordinary assessments retain
their original v1 schema. `checked-reader/v1` names the existing checked-session
semantics for compatibility documentation; older checked-session responses did
not carry that identifier. Missing metadata keeps the existing surface's legacy
interpretation. An assessment's explicit `--profile core/v1` override does not
rewrite a record.

The optional `--history` route returns combined `schema_version: 3`, defined by
`scripts/reasoning/history_assessment.schema.json`. It computes schema v2 once from one
immutable Snapshot, then adds captured history subjects, pins, coverage, assurance and support
reservations without a second evaluator or source read. `findings_revision` is the semantic
identity; `envelope_revision` also binds policy and display selection. Existing core v2 and both
legacy reader profiles remain supported. The same captured context is available explicitly to:

```sh
kpop open --profile core/v1 example.yaml
kpop check --profile core/v1 example.yaml
kpop pull d.order --profile core/v1 example.yaml
kpop affects m.total --profile core/v1 example.yaml
kpop export d.order --profile core/v1 --record example.yaml
kpop search "order" --profile core/v1 --record example.yaml
kpop experimental hub --profile core/v1 --out core.html example.yaml
kpop session open --assessment-profile core/v1 --input example.yaml
```

These routes do not activate or migrate a record. They fail closed on unsupported legacy-only
options and preserve unknown/error independently from false. The core session handle binds the
project identity, Snapshot, findings and consumer-view version; follow-up reads reuse its retained
source-free context and recapture only to reject staleness.

`consolidate`, watch compatibility/uncertainty, followups, and `remeasure` remain explicit
legacy-only operations with this experimental profile. A declared core record fails them closed with
`unsupported_capability: use core/v1 consumer`; they never fall back to the legacy evaluator.
Use the shared read consumers above for current findings. Activation remains blocked while these
operation-specific contracts are not migrated.

## Composable conditions

Composition is an additive capability of `core/v1`, not a new profile. A record
that authors a composed expression or stores a list or record declares the
sorted capability union:

```yaml
meta:
  reasoning:
    version: 2
    profile: core/v1
    requires: [arithmetic/v1, composition/v1]
known:
  p.ready: {v: true}
  p.details: {v: {region: il, tags: [priority, null]}}
calculations:
  m.selected:
    rule: {expr: 'field(p.details, "tags") if p.ready and not false else []'}
```

The authored syntax accepts `and`, `or`, `not`, `A if C else B`, list and
string-keyed dictionary literals, and `field(EXPR, "literal key")`. Boolean
chains lower left-to-right. Empty and heterogeneous containers are valid.
`p.details` remains an opaque reference ID; attribute syntax is never field
access. Use `ref("opaque.id")` for IDs that cannot be written as an attribute.
Dynamic keys, indexing, comprehensions, spreads, calls other than `ref` and
`field`, assignment and implicit coercion are refused.

The equivalent data-only nodes are:

```text
{op: "and"|"or", args: [boolean, boolean]}
{op: "not", args: [boolean]}
{if: condition, then: expression, else: expression}
{list: [expression, ...]}
{record: {"literal key": expression, ...}}
{field: recordExpression, key: "literal key"}
```

Static validation covers every child before execution. Potential dependencies
therefore include both conditional branches and every boolean operand or
container member. A potential cycle in a composed closure refuses the request
before evaluation. Executed reads remain separate: a known conditional executes
only its selected branch; an unknown guard executes neither branch. An
unselected branch contributes no runtime read or runtime diagnostic.

`and` and `or` evaluate both operands, left then right. They use three-valued
dominance: `false and unknown` is known false, `true or unknown` is known true,
and `not unknown` is unknown. The same dominant results can survive an evaluated
runtime error, but the diagnostic is retained. Without a dominant truth value,
an error takes precedence over unknown; `not error` and an error guard are
errors. Resource limits and operational failures are not truth values and cannot
be dominated.

Lists evaluate in order and records in sorted key order. Any member error makes
the complete container an error; otherwise any unknown member makes it unknown.
No partial container value is returned, and all member diagnostics remain.
`field` requires a complete record value. An executed absent key returns unknown
with `missing_field`; a statically known non-record target is a type error. An
unselected missing field emits no runtime diagnostic. Exact equality is typed:
lists compare in order and records by canonical key/value pairs. Different list
lengths or record key sets are known unequal; equal shapes require recursively
comparable members. Ordering remains exact-number only.

The public result keeps potential and executed evidence distinct. For example,
an abridged successful composed result has this shape:

```yaml
modules: [arithmetic/v1, composition/v1]
status: ok
value:
  type: list
  items:
    - {type: text, value: priority}
    - {type: null}
diagnostics: []
potential_ids: [p.details, p.ready]
executed_reads:
  - {kind: node, id: p.details, fingerprint: <input-basis-digest>}
  - {kind: node, id: p.ready, fingerprint: <input-basis-digest>}
resource_profile:
  version: resources/v3
  steps: 1000000
  depth: 128
  digits: 256
  value_nodes: 10000
  value_depth: 128
  value_bytes: 16777216
implementation: {protocol: KP3}
```

Diagnostics can accompany a known dominant boolean. Read-only evaluation keeps
that supported envelope, but normal authoring refuses any diagnostic. An
explicitly blocked write admits only `missing_reference` and `missing_field`,
and only when every diagnostic is one of those missing-data codes.

Protocol selection is closure-local. A request whose root or potential closure
contains a composed node or stored container uses KP3/KR3 and `resources/v3`.
A scalar closure continues to use the byte-compatible KP2/KR2 and
`resources/v2`, even inside a record that declares `composition/v1`; unrelated
composition therefore does not change its basis. A target lacking
`composition/v1` returns `unsupported_capability` for dependent interpretation
instead of partially evaluating the tree. Retained declarations are unioned and
never dropped by a later scalar edit.

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

New core writes declare record metadata version 2 with the unchanged semantic
profile `core/v1`. Scalar writes require `arithmetic/v1`; composed expressions
and stored containers retain it and add `composition/v1`. Metadata version 1 remains
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

## Explicit migration

Preview a complete conversion before writing a new directory:

```sh
kpopper expressions migrate --record example.yaml --profile core/v1 --destination candidate --json
kpopper expressions migrate --record example.yaml --profile core/v1 --destination candidate --apply --json
```

The copy retains record shards and pointers, named hypotheses, layout sidecars,
local referenced evidence, original pending contributions and captured target
observations. It never runs measurement recipes or grants publication permission.
Unresolved pointers, unreadable files, mixed layouts and ambiguous conversions
block a complete copy. External evidence locators remain locators.
Local evidence must name explicit files; directory locators are refused. Retired
unsupported contributions remain immutable evidence and are marked incompatible,
without being interpreted as active core facts.

The `.kpopper-migration` directory contains exact originals, the conversion
receipt, and typed `original.json` and `candidate.json` snapshots. Replay them
with `Snapshot.from_json`; the candidate evaluates the transformed document
against its captured context without looking up current refs or configuration.
Historical `seen` values remain unchanged. Known legacy results must preserve
type, value and availability. Newly executable formulas and conditions are
reported explicitly; an unavailable legacy runtime is not equivalence evidence.

Without `--destination`, `--apply` is allowed only when one canonical file changes
and the remaining captured closure stays unchanged. It retains a backup and
rechecks source bytes, configuration and pending state before replacing the file.
Copies publish with one atomic rename into an absent destination; a late-created
empty directory is not overwritten.

A copy does not change the live record route. `kpopper config --record PATH
--migration-receipt candidate/.kpopper-migration/receipt.json --check --json`
previews a separate routing change; omit `--check` to apply it. The transition
recomputes conversion evidence under the existing locks. Unsettled contributions,
unreconciled hypotheses, unavailable destinations and incompatible worktrees still
block it. Advanced destinations must be repository-relative and prepared with
compatible captured bytes in every participating worktree.
The destination must be the copied entry record, including its complete shard
closure. Portable snapshot identity binds publication content and dispositions;
repeated verification times and retry counters do not change that identity.
Private capture checks still detect changes during copying.

An exact restoration uses the original record path and the same receipt with
`--rollback`. It verifies the retained originals and recomputes the conversion;
further core authoring can make restoration unavailable. Receipts bind retained
evidence and current validation; they are not signatures or independent proof of
the source claims.

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

Composed closures retain those limits and use `resources/v3`, which additionally
bounds each recursively expanded value to 10,000 nodes, depth 128 (root depth
zero), and 16 MiB of canonical KR3 value tokens. Repeated references count once
per serialized occurrence. Recursive type joins, comparisons and size checks are
metered; native construction and serialization return `collection_limit` before
retaining or emitting an oversized value. These fixed public maxima cannot be
raised by callers.

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
IR; the compiled registry computes the result. Arithmetic, composition and the
finite-query adapter consume this boundary. Module/version definitions live in
`reasoning.contract` and `reasoning.modules`. Rendering depends on value/witness
types, not operator names. The compiled registry is the installed set of reviewed
modules and exposes a real extension interface: a module prepares a bounded
request, validates the native response and finalizes its evidence basis. Adding a
module is additive; revising a core module is a compatibility change. Record text
cannot load code or widen the captured view.

## Query operations

The optional `query/v1` module evaluates finite captured scopes through KP4/KR4
and `resources/v4`. A completed scan retains five row counts and the scope
witness in its `query-inputs/v1` basis. Member changes invalidate that basis even
when the aggregate value stays equal. Row expressions read only declared authored
fields; they do not evaluate member formulas or read another scope.

See [the query specification](query.md) for syntax, empty/unknown/error behavior,
resource limits and the contributor boundary. The [revisit exercise](../examples/scoped-query/README.md)
uses an installed package to change two assumptions, add a scope member, retain
an unknown forecast and replay an earlier snapshot.

## Runtime, licensing and assurance

The package and plugin carry the same platform archives. Extraction is automatic
and offline; ordinary computation needs no Lean compiler or session setup. The
runtime manifest schema is version 3 and must advertise protocols `[KP2, KP3, KP4]`
and modules `[arithmetic/v1, composition/v1, query/v1]`. The adapter verifies those pins,
the source identity and the request/response protocol before interpreting output.
Old or partially rebuilt archives fail closed. The maintainer builder compiles
pinned sources, checks proofs, audits native linkage and builds replaceable GMP
shared libraries. Supported target evidence and build instructions are in
`scripts/reasoning/native/README.md`. Source support alone is not a claim that
every target archive or installed distribution has passed its platform gate.

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
Composition reuses the scalar primitives but adds no broader theorem or formal
assurance claim; composed results keep `assurance.kind: computed` with an empty
formal scope.
