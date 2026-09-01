# kpopper adapters

kpopper's three scripts (`scripts/provenance.py`, `render_page.py`, `verify_page.js`)
and its dispatcher (`scripts/kpopper`) are harness-neutral — nothing under `scripts/`
knows which coding agent is running it. What *is* harness-specific is the glue: how a
session gets told to open the record, how (or whether) a harness can stop a session
that leaves it worse than it found it, and where each harness looks for prose and
skills. This directory holds that glue, one subdirectory per harness, and changes
nothing under the kpopper root itself.

## Design principle

**Content is placed, not rewritten.** Every harness gets the same seven paragraphs —
`_shared/method-summary.md` — copied verbatim into whatever file that harness reads on
its own (`AGENTS.md`, a Cursor `.mdc` rule, `GEMINI.md`, a Windsurf rule, Copilot's
instructions file). None of them get a paraphrase or a harness-flavored rewrite; the
method doesn't change shape depending on who's reading it, only its container does.

**Hook glue is per-harness**, because the underlying mechanisms genuinely differ, not
because it needs to look different. Two harnesses (Codex, Gemini) turned out to share
enough of Claude Code's own hook contract that they reuse `scripts/session_open.sh`
and `scripts/session_gate.sh` unmodified. Two (Cursor, and marginally Windsurf) don't,
and get small translating wrappers instead. One (Windsurf) and one surface of another
(Copilot's cloud agent) can't run a blocking session-level hook at all, at which point
the honest move is to say so — in prose, in the tool's own voice — rather than ship a
hook that looks like a gate and isn't one.

**The prose fallback is guarded by a drift check.** Every file that embeds the method
wraps its copy in `<!-- kpopper:method -->` / `<!-- /kpopper:method -->` markers.
`_shared/check-drift.sh` extracts every wrapped block across this whole tree and diffs
each one against `_shared/method-summary.md` — the one place the text is actually
authored. A change to the method that only lands in one harness's file is a failing
check, not a silent fork. Run it after touching any embed:

    sh adapters/_shared/check-drift.sh

## Capability matrix

| harness | opener (session start) | gate (session end) | skill / rule placement | page viewing |
|---|---|---|---|---|
| Claude Code *(baseline, unchanged)* | `SessionStart` hook, blocking-capable | `Stop` hook, exit 2 blocks, bounces once | `skills/kpopper/` loaded natively by the plugin engine | `kpopper page` writes `record.html`; open in any browser |
| **Codex CLI** | `SessionStart` hook — reuses `session_open.sh` as-is | `Stop` hook — reuses `session_gate.sh` as-is, exit 2 blocks | `AGENTS.md` snippet + `skills/kpopper` symlinked into `.agents/skills` | same `record.html`, opened by hand |
| **Cursor** | `sessionStart` hook, translating wrapper (fire-and-forget by design) | `stop` hook, translating wrapper — no true block; `followup_message` bounces once, then yields | `rules/kpopper.mdc` — agent-requested + auto-attached on `PROVENANCE.yaml` | same, opened by hand |
| **Gemini CLI** | `SessionStart` hook (extension-bundled) — reuses `session_open.sh` as-is | `SessionEnd` hook — advisory only, cannot block by design | `GEMINI.md`, the extension's context file | same, opened by hand |
| **Windsurf / Cascade** | none — no session-level hook exists here at all | none — `post_write_code` nudge is UI-only, never reaches the agent and can't block | `rules/kpopper.md` — carries open *and* close duties in prose, since nothing else will | same, opened by hand |
| **Copilot (VS Code)** | `copilot-instructions.md` carries the instruction; `hooks.json` shipped but **not load-bearing** (Preview, unconfirmed cwd/payload) | same caveat | `copilot-instructions.md` (auto-read), same content also valid as `AGENTS.md` | same, opened by hand |
| **Copilot (cloud agent)** | `copilot-setup-steps.yml` runs `open` once, visible in the agent's setup logs | `provenance-check.yml` — a real gate, as a PR status check (CI red/green, not a session bounce) | `AGENTS.md`, if the repo has one (cloud agent has no interactive rule/skill surface) | not applicable — no interactive session to view a page in |
| **Cowork** | identical to Claude Code — same plugin engine | identical | identical | identical |

## Install, per harness

- **Codex CLI** — `adapters/codex/README.md`
- **Cursor** — `adapters/cursor/README.md`
- **Gemini CLI** — `adapters/gemini/README.md`
- **Windsurf / Cascade** — `adapters/windsurf/README.md`
- **Copilot** (VS Code + cloud agent) — `adapters/copilot/README.md`
- **Cowork** — `adapters/claude-cowork/README.md` (enable the account plugin; nothing to copy)

Every per-harness README cites the doc URLs its schema claims were checked against,
and has a section naming what could *not* be confirmed from official docs — treat
those as shipped-but-unverified, not as settled.
