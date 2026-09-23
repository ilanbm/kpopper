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

Reuse an opening the hook already supplied. Otherwise run `kpop open` for the workspace's
context, including its first-use options. Use `kpopper _agent guide`
for the optional choice of learning while working, an initial map, or a deeper investigation.
Only an explicit choice authorizes mapping; missing files create no obligation to survey or
write. An unavailable registered record must not trigger a duplicate. The guide records
user/project choices and explanations actually shown, so sessions do not repeat onboarding.
Run an explicit mapping with `kpop map --json` (`--deep` for deeper work), then accept
and execute the task using its internal protocol. A ready task is not completed work.
Use `kpop config --guidance off` to disable explanations. Prefer the canonical
executable from `KPOPPER_AGENT_CONTEXT.command`. If it is unavailable, resolve the
active plugin's runtime with `sh <plugin>/scripts/native_runtime.sh --path`; do not
silently use PATH, pip or Python. The source-only Python dispatcher is available
only when `KPOPPER_RUNTIME=python` is explicit.
After finding relevant IDs, prefer `context <id>` for declared dependencies and checks.
Use `pull` for concise reads or if the checked reader is unavailable. Trace changes with
`affects <changed-id>` or `context <changed-id> --direction impact`; gate on `check`.

Use the project's mode: Simple shares one graph with named hypotheses across sessions;
Advanced adds branch contexts and `pending_grounding` for shareable findings independent
of a feature, even with one checkout. Several sessions may work on the same hypothesis.
Private or unclear sharing permission means a structured private draft outside Git.
Ordinary reads include relevant pending contributions with their scope and state; use an
explicit frozen read for committed PR/CI evidence. A measurement of an unmerged commit is
a fact only in its stated scope. Merge accepts content; it never proves truth, increases
confidence or refreshes `seen`. Remote publication needs explicit project authority.
<!-- /kpopper:method -->

## This adapter has no end check

`SessionStart` opens an existing record or supplies first-use guidance through
Gemini's JSON `additionalContext` field. Reuse that opening and the CLI command it
provides. The extension supplies this condensed method; it does not install the
separate kpopper skills.

No `SessionEnd` or `AfterAgent` hook checks the record when the session ends. Before
finishing work you were asked to leave in a state worth returning to, run `kpop check`
yourself and read the result.
