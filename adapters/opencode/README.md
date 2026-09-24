# kpopper for OpenCode

The verification below predates the application rename and the native runtime: it
ran the earlier implementation, and the native install steps here have not had a live host run.
The current bundle has eight canonical skills plus the `page` and `document`
compatibility aliases; the application skills are now `hub` and `annotated-doc`.

Use OpenCode's native skills loader with the `kpop` command. This adapter provides
instructions, not an OpenCode JavaScript plugin: it does not install automatic session
hooks, a blocking stop gate, or background delivery.

**Validation:** OpenCode 1.18.31 on macOS arm64 accepted the configuration below and
discovered all eight canonical skills from their original locations. CLI checks ran
separately in an isolated project. Model-driven skill use, session resumption, and the
desktop/IDE surfaces still need live validation.

## Install

Keep a full kpopper checkout at a stable absolute path. The skills link to sibling
skills, references, and `../../docs/`; copying just their `SKILL.md` files loses those
resources. A temporary worktree or an expiring plugin cache is not a stable install.

Install that checkout's native runtime and put its `scripts/bin/` on `PATH`. The installer
downloads the release matching the checkout's `VERSION` and checks its SHA-256; see
[Install the native runtime](../../docs/plugin-runtime.md#install-the-native-runtime).

```sh
KPOPPER_CHECKOUT=/absolute/path/to/kpopper
sh "$KPOPPER_CHECKOUT/scripts/install_native.sh"
export PATH="$KPOPPER_CHECKOUT/bin:$PATH"
```

In the project where you want to use kpopper, merge these fields into `opencode.json`
or `opencode.jsonc`. Preserve existing configuration and instruction entries. Replace
each placeholder with the same absolute checkout path; JSON does not expand shell
variables.

```json
{
  "$schema": "https://opencode.ai/config.json",
  "skills": {
    "paths": ["/absolute/path/to/kpopper/skills"]
  },
  "instructions": [
    "/absolute/path/to/kpopper/adapters/_shared/method-summary.md",
    "/absolute/path/to/kpopper/adapters/opencode/instructions.md"
  ]
}
```

`skills.paths` loads the original skill directories, so their supporting files stay
in place. The `instructions` entries load the shared method and this host's operating
instructions without replacing the project's `AGENTS.md`.

Launch `opencode` from the target project using the shell where the CLI is on `PATH`.
Other launchers may not inherit that environment; in that case configure their runtime
path explicitly before relying on `kpop`. Keep the existing `GROUNDING.yaml` or
registered record location; this adapter does not need another record.

OpenCode also discovers `.agents/skills` and `.claude/skills`, including global ones.
Avoid duplicate names such as `ground` or `record`: check which location is loaded.
If the host asks for access to files in the checkout, review that specific access.
Skill and shell permissions must permit the selected work; the adapter does not
change permissions.

## Verify and use

From the target project, inspect the resolved configuration and skill locations:

```sh
opencode debug config
opencode debug skill > /tmp/kpopper-opencode-skills.json
kpop open
kpop check
```

The skill listing should contain `kpopper`, `ground`, `record`, `map`, `document`,
`page`, `consolidate`, and `watch`, each with a location inside your chosen checkout.
`check` can report existing record problems; a non-zero result is not necessarily an
installation failure. Opening a project without a record should show the first-use
guidance rather than create a record.

In OpenCode, ask: “Use the ground skill to open this project's knowledge record and
read the entries relevant to my question.” Verify the actual tool calls and output.
Later, ask it to record an authorized finding and run `kpop check`. Start a new
session and verify that it reads that finding before answering from memory.

The commands are the same as in other hosts:

```sh
kpop pull <entry-or-prefix>
kpop affects <entry>
kpop check
# Optional experimental application.
kpop experimental hub
```

Open the generated `.kpopper/build/page.html` in a browser. Browser automation and
page verification have their own runtime requirements; skill discovery alone does
not validate them.

## Boundaries

- Skills are loaded on demand. Instructions ask the agent to run `open` and `check`;
  they do not enforce execution or prevent the agent from ending a turn.
- Claude/Codex plugin manifests and hook JSON are not installed by this route.
  OpenCode has its own plugin API; event availability alone does not establish a
  compatible blocking gate or an automatic grounding opener.
- Loading `map` or `watch` does not configure a worker, scheduler, source connector,
  or notifications. Their host-dependent behavior needs separate validation.
- The documented setup uses a POSIX shell. Windows, WSL, OpenCode Desktop, and IDE
  launches have not been validated by this adapter.

## Official references

Checked 2026-09-16:

- [Agent skills](https://opencode.ai/docs/skills/) — discovery and skill permissions.
- [Configuration schema](https://opencode.ai/config.json) — `skills.paths`.
- [Rules](https://opencode.ai/docs/rules/) — `AGENTS.md` and additional `instructions`.
- [Plugins](https://opencode.ai/docs/plugins/) — the separate native plugin interface.
