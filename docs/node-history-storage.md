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

`history_node_observation` stores exact observation sets as sparse deltas over named
encoding bases. It supports unordered DAG loading and a visitor that applies and undoes
deltas while holding one expanded set across branches. It validates cardinality and
observation digests. The original semantic ID is supplied by the caller; verifying the
legacy object identity and integrating acceptance reduction remain caller obligations.

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
parsing before the public authoring path can use this layout. In particular, passing
codec growth tests does not establish the throughput or memory cost of the full writer.
