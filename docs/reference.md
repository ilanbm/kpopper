# Command and storage reference

For the product overview and a runnable example, start with the [README](../README.md).
The commands here use the record found for the current directory unless an explicit path
is supplied. Run `kpop` for the command summary; the linked guides cover detailed options.

## Try it from the command line

The standalone native CLI works independently of an agent plugin and needs no Python,
Node or Rust at runtime. Download `install.sh` (Unix) or `install.ps1` (Windows) from the
[Latest GitHub release](https://github.com/ilanbm/kpopper/releases/latest). The installer
downloads the matching archive for `linux-x86_64`, `linux-aarch64`, `darwin-arm64`,
`darwin-x86_64` or `windows-x86_64` and verifies its SHA-256. On Unix:

```sh
sh install.sh --prefix "$HOME/.local"
```

On Windows:

```powershell
pwsh -File install.ps1 -Prefix "$HOME/.local"
```

These commands select the release marked Latest. To pin a release, add `--version VERSION`
on Unix or `-Version VERSION` on Windows. Both installers also accept an explicit offline
archive and checksum. The binaries land in `PREFIX/bin`; put that directory on PATH or use
the absolute `kpop` path. Release assets are named `kpopper-VERSION-TARGET.tar.gz` (Unix)
or `.zip` (Windows).

The earlier Python implementation is no longer part of this tree. Its last release is
`kpopper` 1.8.1 on PyPI, and its source is the tag
[`python-final`](https://github.com/ilanbm/kpopper/releases/tag/python-final).

The command is `kpop`. `kpopper` names the project and the installed package, and is
kept as a second command name, so either spelling runs.

Save [the launch-party example](../examples/launch-party/GROUNDING.yaml) as `GROUNDING.yaml` in
an empty directory. Run these commands there:

```sh
kpop open                   # Project context, questions and attention signals
kpop pull launch            # The announcement decision and its grounding
kpop affects venue.status   # Decisions reachable from the venue's booking status
kpop check                  # Check structure and declared breaking conditions
```

Then, in that example directory, record the cancellation:

```sh
kpop set venue.status cancelled --as-of 2026-09-10 --why "Venue cancellation email"
kpop check
```

The second check exits with a failure because `venue.status` is no longer `confirmed`.
The launch announcement relied on that confirmation, so it needs review. The original
reasoning remains in the record. This is a text comparison; executable predicates are not
limited to numerical thresholds.

## Find and read the record

| Command | Purpose |
|---|---|
| `kpop where` | Locate the record for this directory. |
| `kpop open` | Read a bounded project orientation, namespace and attention report. |
| `kpop pull <entry-or-prefix>` | Retrieve a subject's entries, sources and changed premises. |
| `kpop search "terms"` | Find matching claims, native hypotheses and local source passages with their status. |
| `kpop affects <entry>` | Follow the downstream reach of an entry through judgments and rule references. |
| `kpop export <entry> [entries...]` | Export a focused excerpt with historical/current readings and optional Mermaid. See [graph export](graph-export.md). |
| `kpop check` | Report structural problems, declared gaps, movement and fired conditions. |
| `kpop assess <entry> [entries...]` | Read versioned findings and scoped attention as JSON. See [assessment contract](assessment.md). |

In a Git checkout, `check` notes a `file:` locator whose path is absent from the repository.
It also checks single path tokens in `from:`, including bare filenames with common extensions
such as `pyproject.toml`; recorded entry IDs take precedence. Use `./` or a source's `file:`
for an unfamiliar root filename. Relative paths start beside the primary record, as in the Hub;
Git pins use repository-relative paths. Absolute paths outside the repository and glob patterns are
not checked. A missing file is a note rather than a failed claim: re-read the surviving source,
or preserve a historical locator as `file: "python-final:scripts/example.py"` (Git's
`revision:path` form). It remains openable with `git show python-final:scripts/example.py`.
The named revision must contain that file; an unknown revision or absent blob is noted too.
`at:` still identifies the place within the source, and `read:` still records when it was read.

The legacy opener uses line and character budgets; its output reports omitted attention
items. It is not a complete read of every entry. The optional
[checked session mode](checked-sessions.md) provides complete branch accounting, exact
field references and reads bound to a record revision under a token budget.

Search uses a temporary local FTS5 index and calls no model. Results include source anchors,
scope, status, explicit omission counts and a `search-corpus` revision for exact source
reads. Use `--limit` and `--chars` to bound output; `--read REF --revision REV` reads a hit,
with `--offset`/`--length` for long text. It supports local UTF-8 text sources up to 1 MiB,
reports unindexed sources and never fetches remote material. See [retrieval](../skills/kpopper/RETRIEVAL.md).

Both search and `open --json` return `record_sha256`, which binds additions to the primary
record bytes the agent read. It is distinct from checked-session and search-corpus revisions.

## Record location and shape

For deferred work, `kpop followups` links tasks to graph entries and explicit triggers.
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
kpop where
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
    rule: {expr: "stock.packages - workshop.guests"}
```

Here the two inputs refer to the [workshop example](../examples/workshop/GROUNDING.yaml).
The reader parses the stored formula, derives dependency links and uses the packaged Lean core
to compute the value. A missing core or unavailable input is explicit. Legacy text rules
remain unevaluated until an explicit conversion. Normal `add` and report `update` writes
store supported new formula strings in this readable structure, with diagnostics for text
fallbacks. The old tagged trees stay supported; changing only representation or whitespace
does not reopen a judgment. Parsed formulas are cached independently of changing values.
See [structured expressions](../skills/kpopper/EXPRESSIONS.md)
for supported operators, exact fractions, snapshots and checked migration commands.

## What check means

| Outcome | Meaning |
|---|---|
| Failure: fired predicate | A supported `wrong_if` comparison is true on the recorded values. |
| Failure: structural or undeclared gap | Examples include unresolved dependencies, missing dependency snapshots within an inferred snapshot field, undeclared predicate references, or unsupported predicates without an explanation. |
| `MOVED` | A comparable dependency differs from the last-review snapshot. This calls for attention and does not itself fail the check. |
| Movement inside a condition | A changed dependency is named by a predicate that still evaluates false; the movement is muted. |
| `UNKNOWN` | A named condition currently has no result because an input or the Lean core is unavailable, or the operand types differ. This does not mean the judgment holds. |
| `NO_PREDICATE` / `DECLARED` | No executable condition was recorded, with `DECLARED` indicating an explained gap. Reviewing a changed premise can settle its movement alert without inventing a falsifier. |
| `UNCHECKED` after formula conversion | The old snapshot recorded formula text only; an explicit review is needed to capture a calculated result. Historical snapshots are preserved. |
| Declared gap | `blocked_on` explains why a condition cannot currently be checked. This is reported as a note. |
| Human re-opener | `reopened_by` names a sign a person must interpret. It is reported, not mechanically evaluated. |

A record with no inferred snapshot field is reported as lacking a basis for drift detection.
`graph.flagged` and `page.spill` count attention, including unknown results. An unavailable
core can increase them even when the stored record has not changed. A condition on those
counts reports that attention threshold; it does not establish that the original judgments
are false. Conditions waiting on builtin counts do not feed back into those same counts.
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
| `kpop set <id> <value> --why "reason" --as-of YYYY-MM-DD` | Record a scalar reading with its explanation and date. |
| `kpop add <id> field=value ...` | Add an entry or judgment. Judgment snapshots are filled from the record. Where no record resolves for the workspace, the first `add` creates `GROUNDING.yaml` at its root with that entry. |
| `kpop review <id>` | Refresh a judgment's snapshot after reviewing it against the current record. |
| `kpop review "section title"` | Refresh the page section's review snapshot. |
| `kpop same <a> <b>` | Record that two IDs describe one subject; by default, retire `b` into `a`. |
| `kpop distinct <a> <b> "reason"` | Keep a similar-looking pair distinct with a recorded reason. |
| `kpop answer <question> <id> [--why "reason"]` | Close an open question with the entry or judgment that answered it. The question stays in `open:` with what answered it, what that said and the day; `pull` shows it, the opener stops counting it, and `open` and `check` flag it again if the answer later moves or disappears. On a history-backed record the answer's exact version is pinned in history. |
| `kpop answer <question> --dropped "reason"` | Close an open question that no longer matters, keeping the reason. |
| `kpop correct <id> field=value ... [--unset field] [--why "what was wrong"]` | Fix an entry that no commit holds yet (outside Git, one this session wrote) in place. Everything resting on it must be unlanded too; it is flagged for review, never rewritten. A judgment keeps its kind and gets its snapshot taken again. Anything already landed is refused with the hypothesis route, since correcting work others may have read is a decision for a person. |

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
kpop consolidate --dry-run   # Check the proposed combination without applying it
kpop consolidate            # Fold eligible hypotheses through the guarded writer
kpop consolidate --refute NAME "Reason for refutation"
kpop consolidate --from BRANCH_OR_REF --dry-run
kpop consolidate --resolve --dry-run # Preview a stopped Git record merge
kpop consolidate --resolve           # Write a checked resolution; no staging or commit
```

The dry run reports changed premises, fired falsifiers, structural gaps, contested IDs and
possible duplicates. A clean structural check is not enough to fold a hypothesis whose
premises still need review. In an Advanced project, a live dry run with no hypothesis named
also tests the pending shared findings against the record: it is red when accepting one would
break something, adopts none of them, and is left out of a `--frozen` run. Refutation retains a negative finding. `--from` reads another
branch's committed record; it never pushes to that branch.

These checks can run during work; they do not require a pull request or merge. The receiving
record is the base, and `--from` overlays the named ref's committed differences. Importing
`main` into a worktree therefore asks a different question from applying that worktree's
changes to current main. Refs come from local Git objects; the command does not fetch them.

The callable `union_of(base, hypotheses)` also accepts an in-memory hypothesis for a read-only
what-if. That is a building block for previewing a captured working-copy delta on main. The opt-in `watch` runner uses this core for asynchronous worktree-versus-main previews.
The existing background ingestion path continues to process explicit reports for one record.

For a stopped Git merge, `consolidate --resolve` combines independent record changes and
refuses competing claims. It validates the staged candidate before replacing the record.
Compact history also adds an immutable union manifest; stage both paths named by the command.
See [supported layouts and limits](coding-and-ci.md#resolve-a-stopped-git-merge-locally).

See the [record](../skills/record/SKILL.md) and [consolidate](../skills/consolidate/SKILL.md) skills for the full write and consolidation discipline.

## Background branch watch and shared facts

```sh
kpop watch setup                         # local checks, no host schedule
kpop watch setup --base-ref origin/main # preserve a deliberate base
kpop watch scan                          # queue and return immediately
kpop watch status                        # current versions and findings
kpop watch scan --all                    # include registered worktrees
kpop watch pause                         # pause local checks and shared writes
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
the command cannot call host tools itself. Persistent writes require POSIX locking. See the
[watch protocol](../skills/watch/references/compatibility.md) for scope, delivery and recovery.

## Background capture

```sh
kpop update --file report.json        # apply one report now and return its receipt
kpop ingest capture --file report.json
kpop ingest status --event-id EVENT_ID
kpop ingest pending
```

Capture retains the supplied report and normally starts a separate worker. Supported
updates pass through the canonical writer. Raw reports, journals and receipts live in a
private state directory outside the repository, keyed by the canonical record path.
`update` uses the same atomic path synchronously for the one supplied report. Its receipt
includes normalization diagnostics and affected judgments; exit 0 means applied, exit 1
means retained for a decision, and exit 2 means an invalid request. Existing judgments are
not reviewed by applying new readings.
Use an `updates` list for one or many changes already known from the same source; there is
no need to wait for a larger batch. Refused prepared updates retain their actual
`validation_issues`. Captured sources record their purpose in `recorded_for`, so ordinary
evidence ingestion does not create a new `asked` reading occasion that needs its own tab.

Automatic writes require explicit reports in a single file. An `updates` list can combine
existing scalar updates with new grounded facts, rules and judgments in one atomic write.
The primary agent supplies their meaning; the worker preserves citations and checks the
final graph. New entries and judgments require `record_sha256` from the primary's prior
read; they cannot silently adopt changed premises. Existing judgment rewrites,
reader/page-count judgments, pointer records and ambiguous messages
require further handling. Delivery acknowledgment does not approve a change or clear a
failed condition.

Use the [capture guide](../skills/kpopper/INGESTION.md) for report envelopes, state paths,
permissions and host behavior, and [native delivery](../skills/kpopper/DELIVERY.md) for
returning important findings to an originating Codex task on supported hosts.

## Measurement

An entry can name a recipe with `measure: recipe_name`. `.kpopper/measure.yaml` beside
the record maps that name to an argument list. Treat this file as executable configuration
and review it as code.

```sh
kpop remeasure       # Inspect the plan; run nothing
kpop remeasure --run # Execute the declared recipes and check the proposed readings
```

Recipes run without a shell, with a timeout and output limit. Differences are checked as
a hypothesis; measurement does not silently rewrite the record. This can support repeatable
checks of repository facts, or other deliberately configured observations. It is not an
automatic external monitoring service. See [Contributing](../CONTRIBUTING.md) for the
repository's measurement and CI contract.

<a id="page-and-browser-checks"></a>

## kpopper Hub and browser checks

This is an optional experimental application, carried by the installed command;
see [application boundaries and compatibility](applications.md).

```sh
kpop experimental hub --open
kpop experimental hub --open --tree
kpop experimental hub --verify
kpop experimental hub                      # written to .kpopper/build/page.html
kpop experimental hub --checks .kpopper/build/page.html
```

The renderer generates a self-contained HTML snapshot. The deterministic `--verify` checks
need no browser. Interactive browser checks additionally need Node 18+, Chrome or Chromium,
and `playwright-core`, which is not bundled:

```sh
npm i --no-save playwright-core
kpop experimental hub --checks .kpopper/build/page.html
```

The checker looks for the driver beside the page; `NODE_PATH` can point at an existing
installation. Set `CHROME=/path/to/chrome` if the browser is not on a known path. The browser
checks exercise both themes and reduced motion. A real browser is needed to inspect the
interactive page; a preview that strips JavaScript will show only part of its behavior.

See [PAGE.md](../skills/kpopper/PAGE.md) for arrangements, components, reference cards,
localization, coverage and prose-drift checks.

<a id="authored-html-documents"></a>

## Annotated Documents

This is an optional experimental application, carried by the same installed command.

`kpop experimental annotated-doc build` packages an authored document with its selected evidence,
`annotated-doc inspect` validates and reads a saved copy without running its scripts, and
`annotated-doc refresh` prepares a new copy against explicitly supplied sources. The authoring
agent creates anchors and mapping during ordinary document work. The final HTML contains
all display resources and review state; only source refresh needs the agent and inputs.
See [the user flow](documents.md) and run `kpop experimental annotated-doc guide` for the author contract.

## Distribution and implementation

| Part | Source |
|---|---|
| Method and agent guidance | [skills/kpopper](../skills/kpopper/SKILL.md), one skill per occasion beside it: [ground](../skills/ground/SKILL.md), [record](../skills/record/SKILL.md), [map](../skills/map/SKILL.md), [annotated-doc](../skills/annotated-doc/SKILL.md), [hub](../skills/hub/SKILL.md), [consolidate](../skills/consolidate/SKILL.md), [watch](../skills/watch/SKILL.md) |
| The command: reader, checks, writer and applications | [native/src](../native/src), [installation and boundaries](applications.md) |
| Display resources and schemas it embeds | [native/shared](../native/shared) |
| Optional checked-session Lean core | [scripts/session/lean](../scripts/session/lean), [setup guide](checked-sessions.md) |
| Browser verification | [native/shared/verify_page.js](../native/shared/verify_page.js) |
| Agent integration | [hooks](../hooks/hooks.json), [adapters](../adapters/README.md) |

The GitHub release carries one archive per platform, and crates.io the sources to build the
same command. Agent plugins add the method and host-specific hooks. The npm package `kpopper`
provides the Node browser checker alone and keeps its own 1.x version line; the release
archives, the crate and the plugin manifests share one version.

Every command reads the whole record, so a file's parsed form is kept under
`$XDG_STATE_HOME/kpopper/cache`, or `~/.local/state/kpopper/cache`, one private entry per
file, and taken again whenever the file's path, length, last write or content differs. The
files are the authority: an entry that cannot be read, or holds anything but a document, is
simply a parse, and every write drops the entry for the file it wrote. `kpop --no-cache
<command>`, or `KPOPPER_NO_CACHE=1`, parses every time.

The plugin's hooks are the layer every session gets without choosing it, and they carry
pointers, never values. The session opener prints the record's head and what needs a person,
and names the next move as the host invokes a skill (`/kpopper:ground` in Claude Code,
`$ground` in Codex); in a project without a record it says so in two lines. At every prompt
a grounding line names at most three entries whose ids, names or verdicts the prompt's words
touch, with the skill that reads them; an entry is named until it is read, then again only
when its recorded body changes or the session compacts, and an unread one repeats after a
cooldown of ten prompts. Before a file is edited, the entries whose source it is, or whose
reading a recipe takes from it, are said once. Diagnostics compare with the session-start
baseline and are delivered as `UserPromptSubmit` context: new check failures, entries with
no recorded intent, and an unavailable assessment. At most eight new findings are delivered
per prompt, each capped at 500 characters with an explicit pointer to `check` when clipped;
omitted findings remain eligible for the next prompt. The current user request stays active.
No supplied hook blocks Stop, replaces a tool result, requests a continuation, or wakes an
idle conversation. Legacy Stop wrappers are silent so older registrations are harmless.
`check`, direct `gate` commands and write admission retain their failure behavior.

A session that changed files or ran eight prompts with the record untouched also receives
advisory prompt context with a ten-prompt cooldown. Context never requires a bookkeeping
reply or grants write permission. A user can still explicitly ask about a reminder.
Diagnostics use `additionalContext`; host interfaces may expose hook activity.

Session attribution requires a successful-write receipt matching the session, source file
and exact current entry body. Git updates and manual edits alone do not establish authorship.
Writers use `KPOPPER_AGENT_SESSION` from the opening context (or `CODEX_THREAD_ID` in Codex).
Legacy and active-history direct writes retain these receipts after publication; previews,
failed writes and private drafts do not. Missing receipts leave ownership unknown.

Private delivery receipts suppress repeated findings across prompts and resume. Diagnostics
are reassessed before delivery, so a resolved problem is not replayed from a stale queue.
Unavailable delivery storage suppresses optional notifications without changing validation.
Claude and Codex use regular async context for ingestion/watch findings; there is no
`asyncRewake` route. Other adapters expose current state at startup or through explicit
checks, as described in the [capability matrix](../adapters/README.md#capability-matrix).

From a source checkout, build the command with `cargo build --bin kpop` inside `native/`.
When using an installed plugin without a `kpop` command on `PATH`, run `scripts/bin/kpop`
from that plugin's directory. In Claude Code the session opener adds that directory to the
end of `PATH`. Installation paths are versioned; locate the active installation rather
than retaining a path to an older copy.

The method's lasting constraints are traceable grounding, preserving derivation rules,
propagating declared impact without automatically adopting conclusions, and migrations
that keep the checks passing when the record's shape changes.
