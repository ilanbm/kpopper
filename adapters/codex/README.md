# kpopper for Codex

The native `.codex-plugin/plugin.json` selects `plugin-hooks.json` from this directory.
Its commands use `$PLUGIN_ROOT`; the manifest override replaces default discovery of
`hooks/hooks.json`, which belongs to Claude Code. This keeps each host's hook configuration
out of the Codex path.

## Native plugin

Install the plugin first. Plugin installation alone does not install or download the
native runtime, and a globally installed `kpop` is not used by plugin hooks. Start a
task and run the exact native installer command printed by the active plugin when its
opener reports the runtime missing (`install_native.sh` on Unix, `install.ps1` on
Windows). All commands in `plugin-hooks.json`,
including prompt diagnostics, invoke the exact native package binary under `$PLUGIN_ROOT`.
For the source-only compatibility path, set `KPOPPER_RUNTIME=python` and follow the
[private Python setup](../../README.md#python-compatibility-mode-for-claude-code-and-codex).
If opening reports a missing native runtime, run the exact installer command shown by
the diagnostic and start a new task. In Python compatibility mode, compare
`doctor`'s `Hook Python` with `KPOPPER_AGENT_CONTEXT.command[0]`.


The Codex package loads the shared skill and these hooks:

| Event | Behavior |
|---|---|
| `SessionStart` | The existing record opener and ready important ingestion findings |
| `PostToolUse`, `UserPromptSubmit` | Asynchronous delivery of newly actionable ingestion results |
| `UserPromptSubmit` | Current record diagnostics as `additionalContext`; no Stop continuation |

The hooks only read the queue. `kpop ingest capture` retains an explicit report and starts the
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

Prepare the same [native runtime or explicit Python compatibility mode](../../docs/plugin-runtime.md)
before installing project hooks. For native mode, install into the active checkout's
`scripts/runtime/TARGET`; a global CLI does not satisfy project hooks. If using
`KPOPPER_RUNTIME_HOME` for Python compatibility, pass the same absolute value to setup
and the Codex process.

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
that messaging tool does not gain it by installing these Python scripts.
