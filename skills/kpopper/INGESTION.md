# Capture now, receive only what needs attention

For an update needed in the current answer, use `kpopper update --file report.json`.
It accepts the same report contract below, processes only that report, and returns its
durable receipt. Exit 0 means `applied`; exit 1 means retained but needing a primary
decision; exit 2 means an invalid request. Inspect `reason`, `diagnostics`, `reach` and
`newly_fired_judgments` before relying on the result. `applied` does not review judgments.
Use one `updates` array for related changes from one report: all changes commit together,
share one source and are checked in their final state. Reuse `event_id` to retry the same
envelope safely. Supported newly authored formulas normalize to structured expressions.

Use the existing graph to find what new information changes while its context is still fresh.
`kpopper ingest capture` retains a source report immediately and starts a separate worker. The
worker writes an explicit update through the same writer as `set`, follows declared dependencies,
and stores a receipt. A move inside a judgment's still-valid condition stays quiet. A newly fired
condition or a new unresolved review question becomes an attention signal.

Use this when the project authorizes ongoing record maintenance and the current answer does not
need to wait for that update. If an answer depends on it, read the resulting status and record
before relying on the changed value. `captured` does not mean `applied` or `checked`.

## One capture command

Given an existing scalar entry `stock.packages`:

```sh
kpopper ingest capture --file - <<'JSON'
{
  "event_id": "stock-report-2026-09-09-1",
  "source_quote": "There are four completed packages in stock.",
  "target": "stock.packages",
  "value": 4,
  "date": "2026-09-09",
  "kind": "report"
}
JSON
```

The worker runs independently of the capture command. Continue unrelated work; do not poll or
wait for routine completion. The plugin's hooks deliver only important findings.

On a Codex host with native background agents and `send_message_to_thread`, add
`--notify-task "$CODEX_SESSION_ID"` and dispatch the returned delivery job. This gives important
findings a route back after the primary has finished answering. See [native delivery](DELIVERY.md)
for the compact handoff and delivery confirmation; no routine receipt needs a primary ACK.

`source_quote` is required. Supply the target, scalar value, report date, and kind when known.
`reason` can preserve your existing interpretation. Optional `event_id` makes resubmission of the
same envelope idempotent; reusing it with different content is refused. Otherwise a fresh ID is
assigned. `session_id` is provenance metadata, never permission to send messages to another task.

When the target or meaning is unresolved, preserve the quote without inventing a mapping:

```json
{
  "source_quote": "The cheque has not been deposited.",
  "question": "Does this refer to cheque A or cheque B?"
}
```

This stays as a captured report needing a decision. No fact is guessed or overwritten. This
version does not invoke a language model to infer missing identity, intent, time, or relationships.

## Several related changes from one source

The primary agent can supply `updates` instead of `target`/`value`, using `update` now or
`ingest capture` in the background. One source quotation and date ground the whole batch:

For a batch with new entries, first read the relevant record with `open --json` or
`search`, and copy its `record_sha256` into the envelope below. Use the hash returned
with the context you actually interpreted; computing a fresh hash just before submission
would hide a stale interpretation. Replace the placeholder before submitting.

```json
{
  "event_id": "cedar-delivery-1",
  "record_sha256": "RECORD_SHA256_FROM_PRIOR_READ",
  "source_quote": "Cedar has five packages. The van capacity is ten packages.",
  "date": "2026-09-10",
  "updates": [
    {"kind": "add", "id": "cedar.packages", "body": {"v": 5, "name": "Cedar packages"}},
    {"kind": "add", "id": "cedar.capacity", "body": {"v": 10, "name": "Van capacity"}},
    {"kind": "add", "id": "c.cedar", "body": {
      "rests_on": ["cedar.packages", "cedar.capacity"],
      "verdict": "Cedar fits one van",
      "because": "The reported package count is within the reported capacity.",
      "wrong_if": "cedar.packages > cedar.capacity"
    }}
  ]
}
```

An existing reading uses `{"kind":"set","id":"cedar.packages","value":6}`. Each
operation may specify `at` for its location within the retained quotation. There are at
most 32 operations and one operation per ID. `add` accepts a new scalar reading (`v` or
`quoted`), a `rule`, or a new judgment in the record's existing vocabulary. `into` can name
an existing collection. Stored readings receive the captured source citation and date;
judgment snapshots are filled by the canonical writer. Do not supply `from`, `at`, `of`,
`src`, `source` or snapshot fields inside `body`.

The agent supplies interpretation and declared links; the worker runs no model and adds no
inferred relationships. Keep source speech in `source_quote` and the agent's conclusion in
the judgment body. Search candidates are not proof that two IDs describe the same subject;
use the existing `same`/`distinct` process for identity decisions.

Existing readings are staged first; additions follow in their supplied dependency order.
Add premises and rules before judgments. The existing writer and final gate validate the
whole staged record, which is then replaced once. Failed operations leave the canonical
record untouched and retain the report for review. Attention is derived from the final
graph, so intermediate states do not produce notifications. A reading that really falsifies
an existing judgment is preserved; ingestion never refreshes that judgment's snapshot.

New entries require and bind to the record hash from the primary's read, checked both at
capture and before commit. This binds recorded premises, not external source-file contents.
If the hash is missing or the record changes before
commit, the batch stays pending for primary review; it cannot silently give a conclusion
new premises the agent never read. Reading-only batches protect all touched targets;
conflicting queued batches require primary review. An applied batch is not reapplied after
a recorded commit is later changed or reverted.

A revised report uses a new `event_id`. Exact retries reuse the original envelope, including
its hash; changing its content under an existing ID is refused.

Existing judgments cannot be replaced or reviewed in a batch. New judgments about the
reader's own `graph.*`/page counts also require ordinary primary authoring, as those counts
can change during the batch itself. Structured rules are computed by the local Lean core;
legacy textual rules retain their unevaluated status. See [expressions](EXPRESSIONS.md).
Multi-file, pointer and hypothesis-backed records retain the
existing review requirement. No routine user confirmation or second agent review is added.

## Status and attention

```sh
kpopper ingest status --event-id <event-id>
kpopper ingest pending
kpopper ingest acknowledge <signal-id>
```

Acknowledgment is optional and concerns delivery only. Use it after addressing an important
finding; it does not rewrite a judgment, refresh `seen`, or make `check` pass. Routine updates
need no acknowledgment. Stored findings are checked against the current graph before delivery,
so a resolved or reverted contradiction is not replayed as if it were current. Immutable source
events and receipts remain available.

`process` performs an explicit bounded queue pass. `capture --no-start` only retains the input;
it is intended for a caller that owns worker execution. Both support `--record <file>` and
`--state-dir <directory>`, as do the read commands. An override must be a new/empty dedicated
directory or existing ingestion state for that exact record; it cannot contain the record or
other unrelated files. Normal operation resolves the same record as `kpopper where`.

## Host behavior

| Host | While the primary is working | After its answer, while the client remains open |
|---|---|---|
| Claude Code | Background findings return through a hook | An important result uses `asyncRewake` and exit 2 to resume Claude; quiet exit 0 does not wake it |
| Codex with native delivery jobs | The native worker sends important findings to the originating task | The same host messaging tool starts a follow-up turn; quiet jobs send no message |
| Codex with ordinary hooks | Background context enters the next available model request | Async hooks queue context until the next user turn; they do not start a new turn |

When a client closes, the worker and durable outbox remain separate from the delivery hook. On
startup/resume, ready unresolved findings are offered again. Hooks only read and deliver results:
even when Claude routes a wake notification through `UserPromptSubmit`, it cannot create a new
capture or another worker. Compaction within the same session does not repeatedly offer the same
batch.

The native Codex path uses the host-provided agent messaging tool. The Python worker and hook do
not call that tool themselves. Capture reserves the job; the primary dispatches the native agent
using [DELIVERY.md](DELIVERY.md). A finite lease preserves hook fallback if dispatch or messaging
fails. This capability is conditional on the actual host's available tools, rather than inferred
from the Codex name or from an active-turn hook test.

Host references: [Claude hooks](https://code.claude.com/docs/en/hooks#command-hook-fields),
[Codex background hooks](https://learn.chatgpt.com/docs/hooks#how-background-hooks-run).

## Storage and write limits

The record stays in its existing location. The queue, raw captured sources, journals and receipts
live outside the repository under `$XDG_STATE_HOME/kpopper/ingestion/<record-hash>`, or
`~/.local/state/kpopper/ingestion/<record-hash>` when that variable is unset or is not an absolute path. Each canonical record
path has its own private directory. New source entries point to the retained text; these paths are
local source references and are not a cross-machine source-distribution mechanism.

The command needs write access to both the record and its private state directory. In a
workspace sandbox, grant the dedicated state path through the host's supported permissions, or
set an absolute `XDG_STATE_HOME` to an approved location. A permission error is a failed capture;
do not report that the source was saved.

This writer supports explicit `report` updates to existing stored scalar entries and batches
of readings/new grounded entries in one record file. Pointer/multi-file records, hypotheses,
computed-value rewrites and existing judgment rewrites remain questions for the primary or
the project's own adapter. Capturing their source does not
silently change those layouts. Preserve a custom project's reader/writer; do not migrate or
duplicate its record to enable this command.

Capture and processing have separate locks. Reports are retained and processed in capture order;
arrival order never substitutes for report date or hides conflicting same-day readings. The
canonical writer adjudicates those dates and conflicts, including after an earlier queued update
was applied. External changes to that target require review. A commit journal prevents a worker restart from
applying the same write twice. Background execution requires POSIX file locking; unsupported
platforms fail closed rather than pretend that writes are serialized.
