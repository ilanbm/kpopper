# Node-local history storage primitives

The native library contains experimental primitives for a future node-local history
format. Public commands still use the existing history format. These modules do not
activate or migrate a record, and the format is not a supported interchange contract.

`history_node_codec` encodes independently hashed JSONL events with typed mapping
patches, explicit initial/change/merge relationships, a bound encoding predecessor,
and a result digest. Missing state differs from a present null. Ordered lists retain
their order. The root history object's sorted `saw` set supports exact additions and
removals; it never infers observation from acceptance. Full stream decoding checks all
parents regardless of file order and bounds frame, stream and reconstructed sizes.

`history_node_current::Original` retains an original event's metadata alongside its
body in the readable current document. The metadata does not repeat the body. First
change reconstructs and verifies the original event before retaining it in a stream.
Pins to that original event continue to identify the same typed state. Storage event
identity is distinct from a legacy semantic object identity; callers must retain and
validate legacy identities when importing them.

`history_node_evidence` separates verified computation recipes from a node's reusable
payload. A recipe replaces an identifier only after recomputing the exact original
identifier. Restoration requires the recorded snapshot context. Unknown recipes and
unrelated snapshot identifiers remain literal. This representation does not authorize
reusing a computation in a different world; semantic admission must still validate all
inputs, clocks, runtime provenance and capabilities.

`history_node_receipt` partitions supported receipts into node pieces and record context.
Document entries, assessment nodes, baseline heads and open acts leave the record context;
full selection is reconstructed from exact sorted assessment membership. Original receipt
digests are checked after typed reconstruction. Unsupported nested reports, physical
hypothesis evidence, nonempty hypothesis documents, and unpartitioned temporal evidence
refuse. This is a decomposition API, not a persistence layout: writers must persist only
changed pieces and derive historical selection from the transaction and node histories.
Storing every partition on each write would still reproduce the graph.
Its `Nodes` preparation view emits before/after images only for changed subjects. A
component absent from an entire receipt side remains retained but inactive; absence
inside an active component removes that node's value. This avoids repeatedly deleting
and restoring baseline fields as before/after sides alternate. Application checks all
before-images before replacing the in-memory state. The typed evidence embedding keeps
normalized maps patchable instead of embedding an opaque tagged transport list.

`history_node_observation` stores exact observation sets as sparse deltas over named
encoding bases. It supports unordered DAG loading and a visitor that applies and undoes
deltas while holding one expanded set across branches. It validates cardinality and
observation digests. The original semantic ID is supplied by the caller; verifying the
legacy object identity remains a separate semantic boundary.

`history_node_semantics::History` restores one exact observation set at a time, verifies
the original object identity and every cross-object reference, then retains the object
without its expanded `saw`. A sparse interval index supplies direct membership to the
existing acceptance reducer. It does not infer transitive observation. Legacy byte-based
reduction keeps its original source-order behavior; the compact provider uses canonical
typed order and does not substitute for retaining original imported YAML bytes.

`history_node_capture::Capture` consumes the complete byte-verified publication snapshot,
reuses its decoded versions, checks semantic operation bindings, and verifies the visible
current bodies against the reduced history. Pin lookup takes an original semantic ID;
storage-event lookup is separate. Pure `prepare_claims` uses the same add/set/review logic
as legacy authoring before receipt materialization or file-image preparation. It does not
write files or grant admission to publish. The legacy path separately constructs its
original receipts and transaction images, preserving its replay contract.

`history_node_publication` exercises append publication behind an explicit version-3
`node-history/v1` experimental authority marker. Existing readers refuse this marker.
The marker must already exist in a disposable fixture; no activation command is exposed.
The current view binds its operation through `meta.node_publication`. Lazy originals
live in `meta.node_history.originals`; a node stream appears only when those originals
need to be retained. Transaction manifests contain touched event hashes and parent
transaction digests, never the whole graph or the complete receipt.

Publication uses the existing directory guard. The journal retains touched stream
prefix hashes, offsets and intended append bytes, plus current-view before/after bytes.
The durable manifest is the commit decision. Recovery without that manifest rolls back
only exact journal-owned tails; recovery with it finishes forward. Unknown tails,
changed authority, changed inventory and unrelated committed corruption refuse recovery.
The verifier receives the exact prepared operation, views and touched frames, and its
execution is bracketed by revalidation. The semantic admission adapter remains to be
integrated. A successful byte-level publication is not a claim of knowledge acceptance.

A complete export can reconstruct an isolated temporary copy without source paths.
It preserves the exact new-format bytes and audits the entire committed closure. It
is not a legacy migration archive and does not preserve original legacy YAML lexemes
unless the caller retains that evidence separately.

The primitives retain full-closure verification. No scoped fast-read integrity policy
is enabled. Further integration must replace eager `saw` expansion and repeated receipt
materialization in public publication before authoring can use this layout. The semantic
provider retains the existing object and reduction-work bounds; it does not establish
10k full-writer support. In particular, passing
codec growth tests does not establish the throughput or memory cost of the full writer.
