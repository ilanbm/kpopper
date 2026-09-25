# Compact node-local history storage

New records use compact node history automatically. It is a native format for a single
`GROUNDING.yaml`; authors use the usual record commands without choosing a backend. Its
authority marker is version 3 with `profile: node-history/v1`, `authority: history`, and
`requires: [node-history/v1]`. The marker keeps legacy writers from changing a
node-history record. Existing legacy records remain readable and writable in their
original format until explicitly migrated; reading them does not convert them.

In Advanced mode, the first canonical publication also creates compact history.
Automatic publication into an existing compact target currently accepts complete
same-authority history. Later plain v1/v2 pending contributions cannot use that path;
scoped history contributions require explicit local adoption. This compatibility gap
must be resolved before enabling the default for workflows that rely on repeated
automatic publication of plain contributions.

To make a verified compact copy of an existing record, run:

```sh
kpop history migrate --to /path/to/new-copy
```

`--node-history` remains accepted as an alias for the same command.

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

A copy of an ordinary record is one `node-history-import/v1` transaction. It creates
the claims and acts of the established history import: current entries, replaced
versions with accept acts, and retirements, all with explicit unknown historical
author, operation, and pin provenance. It does not infer past observations from the
present document. One hash-bound ZIP archive keeps every original file under its import
member name, with the original entry name and the relative layout of members outside
the record directory. Every read reconstructs that layout and replays the import;
changed semantic objects, storage bindings or map fields are refused.

The new entry is `GROUNDING.yaml`; the preview reports any changed entry name and
members kept only in the archive. Referenced evidence files stay at their relative
paths. A live read retains its snapshot as separate evidence. Physical hypothesis files
keep their exact bytes under `.kpopper/hypotheses/` and enter the copy as named proposals,
without acceptance, through a following physical-import transaction.

Absolute original pointers stay exact in the archive. Replay relocates them only in its
temporary copy, using the archived member mapping, and does not read their old locations.
Git and project discovery stop at that temporary directory. An earlier deactivated
history generation keeps its record ID and is archived as evidence; the copy takes the
next generation. A keyed dependency map is refused because the history reader interprets
such maps as version pins. Earlier `node-original-bootstrap/v1` copies remain readable.
Ordinary-reader folding remains unsupported; the implemented fold boundary is `core/v1`
known nodes.

For native authored actions, a subject-local ledger groups the objects emitted by one
action into one frame for each touched subject. Slots keep the distinct original
semantic IDs for claims and acts, while common authored fields are stored once. An
`emit` list identifies this action's objects; old slots do not become new claims. The
guarded writer serves supported Simple-mode add/set/review and explicit disposition acts
on marked records. The current readable body stays in `GROUNDING.yaml` for a new
unchanged singleton. Its original event binding and any receipt tail live in
`meta.node_history`; the stream is created when later changes require retention.
Lazy bindings have a bounded share of the current document's size budget. Once that
budget is full, additional originals retain the same first event directly in their
subject streams, keeping the current view readable without discarding history.
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

Full-closure verification is retained; there is no reduced-integrity fast-read mode.
Migrated legacy archives remain part of that closure. Compact writes avoid adding full
world snapshots to each new manifest, but this is not a throughput or memory guarantee.
In particular, checking an archived legacy history can still dominate the time needed
to read or change a migrated record.

Pending branch-union or source-clock journals written by earlier unreleased node-history
prototypes must be recovered with their original runtime before upgrading. This runtime
refuses those journals explicitly instead of replaying them through a different planner.
Original retained legacy receipts remain readable; asking for an original full receipt
from a compact native operation returns `node_receipt_not_retained`.
