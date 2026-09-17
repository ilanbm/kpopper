# Host compatibility and release checks

kpopper shares a record and runtime across agents. Each host still has its own
installation, skill discovery, hook protocol, filesystem and background behavior.
Report those capabilities separately when deciding whether a release is ready.

## Verification matrix

Compatibility work started 2026-09-16, with follow-up checks on 2026-09-17, on macOS arm64.
These results apply to the tested versions and routes, not all releases or clients.
The initial loader/protocol pass ran no real-model conversation. Subsequent live
exercises are qualified separately below; they do not turn every route into a
supported end-to-end integration.

| Host / version | Verified boundary | Still to verify |
|---|---|---|
| **Claude Code / Codex shared scripts** | Record birth, reopen, relocation, Claude-to-Codex payload handoff, failure reporting and stop continuation in real subprocesses; named-main regression fixed | Installation and consumption in clean clients, live named-main delivery, active/idle background delivery. |
| **Codex CLI 0.153.3** | Two real ephemeral sessions using the existing profile: CLI-authored source/readings/judgment and independent recall of a random code after reading the record and source | Clean plugin installation, changed-premise live turn, Desktop/IDE behavior and active/idle delivery. |
| **OpenClaw 2026.9.4** | Bundle/skill discovery; fresh checkout/runtime and native user/project customizations excluded; live Claude CLI writes, independent random-code recall in two projects, changed-premise detection with unchanged judgment and snapshot | New account/OS profile, other providers, sandbox/remote paths, automatic hooks and delivery. |
| **OpenCode 1.18.31** | Isolated native binary accepts configuration and discovers all eight canonical skills at original paths | Model tool use, session resumption, Desktop/IDE behavior. |
| **Gemini CLI 0.43.0** | Isolated extension link/list; installed host hook runner and aggregator accept startup JSON context; adapter regression tests | Live probe stopped before a model call because the selected API-key authentication had no key in the test environment; lifecycle/shutdown, resume and sandbox execution remain unverified. |
| **Copilot CLI 1.0.75** | Actual CLI hooks and native payloads; real-model creation, fresh record read and changed-premise detection with canonical skills | Complete citation metadata, randomized fresh-session recall, live cross-agent handoff and other operating systems. |
| **Cursor** | Shell wrappers handle native-shaped payloads, executable symlinks, paths with spaces and one follow-up; documentation reviewed | Actual local client delivery, resume/compaction and multi-root behavior. Hosted cloud lacks the opening event used by this adapter. |
| **Claude Cowork** | Package route and runtime requirements reviewed against official documentation | Installation in Cowork, Python/runtime access, hook behavior and persistence across conversations. |
| **Copilot VS Code / cloud** | Current documentation reviewed and guide corrected | Local/hosted executions of the provided route. |
| **Windsurf / ChatGPT Work** | Existing guides retain their limitations; no new runtime verification | Host installation and session behavior. |

Separately, an isolated Python package install exercised first use, source and
judgment writes, reopening, a later reading, impact tracing, an expected failing
condition and HTML page generation. The earlier `seen` value remained intact.
This validates the common runtime; it is not an OpenClaw or OpenCode model test.

The Gemini adapter now translates raw opener text to the host's JSON context
format. The Copilot CLI bridge translates both opening and blocking responses.
The [adapter matrix](../adapters/README.md#capability-matrix) describes the resulting
behavior and links the installation guides.

### OpenClaw live exercise

The Claude CLI backend with Fable 5.1 completed the core workflow on two isolated
project directories. Each held a different random confirmation code, absent from
its fresh-conversation prompt. Both native transcripts show a successful record
read before the correct answer, with no writes in the recall turns. Updating one
project's capacity from 47 to 19 fired its existing condition against requirement
32 while the verdict and `seen: 47` remained intact. The other project's record
remained unchanged and neither record appeared in the default Gateway workspace.

This was an **operator-assisted workflow pass for that backend**, not a clean-client
or all-provider certification. Initial calls failed under the test's tool policy;
explicit approvals and JSON source reports allowed completion without changing
that policy to unrestricted execution. The backend retained an existing Claude
login/profile, including native skills/hooks, so its own hook output cannot prove
OpenClaw bundle-hook support. The virtualenv CLI was verified independently.
Messaging, sandboxing, remote execution and automated delivery remain untested.

### OpenClaw customization-isolated follow-up

A second run closed the native-plugin fallback gap: a fresh checkout at the merged
commit and fresh Python environment were used in separate OpenClaw state. A
test-only launcher removed user/project setting sources while preserving OpenClaw's
explicit plugin, MCP/transport and permission arguments. Auto memory was disabled;
existing subscription authentication and managed policy remained. This isolates
customizations, not the account or operating system.

A negative control exposed no kpopper skills. Native initialization in all five
accepted sessions listed only `openclaw-skills`, and captured symlink targets led
to the fresh checkout. Independent assertions checked two distinct random-code
recalls, observed record reads before answers, unchanged records during recall,
and a source change from capacity 53 to 23 against requirement 37. The entire
decision and its historical snapshot stayed unchanged, and the unrelated project
stayed intact. Session-history tools were unavailable. An earlier read that
overlapped source revision was excluded and replaced by a stable fresh read.

The original long inline writes failed before approval. The revised guide asks for
JSON source reports with a short `update --file` command and names the exact report
contract. One first report was rejected for missing fields; the model corrected it
after reading that contract. The accepted run used ordinary scoped tool approvals,
not unrestricted execution, and no hand-edited record. This is functional proof
with visible setup/authoring friction, not unattended-operation or all-provider
certification.

### Copilot live exercise

Three independent skill-enabled sessions used `claude-haiku-4.5` selected by the
CLI's `auto` mode. Creation produced a real executable judgment. A fresh session
read the record before answering without changing it. A revised source contained
a new capacity and an arbitrary reference absent from the prompt; the model read
both, updated the capacity, and left the complete decision and its earlier `seen`
value unchanged. An independent checker reported the intended fired condition.

This was a **core workflow pass with provenance gaps**. The model omitted a source
read date despite claiming it in prose, incompletely described a source on recall,
and left a revised citation in an update comment after a structured citation change
was refused. The fresh-read turn had no random nonce, so it proves read-before-answer,
not the stronger randomized recall check below. An earlier instructions-only run
used a different model and tighter permissions and failed; it is not a controlled
comparison of skills versus instructions.

## Release acceptance scenario

Run this small scenario in each host for which the release promises operational
support, using a disposable project and an isolated host profile where available:

1. **Install the documented route.** Record the host/version, OS, runtime and paths.
   Confirm the intended skills or instructions and any configured hooks load.
   For an automatic opener, capture the model receiving the opening, not just a
   notification shown to the user. A seeded unique opening marker can distinguish
   the supplied context from a guess. For an instruction-only route, capture the
   actual `open` call and do not describe it as a hook.
2. **Start empty.** Opening should give guidance without creating a record. Put an
   arbitrary, newly generated confirmation code and a numeric reading in a local
   source file. Require captured CLI `add` calls for the source, reading and a
   dependent judgment, followed by a green `check`. Hand-written YAML does not
   establish that the installed CLI is reachable.
3. **Reopen.** Use an independent fresh conversation with no earlier transcript or
   task memory. Ask for the confirmation code without including it in the prompt.
   Pass only when the captured transcript shows a record/source read before the
   answer and the answer matches the planted value.
4. **Change a premise.** Use a synthetic fixture whose initial reading is dated
   yesterday and whose revised source is dated today. The update must name the
   later date with `--as-of`; never use a future date to force acceptance. Verify
   the affected judgment is reported and its historical snapshot is preserved.
   To test a conflicting same-day reading instead, use
   `kpop set <id> <new-value> --hypothesis acceptance`, then
   `kpop consolidate --dry-run acceptance`. The base remains unchanged, so
   `check` alone is not expected to report that hypothetical value. Do not silently
   fold the hypothesis or review the judgment.
5. **Move between hosts.** Open the same project in a second agent. Repeat with the
   full project copied elsewhere, including sources and `.kpopper/` sidecars.
6. **Exercise the promised automation.** Introduce a new structural failure and
   confirm the documented stop behavior and loop guard. If background delivery is
   promised, test an important event during work and while idle separately.
   For Claude Code, include a named main agent and a true subagent, checking both
   delivery to the main agent and suppression for the subagent.

For OpenClaw, also run two agents against two distinct project directories in the
same isolated Gateway. Give each source a different confirmation code. Verify each
fresh conversation reads its own record and that the Gateway's default workspace
does not acquire either project's record. Keep external channels and scheduled
jobs disabled in the test profile.

Capture tool calls, outputs and resulting files. A configuration parser, synthetic
payload or fixed-response provider can prove a protocol boundary; only a real
conversation demonstrates the model using it.

## Scope before launch

Prioritize clean-client tests for Claude Code, Codex, Cowork and Cursor, and the
documented OpenClaw route. Gemini CLI, Copilot CLI and OpenCode have useful bounded
checks above, but should retain that qualification until their conversation tests
pass. Do not advertise feature parity merely because installation succeeds.

Cover macOS, Linux and Windows/WSL explicitly. The shell adapters currently use
POSIX commands; native Windows requires separate proof. CI matrices for the core
runtime are not proof of host installation. Include a non-Git project, paths with
spaces, and a sandbox or remote worker where that route is advertised.

The initial pass on baseline `5c2062b` reproduced a Claude-specific issue: continuing hooks filtered
on `agent_type` and suppressed named main agents as well as subagents. The follow-up
corrects all five continuing hooks to identify subagents by `agent_id`, matching
the opener. Five behavioral regressions failed before the fix and passed afterward;
86 related tests passed. Live named-agent delivery remains a separate host check.
See [Claude's common hook fields](https://code.claude.com/docs/en/hooks#common-input-fields).
