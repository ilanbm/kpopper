# Capture now, receive only what needs attention

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
| Codex | Background context enters the next available model request | Ordinary async hooks queue context until the next user turn; they do not start a new turn |

When a client closes, the worker and durable outbox remain separate from the delivery hook. On
startup/resume, ready unresolved findings are offered again. Hooks only read and deliver results:
even when Claude routes a wake notification through `UserPromptSubmit`, it cannot create a new
capture or another worker. Compaction within the same session does not repeatedly offer the same
batch.

Starting a turn in an idle Codex Desktop task requires a separate host integration. These Python
hooks have no documented API for that operation. Native agent messaging, where available, is a
different capability and is not part of this command.

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

This writer supports explicit `report` updates to existing stored scalar entries in one record
file. Pointer/multi-file records, hypotheses, new entries, computed values and judgment rewrites
remain questions for the primary or the project's own adapter. Capturing their source does not
silently change those layouts. Preserve a custom project's reader/writer; do not migrate or
duplicate its record to enable this command.

Capture and processing have separate locks. Reports are retained and processed in capture order;
arrival order never substitutes for report date or hides conflicting same-day readings. The
canonical writer adjudicates those dates and conflicts, including after an earlier queued update
was applied. External changes to that target require review. A commit journal prevents a worker restart from
applying the same write twice. Background execution requires POSIX file locking; unsupported
platforms fail closed rather than pretend that writes are serialized.
