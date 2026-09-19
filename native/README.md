# Experimental native history CLI

`kpop-native` is an opt-in Rust executable for a bounded subset of kpopper history.
It does not replace `kpop`, modify installed plugins, or activate existing records.
Use disposable workspaces only. macOS arm64 is the initial verified target;
other platforms are not yet verified, and non-POSIX operations currently refuse.

## Build and run

With Rust installed, from this directory:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

The resulting executable does not require Python, Node, Cargo or Rust on the runtime
PATH. Rust and downloaded build dependencies are required only when building it.

Supply an absolute path to an existing **empty** directory:

```sh
kpop-native --workspace /absolute/disposable/workspace init --record-id example
kpop-native --workspace /absolute/disposable/workspace add p.hours \
  --value 10 --source calendar --operation observation-1 --on 2026-09-19T12:00:00Z
kpop-native --workspace /absolute/disposable/workspace set p.hours \
  --value 12 --operation observation-2 --on 2026-09-19T13:00:00Z
kpop-native --workspace /absolute/disposable/workspace open
kpop-native --workspace /absolute/disposable/workspace history p.hours
```

Values use JSON syntax. `null`, booleans, arbitrary-size integers, finite floating
point values, strings, lists and string-keyed maps remain distinct. A repeated
operation with the exact same request is idempotent; different content under that
operation refuses. Optional `--expected-revision` compares the revision returned
by `open`. Each new observation requires a new operation and an explicit recorded time.

`session-start` consumes a host JSON payload containing an absolute `cwd` on stdin.
It reports supported readings and an executable `KPOPPER_AGENT_CONTEXT.command`.
It ignores subagent payloads, and reports failed opening visibly without blocking
the host. This command does not register hooks or establish host trust.

## Supported history boundary

`ordinary_reader` preserves ordinary-reader/v1 value selection, legacy comparison
coercion, conservative formula normalization and graph counters. Captured hypothesis
documents and conflict IDs can be supplied explicitly. Ordinary authoring uses the
same immutable storage boundary for direct writes, acts, proposals and sequential or
final batches, while retaining ordinary receipt versions and historical core replay
versions. Its evidence contains the authored document; it does not invent a core
assessment or promote the record's profile.

Explicit ordinary formulas use the existing ordinary Lean program, whose JSON
protocol is separate from core/v1. A caller opens its selected build with
`ordinary_runtime::Program::open` and supplies it through
`Runtime::with_ordinary_program`. The source hash is bound at build time, and the
manifest and executable bytes are rechecked around each bounded request. Ordinary
scalar reads and writes need no evaluator. These library APIs perform no program
discovery or installation; packaging and public CLI routing remain pending.
The ordinary conformance tests use `KPOP_TEST_ORDINARY_PROGRAM` when supplied,
otherwise the existing ordinary Lean cache for the current platform and source hash.

The library validates final 1.7 temporal claim metadata and the required
`temporal-applicability/v1` manifest capability. Captured history retains at most
64 exact observations, with separate evidence kinds for recorded receipts and
reconstructed committed worlds. Each observation binds to its causal frontier,
claim, predicate and original anchors. Exceeding replay bounds produces explicit
incomplete coverage. Temporal assessment replays through an explicitly supplied
runtime, preserves current/anchored/general applicability, and verifies frozen
contexts without evaluating or reopening sources. Rehashed temporal results must
still match their immutable predicate, anchors, Snapshot inputs and computation
witnesses. Temporal authoring binds accepted claim versions to the before/after
writer Snapshots, preserving computational time independently from recording time.
Direct writes, acts, proposals and batches independently replay before publication.
Fresh sequential judgments capture typed `seen`; old envelopes retain their original
semantics only when the complete replayed mutation matches. The public CLI remains
the subset described above.

Computational scenarios select captured hypothesis bodies and shared observations.
Their Snapshots retain and revalidate the original source, canonical selection,
field roles, collisions and complete derived document. Frozen reconstruction reads
no sources and runs no evaluator. Scenario assessment uses the supplied Lean runtime
and reports computational consequences without granting history acceptance.
`history_watch` independently validates captured record/Snapshot pairs, merges their
committed closures and physical hypothesis changes, and compares those consequences.
Incomplete observations and conflicting closures produce explicit attention findings.

The library also provides an explicit `TypedValue` model and canonical typed JSON
codec. It preserves integers of arbitrary size, finite IEEE754 floats (including
signed zero), dates, naive/offset datetimes, Unicode text, lists and ordered maps.
Validated scalar constructors prevent invalid dates or non-finite floats from
entering that model. Converting a date/datetime to ordinary JSON refuses instead
of silently turning it into text.

`kpop-native identity --typed` reads the canonical tagged representation on stdin,
for example `["date","2026-09-19"]`, and returns its exact typed identity and tagged
value. Plain `identity` retains its JSON-value interface. This is a read-only codec
boundary: the history writer still accepts JSON-compatible values only.

`identity --yaml` reads a strict history YAML mapping. Add `--object` to either
identity input mode to dispatch by the object's declared scheme: absent means the
legacy 40-hex identity; `typed-history/v2` means the typed 64-hex identity. Unknown
declarations refuse. Legacy datetime strings retain their original space separator,
and only the top-level `id` is excluded. Computing an ID does not validate an object
schema, its references, or its acceptance in history.

`history-codec` reads a YAML mapping and returns its typed value, digest and a YAML
serialization that preserves types. `history-codec --typed` accepts the canonical
tagged map instead. This codec supports dates and naive/minute-offset datetimes;
sub-minute timezone offsets refuse serialization, as they do in the Python history
writer. These commands do not publish objects or enable legacy storage operations.

History YAML resolution follows the pinned Python reader, including YAML 1.1 boolean,
octal, sexagesimal and timestamp spellings. Direct merge mappings are flattened before
checking unique text keys. Aliases, reused anchors, nonfinite values and unsupported
tags refuse. The parser retains mapping entries until this validation; ordinary
Serde map deserialization is not the history contract. Source conformance fixtures
name the Python revision and source hashes, and cover both accepted and refused input.
Exotic scalar tags applied to collections (for example `!!str {=: value}`), and
`!!omap`/`!!pairs` collections remain unsupported, including their empty forms.

`history-validate` validates a detached immutable claim/act from YAML, including its
schema, identity and declared references. `--typed` reads a tagged object instead;
`--closure` validates an ID-to-object mapping and requires every reference to have
the exact subject and allowed object kind. A valid detached object or complete
closure does not establish committed membership, DAG consistency, acceptance, or
semantic truth. The experimental store also applies this object validator before
its existing, narrower operation checks.

The library's portable path contract supports both retained legacy directories and
hashed subject directories for 40- or 64-hex objects. Subjects retain exact Unicode,
empty or path-like data; only validated derived paths reach storage. Missing objects,
duplicate physical layouts and missing hashed-path capability declarations refuse.
The experimental store continues using its original hashed, typed-v2-only layout.

Typed decoding accepts canonical encoder output, not noncanonical convenience
spellings: tags and arity are exact, integer/float/date spellings are canonical,
and map keys must be unique and sorted. The library bounds logical depth at 128 and
visited values at 100,000. The CLI additionally retains its 1 MiB input limit and the
JSON parser's default 128-container nesting limit; deeply nested tagged values can
therefore refuse earlier at the CLI boundary.

The executable retains `history/v1` authority/manifest envelopes,
`typed-history/v2` immutable object IDs and `subject-paths/v2` storage names.
It handles a linear chain of explicit reading/accept operations, preserving
original objects and the open disposition acts on historical claims.

Writes stage immutable objects, publish a manifest as the commit point, then
replace the derived `GROUNDING.yaml` view. `recover` recreates a stale view from
committed evidence. `KPOP_NATIVE_FAIL_AFTER=objects` or `commit` injects an error
at either boundary for disposable fault tests. This is a native-private recovery
protocol; Python's prepared mutation journals are unsupported and block opening.

The existing POSIX directory-flock protocol is shared with Python history writers.
Cross-language authoring interoperability remains unverified. The opt-in marker
prevents accidental use, not malicious modification by another local process.
Symlinks, missing/corrupt committed evidence, unknown authority/capabilities,
incomplete or branching commit graphs, duplicate mapping keys and oversized data
refuse. Parsing has explicit depth, node and byte limits. Storage preserves its
previous JSON bytes when the strict YAML reader confirms the same value. Otherwise
it writes explicit YAML types, preventing scientific-notation floats or YAML-sensitive
text from changing meaning. Retained committed source bytes are never rewritten.

The YAML library bounds source nesting at 129 and source nodes at 200,000, followed
by the typed model's depth 128 / 100,000 value bounds and a 16 MiB compact ASCII JSON
budget. Source bytes are also limited to 16 MiB. Numeric YAML constructors cap integers
at 4,300 decimal digits and recognize Unicode 16 decimal digits, matching the pinned
Python runtime. The CLI and experimental store retain their smaller 1 MiB file/input
limit. These are bounded input contracts, not a claim to accept every PyYAML source.

## Library authoring

`history_authoring` prepares core `add`, `set`, `review`, targeted disposition acts
and explicit proposals over a captured history store. Preparation creates a complete
immutable mutation without publishing it. Direct receipt versions 1/7, act version 4
and proposal versions 5/9 preserve their distinct snapshot semantics. Proposals carry
a separate hypothetical assessment and leave accepted claims unchanged.

Its commit entry point reruns the actual Lean evaluator against the original causal
parents and compares the complete prepared mutation bytes. Historical adapter source
fingerprints require a committed causal-parent witness; sibling commits and the
incoming operation cannot establish that witness. Runtime identity, values, reads,
bases, diagnostics and costs must still match. Caller verification adds routing and
policy checks, followed by archive and imported-source revalidation before publication.
The `history_sources` adapter bounds retained members, verifies their hashes and
requires evidence for composite YAML pointers.

`history_authoring_batch` validates up to 64 actions against one final computation
world while comparing each replacement to its actual preceding claim. It binds
dependencies to their final immutable versions and refuses cyclic new pins.
Receipt versions 2/3 retain sequential replay, version 6 retains the earlier final
world, and version 8 captures typed snapshots and declared missing-pin evidence.
Attached report files remain part of the complete mutation and replay comparison.

`history_hypothesis_authoring` prepares named proposals, reviews, refutations and
folds without rewriting retained claims. It checks physical hypothesis names and
imported-file hashes, and rechecks archive and physical evidence after caller
verification. New folds assess the complete proposed history and refuse newly
introduced falsifications or evidence holes. Earlier fold receipts retain their
original replay semantics. `history_prospective` constructs detached snapshots
only after validating the mutation's authority, causal parents, immutable object
bytes and rendered view. Preview does not publish or accept a proposal; retained
branch-adoption evidence still requires its separate audit adapter.

`history_edits` captures the complete set of body changes in a generated view as
proposals. It retains exact edited UTF-8 bytes, including comments and historical
snapshots, as immutable receipt-bound evidence. Partial selections, deletions,
collection moves and header edits require separate dispositions. Its storage path
enforces fresh semantic replay; an edit marker alone cannot authorize publication.
`history_identity` prepares explicit `same` and `distinct` actions across accepted
claims and named proposals. It preserves immutable originals, source-map order,
provenance strings and historical snapshots, and rebuilds affected dependency pins.
The storage adapter enforces fresh semantic replay and source revalidation. When
the brief changes too, an owned journal blocks readers until both views are verified;
recovery resumes the exact operation, or cancels it before its manifest is committed.
Identity dates use the later of the current local and UTC day, independently of the
operation's recording timestamp or the computational snapshot time.

`reasoning_authoring_preparation` selects a writer profile from a frozen snapshot,
refuses reinterpretation of legacy executable fields, and checks an explicitly
supplied pending overlay before promotion. `pending_bundle` binds portable roots,
complete dependency and scope closure, and evidence bytes; accepted revisions remain
effective until a terminal decision retires them. `reasoning_declaration_text` changes
generated YAML capability fields while retaining surrounding source text. These APIs
do not collect live project policy or establish a filesystem route for the caller.

These library interfaces remain separate from the experimental CLI. Ordinary-profile authoring
is not yet exposed through this preparation layer. The temporal and scenario
contracts are checked against the final Python 1.7 release; full product migration
and platform acceptance remain pending.

## Deliberate limits

The detached `history_authority` library additionally validates authority v1/v2,
cancellation receipts, baselines, manifest DAGs, explicit root dispositions and
every retained generation against its exact committed bytes. It resolves declared
objects across legacy and hashed storage paths; unreferenced objects remain staging
residue. Callers supply captured membership, which cannot be established by a hash.
`history-envelope <authority|baseline|commit|cancellation|template>` exposes the
read-only envelope boundary with YAML or `--typed` input. These detached interfaces
do not activate a record or expand the store operations listed below.

`history-capture <entry>` captures active history in either record layout, checks
the journal guard and exact inventory, and reduces claims and acts into acceptance
evidence. It retains raw object bytes, source mapping order, legacy identities,
inactive generations and cancellations. Its tagged result preserves scalar types.
Source clocks follow the retained reader's day, timestamp, revision and commit
rules; commit ancestry is supplied explicitly to the library. Capture does not
evaluate conditions or review sufficiency, reconcile conflicted YAML, or publish a
transaction. `Capture::verify_current` rechecks the captured bytes and membership.

The `source_capture` library captures ordinary records in source order and active
history through two complete, verified loads. It retains exact private file bytes,
source origins, physical and recorded hypotheses, routing observations and the
detached computational Snapshot. Participant journals protect independently opened
members; authored revision metadata excludes private history storage. Frozen and
Simple live reads are supported. Advanced live reads pin the actual pending Git
ledger, cached publisher state and locally available committed target closure.
Contributions stay explicit proposals until an explicit terminal decision; cached
acceptance never removes them. Computational target capture takes an explicitly
supplied verified runtime. Unavailable targets retain their reason and observed
revision. This library does not activate installed command or write routes.

`authoring_source::AuthoringSource` retains the actual source capture and project
policy lock through writer preparation. `history_bootstrap` prepares and publishes
the first core history generation, including scalar questions and named proposals.
It replays retained intent before publication and recovery. Interrupted births can
complete or return to an absent record; rollback removes only the exact new images
owned by that birth while the recovery journal remains present. Existing records,
companion evidence and changed images refuse. Publication requires POSIX locks.

The detached `history_transaction` library decodes Python prepared mutations,
validates semantic receipts and exact file images, and serializes canonical typed
journals up to 64 MiB. Its companion `history_transaction_fs` implements guarded
legacy publication and authority transitions, exact retry collision checks,
participant journal replicas, forward recovery and restoration of before images.
Restoration retains immutable history evidence. Every publisher/recovery call takes
a mandatory verifier for the complete reader-resolved baseline and capabilities;
the prepared digest does not grant publication permission.

Capture and these publishers share reentrant POSIX directory locks. A scoped
auxiliary owner can read only its exact pending auxiliary envelope while holding
the exclusive lock. Other readers refuse pending journals. These are library
boundaries tested on disposable files; the public experimental store commands
above still use their earlier limited writer. Branch-adoption capsule validation
currently refuses with `unsupported_branch_adoption`. General Store preparation,
generation cancellation orchestration, grouped transitions and the public product
write flows are not yet integrated with these primitives.

- No judgments, dependency pins, formulas, competing histories, review/refutation,
  temporal semantics, Lean evaluation, MCP, UI, migration or production activation.
- No authority v2 cancellation metadata, legacy 40-hex storage operations or typed
  date/datetime storage operations. The standalone codec does not expand those limits.
- Receipts explicitly say `semantic_assessment: not_performed`; accepted history
  is not a claim that a condition was checked or that source content is true.
- Python's history contract/store and explicit Snapshot capture can validate this
  subset. Its public legacy CLI refuses this ordinary-reader history profile;
  the executable is not a drop-in replacement for the active core/history product.
- At most 1,000 commits, 1 MiB per serialized file and 16 MiB of selected store
  data. This bounded implementation rereads history and is not a scale benchmark.
