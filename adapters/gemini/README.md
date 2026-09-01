# kpopper for Gemini CLI

Ships as a real Gemini CLI extension — a manifest, a bundled hooks directory, and a
context file — rather than loose files to copy in by hand.

## What this installs

| file | does |
|---|---|
| `gemini-extension.json` | the extension manifest. |
| `hooks/hooks.json` | SessionStart → the existing `scripts/session_open.sh` (works unmodified). SessionEnd → the bundled `scripts/checknote.sh`, advisory only. |
| `scripts/checknote.sh` | runs `provenance.py check` and prints it. Cannot block — see below. |
| `GEMINI.md` | the condensed method, plus an explicit statement that the stop gate is unenforceable here. |

## Verified against docs

Sources: `https://geminicli.com/docs/extensions/reference/`,
`https://geminicli.com/docs/hooks/reference/`, `https://geminicli.com/docs/hooks/writing-hooks/`,
fetched 2026-09-01.

- **Manifest schema.** Only `name` and `version` are required; `description` and
  `contextFileName` are optional (the latter already defaults to `GEMINI.md`, so
  setting it here is just being explicit, not overriding anything). Hooks are not part
  of the manifest — they live in the separate `hooks/hooks.json` shown above, and "the
  JSON structure matches the existing hooks configuration in settings.json," i.e. the
  same `hooks.<EventName>[].hooks[].{type,command}` shape used project-wide.
- **SessionStart's stdout becomes context; SessionEnd cannot block.** Confirmed
  directly: "The CLI will not wait for this hook to complete and ignores all
  flow-control fields" for SessionEnd. There is no exit code or JSON field that stops a
  session from ending — advisory is the ceiling, not a design choice made here. That's
  why `checknote.sh` only ever prints and exits 0, and why `GEMINI.md` says so in
  words, since the hook itself cannot make the point land.
- **`${extensionPath}` is the documented, portable way to reference a file bundled in
  the extension**: "For portability, you should use `${extensionPath}` to refer to
  files within your extension directory." `hooks/hooks.json` uses it for both entries —
  `${extensionPath}/scripts/checknote.sh` for the bundled script, and
  `${extensionPath}/../../scripts/session_open.sh` to reach the existing, un-bundled
  plugin script two directories up (this adapter's own directory, `adapters/gemini/`,
  sits exactly two levels below the kpopper checkout root — the same relationship
  `../../scripts/...` describes literally). Unlike the Codex adapter, this one has a
  verified anchor, so the relative walk from it is portable regardless of how or where
  the extension itself gets installed or symlinked.

## Install

    gemini extensions link <plugin>/adapters/gemini

`link` (rather than `install`) keeps the extension pointed at this checkout instead of
copying it, so the `${extensionPath}/../../scripts/...` hooks keep resolving to a real
`scripts/` directory. If you'd rather vendor a copy, copy the whole kpopper checkout,
not just `adapters/gemini/` — the hooks reach two directories outside this folder on
purpose, to reuse `scripts/session_open.sh` instead of duplicating it.

## Not verified

- The exact working directory `${extensionPath}`-relative commands execute with once
  substituted — the docs confirm the substitution itself but not the process cwd,
  which matters for `session_open.sh`'s own `[ -f PROVENANCE.yaml ]` check (it assumes
  cwd is the project root, same assumption every other adapter here makes, and the one
  the existing Claude Code plugin already relies on).
- Whether `gemini extensions link` is available on every distribution channel (some
  package managers may ship `install`-only workflows) — not checked beyond the
  official CLI reference.
