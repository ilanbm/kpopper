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

These commands are available through the Python package or the plugin's `scripts/kpopper`.
When the command is not on PATH, use `python3 <plugin>/scripts/cli.py start ...`.

| Command | Effect |
|---|---|
| `kpopper start` | Read the current first-use context without writing or scanning |
| `kpopper start status` | Read record location, saved choice and unseen explanations as JSON |
| `kpopper start guide` | Read the agent's mapping and contextual-help workflow |
| `kpopper start choose work` | Remember learning during ordinary work |
| `kpopper start choose map` | Record a request for an initial map |
| `kpopper start choose deep` | Record a request for a deeper investigation |
| `kpopper start complete --request REQUEST` | Mark the requested mapping finished so later sessions do not repeat it |
| `kpopper start shown welcome` | Acknowledge an offer actually displayed in this project |
| `kpopper start shown source` | Acknowledge an explanation actually displayed about a source |
| `kpopper start guidance off` | Disable contextual explanations for this local user |
| `kpopper start guidance on` | Restore explanations for concepts still unseen |

`--workspace PATH` goes before the subcommand, for example `kpopper start --workspace
"/path/to/Weekly planning" status`. Use it when the session's current directory differs from
the directory chosen for this project's record. Other explanation names are `record`,
`decision`, `conflict`, `reuse`, and `review`.

`choose` records the user's selection; the agent performs the mapping. `shown` records that an
explanation was displayed; it does not assert that a finding was saved or a check passed. The
agent must verify that work before showing a success card. Mapping scope and progress remain
in the task and its recorded source; the preference flag is not a replacement for that context.

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
without an active start hook use the shared method's `kpopper start` instruction instead.
Hook wiring and capabilities remain subject to each adapter's documented limitations.

The temporary session baseline can be recorded before a knowledge file exists. If the session
then creates a record, the existing Stop gate can check it. This is a consistency check on the
record, not a requirement to create one or to finish an onboarding tour.
