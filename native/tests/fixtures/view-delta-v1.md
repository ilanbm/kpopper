# Selected-view delta V1 contract

`view-delta-v1.schema.json` describes the experimental delta payload.
`view-delta-v1.json` contains an independently selected canonical V3 base and
target, tagged base text, delta JSON/text, expected cumulative bodies, original
ID resolution results, and complete wrapper byte measurements. Run
`cargo test --manifest-path native/Cargo.toml --test view_delta` and
`python3 native/tests/view_delta_resolver.py` for Rust/Python interoperability.
The Python program is an executable golden checker with negative cases. It is
not a production receipt client or a complete replacement for Rust V3 validation.
`view-delta-v1-reuse.json` adds a fixed-target case with seven retained bodies
and one newly received body. The first fixture deliberately exercises full
fallback when the delta is larger; the second exercises delta selection.

The public kernel is `view_delta::{MaterializedView, between, apply, render,
decode, choose}`. Register `pub mod view_delta;` in the product crate to use it.
Tests include the module by path so it can be checked independently.

## Identity and lifecycle

`MaterializedView::from_full` validates and captures a decoded V3 packet. It
does **not** establish acknowledgment, context retention, or a receipt head.
Only a caller with proof of those conditions may offer it as a delta base.
No source graph, selector, session or persistent cache is changed here.

`between(base, target)` compares the normal independently selected target. It
rejects a changed revision, scope, project or typed project identity, or a
changed body under an already received source ID within the same revision.
Receipt issue/acknowledgment, context epochs, forks, eviction and reset are
the caller's responsibilities. Receipt IDs must reject duplicate or sibling
heads **even when their content is identical**: content hashes alone cannot
distinguish a no-op receipt replay. A new full checkpoint starts a new cumulative
body set.

`apply` is transactional and verifies both the exact selected target and the
cumulative body digests. Body receipt is additive. `focus.fold` removes active
node rows only; it does not retract a received body. Dictionary/group/facet
membership is navigation and never populates the body ledger. `resolve` accepts
an original source ID and returns its typed body with full SHA-256. It never
resolves display aliases from a later view. Current attributes, provenance,
uncertainty, exact expanded edges and unknown metadata remain in the selected
packet; the cumulative ledger records bodies, not prior versions of attributes.

## Wire and reconstruction

The text envelope is three lines and ends with a newline:

1. `# Checked graph delta (kpopper.selected-view-delta/v1)`
2. `Evidence additions remain received; focus.fold only folds active rows. References name original identities. Group coverage is not a body read.`
3. Compact JSON serialized from the `Delta` struct in declaration order.

`decode` requires this canonical encoding, rejecting duplicate keys, alternative
field ordering, trailing material and truncation. JSON Schema is structural;
`apply` additionally enforces sorted unique additions/upserts/folds/unsets,
consistent body references, exact focus order, dictionary resolution, snapshot
equality, typed-body validity and digests. `focus.order` is the exact target row
order, not a sorted source set. Map keys are sorted in the JSON values carried
inside the struct. Rust struct fields retain declaration order on the text wire.
JSON nesting is explicitly bounded at 288 levels, allowing the complete
128-level typed-value grammar plus its JSON arrays and the delta envelope.
Both rendering and decoding enforce that bound; parsing does not rely on
serde_json's shallower default recursion limit.

Every source reference in the delta is an original identity. The reversible
projection replaces reference positions with complete dictionary entries:
`{"kind":"node","original":"source.id"}`, group identities or explicit
external identities (including missing versus null). The dictionary becomes a
list of `{slot:[family,ordinal], identity:entry}` bindings. Slots describe the
target's **local encoding**, not cross-read identity. Families are `n`, `m`,
`g`, `f`, `x`; they reconstitute exactly the target dictionary without exposing
a recycled alias as a reference. Unrecognized aliases fail closed.

Node focus rows carry `source_id`, `body_sha256`, and exact compact attributes.
New bodies occur once in `evidence_additions`; old bodies are recovered by
original ID and digest. Other changed top-level metadata is explicitly set or
unset. Group/link/facet rows contain original identities. Link edge-set slots
and expanded-edge handles become numeric ordinals, always scoped by the target
`view_id`. Exact source edge objects remain untouched. The attribute compaction
manifest's node keys become `[identity,rule]` pairs; generated assessment
references become `["a",ordinal]`, and its assessment dictionary becomes
`[ordinal,value]` pairs. All other source strings and metadata are literal:
a quoted `n3` in a body or an unrelated attribute is never rewritten.
`shared_assessments` is transformed only when the manifest actually references
pooled assessments. A preexisting source field of that name, with no pool
references, remains literal metadata even when its value is null or an array.

## Digests and size

All hashes use the full lowercase SHA-256. Semantic digests hash compact,
sorted-key, UTF-8 JSON with no newline. Hash domains are distinguished by the
fields that carry them:

- `project_sha256`: `[project, project_identity]`.
- `base_sha256` / `target_sha256`: the complete canonical V3 packets, including
  dictionaries, metadata, row ordering, unknown attributes and explicit nulls.
- `body_sha256`: the canonical typed-body JSON array.
- `base_evidence_sha256` / `target_evidence_sha256`: the object keyed by original
  source ID containing each received `{source_id,body_sha256,body}` object.
- `Delivery.wire_sha256`: the exact final UTF-8 bytes including the host wrapper.

Rust normalizes JSON token spelling when parsing (for example `-0` becomes `0`
and `1E2` becomes `1e+2`). Its emitted numeric tokens must survive later parsing.
Do not round arbitrary integers or
reserialize decimal/exponent tokens through a binary float before hashing. The
reference Python consumer retains decimal tokens; source typed numeric values
already use exact tagged integer strings and canonical float-hex strings.
Golden files are emitted by Rust so the canonical numeric spellings are fixed.

`choose` validates the target, renders both candidates, and invokes the same
pure host wrapper with `full_checkpoint` or `delta`. The wrapper must include
all model-visible metadata and serialize those kind labels visibly. A delta
is used only when its **complete UTF-8 byte size** is strictly smaller. Missing
or incompatible bases and larger/equal deltas produce an identified full
checkpoint with a distinct `FullReason`. Other integrity errors are errors,
not disguised successful deltas. The returned wire contains only the chosen
candidate. The kernel never sends a full target alongside a delta. Token
budgets remain a separate host check; byte reduction does not prove token,
latency or answer-quality improvement.

This V1 projection supports the current canonical V3 producer, not arbitrary
future codec extensions. Unrecognized alias families or structural encodings
return an error even through `choose`; a caller adding such an encoding must
extend this contract or route its independently validated full response outside
the delta chooser. Integrity errors must not be hidden by a permissive fallback.
The complete changed structural fields are resent in original-identity form;
this can be larger than the ordinary full view. The size decision is mandatory.
