# Native Rust CLI (default runtime)

The native distribution provides the public `kpop` CLI and `kpopper` alias. Its
internal library crate is `kpop_native`; the 0.9.0 line is the first pre-1.0 series.
Prebuilt GitHub bundles are the primary install and include adjacent reasoning
resources. Archives carry the project MIT license and dependency license texts and notices
under `bin/resources/reasoning/notices/`. The exact GMP source and build recipe are
included at `bin/resources/reasoning/native/gmp-source-and-build.tar.gz`. Python, Node and
Rust are not required at runtime, and `KPOPPER_RUNTIME` accepts `rust` alone.

## Install from crates.io

Each release is also published to crates.io as `kpopper`, from the commit its GitHub
release was built from. Cargo builds the same two commands from source:

```sh
cargo install kpopper --locked
```

This needs Rust 1.98 or later and a C compiler; `--locked` keeps the dependency versions
the release was tested with. The crate carries the commands only, not the verified
reasoning resources a release archive places beside them. Without those resources the
commands read and write an existing record, but starting a new record fails with
`runtime_unavailable` and explicit computations report themselves unavailable. To provide
them, set `KPOPPER_NATIVE_RESOURCES` to the `bin/resources` directory of the release
archive for the same version and platform, or install that archive instead.

For source builds, run the commands below from `native/`; the release build can
also be invoked from the repository root with
`cargo build --manifest-path native/Cargo.toml --release`.

## Build and run

With Rust installed, from this directory:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

Development and test builds optimize SHA-256 hashing because launcher attestation
verifies complete executable and runtime artifacts within a bounded deadline.
Run validation separately from other compilation jobs to avoid resource contention.

`cargo test` stays the command for running the tests locally. CI runs the same tests through
nextest, in one pool across all the test binaries, with the `ci` profile of
`.config/nextest.toml`: `cargo nextest run --locked --profile ci` runs them as CI does.

The full integration suite needs the same verified reasoning resources as a runtime
bundle. For a macOS ARM64 bundle, run (use its matching target directory elsewhere):

```sh
KPOP_TEST_ORDINARY_PROGRAM=/absolute/bundle/resources/ordinary/darwin-arm64 \
KPOP_CONSOLIDATION_RESOURCES=/absolute/bundle/resources \
cargo test --locked --no-fail-fast
```

Without a bundle, compile the ordinary Lean program from this repository. `ci/build_ordinary_program.sh`
needs a Lean 4.33.1 toolchain and nothing else; it prints the cache directory holding the
program and its `build.json`, which is what `KPOP_TEST_ORDINARY_PROGRAM` expects:

```sh
KPOP_TEST_ORDINARY_PROGRAM=$(sh ci/build_ordinary_program.sh) cargo test --locked --no-fail-fast
```

Pass the toolchain prefix as an argument, or in `KPOPPER_LEAN_ROOT`, when `lean` is not on
PATH; `--rebuild` replaces an existing cached program. The cache lives under
`~/.cache/kpopper/lean/<target>/<source hash>`, which `KPOPPER_CORE_CACHE` or
`XDG_CACHE_HOME` can relocate, and which the conformance tests fall back to when
`KPOP_TEST_ORDINARY_PROGRAM` is unset. Rebuilding is only needed when the Lean source
changes: each source hash keeps its own directory.

The platform acceptance workflow builds these resources before testing. Tests that
require an explicitly selected Python oracle or managed deployment remain opt-in.

The host hook tests compare the native hooks with a fixed reference: the Python
implementation at the v0.10.0 release tag (commit
`dd099226bba989f4f22c2979cd7f96e99a36d733`, Python package 1.8.1), never the Python in
the checkout under test. `KPOP_HOST_ORACLE_ROOT` is the absolute path of a tree of that
commit, and `KPOP_HOST_ORACLE_PYTHON` an interpreter with PyYAML (`python3` by default,
`python` on Windows). Without the root, the tests check the native hooks alone and print
that the comparison was skipped; when `CI` is set they fail instead. The platform
acceptance workflow fetches the commit and prepares that interpreter on every target. To
run the comparisons locally, extract the tag with `git archive` (or check it out with
`git worktree add --detach /tmp/kpopper-v0.10.0 v0.10.0`):

```sh
mkdir -p /tmp/kpopper-v0.10.0
git archive v0.10.0 | tar -x -C /tmp/kpopper-v0.10.0
python3 -m venv /tmp/kpopper-v0.10.0-python
/tmp/kpopper-v0.10.0-python/bin/python -m pip install PyYAML==6.0.3 tzdata==2025.2
KPOP_HOST_ORACLE_ROOT=/tmp/kpopper-v0.10.0 \
KPOP_HOST_ORACLE_PYTHON=/tmp/kpopper-v0.10.0-python/bin/python \
cargo test --locked --test host_hooks
```

Core commands and the compiled Hub/Annotated applications do not require Python,
Node, Cargo or Rust on the runtime PATH. Optional browser checks use Node,
playwright-core and Chrome. The applications themselves remain experimental and
opt-in even though their native runtime is bundled.

To install a checkout or plugin cache, run the explicit installer, which places the
matching target runtime under `scripts/runtime/<target>`:

```sh
sh /absolute/path/to/kpopper/scripts/install_native.sh
```

The same installer accepts an offline archive and SHA-256. Hooks never download or
compile the runtime; a missing runtime is reported as a diagnostic.

### Assess an existing record

```sh
kpop --workspace /absolute/workspace assess d.decision p.input
kpop --frozen assess d.decision --record /absolute/GROUNDING.yaml --attention-only
kpop assess d.decision --profile core/v1 --as-of 2026-09-19 --history
```

`assess` discovers the real record within the workspace's ancestor boundary,
honors configured and registered locations, follows record pointers, and captures
the actual pending, target and hypothesis observations. It preserves the ordinary
reader's versioned findings, typed conflict identity, historical formulas and
missing or unavailable readings. Attention selection does not rerun evaluation or
change assessment scope. The ordinary reader and explicitly declared core records
retain their distinct interpretation; declaring core does not translate legacy text.

Explicit computations use verified Lean programs. A runtime bundle contains
`resources/reasoning/<target>.kpopper-runtime` and
`resources/ordinary/<target>/{build.json,epistemic-core}` beside the executable
(`epistemic-core.exe` on Windows). `KPOPPER_NATIVE_RESOURCES` can explicitly select
that resource directory and `KPOPPER_NATIVE_CACHE` can select its extraction cache.
Absent programs produce unavailable computation findings; invalid selected resources
refuse. Reads do not build programs, install dependencies, or search PATH for a substitute.

### Read findings and impact

Ordinary records support seeded reads and transitive impact, including physical
hypotheses, pending contributions, source labels and private-draft counts:

```sh
kpop --frozen pull p.input d.decision
kpop affects p.input
```

Core records also support `open --json` and `check`. They share one captured
schema-v3 assessment with `pull` and `affects`. Ordinary and core reads do not
parse or validate optional page layouts; `check` names the explicit application
verification command when a layout exists. The bounded page-secondary envelope
and its checks remain available to application consumers. Findings keep the executing adapter's fingerprint, so a Rust result and
a Python result can carry different revision identities even when their semantic
findings match.

Ordinary `open`, `check` and `pull --history` use the captured record and retained
replacement file. They preserve namespace ordering, review flags, pointer and
hypothesis orientation, private-draft counts, historical decisions and final source revalidation.
`open --json` returns the view with the workspace, record and `record_sha256` of the
bytes it read; the other read and write commands wrap their text, error and exit code
in `--json`. Existing feasibility-
store workspaces retain their explicitly marked experimental opener. Complete
command compatibility and final distribution acceptance remain in progress.

Core records can be exported as bounded, read-only Markdown or Mermaid excerpts:

```sh
kpop --frozen export d.decision --direction support --depth 2
kpop --frozen export d.decision --format markdown-mermaid --details
```

The export uses the complete captured assessment before selecting nodes, keeps
missing/potential/executed relationships distinct, and reports omitted nodes and
boundary links for both ordinary and core/v1 records. `--json` wraps the text and
exit status; it does not turn the excerpt into a new assessment schema.

Local contribution state can be inspected or materialized without remote reads:

```sh
kpop knowledge status
kpop --frozen knowledge status
kpop knowledge materialize REVISION --out /absolute/new-snapshot
kpop pending status
kpop pending configure --remote origin --target main --branch contributions --grant
kpop pending pause --reason "review pending changes"
kpop pending resume
```

Status preserves live/frozen contributions, conflicts, cached publication state,
private draft descriptors and unavailable targets. Materialization validates the
complete portable evidence closure and atomically creates an absent destination;
it refuses an existing output. Pending configure/pause/resume retain local scope,
permission and decision receipts under publisher and project locks; these commands
do not contact a remote. `history adopt --revision REVISION [--choose SUBJECT=VERSION]`
is the public adoption route; migration import is exposed by `history migrate` with
an explicit destination copy.

### Build optional HTML applications

```sh
kpop experimental hub --out report.html
kpop experimental hub --verify
kpop experimental hub --checks report.html
kpop page --open
```

`page` is a compatibility alias. The native Hub supports ordinary and core/v1 records,
including all ten ordinary layout components and arrangement history. It renders one captured
assessment, preserving exact typed display values and the bound page envelope.
Source links are relative to the output location, including paths with spaces,
Unicode, `#` and `%`. `--verify` writes no HTML. Default builds go to
`.kpopper/build/page.html` for `GROUNDING.yaml` and ignore that build directory.
Explicit output must be an HTML file; captured inputs and leaf symlinks cannot be
overwritten. Successful builds replace the output atomically.

Annotated Documents package authored HTML and explicit source checks in a portable copy:

```sh
kpop experimental annotated-doc guide
kpop experimental annotated-doc build --html report.html --manifest manifest.json --out checked.html
kpop experimental annotated-doc inspect checked.html
kpop experimental annotated-doc refresh checked.html --sources sources.json --out refreshed.html
```

`document` is a compatibility alias. Build and refresh protect their inputs and
declared sources, and require `--overwrite` to replace an existing output copy.
Refresh retains omitted source snapshots and prepares grouped corrections for review.
Inspection validates embedded evidence without executing authored scripts.

### Retain a checked core session

For a direct read, use `kpop context d.decision`. It captures the current record
and defaults to support depth 1 and 2,000 tokens. Use `--direction impact` for dependents
or `--revision REVISION` to require a previously captured view. It shares the reader,
resource prerequisites, source revalidation and omission reporting below; it does not
need a prior `session open` or modify the record. The transport spelling remains supported.

```sh
kpop session open --no-settings --input GROUNDING.yaml --project example --state /absolute/private-state
kpop session read --no-settings --input GROUNDING.yaml --project example --state /absolute/private-state --ref / --revision REVISION
kpop session context --no-settings --input GROUNDING.yaml --project example --state /absolute/private-state --id d.decision --direction support --revision REVISION
kpop session search --no-settings --input GROUNDING.yaml --project example --state /absolute/private-state --query "decision" --revision REVISION
kpop session propose --no-settings --input GROUNDING.yaml --project example --state /absolute/private-state --revision REVISION --kind inferred --text "A proposed reading" --basis node:p.input --revisit "Recheck when the input changes"
kpop session serve --no-settings --input GROUNDING.yaml --project example --state /absolute/private-state
```

Opening a `core/v1` record captures and assesses once, then retains the detached
context under its revision. Later processes check a fresh source Snapshot and
read retained findings without loading an evaluator. Changed sources, project/input
identity, navigation profiles or retained bytes require reopening. Budgets use the
real `o200k_base` or `cl100k_base` tokenizer; records and workspace discovery require Git.
`context` follows declared support or impact edges within explicit depth, node and
token limits. It returns exact complete node reads and lists omitted values and
unread incident edges. `search` ranks candidate references, prioritizes known IDs,
and binds continuation cursors to the exact query, revision and ranking. Branch
hints only break ties. Native term ordering is deterministic; legacy Python can
vary its floating-point score sums across processes. Unicode folding retains the
Python 3.14 Unicode 16 mappings. Semantic/hybrid requests can use the optional local
`Xenova/multilingual-e5-small` model through `--embedding-dir`. The directory must
contain `model_quantized.onnx` and `tokenizer.json` from revision
`761b726dd34fb83930e26aab4e9ac3899aa1fa78`; both files are checked against their
pinned sizes and SHA-256 hashes before loading. The native provider uses tract-onnx
and tokenizers, keeps its index in memory, and never downloads model files or sends
record text to a service. Missing, mismatched, unsupported or oversized inputs
produce an explicit lexical fallback; ordinary record reads do not require the model.
The stdio MCP server exposes `kpopper_open`, `kpopper_read`, `kpopper_context`
and `kpopper_search`, plus `kpopper_propose` for private pending proposals.
The compatibility `kpopper_verify_claims` tool explicitly refuses the legacy
assertion grammar on `core/v1`; its callers must read the bound core finding.
Proposals retain their base revision, recorded references and unverified external
locators without changing the canonical record. Repeated identical proposals retain
their first bytes and timestamp; `pending` and `proposal:ID` reads show stale bases.
`session status` validates the packaged programs without creating a cache. `session setup`
prepares the verified native runtime cache from the bundled programs. `--rebuild` retains
the previous cache and extracts a fresh verified copy. Both commands require
the complete native distribution; they do not download or compile code.

`session enable [--tokens N]` saves native preferences for the current project; `--global`
applies a machine default. `session disable` stores an explicit disabled preference.
Native preferences live beside existing Python preferences, which remain a read-only
fallback. Project preferences take precedence over machine defaults. `KPOPPER_SESSION_CONFIG`
selects an explicit settings file; `KPOPPER_SESSION_DISABLE=1` disables inherited settings.
Enablement records the native executable and validates declared navigation profiles.
These commands do not install or modify host hooks. Session-state storage has been exercised on Unix
and Windows, including physical identity, no-clobber creation and changed-source checks.

### Author an active history record

```sh
kpop add p.hours v=10
kpop set p.hours 12 --as-of 2026-09-19
kpop review d.schedule
```

An implicit first `add` creates a compact node-history record, in both Simple and
Advanced projects. There is no storage choice to make. A reserved identity without
a first committed entry is not an empty published record; an interrupted creation
can be retried or recovered through the same guarded publication path.
Core authoring needs the selected core runtime archive; it does not
require the ordinary computation program. Subsequent add/set/review operations on
active history retain exact retry journals and immutable prior versions. Plain
command-line numbers and `true`/`false` retain their types; other scalar text stays
text, while explicit YAML lists/maps retain their structure.

Private writes are retained outside the project. `recover` also resumes or rolls
back an interrupted first write. `--hypothesis NAME` supports named authoring during
bootstrap and on active records, including recovery of its own receipts.
The readable record and its history retain exact bytes across Git checkouts; first
creation appends the required scoped rules to `.gitattributes`, preserving existing
rules. A single ignored accepted-view checkpoint helps turn manual edits into
proposals. It is authenticated against committed history, never treated as authority;
the index or `HEAD` can supply an exact baseline after a clone or branch switch.
An explicit `history reconcile --record-proposals --baseline FILE` always uses that
file and never silently substitutes another baseline.
Existing Simple-mode ordinary records support byte-preserving `add`, `set`,
`review`, `same` and `distinct`, including pointer/shard ownership, comments,
quoted Unicode values, private-draft refusals and interrupted-write recovery. These
writes preserve the existing record format. An Advanced project's own ordinary
record takes the same local writes, named hypotheses included; a write routed to
the project is captured as a contribution instead. Named hypotheses, legacy
hypothesis files and advanced contribution routing are available through
`add/set/review --hypothesis NAME`, `consolidate`, and
`history adopt --revision REVISION`; their privacy, source, journal and recovery
boundaries still apply.

```sh
kpop answer q.second_boiler d.one_boiler --why "the inspection settles it"
kpop answer q.annex_floor --dropped "the extension was cancelled"
kpop correct m.annex_load v=45 --why "page 3 says 45"
```

`answer` closes an open question in place, in both record kinds. The question keeps its
text and its own fields and gains one key: `answered`, holding the answering entry in `by`,
its verdict or value in `said`, the day in `of` and the reason in `because`, or instead
`dropped` with the day and the reason. On active history, the accept act pins the
answering entry's head in `read`. `open` and `check` flag the question again when that
answer later moves, breaks or disappears. `correct` rewrites an entry that no commit holds
yet; outside Git, only the session that wrote it may do so. On active history it records a
`correct` act over the old head, so that version is marked corrected rather than replaced.
Everything resting on the entry must be unlanded too, and it is flagged rather than
rewritten. Anything already landed is refused and directed to a named hypothesis and the
fold. Unlike the explicit `history correct` act, the command addresses the entry by id and
applies these checks.

Named hypotheses on active native history can be previewed, folded or refuted:

```sh
kpop consolidate --dry-run trial
kpop consolidate trial
kpop consolidate --refute trial "tested and refuted"
```

Omitting names selects every active group. Preview preserves record and evidence
bytes; both implementations may create the empty project coordination lock.
Fold and refutation use the same privacy, journal and recovery boundaries as other
public history writes. Cross-branch `--from` and legacy-record consolidation use
the corresponding branch and ordinary consolidation paths, with explicit source,
choice and evidence checks. `--json` wraps the command's text, error and exit code.

For the earlier feasibility writer, supply an absolute path to an existing **empty** directory:

```sh
kpop --workspace /absolute/disposable/workspace init --record-id example
kpop --workspace /absolute/disposable/workspace add p.hours \
  --value 10 --source calendar --operation observation-1 --on 2026-09-19T12:00:00Z
kpop --workspace /absolute/disposable/workspace set p.hours \
  --value 12 --operation observation-2 --on 2026-09-19T13:00:00Z
kpop --workspace /absolute/disposable/workspace open
kpop --workspace /absolute/disposable/workspace history p.hours
```

Values use JSON syntax. `null`, booleans, arbitrary-size integers, finite floating
point values, strings, lists and string-keyed maps remain distinct. A repeated
operation with the exact same request is idempotent; different content under that
operation refuses. Optional `--expected-revision` compares the revision returned
by `open`. Each new observation requires a new operation and an explicit recorded time.

`session-start` consumes a host JSON payload on stdin and resolves its `cwd` (or the current directory).
`--host claude|codex` supplies the matching skill guidance; `--cursor` returns
`additional_context` JSON and binds the cursor conversation identity. Enabled checked
sessions use the invoking project preferences, including when its record is external.
Opening also carries first-use guidance and a bounded followup summary.
For ordinary public records it runs the same captured opener and returns an executable
`KPOPPER_AGENT_CONTEXT.command`; explicitly marked feasibility stores retain their
older private opener. It ignores subagent payloads, and reports failed opening visibly
without blocking the host. It saves a private baseline for a valid session ID and
preserves that baseline on resume or compaction. An absent record gets an empty
private mark so its first write can be checked without creating a record at opening.
The command does not install hooks or establish host trust.

`session-context` consumes a real prompt payload and assesses against that baseline.
In a workspace without a record it prints nothing until the session's first write creates one.
It returns `UserPromptSubmit` `additionalContext` with exit 0. Private receipts attribute
exact published bodies and suppress repeated findings; child agents do not consume them.
An unavailable assessment is context rather than a request to continue the conversation.
`session-stop` is a silent compatibility no-op for older hook registrations.
`mark STATE [RECORD ...]` and `gate STATE [RECORD ...]` expose the same baseline and
assessment for explicit callers; `gate --session ID` enables once-only delivery.
They preserve compatible Python marks, inherited failures and unchanged judgments
newly falsified by updated readings. Neither command checks optional HTML layouts.
Only verified ingestion receipts can establish the purpose of a captured report.

Successful direct and first-record writes retain optional private session receipts
for their exact changed bodies. Failed writes, previews and private drafts receive
no authorship receipt; an unavailable receipt store never changes a committed
write's success. These receipts support session attribution and grant no source
access or record-write permission.

For a record whose native history authority is already active, `history status`
captures and revalidates the authority, committed objects and reduced subject heads
without writing:

```sh
kpop --workspace /absolute/workspace history status
```

`history reconcile` describes differences between the rendered history and the
editable record. `history rebuild` replaces a view only when reconciliation proves
that no unrecorded edits would be lost. Both retain the existing immutable evidence.

`history migrate` previews an import; `history migrate --to /abs/absent-directory`
materializes and verifies a separate copy. It preserves source bytes and does not
activate the source record. `--record` selects an explicit record and `--read-mode`
selects live or frozen migration capture. The default is frozen in Simple mode and
live in Advanced mode. Use `history migrate --read-mode frozen` to select frozen
migration input explicitly; the general reader `--frozen` flag does not override it.

Explicit history acts target an immutable version with a recorded reason:

```sh
kpop history retire --subject p.input --of VERSION --because 'no longer used'
kpop history accept --subject p.input --of VERSION --because 'verified again'
kpop history reconcile --record-proposals --proposal-subject p.input \
  --because 'retain the edited reading for review'
kpop recover --json
kpop recover --rollback --json
```

`refute`, `correct` and `propose` use the same explicit-act interface; `--over`
names each version superseded by an acceptance or correction. Private claims are
retained as drafts outside the project. Edited-view proposals remain unaccepted.
An exact, policy-bound journal is retained before publication. Recovery completes
those bytes; rollback can cancel only an uncommitted operation with unchanged
mutable before images. Interrupted immutable object publication is preserved.
If an external edit prevents completion, the refusal identifies the retained
journal. Preserve that edit and inspect the transaction's verified images before
restoring the conflicting file and retrying `recover`. An already committed
operation cannot be rolled back; deleting its journal bypasses the recovery guard.

Adoption and runtime capability declarations are public as `history adopt --revision`
and `history capabilities --nonce NONCE`. The old subject-oriented `history <subject>`
command is available only inside an explicitly marked feasibility store.

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
discovery or installation; the CLI routes explicit runtime selection and reports
unavailable or invalid resources rather than searching PATH for a substitute.
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

`kpop identity --typed` reads the canonical tagged representation on stdin,
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
The strict history decoder rejects YAML aliases, including recursive aliases;
ordinary-source decoding accepts bounded nonrecursive aliases and refuses cycles,
including cycles in custom collections. A write targeting a cyclic record or named
hypothesis refuses before changing its source bytes. An invalid, unselected hypothesis
remains separate from the canonical record; it is not silently folded or discarded.

## Library authoring

### Authority lifecycle

`history_activation` prepares and publishes same-record activation, lossless
deactivation and recovery. It requires an explicit launcher `Selection` and a
caller-owned `Deployment` guard that excludes managed writers and source replacement
through publication. The lifecycle holds project policy and ordered record/member
directory locks, validates the actual pending ledger and publisher observation, and
reimports retained originals independently before accepting the candidate images.
Launcher probes execute the selected programs with a fresh nonce and expected
source/schema/runtime digests. They currently validate `product-python/v1` declarations.

`history_group_activation` applies the same checks to an explicit set of Git
worktrees sharing one project. Readiness and durable completion precede release of
the per-member journals. Cancelling a started activation retains its immutable
evidence and fences the reserved generation at a new legacy epoch. Divergent user
edits stop recovery. A decoded mutation or group envelope cannot supply the live
deployment and lock context. These library entry points do not select real migration
targets or activate an installed deployment; callers must provide the explicit
target, launcher selection and deployment guard.

### Immutable authoring

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

`history_migration::Plan` prepares an existing authored record for a new immutable
history generation in a separate directory. It preserves original bytes and topology,
archived claims, original provenance gaps, physical hypotheses, inactive generations
and sealed live observations. Publication verifies the copied record and refuses an
existing destination atomically. Source-free replay and inverse export validate the
complete sealed inventory and its import receipt; inverse export never replaces a live
record. Live authority activation remains a separate integration boundary.

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

- The experimental store commands do not support judgments, dependency pins, formulas, competing histories, review/refutation,
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
