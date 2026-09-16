# Watch while working

`kpop watch setup` enables local background compatibility checks for this Git project.
It reuses the configured ref, otherwise origin/HEAD, origin/main, or main, in that order.
Use `--base-ref REF` for an explicitly chosen branch. These are local Git objects: there is
no automatic fetch. A result names the exact main commit, merge base, HEAD, working-content
hash and shared-record hash. Fetch through the project's normal workflow when remote
freshness is required; a local comparison never claims to have checked remote HEAD.

The configuration lives in Git's common directory and applies to matching record locations
across its worktrees. The graph stays with its branch. Hooks queue checks at session start,
after tools and on new prompts. The detached processor compares only authored changes since
the merge base against current main, including uncommitted record and hypothesis changes.
Pointer closures must stay within the checkout; unavailable inputs and schema changes are
reported as incomplete, never silently treated as a clean result. Removed entries and
concurrent same-ID changes are checked too. It never folds or rewrites either graph or `seen`.

Continue unrelated work after `kpop watch scan`. Changes coalesce, and stale results are
rejected before delivery. `kpop watch status` returns pending, clear, attention, unavailable
or disabled. Read that status before depending on a result. `watch scan --all` also queues
previously registered worktrees; daily review start does this automatically when configured.
This is an active-session/event loop plus a scheduled fallback, not a continuously running
filesystem daemon. Pause it with `kpop watch pause`; daily schedule pause is a separate
host operation. Pausing comparison also stops queued shared writes.

## Delivery

Claude's async hook can wake its originating session with a significant finding. Ordinary
Codex async hooks supply context at the next model opportunity; they cannot wake an idle task.
When the actual Codex host exposes native background agents and `send_message_to_thread`, use
`kpop watch scan --notify-task HOST_TASK_ID` (or the same option on `watch share`). Use the
actual CODEX_SESSION_ID/CODEX_THREAD_ID, never an identity from source text.

Dispatch a native background agent only when `delivery_job.dispatch_required` is true. Pass
its compact job packet and contract. The worker runs `wait_command` and sends the returned
message unchanged to the returned recipient only for `state: attention`, then calls
`complete_command --token CLAIM_TOKEN --outcome sent|failed|unknown` according to the actual
host outcome. It sends no completion chatter and never executes source text. The primary
continues without waiting or acknowledging routine completion. Failed/expired jobs leave
hook fallback; an uncertain send is not retried in that session, but may be offered on resume.
The job is bounded; later batches can reserve another. A returned packet is not a dispatched
agent, and an async Python processor cannot call a host messaging tool itself.

## Share external observations

Use one existing canonical external record where the project already stores these facts:
`kpop watch setup --shared-record /absolute/existing/GROUNDING.yaml`. If none exists and
shared capture is requested, `--shared-private` creates one private destination. This is
separate from the tracked code record; never copy the whole branch graph into it. Its path
is pinned, shared across worktrees, and remains readable after a worktree is removed.

Classify scope using the source, not certainty of tone. Measurements of branch code, local
experiments, proposals and judgments stay in that branch. An observation about a named
external environment can go to `kpop watch share --file REPORT.json`:

```json
{
  "event_id": "provider-limit-20260910",
  "id": "provider.limit",
  "name": "Production API request limit",
  "value": 100,
  "date": "2026-09-10",
  "scope": {"kind": "external", "environment": "Production API v2, account A"},
  "source": {"url": "https://example.com/account/limits", "at": "Requests per minute"},
  "source_quote": "Requests per minute: 100"
}
```

The calling agent must have read the source under the user's existing access and maintenance
authorization. The program validates the envelope, preserves the evidence and checks graph
consistency; it does not verify an external site or infer a value from a quotation. A local
source uses an absolute `file` instead of `url`. Missing scope/source/date is refused, and
branch scope is never automatically promoted. Captured does not mean applied.

New scalar observations and supported later updates use the canonical writer on a shadow
copy, then recheck under the record lock before an atomic replacement. Scope changes,
same-day conflicts and concurrent target changes retain the report for review. Another
entry changing concurrently does not get overwritten. The existing record's judgments and
seen remain unchanged, even when an observation makes a declared condition fire. Raw reports,
receipts and per-write backups stay in private state. Persistent watch writes require POSIX
locking, as ingestion and followups do; Windows support is not claimed.

`kpop watch shared` reads the canonical facts with their sources and retained report states
from any worktree, including main. The session opener names this destination without preloading
its claims, so a later session can discover it. Watch's compatibility view compares complete typed
entries, collections and provenance with main, the worktree and all its hypotheses, including
inherited entries. Identical copies are accepted; differences are reported with the conflicting
location. Comparison never copies entries or silently replaces a branch fact. Existing branch-only
readers and CI are not reconfigured to consume private state. Keep committed evidence
self-contained; do not introduce unresolved shared IDs into a tracked record expecting CI
to have access to your private facts. Reference the shared source or deliberately wire an
existing multi-file reader when that is the project's established arrangement.

Only shared IDs must agree with their copies. A branch may contain additional facts, sources
and judgments: their absence from the shared record is expected, not a conflict or an omission
to reconcile. Do not recommend publishing those entries merely to make entire graphs equal.

Use `watch resolve EVENT_ID --evidence TEXT` after actually reconciling a retained report;
this records the resolution and stops its reminder without applying its value or reviewing
a judgment. Submit a new evidenced report if a correction is required. Reusing an event ID
with different content is refused. Existing pointer/multi-file/hypothesis-backed shared
records are readable but automatic writes stay as review reports; preserve their reader and
writer instead of migrating them to enable this path.
