# History evidence and prepared writes

`scripts.history_contract` defines the versioned evidence boundary used by the
callable history store, pure capture adapter, and prepared legacy writes. These
interfaces do not activate history for an existing record. Direct history writes, local source reports, copied migration and captured Simple/Advanced
reads use these interfaces. Guarded authority transitions are callable for explicitly
selected records. New records start with immutable history and `core/v1`;
ordinary readers select declared core records automatically. Combined assessment
retains computational, historical and operational findings separately. See
[reasoning and history](reasoning-core.md) for the public consumer contract.

## Objects and identity

`make_object` accepts a captured operation id and recorded timestamp. Retrying an
operation reuses its original envelope. A new observation uses a new operation,
even when its value has not changed. New objects declare `schema_version: 2` and
`id_scheme: typed-history/v2`. Their full SHA-256 identity uses the existing typed
contribution encoder over every field except `id`.

Claims retain their original body, including scalar readings, structured predicates and `seen`.
`authored` records collection membership, field roles and the original reasoning
profile; an optional locator retains import provenance. `pins` maps dependency
subjects to immutable version ids. It does not replace or rewrite original
`seen`. Optional `pin_gaps` records `not_recorded` or `unavailable` support without
inventing a version id. Pins and gaps account for the declared dependencies; gaps
make historical support coverage incomplete without changing acceptance. Acts carry `accept`, `refute`, `review`, `correct`, `propose`, or `retire` with
explicit targets and causal observations. A proposal is unaccepted; retirement
records noncurrent standing without claiming falsehood or inventing a successor. Recorded time alone grants no precedence.

`validate_object` dispatches by the declared identity scheme. Existing prototype
objects retain their 40-character ids and original envelopes. Unknown schemes
refuse instead of falling back. Typed identities distinguish dates from strings,
booleans from numbers, absent fields from null, and ordered sequences. They do
not recover precision or lexemes already lost while parsing a legacy source.

Logical subject and collection names retain exact Unicode text. New object files
use a reserved `~` directory key derived from the exact subject's UTF-8 bytes and
declare `subject-paths/v2`; their immutable object IDs do not change. Existing raw
subject directories remain readable, and retained operations replay their original
path scheme. Capture resolves paths from validated manifest membership, so case or
Unicode normalization cannot silently merge names. Duplicate physical representations
refuse. Malformed orphan staging files stay outside committed interpretation.

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

The reader-owned layout names these roles:

| Role | GROUNDING.yaml | Legacy and custom entry names |
|---|---|---|
| `history` | `.kpopper/history/` | `PROVENANCE.history/` |
| `history_commits` | `.kpopper/history-commits/` | `PROVENANCE.history-commits/` |
| `history_authority` | `.kpopper/history.yaml` | `PROVENANCE.history.yaml` |
| `history_cancellations` | `.kpopper/history-cancellations/` | `PROVENANCE.history-cancellations/` |

The authority marker binds record id, authority (`legacy` or `history`),
generation, and profile `history/v1`. Version 1 remains readable; version 2 adds
hash-bound cancellation receipts for reserved generations. Its commits bind the
complete marker digest. A directory's existence does not choose
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

Selected `core/v1` claims require an explicit compatible `meta.reasoning`
declaration at both commit preparation and capture. The store does not publish a
core claim that the capture adapter cannot interpret. A bootstrap marker with no
commit has no authoritative template and cannot be rendered or captured as a
Snapshot; activation must publish its initial commit in the same guarded change.

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
render, and rechecks the capture before writing. `prepare_reconciliation(allow_conflicts=True)` describes original baseline-bound
edits without applying them. `rebuild(allow_conflicts=True)` resolves Git conflict
markers only when every alternative is an exact known generated view. Pending body,
header or comment edits remain untouched and require explicit reconciliation.

## Capture and replay

`Snapshot.capture` and `capture_source` select explicit active history markers
and use the store plus adapter for frozen, Simple live and Advanced live records.
They preserve project, hypothesis, target and read-mode context. A known stale
generated view is projected from committed evidence without a write; unknown hand
edits require reconciliation. Advanced contribution and target histories remain
independent captured alternatives; reading does not combine their authority with
the checkout. Imported pointer layouts bind retained member bytes explicitly and
do not read them as a second authority. Legacy contribution formats refuse
`meta.history` instead of dropping it.

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

Disposition marks for retired versions are evidence pointers. Unless a marked
version also appears among heads, proposals, or review pins, the bounded Snapshot
does not retain its historical body; displaying that body requires the separate
verified closure. The generated view's observed status and baseline digest also
participate in Snapshot identity: stale and refreshed views can have equal
committed computational inputs but different observed-state identities.

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
Publication verification independently resolves membership and verifies those
hashes; a caller-supplied path is insufficient. Nested and external members share
ordered participant locks. The common path is a namespace, not a host-wide lock.
Small directory-local guards contain member names and operation identity without
copying other files' private bytes. A durable ready marker follows all guards and
precedes every changed image. Interrupted preparation can be cancelled without
overwriting a newer unguarded member edit; evidence of partial publication refuses
that cancellation. Pointer-list changes require a separate migration.

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
record reads, including checks for pending operations touching a separately opened
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

`Store.recover_auxiliary(direction=..., verify=...)` recovers the owned auxiliary
journal with the required original application verifier. The journal retains the
mutation, not routing or policy permission; recovery infers no routing permission.

## Integrated write scope

For existing unactivated records, ordinary `provenance.apply` direct writes stage
candidate record, replacement-archive and supported view changes before building
one prepared mutation. They recheck routing, policy, reader-resolved membership
and captured observations under the writer lock. Original structured predicates
and `seen` are retained in replacement history. `recover_direct(paths,
direction=...)` completes or restores the exact prepared generation without
regenerating the candidate. Active history authority routes ordinary add/set/review through immutable history
authoring. Generated baselines never enter the legacy direct path. An explicitly inactive legacy marker allows legacy
writes while retaining its history files without reactivating them.

`kpop recover --record FILE` exposes forward direct recovery through the same
project routing as the write. Add `--rollback` to restore before images and
`--json` for the operation receipt. Recovery reuses the retained mutation and
refuses changed evidence or routing; it does not prepare a replacement write.

Report ingestion retains record, view and replacement-archive images in its
prepared transport, verifies source, routing, capabilities and assessment
receipts, and recovers through the same publication journal. Its durable event
receipt distinguishes a completed operation from coincidentally matching bytes.
History-backed local reports retain one aggregate prepared mutation: sequential
actions are staged into a complete final world, and one final manifest publishes the
complete batch. New authoring receipt version 6 validates final names, computations
and dependency pins while retaining each action's replacement admission and ordered
evidence. A judgment may refer to an input introduced later in the same batch.
Unrelated original `seen` values remain unchanged; an unrepresentable cycle among
new immutable dependency IDs refuses before publication. Retained version-2/3
receipts keep their original sequential replay semantics and bytes.

Recovery replays the retained operation and records durable event
acknowledgement before removing its private retry envelope. Legacy core batches
carry exact replacement archives through the same prepared transport. Advanced scoped history reports retain an independent portable contribution;
they do not commit candidate claims into the checkout.

Replay re-evaluates semantic evidence. A retained adapter audit is accepted only
when its source hash matches the actual current adapter or its exact full audit
appears in a validated causal parent receipt. The incoming operation, its own
pending or committed retry receipt, sibling receipts and caller-supplied hashes
are not witnesses. Thus a first old pending operation after an adapter update,
with no such parent witness, refuses `unknown_retained_adapter_audit` and
requires the original verified runtime; it is not automatically regenerated or
replaced by a native fallback. When replay is witnessed, every computed result,
basis, diagnostic and other native implementation field is still checked. This
does not claim the old adapter executed during recovery. Semantic or native
runtime changes still refuse exact replay; retained receipts are never rewritten
to conceal their original implementation.

The prepared legacy boundary also covers newborn absence, named hypotheses,
fold/refute, same/distinct and expression migration. Hypothesis additions and
deletions bind exact membership and protect separately opened files. Shared
watch reports retain exact prepared generations and durable event completion
receipts; interrupted storage remains retryable in their owning processor. Public
combined assessment and default consumer binding remain separate from storage.
Typed judgments may opt into `temporal-applicability/v1` with
`temporal: {version: 1, applicability: current|anchored|general}`. Their authoring
receipts retain the exact bounded Snapshot and claim-version map for every observed
committed before/after world. Fresh assessment replays those snapshots through the
canonical evaluator once; serialized assessment replay is source-free. A current
claim recovers when its present falsifier clears while retaining the earlier fired
episode. Anchored and general claims retain a verified firing as a counterexample
until an explicit new claim or correction version replaces them. Missing, malformed,
over-limit or non-reproducible evidence remains unknown. No source clock is inferred:
object `on`, optional `at`/`applies`, and Snapshot `as_of` remain separate and may be
unknown. Claims without the explicit metadata retain their existing semantics.
Applicability selects only how a verified counterexample persists for that exact
claim version. It does not infer a date window, reinterpret a current reference as
a past value, change acceptance, or synthesize support. Anchored predicate and
evidence subjects must therefore be authored explicitly; `on`, `at`, `applies`,
and `as_of` remain independent provenance fields rather than selectors.
Authority transitions use the separate guarded interface below.


## History commands and portable contributions

`kpop history status` inspects committed acceptance. `history reconcile` returns
baseline-bound edit descriptions, and `history rebuild` resolves a generated Git
view without creating knowledge acts. `history migrate --record FILE --to DIR`
publishes only into an absent copy destination after capture and replay validation.
It preserves original entry names, supported shard/pointer layouts, hypotheses,
original bytes and historical provenance gaps. `restore_copy` checks the complete
candidate before restoring an exact original copy. It refuses newer history.
Live cutover and rollback of an active record are separate operations.

Direct history commands retain a private retry envelope binding their exact
PreparedMutation and routing policy. `recover` completes the same operation after
a crash; `recover --rollback` can cancel an uncommitted operation, but cannot
delete committed knowledge. Original claim bodies and seen values stay immutable;
a review adds a review act with current dependency pins. New core judgment writes
capture their dependency values and computational bases into reader-owned `seen`
after validating that the caller did not supply that field. Direct receipt v7,
final-world batch receipt v8 and proposal receipt v9 distinguish this behavior;
older v1/v6/v5 receipts replay without retroactive snapshot synthesis. A batch
captures all new judgments against its one final staged world, including forward
references, while a named proposal captures only inside its proposed layer and
does not write the base. Explicit blocked dependencies become `pin_gaps` and are
omitted from `seen`; unblocked missing dependencies still refuse. Computational receipts
assess a named document projection and separately bind history, avoiding a circular
receipt/manifest identity. They are not the public combined assessment envelope.

Version-3 portable contributions bind checksummed history closure through
`history-closure/v1`. Existing version-1/2 identities remain unchanged. Full-history
export requires explicit roots covering every transferred subject and checks the
privacy of historical bodies as well as current ones. Staged orphan objects stay
excluded. Materialization publishes the exact authority, immutable commits and
objects plus canonical view, retaining original artifact evidence. Publication
can union complete contributions into the same authority and rules; a different
authority requires explicit adoption. Retaining an artifact does not prove it was
accepted. Scoped single-action and report contributions use artifact version 2 inside the
version-3 pending envelope. The transport contains whole histories of selected
subjects and their dependency closure. It preserves object IDs and original
scopes, labels prepared candidates, and reports selected rather than original
store completeness. Source completeness is an observation, not authenticated
membership proof. Retained non-entry locators require explicit path/hash disclosure
consent; `--disclose-locator PATH=SHA256` records that consent without reading the
named file. Every overlapping target subject requires an explicit adoption choice.
Acceptance is checked against an actual committed adoption receipt and exact
object inventory, never against artifact retention or equal YAML alone.

Branch transport resolves a pinned local Git source and captures its exact
objects, profiles and proposals. `preview_adoption` and
`preview_adoption_set` expose explicit choices for overlapping subjects; the
required ordinary target evidence must byte-match before adoption. The
`prepare_adoption`/`prepare_adoption_set` and corresponding commit APIs each
prepare or publish one target commit under the caller's verifier, with no ledger
publication. Each adoption retains the exact source audit capsule under
`evidence/branches`; version-2 captures mark prior capsules
`not_transferred`, avoiding recursive copying. The source association records a
local capture observation and is not standalone authenticated Git inclusion
proof.

`consolidate --from REF --dry-run` previews a branch; repeated `--from` flags
preview one atomic source set. Actual adoption requires `--by` and `--choose`
for every overlap. `--source-revision` binds the exact preview. `pull ID --from REF`
shows branch and current standing without adopting either. Recovery reuses the
retained sources even after their refs disappear; it never loops over partial folds.

The target record and its history must be committed, including files matched by
Git ignore rules. Standalone capture and adoption have separate size limits: a
valid audit capture may be too large for the encoded write envelope. Adoption
checks a lower bound before preparation and validates the complete envelope before
creating journal files; size failures report `branch_adoption_limit` without a
partial knowledge write.

`history capabilities --nonce TOKEN --json` declares supported protocol versions,
resolved launch paths and hashes of installed source/schema/native archive files.
It reports native readiness as untested. These hashes identify files rather than
proving which bytecode executed. Deployment probes use an explicit managed-launcher
inventory and reviewed expected digests, reject old or mismatched endpoints, and
never choose a different PATH or cache installation. The caller must keep the
selected installation immutable through a transition. Actual installed validation
is still required; a source declaration is not a release gate.


## Explicit proposals, groups and generation changes

New strict manifests declare `explicit-root-disposition/v1`: a newly introduced
root claim carries an explicit accept, propose or retire disposition. Original
compatibility manifests and their IDs remain readable without retroactive acts.
Transport snapshots of inherited compatibility roots remain explicitly scoped
observations. `propose` and `retire` are rejected by older object readers.

`history accept|refute|correct|propose|retire --subject ID --of VERSION --because TEXT`
records an explicit act; `--over VERSION` names replacement alternatives. Acceptance
and condition outcome remain independent. No act refreshes original seen values.
`history adopt --revision REV --preview` shows choices. Actual adoption names
`--by ACTOR` and `--choose SUBJECT=VERSION` for every overlapping subject.

`history reconcile` only describes edits. Adding `--record-proposals --because TEXT`
explicitly records all selected current-baseline body edits as unaccepted proposals.
Exact edited UTF-8 bytes and comments survive in the immutable receipt and a bounded
evidence file. Header edits, deletions, collection moves, stale/conflicted edits and
partial subject selections refuse with the original file intact. Recovery reuses
the original prepared operation; it never temporarily rewrites the view to bypass
validation.

History-backed `--hypothesis NAME` records immutable named proposal layers. Group
edits retire superseded proposals; reviews preserve actual version pins and original
seen. `consolidate --dry-run` prepares without committing; explicit fold/refute
creates one manifest and preserves base evidence. Standing judgment replacements
still require the established admission or an explicit named take. Physical legacy
groups are preserved by migration as immutable proposals with exact original
headers, profiles, field roles, bytes and provenance gaps. A hash-bound import map
keeps those retained files from contributing a second live layer. A later physical
file change or unmapped name collision refuses.

An identity `same` operation also rewrites supported brief references through the
existing transformation rules. Its version-2 receipt retains exact original brief
bytes, and a reader guard covers publication of the manifest, record and brief.
Forward recovery completes those exact images; cancellation before the manifest
leaves knowledge unchanged. A committed identity operation requires forward repair
or a new explicit act. Unrelated edits and mismatched retry envelopes cannot clear
the guard. An unchanged brief and `distinct` retain the older receipt format.

Copied migration can retain external member topology and sealed Advanced pending,
publication and target observations. `replay_from_copy` restores captured context;
an ordinary frozen read does not silently install that context. `restore_from_copy`
uses only the copy's verified originals and refuses newer or tampered evidence.
Absolute original pointers are explicitly nonrepresentable as an exact relocated
inverse; the mapped copy and archived original bytes remain available.

Callable authority transitions use prepared-mutation version 2 and a required
caller-owned deployment exclusion guard plus fresh explicit launcher probes. The
first manifest, objects, marker and view publish under the shared recovery boundary.
Deactivation preserves immutable history and refuses newer knowledge or pending
evidence. Existing pending ledgers remain independent and unchanged. Distinct
worktree authorities use `history_group_activation` with one guarded transition
across all related authorities in the same configured Git project. Every member's
reader guard is installed before publication; a group journal and completion proof
coordinate recovery. Individual recovery cannot bypass the group boundary.

The marker selects the exact active generation. Complete earlier generations remain
auditable and cannot become current just because their generation is largest.
Full artifacts retain this audit evidence; scoped artifacts explicitly label any
omitted audit coverage. Each migration generation has separate immutable artifacts.
Here, complete history closure means the committed claims, acts, manifests and
cancellation receipts. The migration backup files named by a cancellation receipt
are a separate retained input: an ordinary history capture or full history bundle
does not verify or transfer those backup bytes. Copy restoration and later
activation validate their required files and hashes separately and refuse missing
evidence. A portable history bundle alone is not a promise of lossless downgrade.
Copy receipts retain unchanged immutable originals by verified path and hash instead
of recursively copying old migration directories. Version-1 copy receipts still
restore independently from their original sources.

Cancelling a prepared activation retains the complete intended generation as
inactive audit evidence, restores original authored bytes, and advances to a new
legacy epoch. For example, cancelling reserved history generation 3 from legacy
generation 2 produces legacy generation 4; a later activation uses generation 5.
It does not restore the old authority marker verbatim. A durable cancellation
receipt binds the reserved generation and original images before guards are cleared.
Recovery reports the cancelled generation, resulting authority and receipt path.
Interrupted compensation must finish in the same direction; missing or altered
proofs refuse. Group cancellation validates every member before any guard clears.

If an independent edit changes a member during interrupted group cancellation,
recovery preserves the reader guards and refuses. Save the divergent file before
restoring that member to an exact retained transaction image, verify the expected
hash reported by recovery, and resume cancellation with the original deployment
guard. Do not discard the edit, remove journals or resume forward activation once
cancellation has begun.

Older history clients may report retained generations as `authority_mismatch` or
newer markers as `invalid_schema`; these are refusals, not evidence that a record
should be repaired by deleting history. Deployment probes must establish required
generation and cancellation support before a transition. Adding a marker field
cannot retrofit a more specific diagnostic into an already installed old client.

These callables do not select or activate real
records, and source-level probes do not establish installed release readiness.
