# History evidence and prepared writes

`scripts.history_contract` defines the versioned evidence boundary used by the
callable history store, pure capture adapter, and prepared legacy writes. These
interfaces do not activate history for an existing record. History storage and
legacy transaction support are available separately; ordinary history-backed
reader, writer, report and consumer integration remains incomplete.

## Objects and identity

`make_object` accepts a captured operation id and recorded timestamp. Retrying an
operation reuses its original envelope. A new observation uses a new operation,
even when its value has not changed. New objects declare `schema_version: 2` and
`id_scheme: typed-history/v2`. Their full SHA-256 identity uses the existing typed
contribution encoder over every field except `id`.

Claims retain their original body, including structured predicates and `seen`.
`authored` records collection membership, field roles and the original reasoning
profile; an optional locator retains import provenance. `pins` maps dependency
subjects to immutable version ids. It does not replace or rewrite original
`seen`. Acts carry `accept`, `refute`, `review`, or `correct` with their explicit
targets and causal observations. Recorded time alone grants no precedence.

`validate_object` dispatches by the declared identity scheme. Existing prototype
objects retain their 40-character ids and original envelopes. Unknown schemes
refuse instead of falling back. Typed identities distinguish dates from strings,
booleans from numbers, absent fields from null, and ordered sequences. They do
not recover precision or lexemes already lost while parsing a legacy source.

`validate_closure` checks every reference's subject and kind and refuses missing
objects. It never returns a valid-looking partial subset. This bounded in-memory
helper admits at most 20,000 objects; a larger closure requires another explicitly
bounded interface rather than silent truncation. Individual objects are bounded
to 1 MiB. Values are limited to 128 levels and 100,000 traversal occurrences
before hashing/copying, including repeated references in a supplied Python DAG.
`encode_document` and `decode_document` retain typed YAML values and refuse
duplicate mapping keys and YAML aliases. The encoder emits no aliases. Existing
prototype objects may retain sorted repeated references without changing their
identity; newly typed objects require sorted unique reference lists.

## Authority and visibility

The reader-owned layout names three roles:

| Role | GROUNDING.yaml | Legacy and custom entry names |
|---|---|---|
| `history` | `.kpopper/history/` | `PROVENANCE.history/` |
| `history_commits` | `.kpopper/history-commits/` | `PROVENANCE.history-commits/` |
| `history_authority` | `.kpopper/history.yaml` | `PROVENANCE.history.yaml` |

The authority marker binds version 1, record id, authority (`legacy` or `history`),
generation, and profile `history/v1`. A directory's existence does not choose
authority. Changing authority requires a separate guarded migration; ordinary
prepared writes refuse authority changes.

Generated entries retain the authored collection/id/body shape. `meta.history`
contains the version-1 render baseline: record id, authority generation,
committed-set digest, per-subject heads, and open acts. It is required when
reconciling an edited view. A missing or mismatched baseline is an error.

Each immutable commit manifest binds one operation, record/generation, parent
manifest hashes, captured-baseline digest, exact object identities and file
hashes, semantic receipt, and intended entry-view hash. Objects can be imported
without changing their original operation or time. `committed_objects` takes
captured manifest/object bytes and returns only the complete reachable object
set. Orphan staging objects do not participate. Missing parents, corrupt bytes,
wrong identities, invalid references or cyclic manifest ancestry refuse the
entire result. The in-memory commit count also has a 20,000 limit and ancestry
checking is linear in commit/parent count. This verifies internal consistency of
supplied evidence; the reader still must establish complete file membership
through capture.

New commits name only the current causal frontier as parents; the committed-set
digest binds the complete known set. Sequential history adds one parent edge per
commit instead of repeating all earlier edges. Store directory names are literal
paths, including filesystem glob metacharacters.

A manifest may carry an immutable `view_template`. `document_template` retains
record headers and empty authored collections, removing the generated
`meta.history` baseline. Store rendering requires compatible templates at the
commit frontier; absent or conflicting templates refuse. Mutable entry bytes do
not supply authoritative headers.

`committed_set_digest` hashes the committed effect descriptions, excluding
`receipt` and `view_sha256`. This avoids a circular dependency between the render
baseline, generated view and assessment receipt. Complete raw manifest bytes
remain separately bound by captured closure identity; the effect digest is not a
replacement for that evidence.

## History store

`scripts.history_store.Store(entry)` uses the reader-owned layout. `capture()`
reads and verifies file membership and bytes under the shared reader guard,
validates authority and committed closure, and returns detached evidence with a
private inventory. Orphan staging objects are captured but do not enter the
committed state. Capture has explicit object, commit and total-byte limits and
performs no publication or derived-index writes.

`reduce(objects, rules=None, ancestry=None)` is pure acceptance and act reduction.
It compares typed claim meanings, including kind, body, pins and authored
interpretation. It does not evaluate predicates or decide review sufficiency.
`Store.state()` exposes that reduction; `Store.render()` generates accepted bodies
from immutable templates, preserving original predicates and `seen`. Disputed
alternatives remain evidence and do not become arbitrarily selected scalar nodes.
Unresolved authored mappings and incompatible selected profiles or field roles
refuse rendering.

`Store.commit(mutation, verify=...)` validates the complete existing-plus-new
closure, authority, baseline, parents and generated view. It calls the mandatory
capability/evaluator verifier and rechecks captured inputs before publishing
objects, then the immutable commit manifest, then the entry view. The manifest is
the visibility boundary. Exact retries reuse the operation and bytes and may
repair an interrupted view refresh; they refuse unrelated entry edits. Immutable
file publication uses exclusive creation and byte-exact collision checks.
The generated after view must exactly match canonical render bytes. Rebuild can
also recognize an earlier canonical branch union through a later manifest's bound
before baseline and complete parent closure, without inventing a knowledge act.

`Store.rebuild(capture=None, write=False)` returns regenerated bytes and writes
only when requested. It accepts a known committed view or an already matching
render, and rechecks the capture before writing. Arbitrary edited or conflicted
views still require reconciliation: their reconstruction is not implemented.

## Capture and replay

`Snapshot.capture` and `capture_source` select explicit active history markers
and use the store plus adapter for a single-entry frozen or Simple live record.
They preserve project, hypothesis, target and read-mode context. A known stale
generated view is projected from committed evidence without a write; unknown hand
edits require reconciliation. Advanced live contributions and composite active
history records currently refuse until their versioned closure binding exists.
Legacy contribution formats refuse `meta.history` instead of dropping it.

`scripts.history_adapter.capture_history(objects, projection, document=...)`
consumes committed objects and a validated reduction projection without store or
evaluator I/O. The supplied document must be an immutable template, optionally
carrying the matching baseline. The adapter returns
`CapturedHistory(document, projection)`. It preserves authored collections, field
roles, profiles and original bodies, adapting pinned dependency maps to the
existing dependency-list shape while leaving original `seen` unchanged. Selected
core claims require a supplied compatible reasoning declaration; the adapter does
not synthesize capabilities or convert named legacy profiles.

The projection binds authority, baseline, identity schemes, rules, closure digest,
coverage, per-subject acceptance/heads/acts, selected claims, pin witnesses and
integrity findings. `snapshot()` places it in `Snapshot.context.history`, so acts
or review context change Snapshot identity even when current values are equal.
Snapshot construction and source-free frozen replay validate this binding.

The projection is limited to 8 MiB and contains no full file inventory. Exact
private inventory remains in the source capture; complete audit replay needs an
explicit closure artifact. Incomplete coverage and unavailable or corrupt pins
remain explicit. Invalid mappings cannot claim complete conversion.

`pin_review_evidence(projection, version, subject=...)` exposes detached recorded
support as `evidence_kind: version_pin`, retaining the original subject, version,
body and profile. This is distinct from original `seen` and is not a new review.
Stored typed readings and valid computed-v2 evidence can supply recorded values;
computed-v2 decoding uses the recorded profile and basis. A formula pin without a
stored evaluated result does not acquire a value, InputBasis or synthetic `seen`.
Value and basis availability remain independent of recorded acceptance. Public
combined assessment and consumer policy remain separate integration work; this
adapter does not change the standalone core/v1 version-2 assessment envelope.

## Prepared legacy publication and recovery

`scripts.history_transaction.PreparedMutation` names the entry, captured
operation, authority, baseline, every touched file's role and exact before/after
bytes, and a semantic receipt. File paths are bound to the reader-owned layout.
Serialization is canonical typed JSON with byte hashes and an envelope digest.
Replay validates these before exposing an operation. A receipt binds assessment
and capability evidence; its digest is not permission to write.

The legacy `record_member` role supports additional authored members only when
`baseline.record_members` includes the entry and exact captured member hashes.
Publication verification must independently resolve membership and verify those
hashes; a caller-supplied path is insufficient. Direct writes currently support
members sharing the entry's directory. External or nested authored shards refuse
until a shared lock protocol covers their publication and readers.

`publish_legacy(root, journal, mutation, verify=..., on_committed=...)` acquires the
record-directory writer lock, checks before images, and invokes the mandatory
routing/capability/baseline verifier before publication. It persists a durable
private journal before changing record, archive or view bytes. `journal_for`
selects the integrated journal location; publication enforces its private ignore
file. A pending journal requires recovery before another mutation.

After all after images are durable, the optional `on_committed` callback runs
before the journal is unlinked and its directory synced. Report ingestion uses
this boundary to persist its durable event completion receipt. A failed callback
leaves recovery evidence in place. Matching after-image bytes alone do not prove
an operation completed: an unreceipted retry reports `after_images_match`.

`reader_guard(root, journal)` uses the shared directory lock where available and
refuses a pending journal with `recovery_required`. Ordinary `provenance.load` guards its
record reads, including checks for pending operations touching a same-directory
member; source capture through that loader inherits those guards. Locking protects
integrated readers from concurrent publication. Raw file reads outside the guard
do not inherit that guarantee. Locks are
reentrant only in the owning process and thread. Unavailable `fcntl` locking
refuses writes instead of publishing unguarded files. Read-only installations
remain available without `fcntl`; they check for a pending journal before and
after reading. This does not enable writes on those hosts.

`recover_legacy(..., direction='after')` reuses retained bytes to complete an
operation; `direction='before'` restores its before images. It first checks every
current image against the retained before/after pair and reruns verification.
Unrelated concurrent edits refuse recovery before further changes. Forward
recovery invokes `on_committed` before journal removal; rollback does not. An
absent journal reports `no_recovery_pending`. Filesystem aliases for the chosen
root are permitted, but symlinks below it and journal/member ancestry collisions
refuse.

## Integrated write scope

For existing unactivated records, ordinary `provenance.apply` direct writes stage
candidate record, replacement-archive and supported view changes before building
one prepared mutation. They recheck routing, policy, reader-resolved membership
and captured observations under the writer lock. Original structured predicates
and `seen` are retained in replacement history. `recover_direct(paths,
direction=...)` completes or restores the exact prepared generation without
regenerating the candidate. Active history authority or generated history baselines
refuse this legacy direct path. An explicitly inactive legacy marker allows legacy
writes while retaining its history files without reactivating them.

`kpopper recover --record FILE` exposes forward direct recovery through the same
project routing as the write. Add `--rollback` to restore before images and
`--json` for the operation receipt. Recovery reuses the retained mutation and
refuses changed evidence or routing; it does not prepare a replacement write.

Report ingestion retains record, view and replacement-archive images in its
prepared transport, verifies source, routing, capabilities and assessment
receipts, and recovers through the same publication journal. Its durable event
receipt distinguishes a completed operation from coincidentally matching bytes.
History-backed report preparation and shadow history transport refuse. Core
batch staging and report batch validation also reject `add` operations that would
replace an existing entry or judgment; these guards are not a history replacement
implementation.

The prepared boundary covers the existing-record paths above. Newborn record
bootstrap still precedes creation of the prepared mutation. Named hypotheses,
folds, and legacy expression/watch helpers are not claimed transactional through
this interface. Ordinary history-backed writer/report and reader/consumer binding
remains incomplete. Authority migration, live activation, temporal recovery
policy and combined consumer policy are not supplied by these components.
