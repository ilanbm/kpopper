# kpopper for Gemini CLI

Link this extension from a complete kpopper checkout. It supplies the method in
`GEMINI.md`, opens the workspace's record at session start, and requests a final
record check when the session ends.

## Install

You need Gemini CLI, Python 3.9 or newer with kpopper's dependencies, and a POSIX
shell. Keep the whole checkout: the adapter reuses its `scripts/` directory.

```sh
KPOPPER_CHECKOUT=/absolute/path/to/kpopper
python3 -m venv "$KPOPPER_CHECKOUT/.venv"
"$KPOPPER_CHECKOUT/.venv/bin/python" -m pip install -e "$KPOPPER_CHECKOUT"
export PATH="$KPOPPER_CHECKOUT/.venv/bin:$PATH"
gemini extensions link "$KPOPPER_CHECKOUT/adapters/gemini"
gemini extensions list
```

Launch Gemini from that shell so its hook commands resolve `python3` to the same
environment. A different launcher must provide that runtime path too; installing
dependencies in one interpreter does not make them available to another.

Accept Gemini's extension prompt after reviewing the checkout, then restart Gemini
in the project you want to work on. The extension list should show `kpopper` enabled
and its `GEMINI.md` context file. Inspect `/hooks list` inside Gemini if the opening
does not appear. Disabled hooks still require a manual `kpop open`.

Use `link`, not `install` on the adapter folder: copying just this folder leaves out
the shared Python scripts it needs. To relocate it, retain the whole checkout and
link the new `adapters/gemini` path. Unlink it with `gemini extensions uninstall kpopper`.

## Use

Open Gemini from your project directory. Reuse the record opening it supplies; if
there is no record, the opening gives first-use guidance without creating one.
The extension includes a condensed method, not the separate kpopper skills.

Ask Gemini to ground an answer in the record, record a sourced finding, or trace
what a changed fact affects. The corresponding CLI commands are `kpop pull`,
`kpop add`, and `kpop affects`. Run `kpop check` before finishing. If the
executable is not on PATH, the opening includes the Python command for this checkout.

## Hook behavior

| File | Purpose |
|---|---|
| `gemini-extension.json` | Extension metadata and `GEMINI.md` context. |
| `hooks/hooks.json` | Quoted commands anchored to `${extensionPath}`. |
| `scripts/hook.py` | Wraps the shared opener's text as `hookSpecificOutput.additionalContext`; wraps the end check as `systemMessage`. Stdout contains JSON only. |
| `scripts/checknote.sh` | Finds the record using the hook payload's `cwd`, runs `check`, and returns an advisory result even when problems exist. |

Gemini's `SessionStart` context reaches the model through `additionalContext`.
Plain text is insufficient: in CLI 0.43.0 the host converts it to a user-facing
message without model context. The adapter's JSON wrapper handles this distinction.

**The end check is advisory and best effort.** Gemini does not guarantee completion
of `SessionEnd`, and it cannot block shutdown. This adapter does not install an
`AfterAgent` gate. That is a separate per-turn event which can request a retry;
supporting it would require its own integration and loop-safety tests.

The [hook reference](https://geminicli.com/docs/hooks/reference/) describes these
event contracts. The [extension reference](https://geminicli.com/docs/extensions/reference/)
documents linking, context loading, and path substitution. Checked 2026-09-16.

## Validation and limits

Tested on 2026-09-16/17 with **Gemini CLI 0.43.0**, Node 24.14.0, Python 3.14.5 and
macOS (Darwin 25.4.0):

- Linked and listed the extension using an isolated `GEMINI_CLI_HOME`; Gemini
  discovered the context file and enabled the extension.
- Executed both configured commands through the installed Gemini `HookRunner`.
  Its output parser accepted JSON, and its aggregator retained startup context.
- Passed six regression tests covering record opening, first-use guidance,
  advisory failure reporting, an absent record, malformed input and closed stdin. Paths with
  spaces and a hook process running outside the project are covered.

Run the repository checks with:

```sh
python3 -m unittest tests.test_gemini_adapter -v
```

These are CLI-management and hook-component checks. A model-driven session,
resume/reopen behavior, actual shutdown delivery, sandboxed execution, Linux/WSL,
and native Windows remain unverified. Do not treat successful linking as proof of
an end-to-end session.

The 2026-09-17 live probe stopped before a model call: the selected `gemini-api-key`
authentication required `GEMINI_API_KEY`, which was absent from that execution
environment. No login or global configuration change was made to work around it.
