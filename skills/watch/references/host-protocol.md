# Native host inspection and readback

The command does not possess the host's scheduling tools. The agent uses those tools
and normalizes their actual results into these small receipts. Do not invent tool output or
mark an unknown property verified. Local receipts are not a substitute for host inspection.
Use stdin (`--inspect -` / `--result -`) or a private temporary file, outside product source.

## Inspection

After `daily install --owner HOST_SESSION` returns a token, inspect the bound schedule and
all equivalent dedicated schedules in the host's supported inventory. Match the workspace
marker in the prompt, or verify the older schedule's pinned paths and purpose before adopting
it. Do not classify a different project, a shared coordinator or unrelated work as a match.

Submit `daily install --token TOKEN --inspect FILE` with:

```json
{
  "host": "actual-host-identity",
  "observed_at": "2026-09-10T12:00:00+03:00",
  "complete": true,
  "schedules": [],
  "evidence": "Actual tool results establishing the bound id and complete equivalent-schedule search"
}
```

`complete: false` means listing, access or matching is unresolved and prevents creation.
An empty, complete inventory permits creation only for an installation with no uncertain
prior mutation. A list of multiple matches blocks automatic mutation. Each matching schedule
has these normalized fields, all grounded in its actual readback and environment:

```json
{
  "id": "host-returned-id",
  "state": "active",
  "workspace_key": "the-full-workspace-key-from-the-packet",
  "cadence": "daily",
  "time": "09:00",
  "timezone": "Asia/Jerusalem",
  "prompt": "The entire prompt actually stored in the host",
  "access_verified": true
}
```

Normalize the host's active/paused state and its native schedule into cadence, wall-clock time
and IANA timezone. Use `cadence: daily`, `weekly` or `other` according to the actual schedule.
A weekly or irregular schedule needs repair; do not describe it as daily. If the host cannot
establish its timing or access, report incomplete inspection with the concrete reason.

`access_verified` means the selected scheduled execution environment has been checked for
access to the pinned workspace, record, ledger, task destination and runtime command. Preserve
the prompt's explicit runtime environment so a custom state directory remains reachable.
Installation initializes a new private task directory; an existing missing store needs restoration.
A cloud worker does not
inherit the current laptop's filesystem merely because the plugin is installed.

## Apply once, then read back

The inspection result names `create`, `update` or `none`. It supplies the complete desired
prompt, timezone, time, cadence and `schedule_state: active`. Map that desired state to the
host's enable/resume field as well as updating its text. Use the host's supported operation. Existing
times are preserved unless the user requested a change. Paused schedules require explicit
resume. Apply updates only to the returned id. Preserve unrelated settings.

Use the actual create/update result to locate the schedule, then independently read it.
Submit `daily install --token TOKEN --result FILE` with:

```json
{
  "host": "actual-host-identity",
  "observed_at": "2026-09-10T12:01:00+03:00",
  "schedule": {"id": "host-returned-id", "state": "active", "workspace_key": "packet-key", "cadence": "daily", "time": "09:00", "timezone": "Asia/Jerusalem", "prompt": "Actual complete stored prompt", "access_verified": true},
  "evidence": "Actual independent readback result or its durable reference"
}
```

Observations must be within ten minutes and timestamps need an explicit offset. A mismatched
prompt, timezone, time, workspace or target id refuses completion. A configuration readback
does not prove that the first scheduled execution happened.

If tools are missing or inspection fails before a host mutation is issued, submit
`daily install --token TOKEN --fail REASON` and report the blocker. After a create/update
instruction has been issued, failure is conservatively uncertain. Reinspect it using the
same token. Finding the expected active schedule completes reconciliation; an empty or
mismatched result cannot authorize a duplicate create. Resolve the external uncertainty
before retrying. Other sessions can inspect an outstanding installation but cannot silently
issue another mutation over it.

If the host explicitly rejected the operation before making any change (for example, a
permission refusal whose result states no schedule was created), `--fail REASON --unchanged`
records that definite failure and permits a new inspected attempt after the blocker is fixed.
Never use `--unchanged` for a timeout, lost response, partially completed operation or merely
an empty inventory. It cannot clear an operation already recorded as uncertain.

## Reconcile an outstanding attempt

When the host has established that the original request is no longer pending, submit
`daily install --token TOKEN --reconcile FILE`. The receipt has the same fields as an inventory,
plus `no_pending_request: true`. Include the exact host evidence supporting both the complete
inventory and the resolved request. An empty list by itself is insufficient. At most one
matching schedule may remain; include its actual state even if it is still paused.

This retires the old attempt without creating or changing anything. Invoke install again with
the user's requested options and inspect fresh state before retrying. An uncertain resume
whose schedule is still paused must be reported as an unconfirmed resume, not an intentional
pause or successful installation. Reconciliation also works if the pinned record location
changed during the old attempt; a new reservation uses the current configuration.

Keep user-added prompt instructions. Additions surrounding the current generated prompt are
preserved verbatim. A prior unmodified prompt recorded as managed can be upgraded. A custom
or older prompt that cannot be safely separated needs review before replacement; the command
reports `needs_review` and performs no overwrite. Readback must match the whole preserved or
approved prompt, including any user additions.

Sources for host entry points: [Codex skills](https://learn.chatgpt.com/docs/build-skills),
[Codex scheduled tasks](https://learn.chatgpt.com/docs/automations?surface=app),
[Claude plugin skills](https://code.claude.com/docs/en/skills),
[Claude Routines](https://code.claude.com/docs/en/routines). Actual runtime tool schemas govern
the operation; these links do not establish the current account's capabilities.
