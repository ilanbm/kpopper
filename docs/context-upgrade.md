# Context and conversation upgrade

Codex now opens a canonical view of the project's record and uses cited evidence
to help rank follow-up questions. A selected view contains complete original
bodies, typed relations, and routes to everything still folded. It keeps the
same source identities as `GROUNDING.yaml`.

Read the [conversation walkthrough](session-lifecycle.md) for worked scenarios,
the [runnable example](../examples/context-conversation/README.md) for CLI use,
and the [canonical reference](canonical-view.md) for the wire format and flags.

## What is enabled

| Capability | Codex managed session | Standalone CLI and other hosts |
|---|---|---|
| Canonical opening | Default; explicit opt-out is available | Other hosts retain their previous opening unless opted in |
| Full selected view | Default for each read | `session view` returns the full selected view |
| Automatic follow-up anchors | Default for nonempty queries on an active managed route | Explicit `--anchor` values only |
| Incremental delta delivery | Experimental; off unless `KPOPPER_VIEW_DELTA=1` | Full-view output |
| Persistent record | Existing format and writers | Existing format and writers |

“Full selected view” means the entire chosen packet, including its folded groups.
It does not mean every source body in the graph is expanded. Navigation accounts
for the captured scope; only direct body rows establish that a body was read.

## What changes in a conversation

At startup the agent receives attention items and either a complete opening view
or a bound route for reading it. It can then focus by question, read an exact ID,
expand a group, or inspect group membership. Source and dependency relations remain
distinct from navigation labels. Large navigation trees can fold to a coarser
frontier while preserving routes and exact coverage.

Automatic anchors use original IDs cited in the last two completed answers,
intersected with bodies retained in complete context frames. They influence query
ranking; global discovery and evidence traversal still run. An anchor neither
forces a reread nor establishes truth. Use `--id` when a body must be read again.

On a managed route, the command can return `KPOPPER_CONTEXT_QUEUED`. A PostToolUse
hook inserts a complete `KPOPPER_CONTEXT_FRAME` as developer context. The marker
alone contains no source evidence. At Stop, a local hook verifies complete frames
and completed-answer citations. It sends no model request and does not restart the
assistant.

## Why delivery is checked

A command may generate more text than an outer tool returns. A host can wrap,
truncate, compact or roll back messages. Its append-only transcript can retain
bytes that are no longer in the model's active conversation. The hook can read
those bytes, but cannot assume the model still has them.

The continuation checks therefore accept only complete, hash-valid frames in the
supported host's retained typed developer messages. Raw tool output, a queued
marker and a file on disk are insufficient. This serves runtime correctness as
well as evaluation: automatic anchors need retained bodies, and a delta must
never rely on a base that has disappeared. It is a bounded host-contract check,
not proof that the model understood or used the evidence.

## Delta remains experimental

Delta delivery first constructs the same complete target view. It may send a
smaller change only when an exact compatible base is retained and reconstruction
matches the target. It must be smaller in both bytes and reference tokens. The
space saved is not used to select additional evidence.

This optimization has not established sufficient end-to-end benefit for default
activation. A smaller individual packet does not guarantee fewer model calls,
lower total tokens or a faster conversation. Full delivery remains the default.
Source changes, scope changes and lost history require a fresh full checkpoint.

## Settings and recovery

| Setting or command | Effect |
|---|---|
| `KPOPPER_CANONICAL_VIEW=0` | Use the ordinary opening for this process |
| `kpop session disable` | Durable project opt-out from the configured session opening |
| `kpop session enable --tokens 16000` | Enable with the canonical default token allowance |
| `KPOPPER_SESSION_DISABLE=1` | Hard session opt-out, including explicit canonical opt-in |
| `KPOPPER_CANONICAL_VIEW=1` | Explicitly request canonical opening; overrides `enabled:false`, but not the hard opt-out |
| `--no-auto-anchors` | Disable inferred anchors for one query |
| `KPOPPER_AUTO_ANCHORS=0` | Disable inferred anchors in the process |
| `--anchor ORIGINAL_ID` | Replace inferred anchors with an explicit list; repeat for multiple IDs |
| `--view-transport stdout` | Return the full selected view directly, preserving selection |
| `KPOPPER_VIEW_DELTA=1` | Opt into experimental managed delta delivery |

Boolean feature flags accept `0` and `1`; invalid values disable the feature.
Canonical startup warns on invalid configuration. Default startup falls back
visibly within the original record and scope when possible; an explicit canonical
request reports failure. `session enable` without `--tokens` uses its existing
1,000-token allowance, which often selects the ordinary fallback.

The default canonical allowance is 16,000 reference tokens. Codex view output is
also bounded to 39,000 bytes; the complete startup hook is bounded to 7,000 bytes,
including up to 2,000 bytes of attention items. Large openings provide a route
instead of an inline graph. A body that cannot fit is refused, never cropped.
Provider token accounting may differ from the reference tokenizer.

If a queued marker has no complete frame, repeat its bound command with
`--view-transport stdout`. Preserve its revision, workspace, project, profile,
scope and budgets. If the record changed, reopen first. A delegated agent needs
its own startup route or explicit stdout output; its parent's marker is not a read.

## Installation and compatibility

Install the published plugin and matching native runtime, then start a fresh
session. Existing chats do not retroactively load new hook definitions. A host
may require reviewing changed plugin hooks before running them. Check
`kpop --version` and `kpop session status`, then inspect the fresh startup route;
a version string by itself does not prove hook activation.

There is no record migration for this upgrade. Existing `add`, `set`, `correct`,
`context`, `pull`, `affects` and `check` workflows remain available. Description
indexes and managed continuation state are disposable caches. Disabling the
opening does not delete the record or refresh recorded judgments.

The semantic view schema is `kpopper.canonical-graph-view/v3`. Codex uses tagged
checked text with wire version 4; standalone views default to JSON. Short display
aliases can change between views. Record and cite original IDs, never an alias
as though it were a stable source identity.

## What the checks establish

The implementation checks source revision, typed body fidelity, scope coverage,
addressable navigation, output bounds and safe continuation under the supported
host contract. Tests cover exact expansion, query selection, anchors, delta
reconstruction, damaged state and context-loss recovery.

These checks do not establish source-world truth, authorization to act, model
understanding or a universal improvement in answer quality. The experimental
delta setting should be evaluated on complete tasks before broader activation.
