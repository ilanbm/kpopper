# kpopper for Cursor

This project adapter supplies a rule and two native Cursor hook wrappers. The
wrappers call kpopper's shared reader and checker; they translate opening context
into Cursor's response format. It never requests a stop follow-up.

**Status:** wrapper smoke tests pass on macOS. Installation, context delivery and
opening delivery inside Cursor still need a live session test. A passing shell
test does not establish desktop, CLI or cloud runtime parity.

## What this installs

| File | Behavior |
|---|---|
| `hooks.json` | `sessionStart` → `gate-open.sh`; no stop hook. |
| `scripts/gate-open.sh` | Opens the record in `additional_context` and saves a temporary baseline keyed by `conversation_id`. |
| `scripts/gate-stop.sh` | Silent compatibility handler for old installations. |
| `rules/kpopper.mdc` | Condensed method, requested by relevance or attached when a record file is in context. |

This adapter does not install the root plugin's other hooks, optional background
workers or full skill collection. Use explicit `check` for diagnostics during a session.

## Cursor's documented contract

Checked against [Cursor's hooks reference](https://cursor.com/docs/hooks) on
2026-09-16:

- The common payload includes `conversation_id`. `sessionStart` also documents
  `session_id`, equal to that conversation ID. This adapter uses `conversation_id`
  consistently across both hooks.
- `sessionStart` returns `additional_context`. It is nonblocking, so it cannot
  guarantee the agent waits for the record before starting work.
- Native `stop` uses `followup_message` to request another turn. `loop_count`
  counts automatic follow-ups, and `loop_limit` defaults to five. This adapter
  does not register that event or emit follow-up messages.
- Project hook commands run from the project root. Keep the `.cursor/` prefix in
  `hooks.json`; when the payload has no `cwd`, the wrappers use that working directory.
- Hosted cloud agents run `stop` but do not run `sessionStart`. Self-hosted workers
  have a different lifecycle contract.

The legacy stop wrapper always exits silently. **Hosted cloud agents therefore do not receive the automatic opening
or stop check from this pair.** Run `kpop open` and `kpop check` explicitly
there. Local symlink targets also need to exist in any remote execution environment.

## Install the project adapter

Use a trusted project, a kpopper checkout, and Python 3.9+ with the dependencies
from the repository's installation instructions. Run from the project root,
replacing `<plugin>` with the absolute checkout path. If `.cursor/hooks.json`
already exists, merge this adapter's two hook entries into it instead of copying
over it. Preserve existing rules and scripts with the same names as well.

```sh
mkdir -p .cursor/scripts .cursor/rules
cp "<plugin>/adapters/cursor/hooks.json" .cursor/hooks.json
cp "<plugin>/adapters/cursor/rules/kpopper.mdc" .cursor/rules/kpopper.mdc
ln -s "<plugin>/adapters/cursor/scripts/gate-open.sh" .cursor/scripts/gate-open.sh
ln -s "<plugin>/adapters/cursor/scripts/gate-stop.sh" .cursor/scripts/gate-stop.sh
```

Symlink the individual scripts so they can resolve the checkout containing the
reader. For copied scripts, set `KPOPPER_ROOT=/absolute/path/to/kpopper` in the
environment Cursor uses for hooks. A missing checkout produces a diagnostic on
stderr and the wrapper yields. These shell instructions have not been validated
on native Windows.

## Plugins and Claude hook imports

Cursor now has its own plugin packaging and an Agent Plugins format. The latter
packages skills and MCP servers; Cursor-specific hooks require Cursor's format.
This directory remains a project adapter, not a Cursor marketplace package.
[Cursor plugin documentation](https://cursor.com/docs/plugins)

Cursor also documents importing Claude hooks from `.claude/settings*.json` when
third-party imports are enabled. It translates a Claude `Stop` block response into
a follow-up. That is another integration path, not proof that installing kpopper's
Claude package supplies every capability in Cursor. Do not configure both paths
for the same kpopper hooks: all matching hooks can run.
[Third-party hook documentation](https://cursor.com/docs/reference/third-party-hooks)

## Verification

The existing `tests.test_start.FirstUse.test_cursor_first_use_json_and_first_record_stop_use_the_same_baseline`
checks opening JSON, the first-record baseline and silent legacy stops. A separate
2026-09-16 smoke test also exercised executable symlinks, a project path with
spaces and documented payload fields without `cwd`.

Before claiming a host is verified, start a fresh conversation in that host,
confirm the opening reaches the agent, introduce a checker failure in a disposable
record, and confirm no automatic follow-up is created; explicit `check` must still fail. Test resume and
compaction separately; this adapter installs no compaction hook. Multi-root
workspace selection and remote execution remain unverified.
