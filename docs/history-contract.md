# History evidence and prepared writes

`scripts.history_contract` defines the versioned boundary between a history store,
capture, and writers. These interfaces do not activate history for an existing
record. The ordinary legacy reader and the standalone core/v1 assessment keep
their existing behavior until their integrations are explicitly enabled.

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
checking is linear in commit/parent count. This verifies internal consistency of supplied evidence; the
reader still must establish complete file membership through capture.

## Capture and replay

`CapturedHistory(document, projection)` is a pure adapter result. Its projection
binds authority, baseline, identity schemes, rules, closure digest, coverage,
per-subject acceptance/heads/acts, selected pin objects, and integrity findings.
`snapshot()` places this evidence in `Snapshot.context.history`, so changed acts
or review context change Snapshot identity even if current values are equal.
Snapshot construction and frozen replay validate this binding.

The projection is limited to 8 MiB and contains no full file inventory. Exact
private inventory belongs in `CapturedSource`; complete audit replay needs an
explicit closure artifact. A captured pin preserves the original version/body.
A formula pin without a recorded result does not acquire a computed value,
InputBasis, or synthetic `seen`. Downstream assessment must expose those
unavailable fields independently of recorded acceptance. Public combined
assessment version 3 is a separate consumer integration; this module does not
change the standalone core/v1 version-2 envelope.

## Prepared mutation and recovery

`scripts.history_transaction.PreparedMutation` names the entry, captured
operation, authority, baseline, every touched file's role and exact before/after
bytes, and a semantic receipt. File paths are bound to the reader-owned layout.
Serialization is canonical typed JSON with byte hashes and an envelope digest.
Replay validates all of these before exposing a prepared operation. A receipt
binds assessment and capability evidence; its digest is not permission to write.

For a legacy mutation, `publish_legacy(root, journal, mutation, verify=...)`
acquires the record-directory writer lock, checks every before image, and invokes
the mandatory routing/capability/baseline verifier before publication. It writes
a durable local journal before changing any record/archive bytes. The journal
location must be private and ignored by Git when the writer is integrated.

Integrated readers must hold `reader_guard(root, journal)` while reading the
whole closure. They either see a complete generation or receive
`recovery_required`. The low-level transaction helpers alone do not make an
unguarded legacy reader safe, and they are not yet wired into ordinary writers.
Direct writers must check the same journal before preparing changes; a writer
lock alone does not certify that a previous mutation completed. The shared lock
is reentrant only within its owning process and thread.

`recover_legacy` resumes the stored operation or restores its exact before
images. It first checks that every current file matches either its before or
after image and runs the verifier again. An unrelated concurrent edit refuses
recovery before any additional change. Locks require `fcntl`; unavailable
locking refuses rather than silently publishing unguarded files.
An absent journal reports `no_recovery_pending`. If all after images already
match, publishing reports `after_images_match` and changes nothing. Matching bytes
do not prove this operation completed: durable completion/deduplication receipts
belong to the integrating writer. The chosen root may use a filesystem alias such
as `/tmp`; symlinks below that root and journal/member ancestry collisions refuse.

For history publication, `publish_immutable` provides exclusive atomic file
creation and byte-exact retries. A history mutation binds its objects and
generated entry to the actual immutable commit manifest. Storage integration
must validate full existing-plus-new closure, publish objects first, publish the
commit manifest as the visibility boundary, then refresh the generated entry.
A stale view after commit remains repairable; it cannot become authority by
winning a file-write race. Readers use complete committed closure throughout.

Authority changes, live activation, temporal recovery policy, history-store I/O,
ordinary writer integration and combined consumer policy are deliberately not
implemented by these shared interfaces.
