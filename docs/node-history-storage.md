# Experimental node-local history storage

Node history is an explicitly marked native format for a single `GROUNDING.yaml`. Its
authority marker is version 3 with `profile: node-history/v1`, `authority: history`, and
`requires: [node-history/v1]`. The marker keeps legacy writers from changing a
node-history record. Existing records and ordinary `history migrate` continue to use the
established history format by default.

To make a separate node-history copy, run:

```sh
kpop history migrate --node-history --to /path/to/new-copy
```

The copy includes a scoped `.gitattributes` policy (`-text`) for the record, history,
and retained evidence, preserving exact bytes through Git even with `core.autocrlf`.
Portable exports reconstruct the same policy; existing repository settings are not changed.
When setting up a marked record manually, install these byte-preserving rules before the
first Git commit. Newline conversion of a hash-bound file is an integrity change.

The destination must be absent. The command builds and verifies a sibling temporary
copy, rechecks the source, then publishes with a no-overwrite rename. It does not switch
the original record to the new format. A copy of an active legacy history retains the
exact authoritative entry, marker, commits, objects, cancellations, retained migration
files, and relevant physical originals in a hash-bound ZIP archive. Active transactions
are converted in causal order with their original semantic object IDs and exact source
mapping order. Repeated ancestor object references do not create duplicate claims. The
final checkpoint binds the current readable view and its semantic receipt components.
Earlier legacy transactions retain their original receipts in the archive; the
conversion does not invent historical node-format view hashes. Inactive and cancelled
generations remain archived evidence, separate from the active reduced state.

A copy of an ordinary record archives its exact source closure and creates initial
claims with explicit unknown historical author, operation, and pin provenance. It does
not infer past observations from the present document. Physical hypothesis files keep
their exact raw bytes at their original paths and enter the copy as named proposals,
without acceptance. Ordinary-reader folding remains unsupported; the implemented fold
boundary is `core/v1` known nodes.

For native authored actions, a subject-local ledger groups the objects emitted by one
action into one frame for each touched subject. Slots keep the distinct original
semantic IDs for claims and acts, while common authored fields are stored once. An
`emit` list identifies this action's objects; old slots do not become new claims. The
guarded writer serves supported Simple-mode add/set/review and explicit disposition acts
on marked records. The current readable body stays in `GROUNDING.yaml` for a new
unchanged singleton. Its original event binding and any receipt tail live in
`meta.node_history`; the stream is created when later changes require retention.
Historical streams are JSONL events with explicit parent, merge, encoding base, and
result hashes. Storage event IDs are distinct from semantic object IDs.

Each original `saw` set is represented by a sparse observation node with an explicit
encoding base, additions, removals, cardinality, and digest. Replay reconstructs the
exact direct observation set and validates the original object identity and source-order
witness. Publication ancestry constrains what an observation could contain; it does not
turn acceptance or transaction order into a source observation.

Permanent transaction manifests bind the authority hash, causal parent manifest digests,
touched frame hashes, view hash when present, and evidence paths and hashes. Native
authoring uses a compact `node-ledger-authoring/v1` context: bounded options and action,
archive and evidence references, a current-template replacement or delta from a parent,
compact replay audits, and small result metadata such as notes or diagnostics. Claim
bodies live with their subject. Retained legacy receipts reconstruct exactly; new
receipt-shaped output is a projection of the recorded intent, object IDs, template and
small result metadata, not a retained full diagnostic report. Full reports and complete
per-write world inventories are not stored in manifests. Exact report or edited-view bytes, when
required, are separate immutable evidence files referenced by hash.

Temporal authoring retains a constant-size commitment to the historical snapshot,
attention policy, operational limits, and semantic result digests. Reads reconstruct
that snapshot from the verified history and check replay against the commitment;
serialized results carry transient witnesses so their verification cannot be forged.
These witnesses and full reconstructed snapshots are not permanent transaction data.

Publication is journaled under the directory guard. Before a durable manifest appears,
recovery removes only verified journal-owned additions; after the manifest appears, it
finishes the same transaction forward. Capture and export verify the complete committed
closure, including evidence hashes, and a portable export can reconstruct an isolated
source-free copy. Semantic replay remains required for authoring; a successful byte
publication alone is not evidence that an action was admissible.

The format remains experimental. Full-closure verification is retained, and no scoped
fast-read integrity policy is enabled. Model-free storage and full-writer measurements
are pending; no throughput, memory, 10k-writer, or launch-readiness claim follows from
codec or copy tests.

Pending branch-union or source-clock journals written by earlier unreleased node-history
prototypes must be recovered with their original runtime before upgrading. This runtime
refuses those journals explicitly instead of replaying them through a different planner.
Original retained legacy receipts remain readable; asking for an original full receipt
from a compact native operation returns `node_receipt_not_retained`.
