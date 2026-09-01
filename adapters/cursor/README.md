# kpopper for Cursor

Cursor's hook payloads carry `conversation_id`, never `session_id` — the plugin's own
`scripts/session_open.sh` / `session_gate.sh` key their baseline file by `session_id`
and would silently never fire if pointed at directly. This adapter is a pair of
translating wrapper scripts instead, keyed by `conversation_id`, that call the same
harness-neutral `scripts/provenance.py` underneath.

## What this installs

| file | does |
|---|---|
| `hooks.json` | sessionStart → `gate-open.sh`, stop → `gate-stop.sh`. |
| `scripts/gate-open.sh` | writes `${TMPDIR}/kpopper-base-cursor-<conversation_id>`, prints the opening. |
| `scripts/gate-stop.sh` | compares the current FAIL count to that baseline; bounces once. |
| `rules/kpopper.mdc` | the condensed method, `.mdc` frontmatter, agent-requested + auto-attached on `PROVENANCE.yaml`. |

## Verified against docs

Source: `https://cursor.com/docs/hooks`, plus `https://ntorres.dev/blog/cursor-hooks-json-guide`
(a third-party schema writeup cross-checked against the same page), fetched 2026-09-01.

- **`conversation_id`, not `session_id`.** Confirmed on both the sessionStart and stop
  payloads — this is why a wrapper exists at all rather than pointing hooks.json at the
  existing scripts.
- **sessionStart is fire-and-forget** — "the agent loop does not wait for or enforce a
  blocking response" — and its response shape is `{"additional_context": "..."}`, not
  bare stdout the way Claude Code and Codex both accept. `gate-open.sh` wraps its output
  in that shape. Whether Cursor *also* tolerates bare stdout for this event is not
  documented either way — shipped the documented form since it's the only one confirmed.
- **stop has no block/deny field.** The permission-style response (`{"permission":
  "allow"|"ask"|"deny"}`) is documented for `beforeShellExecution`, `beforeMCPExecution`,
  `preToolUse`, and `subagentStart` — not for `stop`. The only lever `stop` has is
  `{"followup_message": "..."}`, which Cursor resubmits as the next user turn, capped by
  a `loop_limit` (default 5) tracked in the payload's `loop_count` field (0 on the first
  stop of a conversation, incremented each time a followup_message fires). `gate-stop.sh`
  treats `loop_count == 0` as the translation of `stop_hook_active == false` — bounce
  once via `followup_message`, then go quiet on any later stop for the same conversation,
  printing the failures to stderr instead. **This is our own translation, not a
  documented Cursor "block" primitive** — Cursor never actually prevents the turn from
  ending, it just queues another one.
- **Command paths are workspace-root-relative, not `hooks.json`-relative.** For
  project hooks, Cursor runs with the project root as cwd, and the documented convention
  is to write the `.cursor/` prefix explicitly (`.cursor/scripts/foo.sh`, not
  `./scripts/foo.sh` or `scripts/foo.sh`). `hooks.json` above does this.
- **Do not symlink the `.cursor` directory itself** — there's a filed Cursor forum
  report of relative hook paths breaking specifically under a symlinked `.cursor/`.
  Symlinking the individual script files (see install, below) is a different, unaffected
  case, and is what lets `gate-open.sh`/`gate-stop.sh` self-locate the kpopper checkout.
- **`.mdc` frontmatter** is `description` (used for agent-requested matching),
  `globs` (comma-separated, auto-attaches the rule when a matching file is in context),
  `alwaysApply` (bypasses both — loads for every request, so used sparingly per Cursor's
  own guidance). `rules/kpopper.mdc` sets both `description` and `globs: PROVENANCE.yaml`
  so it attaches either by relevance or the moment that file is touched, and leaves
  `alwaysApply: false`.

## Install

    mkdir -p .cursor/scripts .cursor/rules
    cp <plugin>/adapters/cursor/hooks.json .cursor/hooks.json
    cp <plugin>/adapters/cursor/rules/kpopper.mdc .cursor/rules/kpopper.mdc
    ln -s <plugin>/adapters/cursor/scripts/gate-open.sh .cursor/scripts/gate-open.sh
    ln -s <plugin>/adapters/cursor/scripts/gate-stop.sh .cursor/scripts/gate-stop.sh

Symlinking the two scripts (not copying them, not symlinking the directory) is what
lets them find `scripts/provenance.py` on their own, by resolving their own real
location. If you'd rather copy them, set `KPOPPER_ROOT=/absolute/path/to/kpopper` in
the environment Cursor runs hooks in — both scripts check that first.

## Not verified

- Whether `stop`'s `followup_message` is honored identically across Cursor's surfaces
  (desktop app vs CLI vs background agent) — docs describe one mechanism, not
  per-surface differences.
- The exact shell Cursor invokes `command` through (direct exec vs `sh -c`) — the
  scripts' self-location logic reads `$0`, which is reliable under direct exec and
  under most `sh -c "path"` forms, but not guaranteed universally. `KPOPPER_ROOT` is
  the documented-safe fallback if self-location ever comes back empty (both scripts
  print a diagnostic to stderr and exit 0 rather than silently doing nothing).
