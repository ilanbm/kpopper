# kpopper adapters

The record format and the `kpop` command are shared across hosts. Integration determines
how an agent discovers skills, receives the opening, checks its work and delivers
background results. The shared hook scripts already understand Claude and Codex;
other hosts need an adapter or explicit commands.

## Design principle

**Keep the method shared.** Adapters load the canonical `skills/` directories or
the condensed method in `_shared/method-summary.md`. Files that embed the method
wrap it in `<!-- kpopper:method -->` / `<!-- /kpopper:method -->` markers. Check the
copies after editing an embed:

```sh
sh adapters/_shared/check-drift.sh
```

**Translate the host protocol explicitly.** Similar event names do not imply the
same output contract. Gemini requires startup JSON to put text in model context;
Lifecycle output is advisory context; no adapter requests an automatic continuation. Installing
a plugin or discovering a hook declaration does not prove it executes.

## Capability matrix

This table describes the routes shipped here. The [verification matrix and release
checks](../docs/compatibility.md) state which boundaries have actually been tested.

| Host | Opening | End check | Skills / instructions |
|---|---|---|---|
| **Claude Code** | Native `SessionStart` hook | Advisory diagnostics on the next user prompt; explicit checks still fail | Native plugin loads the bundled skills. |
| **Codex CLI / Desktop** | Native plugin hooks, including grounding and ingestion integration | Advisory diagnostics on the next user prompt | Native plugin skills; a separate plain-project configuration is also available. Validate the actual client. |
| **Cursor local** | Native `sessionStart` wrapper emits `additional_context` | Explicit `check`; legacy stop wrapper is silent | Project rule; this route does not install the bundled skills or the other plugin hooks. |
| **Cursor hosted cloud** | Explicit command; this hook pair has no startup baseline there | Explicit command | Configure instructions and reachable runtime in the remote environment. |
| **Gemini CLI** | Extension wrapper runs the checkout's native runtime and supplies JSON startup context | Explicit `check`; no shutdown notification | Condensed method in `GEMINI.md`; no separate skill catalog in this extension. |
| **Copilot CLI** | `sessionStart` bridge runs the checkout's native runtime | Explicit `check`; legacy `agentStop` is silent | Canonical skills linked in `.agents/skills` plus shared instructions; no async/background hooks. |
| **Copilot VS Code** | Instructions; hook sketch remains unverified | Explicit command until verified in VS Code | `.github/copilot-instructions.md` or existing `AGENTS.md`. |
| **Copilot cloud agent** | Setup workflow installs the native runtime and logs the opening | PR check with the native runtime; merge enforcement depends on branch protection | Setup/CI examples require a kpopper checkout in the job. Cloud lifecycle hooks are not connected by this adapter. |
| **OpenClaw** | Explicit command or agent instruction | Explicit `check`; declared bundle hooks are not runnable hook packs | Canonical skills load as a Codex bundle. |
| **OpenCode** | Agent instruction | Explicit `check`; no enforced gate | Native `skills.paths` loads the bundled skills; `instructions` loads the shared method. |
| **Claude Cowork** | Same package import; runtime behavior needs verification | No verified parity with Claude Code | Claude plugin package; validate the native runtime, file access and persistence in Cowork. |
| **Windsurf / Cascade** | Agent instruction in the shipped rule | Explicit command; an optional hook runs `check` after record writes and discards its output | Condensed rule and an unverified host hook route; not reassessed in this compatibility pass. |

`kpop experimental hub` is an optional experimental application; the native runtime
includes it, and it produces a local HTML page. Opening or delivering that file
depends on the host's browser and filesystem capabilities.
Likewise, loading `map` or `watch` does not supply a worker, source connector or
scheduler. Background delivery needs its own verification.

## Install, per host

- [Claude Code](../README.md#claude-code)
- [Codex](codex/README.md)
- [Cursor](cursor/README.md)
- [Gemini CLI](gemini/README.md)
- [GitHub Copilot: CLI, VS Code and cloud](copilot/README.md)
- [OpenClaw](openclaw/README.md)
- [OpenCode](opencode/README.md)
- [Claude Cowork](claude-cowork/README.md)
- [Windsurf / Cascade](windsurf/README.md)
- [ChatGPT Work and current limits](../docs/chatgpt-work.md)

Preserve existing host configuration when adding an adapter. Keep a full stable
checkout where the guide requires one: copying a skill or adapter directory alone
can break its relative references to shared documentation and scripts.
