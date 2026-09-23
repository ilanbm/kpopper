# kpopper for OpenClaw

The verification below predates the application rename and the native runtime: it
ran the Python CLI, and the native install steps here have not had a live host run.
The current bundle has eight canonical skills plus the `page` and `document`
compatibility aliases; the application skills are now `hub` and `annotated-doc`.

**Status: bundle installation, all eight skills and a live record workflow were
verified with OpenClaw 2026.9.4. The live tests used its Claude CLI backend with
Fable 5.1 and explicit tool approvals. A follow-up excluded native user/project
customizations and verified that every selected skill came from the fresh bundle.
Automatic opening, stop checks and background delivery are not connected by this
bundle. Existing subscription authentication was retained; other providers and a
new account/OS profile remain unverified.**

OpenClaw detects this repository as a **Codex bundle**, because it contains
`.codex-plugin/plugin.json`. It loads the canonical `skills/` directory. The
bundle also declares Codex hooks, but OpenClaw's runnable hook packs require
`HOOK.md` and a handler; it does not translate our command-hook JSON into those
packs. A listed `hooks` capability is therefore not evidence of an active hook.

## Install

Use a persistent full checkout. The skills link to sibling skills, documentation
and scripts, so copying only individual `SKILL.md` files loses their dependencies.
OpenClaw 2026.9.4 requires Node `>=24.16.0 <25` or `>=26.1.0`; check the requirements
of the OpenClaw version you install. kpopper's native runtime needs no Python; the
installer downloads the release matching the checkout's `VERSION` and checks its SHA-256
([Install the native runtime](../../docs/plugin-runtime.md#install-the-native-runtime)).

```sh
git clone https://github.com/ilanbm/kpopper.git
KPOPPER_REPO="$(pwd)/kpopper"
sh "$KPOPPER_REPO/scripts/install_native.sh"

openclaw plugins install --link "$KPOPPER_REPO" --force --accept-capabilities
openclaw plugins inspect kpopper --json
openclaw skills info ground --json
```

The install flags accept the local source and its declared capabilities. Review
the checkout before using them. `--link` keeps the bundle at that path; keep the
checkout available. This does not install the native runtime inside an OpenClaw
sandbox or on another machine.

If the Gateway is already running, restart it after installation with
`openclaw gateway restart`, then start a new conversation. A restart can interrupt
its active work. Do not start or restart a Gateway merely to run the local CLI
checks below.

Verify `format: "bundle"`, `bundleFormat: "codex"`, and an enabled plugin in
`plugins inspect`. `skills list --json` should contain `kpopper`, `ground`,
`record`, `map`, `document`, `page`, `consolidate` and `watch`. Check the selected
agent's visibility and allowlists as well. Skill eligibility alone does not test
the runtime or authorize the agent's file access.

## Point the agent at the project

The agent's tool policy must permit reading the project and skill references,
writing the intended record or report files, and executing the configured CLI.
An exec allowlist with no approval route can list the skills while refusing their
use. For a CLI backend, keep an operator approval interface available when the
policy asks; approve only the intended project operations.

The tested Claude CLI backend refused oversized serialized Bash inputs before
requesting approval, even when the executable was allowlisted. Long inline
judgments can hit that limit. For writes after the initial source entry, use the
documented file-based route: write a JSON source report inside the project and run
`kpop update --file /absolute/path/to/report.json`.
This route retains the writer's source/hash checks and requires the same write
authority. A refused tool call is not a successful record update.

Use a durable project directory accessible to the OpenClaw execution environment.
Keep the record in that project, and use `--workspace` explicitly if the agent's
own workspace is a different directory. Two projects should not accidentally
share the Gateway's default workspace record.

Add the following paths and instructions to the existing agent workspace's
`AGENTS.md`, preserving its other instructions. Replace every placeholder with an
absolute path visible to that agent:

```text
Use the installed kpopper skills for this project.
Project directory: /absolute/path/to/project
kpopper command: /absolute/path/to/kpopper/bin/kpop
Read /absolute/path/to/kpopper/adapters/_shared/method-summary.md for the method.
At the start of work, run the command with --workspace /absolute/path/to/project open.
Use pull for the relevant subject before relying on recorded facts, and check
before finishing record changes. Report changed premises without rewriting old
judgments automatically. Mapping and scheduling require the user's chosen scope.
For writes after the initial source entry, use a JSON source report and the CLI
update --file /absolute/path/to/report.json route described in the record skill.
Before composing a report, read
/absolute/path/to/kpopper/skills/kpopper/INGESTION.md for the required envelope.
Keep Bash commands short: this host can refuse long serialized tool inputs before
asking for approval. Use the native file-writing tool for the JSON report, never
for GROUNDING.yaml. A reported refusal is not a successful update.
```

These are agent instructions, not a mechanical session hook. Confirm their
execution in a new conversation before relying on automatic behavior. With a
sandbox or remote worker, make the checkout, its native runtime and the project
available there and use that environment's paths.

## Check the installation

From the machine where the commands will run:

```sh
"$KPOPPER_REPO/bin/kpop" --workspace /absolute/path/to/project open
"$KPOPPER_REPO/bin/kpop" --workspace /absolute/path/to/project check
```

Then ask the agent:

> Use kpopper in this project. Open its record, pull the evidence relevant to my
> question, and show what needs review. Preserve useful findings and their sources
> while we work. Use the configured project and command paths.

For acceptance, plant a unique confirmation code in a local source and save a
numeric fact with a dependent judgment through captured CLI calls. In an independent
fresh conversation, ask for the code without revealing it and verify a read occurs
before the answer. Use synthetic readings dated yesterday and today for the change
test; a conflicting same-day reading needs `--hypothesis` and
`consolidate --dry-run`, not a forced write to the base. Confirm the old `seen`
snapshot remains intact. Test two agents with separate project directories as well
as a handoff to another host. The [shared compatibility checks](../../docs/compatibility.md)
give the exact pass criteria and distinguish command tests from conversation tests.

## Capability limits

| Capability | Current route |
|---|---|
| Skills | Canonical bundle skills; installation and discovery verified. |
| Read, write, check, render | Explicit `kpop` commands; the checkout's native runtime must be reachable. |
| Session opening | Agent instruction or explicit request; no runnable opener bundled for OpenClaw. |
| Stop gate | Run `check` explicitly; no enforced stop hook. |
| Grounding nudges and background delivery | No OpenClaw integration in the bundle. |
| Mapping, workers and scheduled reviews | Host-specific dispatch and scheduling are unverified; skill discovery does not supply them. |
| Record page | `kpop experimental hub` (optional experimental application) creates a local HTML file; viewing or delivering it depends on the host. |

## Verification boundary

On 2026-09-16, OpenClaw 2026.9.4 on macOS arm64 with Node 24.16.0 linked the
repository in isolated state. Its CLI reported all eight skills eligible and
model-visible. Plugin inspection reported the bundle's declared `skills` and
`hooks`, with no registered hook names. A separately installed kpopper 1.6.0 Python
runtime was used for the record smoke check.

On 2026-09-17, the `claude-cli/claude-fable-5-1` route completed source/fact creation,
an executable judgment, fresh-session recall of random confirmation codes in two
separate projects, and a later reading that fired the judgment without changing
its verdict or historical snapshot. Captured native transcripts show successful
record reads before both recall answers. The second project's record stayed
unchanged while the first changed, and the default Gateway workspace acquired no
record. This tests workspace selection, not a filesystem isolation boundary.

The test needed operator approvals and the file-based write route after denied
shell commands. OpenClaw state was separate, while the Claude backend used an
existing login and could see its native kpopper skills/hooks. Warnings from that
native profile's Python hooks were observed; they are not evidence that OpenClaw
executed the bundle's hooks. The explicit virtualenv CLI completed the checks.
No messaging channel, sandbox, default embedded provider, scheduling or other OS
was validated. See the [CLI backend contract](https://docs.openclaw.ai/gateway/cli-backends).

### Follow-up without a native kpopper installation supplying context

A separate checkout of the merged package and a fresh Python environment were
installed in new OpenClaw state. A test-only launcher replaced the backend's
`--setting-sources user` with an empty setting-source list and disabled auto memory
for that process. It preserved OpenClaw's explicit plugin, transport and permission
arguments, and retained the existing subscription login. Managed policy and global
client configuration were not bypassed.

The negative control listed no native plugins or kpopper skills. Every accepted
live session then listed only the `openclaw-skills` plugin, whose selected skill
paths resolved into the fresh checkout. The two projects independently recalled
different random codes after actual record reads. Updating one room's capacity
from 53 to 23 fired its decision against a requirement of 37; the entire judgment
and its `seen` values stayed unchanged. The second project's record was unchanged.
Earlier sessions and source values were absent from the recall prompts, and
session-history tools were unavailable.

This proves the bundle's skills can support the explicit-command workflow without
the preinstalled native kpopper plugin. It is still an operator-approved workflow:
the initial guide's long inline writes were refused, and one JSON report needed
correction by the model. The revised instructions above put the file-based route
and its exact contract path into the agent context. Successful checker output
alone was not the pass criterion; the generated records and captured tool reads
were also checked independently.

Official references:

- [Plugin bundles](https://docs.openclaw.ai/plugins/bundles)
- [Plugin CLI](https://docs.openclaw.ai/cli/plugins)
- [Skills and loading](https://docs.openclaw.ai/tools/skills)
- [Skill CLI](https://docs.openclaw.ai/cli/skills)
- [OpenClaw hooks](https://docs.openclaw.ai/automation/hooks)
