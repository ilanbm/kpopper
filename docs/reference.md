# Command and storage reference

For the product overview and a runnable example, start with the [README](../README.md).
The commands here use the record found for the current directory unless an explicit path
is supplied. Run `kpopper` for the command summary; the linked guides cover detailed options.

## Try it from the command line

The standalone CLI works independently of an agent plugin. With Python 3.9 or newer:

```sh
pipx install kpopper
# Alternatively, in a Python environment: python -m pip install kpopper
```

This installs PyYAML with the CLI. The ordinary reader and HTML page need no Lean setup.
An isolated CLI installation does not supply dependencies to an unrelated Python
environment used by a host's hooks.

Save [the launch-party example](../examples/launch-party/GROUNDING.yaml) as `GROUNDING.yaml` in
an empty directory. Run these commands there:

```sh
kpopper open                   # Project context, questions and attention signals
kpopper pull launch            # The announcement decision and its grounding
kpopper affects venue.status   # Decisions reachable from the venue's booking status
kpopper check                  # Check structure and declared breaking conditions
kpopper page --open            # Explore the record in a browser
```

Then, in that example directory, record the cancellation:

```sh
kpopper set venue.status cancelled --as-of 2026-09-10 --why "Venue cancellation email"
kpopper check
```

The second check exits with a failure because `venue.status` is no longer `confirmed`.
The launch announcement relied on that confirmation, so it needs review. The original
reasoning remains in the record. This is a text comparison; executable predicates are not
limited to numerical thresholds.

## Find and read the record

| Command | Purpose |
|---|---|
| `kpopper where` | Locate the record for this directory. |
| `kpopper open` | Read a bounded project orientation, namespace and attention report. |
| `kpopper pull <entry-or-prefix>` | Retrieve a subject's entries, sources and changed premises. |
| `kpopper affects <entry>` | Follow the downstream reach of an entry through judgments and rule references. |
| `kpopper check` | Report structural problems, declared gaps, movement and fired conditions. |

The legacy opener uses line and character budgets; its output reports omitted attention
items. It is not a complete read of every entry. The optional
[checked session mode](checked-sessions.md) provides complete branch accounting, exact
field references and reads bound to a record revision under a token budget.

## Record location and shape

For deferred work, `kpopper followups` links tasks to graph entries and explicit triggers.
`status`, `list`, `show` and `scan` inspect the queue; capture and lifecycle commands preserve
outcomes and coordinate execution. `daily plan` prepares a recommended daily review and
`daily bind` records a schedule actually created or inspected through the host's tools.
`daily install` coordinates inspection, installation and independent readback with the calling
host agent. Users can invoke the [watch plugin command](../skills/watch/SKILL.md)
to complete that flow without operating the individual protocol steps.
See [the followups guide](../skills/kpopper/FOLLOWUPS.md) for the full input contract,
task-system routing, recovery and daily workflow. Scheduling metadata is kept outside
`GROUNDING.yaml`; scanning and finishing tasks do not rewrite knowledge or `seen`.

`GROUNDING.yaml` lives in the project's working directory, and everything the record keeps
beside itself - hypotheses, the page's brief, the measurement recipes, the session profile,
what is built from it - lives in `.kpopper/` next to it. A record born under the earlier name,
`PROVENANCE.yaml`, is read as it is, with `PROVENANCE.d/`, `PROVENANCE.view.yaml`,
`PROVENANCE.measure.yaml` and `PROVENANCE.session.json` beside it, and is never created again.
A record moved by half - renamed with those files left under the earlier names, or a `.kpopper/`
opened beside a record still under the earlier name - fails `check` and is named by the opener,
since nothing reads what was left. Git is optional. The file can also point to existing material, including multiple record files:

```yaml
record: analysis/facts.yaml
also: analysis/claims.yaml
```

For a Git repository that keeps its record outside the working tree, register the absolute
path in the Git common directory:

```sh
printf '%s\n' '/absolute/path/to/GROUNDING.yaml' > "$(git rev-parse --git-common-dir)/kpopper-record"
kpopper where
```

This registration is shared by the repository's worktrees. It does not add the record to
Git. Use `where` to confirm what a directory resolves before writing to it.

A record of several files is read as one, and written where its subject already is. `set`,
`review` and a superseding judgment go into the file that holds the entry. A new entry goes
into the file whose collection already holds the entries its id shares a head with - `mtg.x`
joins the file that holds the other `mtg.` entries - and a head the record has not met yet
goes into the first file that holds the collection. A record in one file is unaffected.

The recommended fields are `from`, `rests_on`, `wrong_if` and `seen`. The reader infers
dependency, predicate and snapshot roles by shape, so existing vocabularies can work.
Ambiguous roles require an explicit `schema` declaration; the reader refuses to guess.
The [method's record examples](../skills/kpopper/references/shape.md#the-shape) cover the full shape.

Source locations and dates preserve traceability. They do not cause the reader to fetch
documents, query calendars or inspect every linked file. The person or agent records the
relevant reading using the tools and permissions available to that session.

Derived entries store their rules:

```yaml
known:
  workshop.spare_packs:
    rule: "stock.packages - workshop.guests"
```

Here the two inputs refer to the [workshop example](../examples/workshop/GROUNDING.yaml).
The reader follows those references for dependency reach and displays the rule. It does
not evaluate arbitrary arithmetic formulas into current values. A predicate over an
unevaluated derived value cannot be treated as a successful check. The core also has
specific built-in counts; those do not make it a general formula engine.

## What check means

| Outcome | Meaning |
|---|---|
| Failure: fired predicate | A supported `wrong_if` comparison is true on the recorded values. |
| Failure: structural or undeclared gap | Examples include unresolved dependencies, missing dependency snapshots within an inferred snapshot field, undeclared predicate references, or unsupported predicates without an explanation. |
| `MOVED` | A comparable dependency differs from the last-review snapshot. This calls for attention and does not itself fail the check. |
| Movement inside a condition | A changed dependency is named by a predicate that still evaluates false; the movement is muted. |
| Declared gap | `blocked_on` explains why a condition cannot currently be checked. This is reported as a note. |
| Human re-opener | `reopened_by` names a sign a person must interpret. It is reported, not mechanically evaluated. |

A record with no inferred snapshot field is reported as lacking a basis for drift detection.
Keep a `seen` value for every declared dependency so later checks have a meaningful before.
Legacy scalar comparison skips dependencies without comparable current values, including
general rules. Use `affects` to inspect their declared reach; reach alone is not evidence
that a downstream judgment is false.

The ordinary checker accepts a restricted single-comparison predicate. Richer expressions
and prose need an explicit explanation of what cannot be evaluated. The optional Lean
core has its own [typed predicate subset](checked-sessions.md#guarantees-and-limits),
including an explicit unknown result for missing or incompatible values. Do not assume
the two evaluators accept every literal in the same way.

A passing check establishes neither source accuracy nor the sufficiency of a conclusion's
premises. `rests_on` declares a dependency, not a logical implication. Choosing the condition
and confirming the underlying evidence remain part of review.

## Write and review

| Command | Purpose |
|---|---|
| `kpopper set <id> <value> --why "reason" --as-of YYYY-MM-DD` | Record a scalar reading with its explanation and date. |
| `kpopper add <id> field=value ...` | Add an entry or judgment. Judgment snapshots are filled from the record. Where no record resolves for the workspace, the first `add` creates `GROUNDING.yaml` at its root with that entry. |
| `kpopper review <id>` | Refresh a judgment's snapshot after reviewing it against the current record. |
| `kpopper review "section title"` | Refresh the page section's review snapshot. |
| `kpopper same <a> <b>` | Record that two IDs describe one subject; by default, retire `b` into `a`. |
| `kpopper distinct <a> <b> "reason"` | Keep a similar-looking pair distinct with a recorded reason. |

Writes report their downstream reach. `review` records that review happened; it does not
perform the intellectual review for you. Updating `seen` alone cannot make a fired
predicate false.

`add` names nearby existing entries to help identify possible duplicates. Similarity does
not decide identity: `same` and `distinct` record that decision explicitly.

### Hypotheses and consolidation

A competing claim can live in `.kpopper/hypotheses/<name>.yaml` beside the base record. The writer
can refuse a contradictory reading or a premature judgment rewrite and suggest a hypothesis;
write it explicitly with `--hypothesis NAME`. A newly dated observation may supersede an
older reading, while same-day disagreement remains a conflict. A standing judgment has its
own rules for when it can be replaced.

```sh
kpopper consolidate --dry-run   # Check the proposed combination without applying it
kpopper consolidate            # Fold eligible hypotheses through the guarded writer
kpopper consolidate --refute NAME "Reason for refutation"
kpopper consolidate --from BRANCH_OR_REF --dry-run
```

The dry run reports changed premises, fired falsifiers, structural gaps, contested IDs and
possible duplicates. A clean structural check is not enough to fold a hypothesis whose
premises still need review. Refutation retains a negative finding. `--from` reads another
branch's committed record; it never pushes to that branch.

These checks can run during work; they do not require a pull request or merge. The receiving
record is the base, and `--from` overlays the named ref's committed differences. Importing
`main` into a worktree therefore asks a different question from applying that worktree's
changes to current main. Refs come from local Git objects; the command does not fetch them.

The callable `union_of(base, hypotheses)` also accepts an in-memory hypothesis for a read-only
what-if. That is a building block for previewing a captured working-copy delta on main. The opt-in `watch` runner uses this core for asynchronous worktree-versus-main previews.
The existing background ingestion path continues to process explicit reports for one record.

See the [record](../skills/record/SKILL.md) and [consolidate](../skills/consolidate/SKILL.md) skills for the full write and consolidation discipline.

## Background branch watch and shared facts

```sh
kpopper watch setup                         # local checks, no host schedule
kpopper watch setup --base-ref origin/main # preserve a deliberate base
kpopper watch scan                          # queue and return immediately
kpopper watch status                        # current versions and findings
kpopper watch scan --all                    # include registered worktrees
kpopper watch pause                         # pause local checks and shared writes
```

Watch applies only the worktree's authored changes since its merge base to the selected
main snapshot. It includes uncommitted graph changes, coalesces events and suppresses stale
or unchanged findings. Session hooks and daily review start queue checks without waiting for
analysis. No graph, hypothesis or review snapshot is written by comparison. Local Git refs
are not proof of remote freshness; the runner never fetches automatically.

To capture observations independent of branch code, configure an existing external record
with `watch setup --shared-record /absolute/GROUNDING.yaml`, or explicitly select
`--shared-private`. `watch share --file REPORT.json` requires an external environment scope,
source location, quotation, date and scalar value. The background writer preserves evidence,
uses the canonical writer, serializes commits and retains conflicts for review. `watch shared`
reads the same facts and report states from main or any worktree. `watch resolve EVENT_ID
--evidence TEXT` closes a reconciled report without applying it. It never promotes arbitrary
branch content or silently changes a shared observation's scope.

Shared facts participate in watch's read-only compatibility view. Existing branch-only
readers/CI are unchanged; keep committed evidence self-contained. Shared destinations with
pointers, multiple files or hypotheses remain readable, while automatic writes require review.
For an ID present on both sides, comparison checks the complete typed entry, its collection
and provenance against main, the worktree and its hypotheses. A metadata-only difference or
an unchanged inherited conflict is reported; identical copies are accepted. This check
detects divergence but does not copy or synchronize entries between the records.
Branch-only entries are allowed. Their absence from the shared record is not a conflict and
does not call for copying them into it; the complete graphs need not be identical.

Claude async hooks can wake their session. Ordinary Codex hooks deliver on the next model
opportunity; `watch scan --notify-task HOST_TASK_ID` returns an optional native-agent delivery
job for hosts supporting background agents and task messaging. A job must actually be dispatched;
Python alone cannot call host tools. Persistent writes require POSIX locking. See the
[watch protocol](../skills/watch/references/compatibility.md) for scope, delivery and recovery.

## Background capture

```sh
kpopper ingest capture --file report.json
kpopper ingest status --event-id EVENT_ID
kpopper ingest pending
```

Capture retains the supplied report and normally starts a separate worker. Supported
updates pass through the canonical writer. Raw reports, journals and receipts live in a
private state directory outside the repository, keyed by the canonical record path.

Automatic writes currently require explicit reports targeting existing scalar entries in
a single file. New entries, computed values, judgment rewrites, pointer records and
ambiguous messages require further handling. Delivery acknowledgment does not approve
a change or clear a failed condition.

Use the [capture guide](../skills/kpopper/INGESTION.md) for report envelopes, state paths,
permissions and host behavior, and [native delivery](../skills/kpopper/DELIVERY.md) for
returning important findings to an originating Codex task on supported hosts.

## Measurement

An entry can name a recipe with `measure: recipe_name`. `.kpopper/measure.yaml` beside
the record maps that name to an argument list. Treat this file as executable configuration
and review it as code.

```sh
kpopper remeasure       # Inspect the plan; run nothing
kpopper remeasure --run # Execute the declared recipes and check the proposed readings
```

Recipes run without a shell, with a timeout and output limit. Differences are checked as
a hypothesis; measurement does not silently rewrite the record. This can support repeatable
checks of repository facts, or other deliberately configured observations. It is not an
automatic external monitoring service. See [Contributing](../CONTRIBUTING.md) for the
repository's measurement and CI contract.

## Page and browser checks

```sh
kpopper page --open
kpopper page --open --tree
kpopper page --verify
kpopper page                      # written to .kpopper/build/page.html
kpopper page --checks .kpopper/build/page.html
```

The renderer generates a self-contained HTML snapshot. The deterministic `--verify` checks
need no browser. Interactive browser checks additionally need Node 18+, Chrome or Chromium,
and `playwright-core`, which is not bundled:

```sh
npm i --no-save playwright-core
kpopper page --checks .kpopper/build/page.html
```

The checker looks for the driver beside the page; `NODE_PATH` can point at an existing
installation. Set `CHROME=/path/to/chrome` if the browser is not on a known path. The browser
checks exercise both themes and reduced motion. A real browser is needed to inspect the
interactive page; a preview that strips JavaScript will show only part of its behavior.

See [PAGE.md](../skills/kpopper/PAGE.md) for arrangements, components, reference cards,
localization, coverage and prose-drift checks.

## Authored HTML documents

`kpopper document build` packages an authored document with its selected evidence,
`document inspect` validates and reads a saved copy without running its scripts, and
`document refresh` prepares a new copy against explicitly supplied sources. The authoring
agent creates anchors and mapping during ordinary document work. The final HTML contains
all display resources and review state; only source refresh needs the agent and inputs.
See [the user flow](documents.md) and run `kpopper document guide` for the author contract.

## Distribution and implementation

| Part | Source |
|---|---|
| Method and agent guidance | [skills/kpopper](../skills/kpopper/SKILL.md), one skill per occasion beside it: [ground](../skills/ground/SKILL.md), [record](../skills/record/SKILL.md), [map](../skills/map/SKILL.md), [document](../skills/document/SKILL.md), [page](../skills/page/SKILL.md), [consolidate](../skills/consolidate/SKILL.md), [watch](../skills/watch/SKILL.md) |
| CLI dispatcher | [scripts/cli.py](../scripts/cli.py), also exposed by `scripts/kpopper` |
| YAML reader, checks and writer | [scripts/provenance.py](../scripts/provenance.py) |
| Background report processing | [scripts/ingestion.py](../scripts/ingestion.py) |
| Optional session transport and Lean core | [scripts/session](../scripts/session), [setup guide](checked-sessions.md) |
| HTML renderer | [scripts/render_page.py](../scripts/render_page.py) |
| Browser verification | [scripts/verify_page.js](../scripts/verify_page.js) |
| Agent integration | [hooks](../hooks/hooks.json), [adapters](../adapters/README.md) |

PyPI installs the Python CLI and page tools. Agent plugins add the method and host-specific
hooks. The npm package provides the Node browser checker, not the Python CLI. Shared code
comes from the same source files, with one version across distribution manifests.

Every command reads the whole record, so a file's parsed form is kept under
`$XDG_STATE_HOME/kpopper/cache`, or `~/.local/state/kpopper/cache`, one private entry per
file, and taken again whenever the file's path, length, last write or content differs. The
files are the authority: an entry that cannot be read, or holds anything but a document, is
simply a parse, and every write drops the entry for the file it wrote. `kpopper --no-cache
<command>`, or `KPOPPER_NO_CACHE=1`, parses every time.

The plugin's hooks are the layer every session gets without choosing it, and they carry
pointers, never values. The session opener prints the record's head and what needs a person,
and names the next move as the host invokes a skill (`/kpopper:ground` in Claude Code,
`$ground` in Codex); in a project without a record it says so in two lines. At every prompt
a grounding line names at most three entries whose ids, names or verdicts the prompt's words
touch, with the skill that reads them; an entry is named until it is read, then again only
when its recorded body changes or the session compacts, and an unread one repeats after a
cooldown of ten prompts. Before a file is edited, the entries whose source it is, or whose
reading a recipe takes from it, are said once. The stop gate compares with a session-start
baseline and reminds once about new failures, intents no tab serves and entries with no intent;
a session that changed files of the tree, or ran eight prompts, with the record untouched is
asked once whether there was nothing to keep - as a stop, on the turn after the same
question rode a prompt unanswered. An unchanged judgment whose falsifier fires after a new
scalar reading can remain flagged while recording finishes; `check` still reports it.
Adapters differ in their ability to block, remind or deliver asynchronously—consult the
[capability matrix](../adapters/README.md#capability-matrix).

From a source checkout, run `python3 scripts/cli.py <command>` or `scripts/kpopper <command>`.
When using an installed plugin without a `kpopper` command on `PATH`, use the same dispatcher
under that plugin's `scripts/` directory. Installation paths are versioned; locate the
active installation rather than retaining a path to an older copy.

The method's lasting constraints are traceable grounding, preserving derivation rules,
propagating declared impact without automatically adopting conclusions, and migrations
that keep the checks passing when the record's shape changes.
