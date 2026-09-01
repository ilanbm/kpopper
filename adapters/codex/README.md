# kpopper for Codex CLI

## What this installs

| file | does |
|---|---|
| `AGENTS.md.snippet` | the condensed method + session-open instruction + skills placement note. Append to the project's `AGENTS.md`. |
| `hooks.json` | SessionStart/Stop wired to the existing `scripts/session_open.sh` / `scripts/session_gate.sh` — unmodified, same two files Claude Code's plugin uses. |

## Verified against docs

Source: `https://developers.openai.com/codex/hooks` (redirects to
`https://learn.chatgpt.com/docs/hooks`), fetched 2026-09-01.

- **Schema matches Claude's.** `hooks.<EventName>` is an array of `{ "matcher"?, "hooks": [{ "type": "command", "command", "timeout"?, "statusMessage"? }] }` — matcher is optional and omitting it matches everything, same as the plugin's own `hooks/hooks.json`. So this file is structurally the same document, just at a different path.
- **Payload field names match what the two scripts already read.** The Stop payload includes `session_id`, `stop_hook_active`, `cwd`, `transcript_path`, `last_assistant_message` — the exact fields `session_gate.sh` parses. SessionStart carries `session_id` too. Plain stdout text (not just the `hookSpecificOutput.additionalContext` JSON form) is accepted for SessionStart, matching what `session_open.sh` already does. Net: **`session_open.sh` and `session_gate.sh` are reused as-is, no wrapper needed** — unlike Cursor (different payload shape) or Gemini (session-level hooks can't block).
- **Exit code 2 blocks Stop**, feeding stderr back, same contract `session_gate.sh` already implements.
- **Project hooks need a one-time trust step.** Codex requires reviewing and trusting a project's `.codex/hooks.json` before it runs — `/hooks` in the CLI, or the equivalent trust prompt on first load. Nothing this adapter can do about that; it's a one-time manual step per machine.
- **Skills.** Codex scans `.agents/skills` from cwd up to the repo root, plus a user-level `~/.agents/skills` (`~/.codex/skills` is treated the same). It follows symlinks when scanning, so linking rather than copying `skills/kpopper` keeps it live.

## One real gotcha — relative paths in `command`

`hooks.json` above uses `../../scripts/session_open.sh`, matching this file's own
position two directories below the kpopper checkout root (`adapters/codex/` →
`../..` → checkout root → `scripts/...`). That is **not** portable to an arbitrary
project layout: Codex runs hook commands with the **session's working directory** as
cwd — not the directory `hooks.json` itself lives in. This is documented behavior
(same fetch, "commands run with the session cwd as their working directory") and
there is a live upstream report of exactly this surprising anyone who assumed
plugin-relative resolution: `openai/codex` issue #26675, "Plugin PostToolUse hook
relative command resolves from workspace cwd."

So the shipped relative path is correct in exactly one shape: `.codex/hooks.json`
sitting two directories above a `scripts/` that is actually this kpopper checkout's
`scripts/` — e.g. dogfooding kpopper on itself. For any other project, **do not
symlink this file verbatim** — copy it and replace both `command` values with an
absolute path to this checkout's `scripts/`:

    "command": "/absolute/path/to/kpopper/scripts/session_open.sh"

Codex does expose a stable anchor for hooks that ship inside a genuine Codex
*plugin* bundle — `PLUGIN_ROOT`, set for plugin-bundled hook commands (same doc).
This adapter is a plain project-level `.codex/hooks.json`, not a packaged plugin, so
`PLUGIN_ROOT` isn't available here; if kpopper is ever packaged as a Codex plugin,
`$PLUGIN_ROOT/scripts/session_open.sh` would be the portable form instead of an
absolute path.

## Install

    mkdir -p .codex
    cp <plugin>/adapters/codex/hooks.json .codex/hooks.json
    # then edit the two "command" paths per the gotcha above, unless this project IS
    # the kpopper checkout itself
    cat <plugin>/adapters/codex/AGENTS.md.snippet >> AGENTS.md
    mkdir -p .agents/skills
    ln -s <plugin>/skills/kpopper .agents/skills/kpopper

Then, in Codex, run `/hooks` once to review and trust the new project hooks.

## Not verified

- Whether Codex's SessionStart truly renders plain stdout as context in every client
  (desktop vs CLI vs IDE extension) the same way — the fetch found one summary
  statement to that effect and no worked example. If it silently drops plain text in
  some surface, the fallback in `AGENTS.md.snippet` (the explicit `python3 ... open`
  instruction) still covers it.
- The exact wording Codex's trust prompt / `/hooks` UI shows for a new hook — not
  screenshotted, only described in prose by the docs.
