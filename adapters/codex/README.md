# kpopper for Codex

The native `.codex-plugin/plugin.json` selects `plugin-hooks.json` from this directory.
Its commands use `$PLUGIN_ROOT`; the manifest override replaces default discovery of
`hooks/hooks.json`, which belongs to Claude Code. This keeps Claude's `asyncRewake` configuration
out of the Codex path.

## Native plugin

The Codex package loads the shared skill and these hooks:

| Event | Behavior |
|---|---|
| `SessionStart` | The existing record opener and ready important ingestion findings |
| `PostToolUse`, `UserPromptSubmit` | Asynchronous delivery of newly actionable ingestion results |
| `Stop` | The existing record gate; no new ingestion wait or approval gate |

The hooks only read the queue. `kpopper ingest capture` retains an explicit report and starts the
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

`hooks.json` is also provided for installations using project-level hooks rather than a native
plugin. Codex resolves command paths against the session's working directory, not against the
hook file. Replace **every** relative script path with an absolute path to the kpopper checkout
before copying this configuration to another project:

```json
{"command": "\"/absolute/path/to/kpopper/scripts/ingestion_hook.sh\" codex wait"}
```

Then install the project configuration and skill:

```sh
mkdir -p .codex .agents/skills
cp <kpopper>/adapters/codex/hooks.json .codex/hooks.json
# Replace script paths in the copied file.
cat <kpopper>/adapters/codex/AGENTS.md.snippet >> AGENTS.md
ln -s <kpopper>/skills/kpopper .agents/skills/kpopper
```

A plain project hook does not receive `$PLUGIN_ROOT`. Keep the existing record or registered
record pointer in place; do not migrate it for this adapter. Desktop/IDE client behavior should
be validated in the actual client. Native task messaging is a host capability; a host without
that messaging tool does not gain it by installing these Python scripts.
