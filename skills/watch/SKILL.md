---
name: watch
description: Check, install or repair the current project's daily kpopper review through the host's scheduling tools. Use for requests to set up daily review, configure a kpopper routine, or check its installation. Ordinary followup capture only recommends this command; it does not authorize scheduling.
---

# Watch

Check whether this workspace already has a working scheduled-review configuration, then
install or repair it when requested. Complete the host operation and verify its readback.
The user should finish with an actual schedule, a confirmed existing schedule, or a clear
blocker. A local plan or installation packet is an intermediate step.

Invocation: `/kpopper:watch` in Claude Code, or select/invoke `$watch` in Codex.
Natural-language requests to set up the project's daily kpopper review use the same flow.

- Default: check and install as needed. A user's explicit invocation authorizes this setup;
  do not ask them to approve the same setup again. Host-required confirmations still apply.
- `check`: inspection only, without creating, changing or resuming a host schedule.
- `resume`: also permits enabling an intentionally paused daily review.
- Preserve a requested time. Otherwise keep an existing schedule's time, or use 09:00 in
  the user's known timezone for a new one. Ask only if the timezone cannot be established.

If this skill was selected while discussing a followup, with no setup request or prior
opt-in, recommend daily review and obtain the user's choice before installing anything.

## Complete the installation

1. Locate the workspace and existing followup configuration with `kpopper followups status`.
   Use the CLI from the current plugin when needed: `python3 <plugin-root>/scripts/cli.py`.
   The plugin root is two directories above this skill folder. Keep its pinned record and
   canonical task destination. Use an existing destination from user context or its verified
   suggestion; a private fallback is available. If no useful knowledge record exists yet,
   explain that prerequisite rather than inventing a graph solely to install a timer.
2. Read [the host protocol](references/host-protocol.md). Run `kpopper followups daily install
   --owner UNIQUE_HOST_SESSION`, adding `--timezone AREA/CITY` on first setup and the selected
   `--store` or `--private` when necessary. Pass an explicit `--time HH:MM` only when requested;
   pass `--resume` only for that mode. For inspection only, use `--check` instead of reserving
   an installation. The returned packet is work for you to execute, not a finished installation.
3. Discover the host's actual scheduler tools and inspect its schedules. Read the bound id
   directly, and look for the packet's stable workspace marker and equivalent dedicated
   reviews. Include paused schedules. A missing local binding is not proof that no schedule
   exists. Read every candidate before classifying it. A schedule shared with other work is
   not yours to overwrite; report that ownership decision. Incomplete visibility stays unknown.
4. Verify that the scheduled execution environment can reach the pinned workspace, record,
   ledger, task destination and the runtime command in the prompt's payload. The payload also
   carries the state-directory environment required for subsequent runs. Use a compatible local scheduler for laptop-local files.
   Do not upload private state to make a cloud routine work. In Codex use the actual automation
   tool and its current heartbeat/project rules. In Claude use the available persistent
   Desktop schedule or Routine tools. Session-only loops do not install a durable daily review.
5. Submit the actual host inventory using `daily install --token TOKEN --inspect FILE`.
   For `check`, report the inspected status directly and stop without mutating the host.
   Follow the returned action: `none` means preserve the existing state; `create` or `update`
   means apply that operation once through the supported host tool. Preserve unrelated host
   configuration, notification preferences and intentional pauses. Never edit the host's
   scheduler files by hand. Do not install another scheduler or buy access as a workaround.
6. Independently read the resulting schedule back from the host. Submit its actual normalized
   fields using `daily install --token TOKEN --result FILE`. Only matching readback completes
   installation. If a response is uncertain, use `--fail REASON` and inspect for the existing
   schedule before any further action; never repeat a potentially successful create blindly.
   A retained installation token can reconcile a matching schedule in a later session.
7. Report the result briefly: configured/already configured, time and timezone, schedule link
   when supplied by the host, and any limitation that affects execution. A confirmed pause
   should be reported as paused. Distinguish installation from evidence that a scheduled run
   has actually fired. Link to the host's management surface for later changes when available.

For trigger, task and review lifecycle details, use [FOLLOWUPS.md](../kpopper/FOLLOWUPS.md).
Do not reread that entire guide merely to install the schedule.
