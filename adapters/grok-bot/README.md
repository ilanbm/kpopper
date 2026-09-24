# kpopper for Grok Bot

**Status: documented setup proposal; not yet verified in a live Grok Bot session.**
This guide uses the native CLI and explicit instructions. It does not establish
plugin import, automatic hooks or background delivery.

Grok Bot provides a cloud computer with a terminal and a persistent `/workspace`,
shared by your Bots. Install kpopper there, where the Bot executes commands;
installing it on your laptop alone does not make it available in the cloud.
See [Use the computer and apps](https://docs.x.ai/grok-bot/computer-and-apps).

Grok Bot and Grok Build have separate documentation. Grok Build's
[Claude Code compatibility](https://docs.x.ai/build/features/skills-plugins-marketplaces)
does not verify the same plugin-loading or hook behavior in Grok Bot.

## Install

In Grok Bot, paste the [installation request](../../README.md#get-started) and give
it the project folder you want to use. The Bot should follow the steps below on
its **cloud computer**, reporting any unavailable command or unsupported platform.

Keep a full checkout at a stable path under `/workspace`. The skills refer to
sibling skills, scripts and documentation; copying individual `SKILL.md` files
loses those dependencies. The example uses `/workspace/tools/kpopper` for the
installation and `/workspace/projects/my-project` for your work. Replace the
project path with your actual folder, reusing its existing record if it has one.
If a kpopper checkout already exists, inspect and reuse it instead of cloning over it.

Run in a POSIX shell on the cloud computer, with Git and curl available:

```sh
mkdir -p /workspace/tools
git clone https://github.com/ilanbm/kpopper.git /workspace/tools/kpopper
sh /workspace/tools/kpopper/scripts/install_native.sh
```

The installer downloads the release matching the checkout's `VERSION`, selects
the execution platform and verifies its SHA-256. The runtime requires no Python,
Node, Rust or Lean toolchain. If installation fails, preserve the diagnostic and
follow [Runtime setup and troubleshooting](../../docs/plugin-runtime.md); do not
report setup as complete.

After successful installation, open the intended project explicitly:

```sh
mkdir -p /workspace/projects/my-project
/workspace/tools/kpopper/bin/kpop --workspace /workspace/projects/my-project open
/workspace/tools/kpopper/bin/kpop --workspace /workspace/projects/my-project check
```

Use the absolute command path for later calls too. Each project gets its own
folder; neither the shared `/workspace` root nor the kpopper checkout should
become the default record for unrelated work. Opening an empty project should
give first-use guidance without creating a record. An existing record may have
check failures that require review; those are not necessarily install failures.

## Give the Bot the method

Use the following instructions for a first task. Replace the project path before
sending them; preserve the Bot's other instructions.

```text
Use kpopper for work in /workspace/projects/my-project.
The full installation is /workspace/tools/kpopper.
Read /workspace/tools/kpopper/adapters/_shared/method-summary.md.
Read /workspace/tools/kpopper/skills/kpopper/SKILL.md for the skill guide,
then the relevant canonical skills in that checkout as the work requires.
Keep their references pointing to the original files.

At the start of work, run:
/workspace/tools/kpopper/bin/kpop --workspace /workspace/projects/my-project open
Use this same executable and workspace for subsequent commands.
Read relevant entries with context or pull before answering from memory.
Use the record skill and CLI to save authorized findings, then run check.
Preserve the existing record and its .kpopper directory.
If the runtime or project is unavailable, report that instead of creating
a replacement record elsewhere.
```

After a successful task, ask the Bot to save these instructions as a private
skill, for example `kpopper-my-project`. The skill should keep the exact paths
and read the canonical files, rather than copy their contents. Grok Bot documents
creating skills by asking the Bot and selecting saved skills with `/` in the
desktop composer. If the skill is missing, inspect **Marketplace → Your plugins →
Manage plugins and skills → Private skills**.
[Skills and routines](https://docs.x.ai/grok-bot/skills-routines-and-automations).

Select that skill explicitly when starting later work. Its instructions request
`open` and `check`; they are not lifecycle hooks. This guide does not assume that
Grok Bot discovers local `AGENTS.md`, `.claude-plugin` or `.grok` directories.

## Verify before relying on it

Use a disposable project folder for the first write test:

1. Run `open` and `check` through the installed command. Confirm the runtime and
   project paths in the actual tool output.
2. Put an arbitrary confirmation code in a source file. Ask the Bot to read the
   canonical record skill and record that code with its source citation through
   the CLI. Inspect the resulting record and run `check`.
3. Save and select the project skill, then start a fresh conversation with access
   to the same folder. Ask for the code without including it in the prompt.
   Verify a record/source read before the answer; remembered text alone is not
   evidence that the integration works.

Record the host version, execution platform and observed results. Runtime
installation, saved-skill visibility, CLI writes and fresh-conversation reads
are separate checks. See the full
[release acceptance scenario](../../docs/compatibility.md#release-acceptance-scenario)
before claiming broader support.

## Persistence and optional features

Keep project sources, `GROUNDING.yaml` (or an existing `PROVENANCE.yaml`) and the
whole `.kpopper/` directory together under `/workspace`. Grok Bot documents
durable workspace files but treats manually installed packages as replaceable.
Recheck the executable after computer recovery or updates, and reinstall the
matching runtime if needed. All Bots on the account share those files; separate
project folders organize work but do not isolate access.
[Computer persistence](https://docs.x.ai/grok-bot/computer-and-apps).

Hub and Annotated Documents remain optional experimental applications; this
setup does not activate them. Loading `map` or `watch` supplies no worker,
connector, scheduler or notification route. Grok Bot routines are a separate
host feature and need their own setup and validation.

## Official references

Reviewed 2026-09-24:

- [Use the computer and apps](https://docs.x.ai/grok-bot/computer-and-apps) — cloud execution, shared files and `/workspace` persistence.
- [Skills and routines](https://docs.x.ai/grok-bot/skills-routines-and-automations) — private skills, explicit selection and routines.
- [Grok Build: Skills, Plugins & Marketplaces](https://docs.x.ai/build/features/skills-plugins-marketplaces) — the compatibility claim belongs to Grok Build.
