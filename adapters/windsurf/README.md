# kpopper for Windsurf / Cascade

No session-level hook exists in Windsurf at all — not a blocking one, not an advisory
one. `rules/kpopper.md` carries the whole method, including the open and close duties
Claude Code's plugin otherwise automates with `session_open.sh` / `session_gate.sh`.
`hooks.json` adds one small, optional nudge on top; it is not the mechanism.

## What this installs

| file | does |
|---|---|
| `rules/kpopper.md` | the condensed method + open/close duties in prose, since nothing here can run them for you. |
| `hooks.json` | `post_write_code`, scoped in-script to writes of `GROUNDING.yaml` (or `PROVENANCE.yaml`), runs `provenance.py check` and surfaces it in the Cascade UI. Advisory only. |

## Verified against docs

Source: `https://docs.devin.ai/desktop/cascade/hooks`, fetched 2026-09-01.

- **No file-pattern scoping in `hooks.json` itself.** The schema has no glob or path
  filter on a hook entry — "the schema does not support glob patterns or file-specific
  filtering within hook definitions." So "scoped to `GROUNDING.yaml`" has to happen
  inside the command, not in the config: the shipped `command` reads the hook's own
  stdin JSON, pulls the file path out of `tool_info.file_path` (falling back to a
  top-level `file_path`, since two doc fetches described the payload slightly
  differently and I could not fully reconcile them — see below), and only runs
  `check` when that file's basename is `GROUNDING.yaml` or `PROVENANCE.yaml`. Confirmed working against a
  hand-built payload for both the matching and non-matching case.
- **`post_write_code` cannot block anything.** Exit code 2 is documented as a
  blocking error only for *pre*-hooks; `post_write_code` fires after the write already
  happened. Its output is UI-only by design — "hook output is only displayed in the
  UI panel... not inserted back into the agent's conversation context." That is why
  this ships as a nudge, not a gate, and why `rules/kpopper.md` (not this file) carries
  the actual close-the-loop obligation.
- **`show_output: true`** is what makes `check`'s output appear in Cascade's UI at
  all; the default otherwise would run the check invisibly.

## Install

    mkdir -p .windsurf/rules
    cp <plugin>/adapters/windsurf/rules/kpopper.md .windsurf/rules/kpopper.md
    cp <plugin>/adapters/windsurf/hooks.json .windsurf/hooks.json

Then edit the `../../scripts/provenance.py` path inside `hooks.json`'s `command` to an
absolute path to this kpopper checkout's `scripts/provenance.py` — see "not verified,"
below, for why a relative path can't be shipped portable here the way it can for
Gemini.

## Not verified

- **What a `command`'s relative paths resolve against.** There's a `working_directory`
  field on a hook entry, but the docs don't say what it defaults to, and there's
  nothing here playing the role of Gemini's documented `${extensionPath}` or a
  Codex-style `PLUGIN_ROOT`. The shipped `../../scripts/provenance.py` mirrors this
  file's own position two directories below the kpopper checkout root and is correct
  only in that one arrangement (e.g. dogfooding kpopper on itself) — for any other
  project, replace it with an absolute path per the install step above.
- **The exact shape of `tool_info` for a write-triggered event.** One fetch described
  the payload as `tool_info.file_path` alongside sibling fields `old_string`/
  `new_string` (an edit-shaped payload); a second, independent fetch of the same page
  listed only `agent_action_name`, `trajectory_id`, `execution_id`, `timestamp`,
  `model_name`, and an opaque `tool_info` blob without spelling out its keys. The
  shipped command checks `tool_info.file_path` first and a bare top-level `file_path`
  second, which covers both readings, but neither was confirmed against a real
  Cascade-generated payload — only a hand-built one.
- Whether Cascade is being retired in favor of "Devin Local" (mentioned once, off a
  general search, not on the hooks page itself) — not chased further since it doesn't
  change what ships here either way.
