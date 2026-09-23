# kpopper for Gemini CLI

Link this extension from a complete kpopper checkout. It supplies the method in
`GEMINI.md` and opens the workspace's record at session start. Nothing checks the
record when the session ends; run `check` before finishing.

## Install

You need Gemini CLI and a POSIX shell. Keep the whole checkout: the adapter runs
its `scripts/` directory and the native runtime installed there.

```sh
KPOPPER_CHECKOUT=/absolute/path/to/kpopper
sh "$KPOPPER_CHECKOUT/scripts/install_native.sh"
export PATH="$KPOPPER_CHECKOUT/bin:$PATH"
gemini extensions link "$KPOPPER_CHECKOUT/adapters/gemini"
gemini extensions list
```

`install_native.sh` downloads the runtime for the checkout's `VERSION` from the kpopper
GitHub release and checks its SHA-256. The hook runs that runtime and never downloads or
installs one: without it, the opening says so and names the install command. Start Gemini
from a shell with that `PATH`, so `kpop` names this checkout's command; the hook itself
does not depend on `PATH`.

Python is only an explicit compatibility mode. With `KPOPPER_RUNTIME=python` in the
environment Gemini starts hooks with, the hook runs `scripts/hook.py` with `python3`,
which then needs kpopper's dependencies. See
[Python compatibility mode](../../docs/plugin-runtime.md#python-compatibility-mode).

Accept Gemini's extension prompt after reviewing the checkout, then restart Gemini
in the project you want to work on. The extension list should show `kpopper` enabled
and its `GEMINI.md` context file. Inspect `/hooks list` inside Gemini if the opening
does not appear. Disabled hooks still require a manual `kpop open`.

Use `link`, not `install` on the adapter folder: copying just this folder leaves out
the shared scripts and the runtime it needs. To relocate it, retain the whole checkout and
link the new `adapters/gemini` path. Unlink it with `gemini extensions uninstall kpopper`.

## Use

Open Gemini from your project directory. Reuse the record opening it supplies; if
there is no record, the opening gives first-use guidance without creating one.
The extension includes a condensed method, not the separate kpopper skills.

Ask Gemini to ground an answer in the record, record a sourced finding, or trace
what a changed fact affects. The corresponding CLI commands are `kpop pull`,
`kpop add`, and `kpop affects`. Run `kpop check` before finishing. If `kpop` is not
on PATH, the opening names this checkout's runtime in `KPOPPER_AGENT_CONTEXT`.

## Hook behavior

| File | Purpose |
|---|---|
| `gemini-extension.json` | Extension metadata and `GEMINI.md` context. |
| `hooks/hooks.json` | Runs `scripts/session-start.sh` at `SessionStart`, quoted and anchored to `${extensionPath}`. |
| `scripts/session-start.sh` | Runs the checkout's native `kpop session-start --gemini`, which finds the record from the hook payload's `cwd` and returns the opening as `hookSpecificOutput.additionalContext`. A missing runtime is reported in the same field. With `KPOPPER_RUNTIME=python` it runs `hook.py` instead. Stdout contains JSON only. |
| `scripts/hook.py` | Python compatibility mode: wraps the Python opener's text the same way and keeps legacy end invocations silent. |

Gemini's `SessionStart` context reaches the model through `additionalContext`.
Plain text is insufficient: in CLI 0.43.0 the host converts it to a user-facing
message without model context. The adapter's JSON wrapper handles this distinction.

No `SessionEnd` or `AfterAgent` hook is installed. Old `SessionEnd` invocations return
empty JSON. Use the startup context and explicit `check` for record diagnostics; this
adapter never requests a retry or displays unsolicited shutdown bookkeeping.

The [hook reference](https://geminicli.com/docs/hooks/reference/) describes these
event contracts. The [extension reference](https://geminicli.com/docs/extensions/reference/)
documents linking, context loading, and path substitution. Checked 2026-09-16.

## Validation and limits

Tested on 2026-09-16/17 with **Gemini CLI 0.43.0**, Node 24.14.0, Python 3.14.5 and
macOS (Darwin 25.4.0), when the hook ran `scripts/hook.py`:

- Linked and listed the extension using an isolated `GEMINI_CLI_HOME`; Gemini
  discovered the context file and enabled the extension.
- Executed both configured commands through the installed Gemini `HookRunner`.
  Its output parser accepted JSON, and its aggregator retained startup context.
- Passed six regression tests covering record opening, first-use guidance,
  advisory failure reporting, an absent record, malformed input and closed stdin. Paths with
  spaces and a hook process running outside the project are covered.

The native hook, `scripts/session-start.sh`, has not yet run inside Gemini CLI.
Repository tests cover `kpop session-start --gemini` itself (record opening, first-use
guidance without creating a record, subagent and malformed payloads) and run the
manifest's command against a checkout layout: the opening from the checkout's own
runtime, a missing runtime, and Python compatibility selection. Run them with:

```sh
cargo test --manifest-path native/Cargo.toml --test host_adapters
python3 -m unittest tests.test_gemini_adapter -v
```

The Python tests cover `scripts/hook.py` in compatibility mode. These are
CLI-management and hook-component checks. A model-driven session,
resume/reopen behavior, actual shutdown delivery, sandboxed execution, Linux/WSL,
and native Windows remain unverified. Do not treat successful linking as proof of
an end-to-end session.

The 2026-09-17 live probe stopped before a model call: the selected `gemini-api-key`
authentication required `GEMINI_API_KEY`, which was absent from that execution
environment. No login or global configuration change was made to work around it.
