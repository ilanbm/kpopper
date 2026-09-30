# kpopper for Codex

The native `.codex-plugin/plugin.json` selects `plugin-hooks.json` from this directory.
Its commands use `$PLUGIN_ROOT`; the manifest override replaces default discovery of
`hooks/hooks.json`, which belongs to Claude Code. This keeps each host's hook configuration
out of the Codex path.

## Native plugin

Install the plugin first. Plugin installation alone does not install or download the
native runtime, and a globally installed `kpop` is not used by plugin hooks. Run
`sh "<plugin root>/scripts/install_native.sh"` with the plugin root that `codex plugin add`
prints ([Windows](../../docs/plugin-runtime.md#install-the-native-runtime)), then start a
new task. All commands in `plugin-hooks.json`,
including prompt diagnostics, invoke the exact native package binary under `$PLUGIN_ROOT`.


The Codex package loads the shared skill and these hooks:

| Event | Behavior |
|---|---|
| `SessionStart` | Canonical opening or its bound recovery route, attention items, and existing ingestion/watch/grounding hooks |
| `PostToolUse` | Complete managed context frames, grounding/follow-up checks, and asynchronous ingestion/watch delivery |
| `UserPromptSubmit` | Refresh changed sources behind the last acknowledged view, or report unavailable evidence; record diagnostics, session gate, abandoned-read cleanup, and asynchronous ingestion/watch delivery |
| `Stop` | Passive verification of retained frames and completed-answer citations; no new model turn |
| `PreCompact`, `PostCompact`, `Interrupt` | Invalidate reusable continuation state so the next read uses a full checkpoint |

Canonical views and automatic anchors are enabled by default on the managed Codex
route. Delta delivery remains experimental and off. The continuation hook has its
own native byte/token guards; its PostToolUse and UserPromptSubmit definitions use
`additionalContextLimit: 0` to avoid a second truncation of a complete frame.
The prompt refresh has a 15-second timeout. It can report unavailable evidence
only when the host runs the hook and lets it finish. A disabled, killed, crashed
or timed-out hook cannot guarantee that a warning reaches the model; reopen and
read current evidence when hook delivery is unavailable.
See the [upgrade controls](../../docs/context-upgrade.md) and
[conversation lifecycle](../../docs/session-lifecycle.md).

The ingestion hooks read the queue. `kpop ingest capture` retains an explicit report and starts the
independent processor. Routine completion emits no hook output. Important results enter the next
available model request while a turn is active. If the task is idle, Codex queues ordinary async
hook output until the next user turn; the hook does not start one. See the
[ingestion contract](../../skills/kpopper/INGESTION.md).

When the host exposes native background agents and `send_message_to_thread`, the skill uses
`capture --notify-task "$CODEX_SESSION_ID"` and dispatches its returned job to a native agent.
That agent sends only important findings through the ordinary host tool, which can start a turn
after the primary has finished answering. A live job reserves the event for its recipient so
the hook does not duplicate the message. Delivery confirmation leaves the graph and durable
outbox intact; expired or failed jobs retain hook fallback. The
[native worker contract](../../skills/kpopper/DELIVERY.md) describes the handoff.

Codex requires review and trust of the current hook definitions. Use `/hooks` or the equivalent
client prompt. Installing the plugin does not bypass that review.

References: [plugin-bundled hooks](https://learn.chatgpt.com/docs/hooks#plugin-bundled-hooks),
[background delivery](https://learn.chatgpt.com/docs/hooks#how-background-hooks-run).

## Plain project configuration

Prepare the same [runtime](../../docs/plugin-runtime.md) before installing project
hooks: install it into the active checkout's `scripts/runtime/TARGET`; a global CLI does
not satisfy project hooks.

`hooks.json` is also provided for installations using project-level hooks rather than a native
plugin. Codex resolves command paths against the session's working directory, not against the
hook file. Replace **every** relative script path with an absolute path to the kpopper checkout
before copying this configuration to another project:

```json
{"command": "\"/absolute/path/to/kpopper/scripts/ingestion_hook.sh\" codex wait"}
```

Then install the project configuration and skill:

Use the absolute checkout path for `<kpopper>` so the skill symlinks resolve correctly. Each
skill is its own directory under `skills/`, one per occasion, and each needs a link.

```sh
mkdir -p .codex .agents/skills
cp <kpopper>/adapters/codex/hooks.json .codex/hooks.json
# Replace script paths in the copied file.
cat <kpopper>/adapters/codex/AGENTS.md.snippet >> AGENTS.md
for s in <kpopper>/skills/*/; do ln -s "$s" .agents/skills/"$(basename "$s")"; done
```

A plain project hook does not receive `$PLUGIN_ROOT`. Keep the existing record or registered
record pointer in place; do not migrate it for this adapter. Desktop/IDE client behavior should
be validated in the actual client. Native task messaging is a host capability; a host without
that messaging tool does not gain it by installing these hooks.
