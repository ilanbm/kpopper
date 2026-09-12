---
trigger: model_decision
description: Maintain GROUNDING.yaml - an epistemic record of what is known, how each piece is grounded, and what would falsify each judgment. Use when work will be revisited, and when resuming such work.
globs: GROUNDING.yaml,PROVENANCE.yaml
labels: [provenance, epistemic]
modified: 2026-09-01
---

<!-- kpopper:method -->
# kpopper — the method, condensed

Start with the user's goal: planning, research, coordination, decisions or software.
Keep one GROUNDING.yaml entry point (an existing PROVENANCE.yaml is that entry point too,
read as it is) in the project's working directory for work that
will be revisited. Git is optional; sources may span documents, conversations, calendars,
task systems, files and earlier sessions. Record three things while working:

1. Something taken from a source — record where, precisely enough to go back: the source
   AND the location within it. Say whether it is quoted, a paraphrase, or your reading.
2. Something worked out — record the rule, never the result.
3. Something concluded — record what it rests on, a `seen` snapshot of those values, and
   `wrong_if`: what would make it wrong. A judgment that cannot be wrong is an opinion.

Never restate recorded material from memory — reference it. Structure is added only when
something observable forces it. When a dependency moves: flag, never rewrite — re-running
is allowed, applying is not. A declared hole (`blocked_on`) is honest; prose in a
predicate field is not, and neither is a `wrong_if` carrying a second comparison - one
comparison is evaluated, anything richer is refused and reported. A session's own prior knowledge is a source too — a `prior.*`
claim whose value is the confidence — and a judgment decided on one says in `reopened_by`
what would re-open it. A write that contradicts what the record holds is refused into a
hypothesis beside it (`--hypothesis NAME`), which `consolidate --dry-run` tests against the
base and `consolidate` folds only when the test is clean.

Reuse an opening the hook already supplied. Otherwise run `kpopper open` for the workspace's
context, including its first-use options. Use `kpopper _agent guide`
for the optional choice of learning while working, an initial map, or a deeper investigation.
Only an explicit choice authorizes mapping; missing files create no obligation to survey or
write. An unavailable registered record must not trigger a duplicate. The guide records
user/project choices and explanations actually shown, so sessions do not repeat onboarding.
Run an explicit mapping with `kpopper map --json` (`--deep` for deeper work), then accept
and execute the task using its internal protocol. A ready task is not completed work.
Use `kpopper config --guidance off` to disable explanations. Without `kpopper` on PATH,
run `<plugin>/scripts/cli.py` with Python and the same arguments.
Ground a subject with `pull`, trace what a change reaches with `affects`, gate on `check`.
<!-- /kpopper:method -->

## Open and close, by hand

Windsurf has no session-level hook here, so both ends of this are on you, not on glue
code:

**Opening.** Read the workspace's first-use context with:

    python3 <plugin>/scripts/cli.py open

Read that instead of the file whole — it is the record's own head plus only what
needs a person, ranked and cut to a budget.

**Closing.** Before finishing work that has a record, run:

    python3 <plugin>/scripts/provenance.py check

If it fails worse than it did when you started, fix the record — or declare the hole
with `blocked_on` — before you stop. Nothing enforces this: `hooks.json` in this same
directory runs `check` after every write to `GROUNDING.yaml` (or `PROVENANCE.yaml`) and can show you the
output, but it cannot stop you from finishing anyway, and its output never reaches you
automatically — see this adapter's README. This rule, read at the right moment, is the
actual mechanism.
