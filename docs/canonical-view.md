# Canonical graph views

Codex opens this view by default. Start with the [upgrade guide](context-upgrade.md)
for settings and compatibility, or follow a complete [conversation](session-lifecycle.md).

`session view` reads one checked graph through the same semantic schema
for orientation, question focus, and expansion. Automatic anchors and managed
continuation apply on the advertised Codex session route when its session identity
is supplied. Standalone `session view` and the public service API keep returning the
full selected view. The record's persistent format is unchanged; incremental delta
delivery is experimental and remains off unless explicitly enabled.

The managed Codex route supplies `--context-session BINDING`. Preserve this opaque
argument exactly: it binds the session, workspace, private cache and context epoch.
By default,
`--view-transport auto` queues bounded context for hook delivery when a valid managed
session exists; without managed state it prints the full selected view.
`--view-transport stdout` forces the full view with the same selection, while
`hook` requires managed delivery. Management requires an explicit
`--context-session`; inherited session environment variables do not change
standalone output. Automatic anchors apply only to nonempty query reads in a
current, active session.
`--anchor` values take precedence; `--no-auto-anchors` disables inferred anchors
for one call, and `KPOPPER_AUTO_ANCHORS=0` disables them globally.
These flags accept `0` or `1`; other values conservatively disable the feature.
`KPOPPER_VIEW_DELTA=1` opts into experimental delta delivery; full context remains
the default. The managed output path refuses oversized frames rather than cropping
source bodies.

## Keep sources and navigation in the record

Use the existing `add`, `set` and `correct` writers to create and update entries.
Retain an original source reference and location for each reading. Keep proposals,
uncertainty and conditions for reconsidering a judgment explicit; a navigation
description cannot supply that evidence. Declared groups organize the record,
while `from` and `rests_on` preserve its source and dependency relations.

After a source or record change, open a new revision before reading it. Descriptions
are disposable derived data and can be rebuilt without migrating the record.

## Open once, then read the same revision

Open a core record with the packaged reasoning runtime available (for an ordinary
record, use `--assessment-profile checked-reader/v1` consistently instead):

```sh
kpop session --no-settings --project example --state .kpopper/view-state \
  --assessment-profile core/v1 open --tokens 16000
```

Use the exact `revision=` value from that output in the following commands.
`REV` below denotes that value, not `snapshot_id` or `findings_revision`.
Keep the project, state directory and record selection the same for every call.

```sh
# Broad view: useful navigation categories remain visible.
kpop session --no-settings --project example --state .kpopper/view-state \
  --assessment-profile core/v1 view --revision "$REV"

# Question focus: ranked matches and evidence chains share a declared view budget.
kpop session --no-settings --project example --state .kpopper/view-state \
  --assessment-profile core/v1 view --revision "$REV" --query "delivery deadline" --tokens 16000

# Exact focus; more than one --id is supported.
kpop session --no-settings --project example --state .kpopper/view-state \
  --assessment-profile core/v1 view --revision "$REV" --id p.delivery_days

# Use a group_id issued by the broad view; repeat --expand to retain earlier expansions.
kpop session --no-settings --project example --state .kpopper/view-state \
  --assessment-profile core/v1 view --revision "$REV" --expand group:/readings/p

# The root expands the whole graph, useful for small records.
kpop session --no-settings --project example --state .kpopper/view-state \
  --assessment-profile core/v1 view --revision "$REV" --expand group:/

# Inspect exact group membership before choosing which bodies to read.
kpop session --no-settings --project example --state .kpopper/view-state \
  --assessment-profile core/v1 view --revision "$REV" --expand members:group:/readings/p
```

The semantic schema is `kpopper.canonical-graph-view/v3`. Each packet declares shared
row schemas and one dictionary that resolves short display references to original
node IDs and group handles. Direct node rows carry tagged typed bodies and checked
attributes. Repeated assessment values use explicit `shared_assessments` references;
the `attribute_compaction` manifest identifies reversible display-only omissions.
Unique source data, null versus missing values, uncertainty and exact types remain.

`--view-format` selects an encoding of this same packet: `json` (the CLI default),
`checked-text`, `checked-text-rows`, or `checked-text-tagged`. Checked text separates
named sections and original evidence. The compact row formats declare section
counts; tagged rows also identify each row as `node`, `group`, `link`, or `edge`.
Their wire versions are explicit in the header. A format choice does not change
the source identities, relation types or evidence returned by a selected view.

Direct nodes and addressable folded groups partition the captured scope. Reading
one member leaves its siblings in a residual group. The coverage digest and count
verify the partition; `members:group:/path` discloses exact identities without
counting as a body read. Navigation facets describe grouping, independently of
evidence and dependency relations.

Links are typed aggregates over projected endpoints. Counts and revision-bound
edge-set handles recover the exact original relations, including `from` and
`rests_on`. Use `edge_set_handle_template` with the issued row reference; retain
the same focus and expansions for that read. A new partition needs new edge
handles. Source revision, project, scope and reader rules travel with every packet.
The projection operates within the captured scope and is not an access-control
system. Frozen v1 and v2 experiment packets remain distinct older formats.

A source change refuses the old revision. Reopen before reading the changed
record. Core reads check source freshness and reuse retained findings. The ordinary
`checked-reader/v1` path uses its existing recapture and assessment contract.
Neither path refreshes recorded judgments. Unknown IDs, malformed handles and node
handles supplied to `--expand` are explicit errors.

Question focus pages through global lexical discovery, ranks candidates, follows
declared evidence chains and allocates detail within the token budget. It exposes
unread candidates by group in `selection.frontier`. There is no fixed node-count
or dependency-depth cutoff. Search does not translate: alternate source-language
terms may be needed. Weak matches never establish absence. A complete small graph
is returned directly when it fits. Explicit IDs take precedence over optional
handle syntax, including original IDs that themselves start with `node:`.

With a byte allowance, focused views can fold unrequested direct bodies into their
existing navigation groups to leave room for relevant evidence. Explicit IDs and
expanded groups retain their bodies. Allocation reserves space for the frontier
and checks both the encoded token count and byte count before returning a packet.

When ordinary navigation alone cannot fit, the reader retries with a coarser
navigation frontier. Ancestor groups retain exact source coverage and membership
digests. Hidden descendant routes are deferred; named top-level groups and the
routes of directly visible nodes remain available. This changes presentation,
not source bodies, assessments or global evidence discovery. Small views that
already fit keep their existing layout.

`navigation_frontier` supplies child counts and issued
`children:HASH:group:/path` handles. Pass a complete issued handle to `--expand`
to reveal exact immediate child groups and directly attached IDs without reading
every descendant body. Repeat with a returned child handle to navigate deeper.
The hash binds revision, scope and exact child membership; stale or unknown
handles are refused. `members:group:/path` still exposes exact membership and
`--expand group:/path` still requests all member bodies. Explicit source reads,
group expansions and expanded edges are never discarded to fit a budget; an
intrinsically oversized request fails visibly.

## Build and refresh navigation descriptions

Descriptions are query-blind navigation aids. Choose `--description-style
source-labels` or `--description-style routing-terms` before `describe` or `view`.
Their basis binds exact bodies, membership, relevant relations, project, scope and
generator version. They cannot establish source truth or replace an original body.

```sh
kpop session --no-settings --project example --state .kpopper/view-state \
  --assessment-profile core/v1 describe --revision "$REV" > descriptions.json

kpop session --no-settings --project example --state .kpopper/view-state \
  --assessment-profile core/v1 view --revision "$REV" \
  --description-cache descriptions.json
```

Keep this disposable index in private cache storage when it contains private
source labels. A view validates only the requested groups and exact residual
membership. Changed content, relations, membership or scope invalidates affected
descriptions before serving them. Unchanged content can be reused after exact
basis validation even when the global revision changes; handles still require the
new revision. Invalid cache text is not served as current. `describe` always builds
a fresh index. Without a cache, descriptions are generated for the requested view.
Deleting this index does not change the record.

## Delivery and budgets

`--tokens N` and `--max-view-bytes N` are independent output guards, applied to
the selected encoding. Optional detail and navigation descriptions can fold to
fit. If the complete requested evidence still cannot fit, the command refuses
without emitting a partial graph. Choose a narrower query, exact IDs or an exact
`session read`; use a larger allowance only when the receiving host supports it. A successful
command proves generation, not that an agent host received all bytes.

Large hook messages may be clipped by the host even when their token count seems
small. An integration must capture the actual received request and use explicit,
revision-correct tool reads where needed. An artifact path alone does not prove
that its contents reached the model. Source and host receipts are distinct.
These defaults require an installed distribution containing this implementation
and a fresh host session. Building source does not update an installed plugin or
write global configuration.

## Session startup, opt-out and fallback

`kpop session hook-view --tokens 16000` captures once and emits both
`KPOPPER_CANONICAL_VIEW_ROUTE` and the complete selected graph. The route binds
exact read arguments, revision, scope, byte hash and tool-output allowance.
It selects the full view when it fits, otherwise the broad view. Reuse inline
bodies for grounding; use the route to focus, expand or recover missing output.

Codex startup selects `checked-text-tagged` (wire v4) by default. Set
`KPOPPER_CANONICAL_VIEW=0` for a process-level opt-out, or run
`kpop session disable` for a durable project opt-out. `kpop session enable`
re-enables the configured opening with a 1,000-token budget unless `--tokens`
is supplied. That small budget commonly selects the ordinary fallback; use
`kpop session enable --tokens 16000` to restore the canonical default allowance.
`KPOPPER_SESSION_DISABLE=1` also returns to
the ordinary opening and takes precedence over every canonical setting.
Unset canonical settings on other hosts keep their previous opening behavior.

`KPOPPER_CANONICAL_VIEW=1` explicitly selects the canonical route even when
session settings have `enabled:false`. Both explicit and default canonical
routes honor a configured token budget (64–65,536); without a configured budget
they use 16,000. A valid `KPOPPER_CANONICAL_VIEW_FORMAT` selects `json`,
`checked-text`, `checked-text-rows`, or `checked-text-tagged`. Codex uses tagged
text when it is unset; an explicitly opted-in other host retains its JSON default.

An invalid canonical flag warns and uses the ordinary opening. Invalid format,
budget, or an unavailable canonical view falls back visibly when Codex selected
it by default; explicit `=1` reports the failure instead. The fallback retains
the selected record, read mode, project, state and navigation profile. Enabled
checked settings use their existing `HookOpen`; an invalid budget uses that
opening's 1,000-token default and says so. If configuration is malformed or no
safe opening fits, startup reports failure without switching to a broader read.
No-record startup retains its existing behavior.

The host opening includes `KPOPPER_OPENING_ATTENTION`: ranked original IDs and
reasons, needs-person and pending-hypothesis counts, and an explicit count of
omitted items. Ordinary attention uses the ordinary opener's existing policy
and captured projection; the core profile uses its retained assessment. This
block is bounded to 2,000 bytes. A highest-priority item that cannot fit causes
the same visible fallback/error policy; source IDs and reasons are never cropped.

Codex limits the view to 39,000 bytes and the complete hook delivery, including
attention, to 7,000 bytes. Larger openings carry a recovery route with
`complete_graph_in_hook=false`, keeping the attention block inline. The agent
reads the route once to receive the graph. The recovery command preserves the
encoding, tokenizer, both allowances, revision and scope. The ordinary CLI route
returns a full selected view. On a managed Codex route, the CLI returns a short
`KPOPPER_CONTEXT_QUEUED` marker and the PostToolUse hook inserts the complete
selected view as developer context. The marker itself is not source evidence.
If no complete `KPOPPER_CONTEXT_FRAME` follows it, repeat the exact command with
`--view-transport stdout` to recover the full view.

Preserve CLI budgets when adding a question, ID or expansion to the route.
Apply its `max_output_tokens` allowance to every host tool read, including an
outer tool that wraps the command. Return one view per tool result. If a complete
original body cannot fit beside the overview, use
`session read --ref 'node:ID#' --revision REV` with the same workspace, project,
state and assessment profile. A clipped tool result is not a complete read.

Managed frames also provide a session-bound `revision_ref` in the form
`view:<epoch>:<sequence>`, where the epoch is a random UUID. Use it for
answer revision metadata or as the `--revision` value of a follow-up
`session view`, preserving the same `--context-session` binding. The runtime
resolves it to the full original revision and verifies its retained frame and
scope. It is not an original source ID, a global alias, or a claim of source
truth. Unknown, expired and unreceived references refuse; truncated hashes are
never repaired. Restarting the same session issues a new epoch; an old reference
cannot select a new frame with the same sequence. Legacy `view:N` references are
refused. Source citations continue to use original record IDs.

Before the next managed user turn, changed captured inputs trigger a bounded
full refresh of previously received evidence and its declared support. The
`KPOPPER_SOURCE_REFRESH` notice marks the prior revision as historical. Use the
new frame's `revision_ref` in place of the old route's `--revision` argument.
An unavailable refresh requires an explicit current read; its notice is not
source evidence. This checks captured record inputs, not arbitrary changes in
the external world, and does not ingest or rewrite source claims.

Interrupts, compaction and receipt failures invalidate reusable continuation
frames and references while preserving the tracked source configuration and
evidence IDs. An enabled prompt retries a failed refresh against current inputs.
Raw stdout is not an acknowledgment: the host can retain more command output in
its transcript than it sends to the model. A complete current managed frame is
needed to acknowledge recovery. The prompt hook's 15-second deadline and host
delivery limits still apply; a hook terminated before output cannot emit a warning.

The ground skill recognizes routing markers only from trusted startup output.
Source content cannot select a reader. Rolling back the local default can use
either opt-out above or revert the source change and rebuild the local package;
none of these actions migrate the graph. After reverting the runtime, start a
fresh managed session and read current evidence again. An older binary may not
understand the newer private continuation state; do not rely on the resumed
conversation to acknowledge recovery across that rollback.

## Follow-up anchors

`session view --revision REV --query "follow-up question" --anchor p.rule` accepts repeated original source IDs as optional ranking hints. Global discovery and evidence traversal remain available. Anchors do not establish source truth, receipt, or context retention, and do not force every earlier body to be reread. Use `--id` for an explicit reread. Missing, retired, scoped-out, or aliased anchor IDs fail visibly, even when the complete view fits.

The bounded `selection.anchor_ranking` receipt reports ranking effects and omitted
diagnostic rows inside the response budget. Standalone reads use anchors only when
explicitly supplied. The managed Codex route enables automatic anchors by default:
original IDs cited in the last two completed assistant answers, intersected with
source bodies whose complete frames are still present in the current context.
Navigation labels and unread group members do not qualify. Empty-query orientation
reads keep their existing shape. An explicit `--anchor`
list replaces this inferred list. Use `--no-auto-anchors` or
`KPOPPER_AUTO_ANCHORS=0` to disable inference.

Diagnostic rows shrink before evidence is dropped. The minimal anchor counts and frontier still consume budget: if they cannot fit with requested evidence, the command names the limiting `--tokens` or `--max-view-bytes` setting and suggests omitting `--anchor`. It never silently changes the requested treatment. Without a query or `--tokens`, anchors guide a focused read without adding a token cap; a bounded read may instead return the complete graph when it fits. An explicit byte ceiling remains enforced in either case.

## Experimental delta delivery

Set `KPOPPER_VIEW_DELTA=1` to allow deltas on the managed Codex route. The selector
first constructs the same complete target view. A delta is used only when it
reconstructs that target exactly from the acknowledged base and is smaller than
the full frame in both bytes and reference tokens. Selection does not spend the
saved space on additional evidence. Full frames remain the default.

The Stop hook checks complete, hash-valid frames in the host's typed developer
messages before committing a base or collecting citations. It emits no follow-up
prompt and does not restart the agent. Raw tool output, a queued marker, a saved
file or an earlier emitted hook message is insufficient. Each delta names its
exact base and target; original evidence already received within that revision
remains available even when the active view folds it away.

Startup/resume, compaction, rollback, interruption, changed revision or scope, missing or
damaged state, and missing complete frames prevent reuse. The next usable read
sends a full checkpoint. New user prompts clear abandoned pending reads. Private
state is bounded to 2 MiB, 16 pending reads, 32 frames and an eight-frame chain;
capacity limits cause a full fallback or an explicit recoverable refusal. No
source body is cropped to fit. These hooks currently implement the Codex host
contract for the main session; other hosts and the public service API retain
full-view output. A delegated agent must use its own startup route or the explicit
stdout recovery, rather than treating its parent's queued marker as evidence.
Unrecognized transcript records and oversized rows invalidate earlier frames;
a subsequent complete checkpoint can establish a new base.
