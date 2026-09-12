# Followups and daily review

A followup is a commitment to return to a piece of work when a stated condition holds.
Keep its connection to recorded knowledge, its trigger and its outcome. The task itself
belongs in the user's existing task system or directory; when none exists, use the private
fallback. A source's age or a scheduled review does not by itself invalidate a conclusion.

## Capture while the context is available

Use this when the user defers work, wants to revisit a decision, or authorizes a later check.
Avoid inventing dates, sample sizes, acceptance criteria or authority. Ordinary reversible
details can follow the user's existing instructions; ask only for a missing material decision.

1. Run `kpopper followups status`. Honor its existing store, pinned workspace and record.
   Discover the user's task system from the work context and connected tools. A suggested
   local path still needs its project identity checked; the same basename can name two repos.
2. Configure once with `kpopper followups setup --timezone AREA/CITY --store ABSOLUTE_DIRECTORY`
   or `--store https://TASK-SYSTEM/PROJECT`. With no existing destination, omit `--store`;
   `--private` deliberately selects the private fallback if another directory was suggested.
   `--record ABSOLUTE_PATH` can pin a registered/shared record. The default pins the record
   currently located by `kpopper open`. Prefer a durable workspace over a disposable worktree.
3. For an existing task, read it through its native connector or local file and set `task`
   to its canonical HTTPS URL or absolute path. For a new task in a remote system, create it
   there using the user's authorized connector, then link it. Never silently create a local
   duplicate because a provider is unavailable. Local capture creates one Markdown task in
   the configured directory. Existing unrelated files and indexes are not rewritten.
   If the established store has an index, use its existing regeneration workflow after capture
   (for example the user's `/followups` skill); do not hand-edit its generated index.
4. Feed a YAML or JSON spec to `kpopper followups add --file -`. Use a stable id so a repeated
   capture is idempotent. Inspect near-duplicates with `list` before adding another item.

```yaml
id: trial-readout
title: Recheck the trial after its observation window
why: Decide whether the current recommendation still holds
how: Read the new report, compare its sample and guardrail, and report the decision needed
related: [trial.sample, trial.guardrail, c.continue_trial]
when:
  any:
    - all:
        - at: '2026-10-01'
        - condition: {id: trial.sample, op: '>=', value: 500}
    - condition: {id: trial.guardrail, op: '<', value: 0.8}
scope: Read the trial report and propose a decision; no production changes
executor: kpopper
```

The example's thresholds and date illustrate an explicit policy; they are not defaults.
For an existing task, use `task: /absolute/path/item.md` or an HTTPS URL instead of
`title`/`why`/`how`. The file or remote task remains authoritative for the work details.
`scope` records previously granted work, not permission minted by the presence of a field.
If a dedicated routine/session already owns execution, set `executor` to its stable reference
(for example `codex:automation-id`). The scanner reports delegated work without taking it,
including while its owner is paused. Resolve ownership before switching to `kpopper`.

## Conditions and evidence

Each trigger has exactly one key. Combine leaves through nonempty `all` or `any` lists.

| Trigger | Meaning |
| --- | --- |
| `at: '2026-10-01'` | Local midnight in the configured IANA timezone; offset timestamps support exact times |
| `changed: trial.sample` | The recorded semantic value differs from the followup's last reviewed baseline |
| `condition: {id: trial.sample, op: '>=', value: 500}` | A typed scalar comparison; numeric ordering only |
| `completed: previous-followup` | That followup was explicitly finished with outcome `done` |
| `external: {ref: 'https://github.com/org/repo/pull/12', equals: merged, max_age_hours: 24}` | A fresh, evidenced observation of an external event |
| `manual: 'The owner confirms that the budget is available'` | Human condition, reported as unknown until deliberately revised |

All graph references must appear in `related`; prerequisites must already exist and cannot
form cycles. Conditions over computed premises use the current calculated value, including
exact rational numbers. `changed` retains the complete snapshot, so a changed formula can
trigger review even when its result is unchanged. An unavailable calculation stays unknown.
Missing data and unsupported conditions are unknown, not false evidence. A graph
change is visible only once it is recorded; use existing source ingestion to bring in real news.
This syntax does not execute shell commands or interpret arbitrary predicates.

After a connector or human reads an external event, record its observation:

```yaml
ref: https://github.com/org/repo/pull/12
value: merged
observed_at: '2026-09-10T10:30:00+03:00'
evidence: The PR API returned mergedAt and the merge commit reference
```

Pass it to `kpopper followups observe --file -`. Observation times require an explicit offset;
future, older and conflicting same-time readings are rejected. Linked remote tasks additionally
need an observation at their exact task URL with `value: open`, based on a fresh read within
24 hours. Here `open` means confirmed available for this runner to work; normalize the provider's
status and preserve its original state in the evidence. An item owned elsewhere stays delegated.
If the task is completed elsewhere, use `resolve ID --outcome done --evidence TEXT`
after confirming the outcome (or `cancelled` for deliberate abandonment). Do not assume
that losing access or deleting a file means completion.

## Review and finish

`scan` is read-only and reports reasons, occurrence identifiers, omitted counts and at most one
graph maintenance candidate. `show ID` returns the complete item and retained attempt history.
`list` includes closed items for continuity. Read the canonical task before acting.

```text
kpopper followups scan
kpopper followups show trial-readout
kpopper followups claim trial-readout --occurrence SCAN_OCCURRENCE --owner UNIQUE_HOST_SESSION
kpopper followups finish trial-readout --token RUN_TOKEN --outcome checked --next-at OFFSET_TIMESTAMP --evidence 'The data window is incomplete; the source specifies this next read date'
```

Claims last 30 minutes; `renew ID --token TOKEN` extends a live run. Use unique session identities,
not branch names. Another session's claim is never yours merely because it uses the same branch.
Recheck before each material action and keep the token while working.

- `checked` retains the finding and rearms a justified future check. It is not completion.
- `done` or `cancelled` closes the obligation with evidence, retaining the task and attempt history.
- `needs_user` parks a material decision. Do not repeatedly notify about an unchanged parked item.
  After reconciliation, `resume ID --evidence TEXT` preserves the baseline and next check date;
  use `refresh` only when deliberately changing the specification or accepting reread task content.
- `release ID --token TOKEN --evidence TEXT` returns uncompleted work to the queue after confirming
  no uncertain action remains. An expired claim becomes `interrupted`; reconcile its effects first,
  then `recover ID --evidence TEXT`. Expiry never authorizes blind repetition of an external action.

Inputs that change during a run preserve the result as `needs_user`, rather than silently closing
against a different world. An edited local task also needs a reread: `refresh ID --file SPEC
--evidence TEXT` updates the specification and baseline deliberately. Closed items stay closed;
capture a new linked obligation when another cycle is needed. To relocate a missing pinned record,
use `relocate --record ABSOLUTE_PATH --evidence TEXT` after reconciling active runs. Other worktrees
share the queue but never silently replace its pinned record with their branch's version.

## Recommend and connect daily review

The user-facing setup command is **`/kpopper:watch`** in Claude Code, or **`$watch`**
in Codex. It checks the host, adopts a matching schedule, installs or repairs it when requested,
and independently reads the result back. `check` inspects only; `resume` permits enabling a
paused review. The underlying `kpopper followups daily install` returns the host-agent work
packet and validates inspection/readback receipts. Follow [the watch skill](../watch/SKILL.md)
through completion; do not hand the user a packet and call it installed.

Strongly recommend a short daily review for ongoing work, in addition to event checks. Explain it
once per workspace when deferred work first arises; respect `kpopper config --guidance off`. Acknowledge the
explanation through `kpopper _agent shown followups`. Use prior authorization; do not ask again
after the user has opted in. Choosing the precise daily time is a reversible preference.

1. Run `kpopper followups daily plan --time 09:00`. It returns the timezone, complete daily prompt
   and any existing binding. The default is a suggested time, not an activated schedule.
2. Inspect existing host schedules first. Reuse the equivalent daily coordinator or dedicated
   owner. A local Claude cron list does not establish that another host has no automation.
3. Within the user's opt-in, use the host's actual scheduling tool to create/update the daily
   review using the returned prompt and timezone. In Codex use the available automation tool;
   preserve its heartbeat-versus-standalone semantics. In Claude use its supported persistent
   Desktop task or Routine capability. Session-only loops are unsuitable for durable daily work.
   Detect available capabilities; do not invent tool names or write scheduler configuration files.
4. Read back the created schedule and pass `{host, id, state, evidence}` to `daily bind --file -`.
   `state` is `active`, `paused` or `missing`, as actually reported by the host. This command records
   an observation; it does not create, pause or resume anything. Re-inspect older bindings:
   `daily status` marks reports older than 24 hours unverified. Unsupported/inaccessible scheduling
   stays visible; manual and session-driven reviews still work.
   A fresh `missing` report for a deleted schedule permits binding its replacement id; the old
   binding remains in history. An active or paused owner cannot be silently replaced.

The scheduled host must reach the pinned workspace, record, ledger and task destination. A cloud
Routine cannot read private laptop paths without an explicitly provided connection. Keep local
data on a compatible local host; do not upload it to make a schedule work.

The daily prompt starts with `daily start --owner UNIQUE_HOST_SESSION`. Its token serializes the
workspace's daily coordinators and its local-date receipt prevents repeating a completed day.
Claim tasks with `--daily-token TOKEN`; at most three actions are reserved per run. The prompt
allows one useful graph maintenance action: a relevant source refresh, an open question or a flagged
decision. Use the existing writer and authority boundaries. Rereading YAML never proves a claim or
refreshes `seen`. If there is no useful authorized work, record a quiet outcome with `daily finish
--token TOKEN --evidence TEXT`. `daily renew` and `daily recover` mirror the item lifecycle.

Only meaningful new findings, completion, failure or required user action deserve notifications.
Daily activity and unchanged holds do not. Event hooks in Claude Code and Codex surface changed
readiness while a session is active; they neither start actions nor provide a durable wakeup.
The daily host schedule catches elapsed dates and missed events. Exact-deadline work can use an
additional host trigger after checking ownership, and still uses the same item claim.

## Storage and rollback

### Record scope and retention

The daily review uses one explicitly configured record. Setup pins `--record` when supplied,
otherwise the record located from the selected workspace. It does not automatically select
the main checkout, enumerate sibling worktree graphs, or merge their findings. The queue's
identity is shared across related worktrees, while its record path remains pinned.

The scheduled host starts or resumes an agent session, whose prompt instructs it to perform
ready authorized work. The Python coordinator selects and claims work; it does not execute
task instructions or spawn an independent session per followup. An external executor is
preserved as an owner, not automatically awakened. A blocked or undecided item is reported
for the user instead of being treated as completed.

Maintenance selection is currently limited to one flagged judgment from the configured
record. The agent may perform one useful authorized source refresh or open-question check,
but broad discovery of ageing sources and branch-specific knowledge is not implemented.
Completed attempts remain in followup history. Material knowledge belongs in the project
record through the ordinary source/reading writer; `finish` itself never inserts it there
or refreshes a judgment's review snapshot.

Use a durable workspace/record and an installed runtime for a production schedule. Removing
a worktree does not remove the private followup ledger, but a record or runtime pinned inside
that tree becomes unavailable. Preserve record changes in commits or the shared knowledge
store before cleanup. Then relocate the record explicitly and recheck installation if needed.
An explicit relocation changes the pointer; it does not recover uncommitted deleted content.

This repository's tracked record additions travel with their commits and merge into the main
record through the usual review process. Private execution logs outside the repository survive
worktree removal, but they are not automatically part of the project's knowledge graph.

The versioned `followups.yaml` ledger lives outside the product repository at
`$XDG_STATE_HOME/kpopper/followups/<workspace-key>` (absolute XDG path), otherwise under
`~/.local/state/kpopper/followups`. Its identity is shared across Git worktrees and separates
subprojects. Local details use the chosen Markdown directory or the ledger's `items/` fallback.
New local files carry a `Routine` ownership marker understood by the legacy `/followups` runner;
existing items are linked explicitly and their existing owner must be respected.

The generated task's original text stays readable; edits require an explicit reread and refresh.
Completion retains it and the ledger history. The obligation's status is read through kpopper;
remote task state remains managed in its own service and is reconciled through evidenced reports.
Writes require POSIX locking and atomic replacement. Reads remain available without file locking.
Ledger corruption, unsupported versions and unavailable stores are reported, never replaced.
To stop automatic review, pause its real host schedule and record the inspected paused binding.
The files remain usable for manual review; removing the plugin does not delete task history.

Each state-changing transaction keeps the preceding ledger as `followups.previous.yaml`.
Event-delivery receipts live separately and do not rotate this rollback point.
Keep normal backups for longer history. After inspecting a valid backup, `restore --backup
ABSOLUTE_PATH --evidence TEXT` quarantines the current ledger byte-for-byte and restores the
same workspace identity. Unfinished tasks are parked for reconciliation and restored claims
become interrupted, so rollback cannot silently replay uncertain effects. A backup can omit
work that occurred later; reconcile against canonical tasks before reopening anything.

`next_at` is the explicitly requested retry time. `wake_hint` is the evaluator's next possible
time boundary (including evidence expiry); it never postpones a currently ready item. A
checked outcome advances its baseline to the inputs actually read. A new relevant graph or
external event can trigger a check earlier, while an unchanged item waits for its retry time.
Refreshing the timestamp of the same external value does not manufacture a new occurrence.

Location indexes preserve the configured workspace identity even when its record disappears.
Other worktrees share that identity; the pinned record remains explicit. Persistent writes
currently require POSIX (macOS/Linux). Windows supports inspection of copied state and the
pure trigger evaluator; it cannot operate this persistent queue. IANA timezone data ships as
a dependency so time interpretation does not depend on an operating system's timezone files.
