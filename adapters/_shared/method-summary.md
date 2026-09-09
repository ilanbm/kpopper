# kpopper — the method, condensed

Start with the user's goal: planning, research, coordination, decisions or software.
Keep one PROVENANCE.yaml entry point in the project's working directory for work that
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

Reuse an opening the hook already supplied. Otherwise run `kpopper start` for the workspace's
first-use context, and `provenance.py open` when a record exists. Use `kpopper start guide`
for the optional choice of learning while working, an initial map, or a deeper investigation.
Only an explicit choice authorizes mapping; missing files create no obligation to survey or
write. An unavailable registered record must not trigger a duplicate. The guide records
user/project choices and explanations actually shown, so sessions do not repeat onboarding.
Without `kpopper` on PATH, run `<plugin>/scripts/cli.py start` with Python instead.
Ground a subject with `pull`, trace what a change reaches with `affects`, gate on `check`.
