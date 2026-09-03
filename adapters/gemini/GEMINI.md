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
predicate field is not. A session's own prior knowledge is a source too — a `prior.*`
claim whose value is the confidence — and a judgment decided on one says in `reopened_by`
what would re-open it. A write that contradicts what the record holds is refused into a
hypothesis beside it (`--hypothesis NAME`), which `consolidate --dry-run` tests against the
base and `consolidate` folds only when the test is clean.

Open a session with `provenance.py open` instead of reading the record whole; ground a
subject with `pull`, trace what a change reaches with `affects`, gate on `check`.
<!-- /kpopper:method -->

## This harness cannot enforce the stop gate

`SessionStart` opens the record automatically. There is no `SessionEnd` equivalent of
the stop gate the Claude Code plugin ships — Gemini CLI does not wait for a
`SessionEnd` hook and ignores any flow-control field it returns, so nothing can stop
a session from ending, however far `check` has regressed. `hooks/checknote.sh` still
runs `check` and prints what it finds, but treat that as a request, not a gate: a
message you can walk away from, not one that stops you. Before finishing work you
were asked to leave in a state worth returning to, run `check` yourself and read it.
