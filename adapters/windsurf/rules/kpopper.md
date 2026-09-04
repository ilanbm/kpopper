---
trigger: model_decision
description: Maintain PROVENANCE.yaml - an epistemic record of what is known, how each piece is grounded, and what would falsify each judgment. Use when work will be revisited, and when resuming such work.
globs: PROVENANCE.yaml
labels: [provenance, epistemic]
modified: 2026-09-01
---

<!-- kpopper:method -->
# kpopper — the method, condensed

Keep one file, PROVENANCE.yaml, at the project root, for work that will be revisited.
Record three things while working, never as a separate step:

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

Open a session with `provenance.py open` instead of reading the record whole; ground a
subject with `pull`, trace what a change reaches with `affects`, gate on `check`.
<!-- /kpopper:method -->

## Open and close, by hand

Windsurf has no session-level hook here, so both ends of this are on you, not on glue
code:

**Opening.** Before reading or changing anything in a project that has a
`PROVENANCE.yaml` at its root, run:

    python3 <plugin>/scripts/provenance.py open --chars 2000

Read that instead of the file whole — it is the record's own head plus only what
needs a person, ranked and cut to a budget.

**Closing.** Before treating the work as finished, run:

    python3 <plugin>/scripts/provenance.py check

If it fails worse than it did when you started, fix the record — or declare the hole
with `blocked_on` — before you stop. Nothing enforces this: `hooks.json` in this same
directory runs `check` after every write to `PROVENANCE.yaml` and can show you the
output, but it cannot stop you from finishing anyway, and its output never reaches you
automatically — see this adapter's README. This rule, read at the right moment, is the
actual mechanism.
