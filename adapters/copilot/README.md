# kpopper for GitHub Copilot

Copilot CLI, VS Code, and the cloud agent have separate installation and verification
paths. Use the instructions for the surface you actually run.

| Surface | Route | Verification, 2026-09-16 |
|---|---|---|
| **Copilot CLI** | Canonical skills, shared instructions and the [CLI hook bridge](cli/hook.py) | CLI 1.0.75 on macOS: actual hook discovery, context delivery, resume and one stop continuation passed against a local stub provider. Eight component tests passed. See the live-workflow qualification below. |
| **VS Code local agent** | [Instructions](vscode/copilot-instructions.md); optional hook sketch | Documentation reviewed; no VS Code runtime test. |
| **Copilot cloud agent** | [Setup workflow](cloud-agent/copilot-setup-steps.yml) and [PR check](cloud-agent/provenance-check.yml) | Documentation reviewed; no hosted job tested. |

## Copilot CLI

The CLI bridge opens the record at `sessionStart`, including resume, as `additionalContext`.
No `agentStop` hook is installed. Its legacy handler returns empty JSON for old configurations.
Run `check` explicitly for current diagnostics; failures still return a nonzero exit code.
The bridge preserves the session identity and resolves the workspace from the payload.

Historical protocol note (the adapter no longer emits block JSON): exiting with code 2 alone does **not** block
`agentStop`. [GitHub hook reference](https://docs.github.com/en/copilot/reference/hooks-reference)

The native payload deliberately mixes naming styles: `sessionId` is camelCase,
`source` is `startup`, `resume` or `new`, and `agentStop.stop_hook_active` is
snake_case. The bridge translates only the session ID. CLI 1.0.75 emitted
`source: "new"`, then stop payloads with `stop_hook_active: false` and `true` in
the recorded two-response probe; resuming emitted `source: "resume"` and the same
false/true stop sequence. Those historical continuation probes do not describe the current silent stop handler.

### Install

Use a local checkout of kpopper and Python 3.9+ with its dependencies installed. The
bridge uses the shared opener; earlier installations also used a POSIX shell
(macOS, Linux or WSL). Native Windows has not been validated.

From the **project where you want to keep the record**, using the Python environment
that can run kpopper:

```sh
mkdir -p .github/hooks
python3 /absolute/path/to/kpopper/adapters/copilot/cli/hook.py config > .github/hooks/kpopper-cli.json
```

If that file already exists, merge or review it before replacing it. The generator
prints configuration only; it records absolute paths to this checkout and the selected
Python interpreter. Regenerate after moving either. Those machine-specific paths should
remain local unless everyone uses the same layout. Keep other hook files unchanged.

Also add the contents of
[`vscode/copilot-instructions.md`](vscode/copilot-instructions.md) to the project's
`.github/copilot-instructions.md`, preserving any existing instructions, and replace
`<plugin>` with the checkout's absolute path. If the project's `AGENTS.md` already carries
the method, avoid duplicating it. The same condensed instructions apply to CLI use.

Install the canonical skill directories as well. The condensed instructions alone
do not provide the full authoring reference. Keep their supporting files in the
complete checkout, and preserve any existing skills with these names:

```sh
KPOPPER_CHECKOUT=/absolute/path/to/kpopper
mkdir -p .agents/skills
for skill in "$KPOPPER_CHECKOUT"/skills/*/; do
  ln -s "$skill" ".agents/skills/$(basename "$skill")"
done
```

Copilot discovers these project skills without importing the repository's Claude
plugin hooks. If a name already exists, check its target before continuing; do not
overwrite a different skill. Start a fresh session after installing them and verify
that invoking `record` loads the chosen checkout's `skills/record/SKILL.md`.

Start a new trusted Copilot CLI session in the project. If no opening arrives, ask it
to run the following explicitly:

```sh
python3 /absolute/path/to/kpopper/scripts/cli.py open
python3 /absolute/path/to/kpopper/scripts/cli.py check
```

This route does not install async ingestion, watch, grounding, edit or follow-up
hooks. The canonical skills and shared instructions carry the method.
It also does not configure checked-session MCP transport. Test those separately before
claiming parity with Claude Code or Codex.

### Native plugin loading is a separate capability

Copilot CLI recognizes `.claude-plugin/plugin.json`, skills, and plugin hooks. The local
command below discovered this checkout as `kpopper` on CLI 1.0.75:

```sh
copilot --plugin-dir /absolute/path/to/kpopper plugin list
```

This is a **discovery check**, not the recommended kpopper installation. Loading the
repository root also selects its Claude hooks, whose output and background behavior
have not been adapted for Copilot. Do not combine that import with the CLI bridge and
assume they are equivalent. The repository hook route above deliberately selects only
the two translated events.

Copilot supports installation from repositories and marketplaces, but discovery alone
does not verify the installed plugin's runtime behavior.
[CLI plugin reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-plugin-reference)

### What has been tested

`python3 -m unittest discover -s tests -p test_copilot_adapter.py` checks the bridge
against real temporary records: payload workspace selection, opening context and session
identity, a new failure returning block JSON, continuation yielding, resume preserving
the original baseline, first use without creating a record, and configuration generation.

A separate offline CLI run used isolated `COPILOT_HOME` and `COPILOT_CACHE_HOME`, a
temporary Git repository and a localhost provider that returned fixed responses. The
provider received the opening context, then received the new failure after the stop
hook forced one continuation. This verifies the host protocol, not whether a model
chooses skills, records useful findings or completes a task correctly. Linux, WSL,
native Windows, complete source fidelity and cross-agent handoff still need checks.

A subsequent real-model probe exposed issues the protocol tests could not detect.
An instructions-only run guessed field names and produced no executable judgment.
A run with the canonical `record` skill used `v`, `rests_on`, `wrong_if` and `seen`
and produced a clean checker result; fresh reading and changed-premise detection
also worked. The model and tool permissions changed between the two runs, so this
comparison does not isolate the effect of skills. The installation includes the
full catalog to provide the actual authoring reference.

The successful core sequence still omitted some source metadata and did not
complete a structured citation update. A clean checker result does not prove that
the model recorded every requested source detail. Consult the
[current verification matrix](../../docs/compatibility.md) for the exact limits.

## VS Code local agent

Merge [`vscode/copilot-instructions.md`](vscode/copilot-instructions.md) into
`.github/copilot-instructions.md`, replacing `<plugin>` with the checkout path. VS Code
reads that file automatically and also supports `AGENTS.md`.
[Custom instructions](https://code.visualstudio.com/docs/agent-customization/custom-instructions)

[`vscode/hooks.json`](vscode/hooks.json) remains an **unverified sketch**, not an install
step. Its relative commands must be replaced with valid paths for the project. VS Code
hooks are Preview, may be disabled by organization policy, and are discovered under
`.github/hooks/*.json` by default. The current documentation also describes CLI-format
compatibility, but the CLI bridge's direct `exec` configuration has not been validated
in VS Code. Keep explicit opening/checking instructions until a local-agent session
proves the chosen hook route.
[VS Code hooks](https://code.visualstudio.com/docs/agent-customization/hooks)

## Copilot cloud agent

The cloud agent now supports repository hooks, including session start and stop events.
This adapter still uses setup plus CI; it does not claim a tested cloud lifecycle gate.
The CLI generator emits local absolute paths and `exec` entries, so do **not** copy its
generated configuration into a cloud job.
[Cloud hook behavior](https://docs.github.com/en/copilot/reference/hooks-reference)

Copy or merge these workflow files into the repository's `.github/workflows/`:

- [`cloud-agent/copilot-setup-steps.yml`](cloud-agent/copilot-setup-steps.yml) installs
  PyYAML and opens an existing root record in the setup logs.
- [`cloud-agent/provenance-check.yml`](cloud-agent/provenance-check.yml) checks a root
  record on pull requests. It is a CI result; merge enforcement requires the project's
  own branch protection configuration.

Set the repository variable `KPOPPER_ROOT` to the location of a kpopper checkout available
inside the job, or edit the workflows to use the project's actual vendored path. The
setup workflow does not fetch kpopper itself. Without this variable the setup opening
skips; the PR check fails when a root record exists and the tool cannot be located.

The setup job must be named `copilot-setup-steps`, and the workflow must be present on
the default branch for the cloud agent to use it. Review and validate the setup logs in
the target repository before relying on it.
[Cloud environment setup](https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/customize-cloud-agent/customize-the-agent-environment)
