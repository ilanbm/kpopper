# kpopper for GitHub Copilot

Two different Copilot surfaces, two different mechanisms: VS Code's local agent reads
Markdown instructions and (in Preview) can run hooks; the cloud coding agent gets no
interactive session at all, so the method has to show up as CI instead.

## What this installs

| file | does |
|---|---|
| `vscode/copilot-instructions.md` | the condensed method + explicit open/close instructions. |
| `vscode/hooks.json` | SessionStart/Stop, VS Code's Preview hook schema. **Not load-bearing** — see below. |
| `cloud-agent/copilot-setup-steps.yml` | installs PyYAML, runs `provenance.py open` once, visible in the coding agent's setup logs. |
| `cloud-agent/provenance-check.yml` | runs `provenance.py check` on every PR as a status check — the closest thing the cloud agent has to a stop gate. |

## Verified against docs

Sources: `https://code.visualstudio.com/docs/agent-customization/hooks`,
`https://code.visualstudio.com/docs/agent-customization/custom-instructions`,
`https://docs.github.com/en/copilot/how-tos/use-copilot-agents/coding-agent/customize-the-agent-environment`,
fetched 2026-09-01.

- **`copilot-instructions.md` and `AGENTS.md` are both read automatically** for the
  VS Code local agent — no setting needed for either (a separate setting,
  `chat.useAgentsMdFile`, exists to turn `AGENTS.md` support off, but it defaults on).
  This adapter ships the Copilot-specific filename since that's what the BUILD asked
  for; if the project already has an `AGENTS.md` (e.g. from the Codex adapter),
  Copilot reads that too and there is no need to duplicate the method into both.
- **Hooks are explicitly Preview**: "the configuration format and behavior might
  change in future releases," and an organization can disable them by policy. The
  schema is close to Claude Code's — `hooks.<EventName>` is an array of flat
  `{ "type": "command", "command", "timeout" }` objects (no `matcher`/nested `hooks[]`
  wrapper the way Claude/Codex/Gemini have it) — PascalCase event names including
  `SessionStart` and `Stop`, exit code `2` blocking. **Notably, the docs list
  `.claude/settings.json` and `.claude/settings.local.json` as hook file locations VS
  Code reads directly** — i.e. Copilot has its own compatibility layer for Claude
  Code's hook format specifically. This adapter still ships its own
  `.github/hooks/*.json`-shaped file rather than relying on that, since depending on
  an undocumented-elsewhere compatibility layer for another vendor's config format
  seems the more fragile choice long-term.
- **`copilot-setup-steps.yml` has a hard contract**: the job must be named exactly
  `copilot-setup-steps`, only `steps`/`permissions`/`runs-on`/`services`/`snapshot`/
  `timeout-minutes` are honored (anything else in the job is silently ignored by the
  agent, though the file still runs as a normal workflow otherwise), it only fires
  from the *default* branch, and Ubuntu/Windows runners only (no macOS).
- **`copilot-setup-steps.yml` and `provenance-check.yml` both need to find this
  checkout's `scripts/provenance.py`.** There's no portable in-repo convention for
  where a plugin like kpopper lives once vendored into an arbitrary project, so both
  workflows read a `KPOPPER_ROOT` repository variable rather than guessing a path —
  set it under Settings → Secrets and variables → Actions → Variables, or hardcode the
  path directly in the workflow if this project vendors kpopper at a fixed location.

## Install

    mkdir -p .github/hooks .github/workflows
    cp <plugin>/adapters/copilot/vscode/copilot-instructions.md .github/copilot-instructions.md
    cp <plugin>/adapters/copilot/vscode/hooks.json .github/hooks/kpopper.json
    cp <plugin>/adapters/copilot/cloud-agent/copilot-setup-steps.yml .github/workflows/copilot-setup-steps.yml
    cp <plugin>/adapters/copilot/cloud-agent/provenance-check.yml .github/workflows/provenance-check.yml

Then set the `KPOPPER_ROOT` repository variable (or edit the two workflow files
directly) so the two cloud-agent workflows can find `scripts/provenance.py`, and edit
`hooks.json`'s two relative `command` paths the same way you would for the Codex
adapter — same caveat, immediately below.

## Not load-bearing: `vscode/hooks.json`

This ships because the BUILD asked for it, not because it's something to depend on:

1. It's Preview, can change shape, and an org can turn it off entirely.
2. Its `command` paths (`../../../scripts/session_open.sh`, matching this file's own
   position three directories below the kpopper checkout root) assume hook commands
   run with the workspace root as cwd — the same assumption the Codex adapter has to
   make, for the same reason: no fetched doc states the execution cwd for this
   specific hook system, only that a `cwd` field is *sent to* the hook, which is not
   the same claim. Treat this file as a documented sketch of what SessionStart/Stop
   would look like here, not a working install step, until you've confirmed the cwd
   assumption in your own VS Code build.
3. Whether the Stop payload includes a `stop_hook_active`-equivalent field at all was
   not confirmed by the fetched docs (only a general "common fields" table, which
   didn't list it) — reusing `session_gate.sh` unmodified risks it re-bouncing every
   single stop instead of yielding after one, if that field is simply absent here.

`copilot-instructions.md` carries the actual obligation for the local agent, the same
way `rules/kpopper.md` does for Windsurf; the workflows carry it for the cloud agent.
Nothing here depends on `hooks.json` working.

## Not verified

- Whether `chat.hookFilesLocations` needs to be edited to pick up `.github/hooks/*.json`
  by default, or whether that's already the default (one fetch said it's included by
  default; another showed it being explicitly listed in an example — consistent with
  "on by default, shown for clarity," but not stated in so many words).
- The exact wording/UI of the org-policy control that can disable hooks entirely.
