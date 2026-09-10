# Starting a knowledge record

kpopper can begin in a workspace with no record, including one without Git. The session
opener gives the agent a short instruction to continue your task and offer a starting choice
at a suitable moment. It creates no record and scans no materials by itself.

Choose **Learn while working**, **Initial map**, or **Deeper investigation** in the conversation.
An initial map covers your goals, current situation, commitments, decisions, evidence and
open questions. A deeper investigation adds historical context within agreed subjects,
sources and dates. The agent uses the tools and materials available in that session; choosing
mapping does not install connectors or grant access to other accounts.

You can skip the introduction, change your choice later, or turn off explanations. A one-off
task may finish without creating a record. During work that will be revisited, the record
starts with an actual useful finding and its source. Conclusions are added when there is
something to conclude, and their snapshots are filled by the existing writer.

## Commands

The first-use interface has three operations. When deferred work first arises, the agent
also recommends a short daily review once per workspace. This recommendation is optional
and respects the guidance preference; it does not activate a schedule. The separate
[followups guide](../skills/kpopper/FOLLOWUPS.md) covers capture and host scheduling.

```bash
kpopper open
kpopper map
kpopper map --deep
kpopper config
kpopper config --guidance off
kpopper config --guidance on
```

`open` reads the current context. When no record exists it explains the starting options;
it creates no file. `map` returns a concrete task to the calling agent, which accepts it,
performs the mapping with its tools and reports the result. `--deep` requests a deeper
investigation. `config` reads preferences or updates the guidance setting. Learning during
ordinary work is the default and needs no command.

Select the working directory before the operation:

```bash
kpopper --workspace "/path/to/Weekly planning" open
kpopper --workspace "/path/to/Weekly planning" map --deep
```

Every operation has `--help`. `--json` returns structured output; the existing record
operations preserve their exit codes and include their output and error text in a JSON
envelope. For example, `kpopper check --json` still exits nonzero when its check fails.
When the command is not on PATH, use `python3 <plugin>/scripts/cli.py` with the same arguments.

Mapping currently runs **inside the calling agent session**. It does not launch another LLM
process from a plain terminal. The host hook supplies a session routing identity; Codex tool
calls also carry their task identity. If no session identity is bound, `map` fails explicitly
and saves no task. A session identity is not permission to access additional sources.

The mapping result distinguishes `ready` (returned to the caller), `running` (accepted by
that agent), `complete` (the agent supplied a real report), and `failed`. A ready task is not
execution. The agent's private protocol handles acknowledgement and completion, including a
check of the record when one exists. `open --json` includes the current mapping status, report
path and checker findings. Completion means a mapping result was returned; a nonzero checker
result remains visible and does not become a claim that the record is clean.

The agent receives the detailed workflow and receipt commands through `map --json`. Request
IDs, tutorial acknowledgements and completion calls are internal protocol, not steps users
need to learn. The host's implementation must consume and execute the returned task; merely
printing a task to a terminal does not perform a mapping.

## Location and continuity

Record discovery checks the current directory and its ancestors. It stops at a Git repository
boundary or, for other workspaces, before leaving the user's home or reaching the filesystem
root. It never searches sibling folders or connected accounts. A Git-registered external
record is also supported. A registered path that is empty, broken or unavailable is reported
as a location problem; it does not invite creation of a replacement record.

Preferences live under `$XDG_STATE_HOME/kpopper/first-use`, or
`~/.local/state/kpopper/first-use`. Choices are stored per workspace, shared across worktrees
of the same repository. Introduction and explanation acknowledgements are stored for the
local user so a new project does not repeat the tutorial. These files contain preferences
and acknowledgements, not copies of source material. They are separate from `PROVENANCE.yaml`
and are not synced between devices by kpopper.

The existing record opener still supplies the current view, including an enabled checked
session view. The first-use path works without the optional session dependencies. Claude,
Codex and Gemini use the shared opener; Cursor wraps it in its own response format. Environments
without an active start hook use the shared method's `kpopper open` instruction instead.
Hook wiring and capabilities remain subject to each adapter's documented limitations.

The temporary session baseline can be recorded before a knowledge file exists. If the session
then creates a record, the existing Stop gate can check it. This is a consistency check on the
record, not a requirement to create one or to finish an onboarding tour.
