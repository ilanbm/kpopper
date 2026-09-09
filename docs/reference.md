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

Save [the launch-party example](../examples/launch-party/PROVENANCE.yaml) as `PROVENANCE.yaml` in
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

`PROVENANCE.yaml` lives in the project's working directory. Git is optional. The file can
also point to existing material, including multiple record files:

```yaml
record: analysis/facts.yaml
also: analysis/claims.yaml
```

For a Git repository that keeps its record outside the working tree, register the absolute
path in the Git common directory:

```sh
printf '%s\n' '/absolute/path/to/PROVENANCE.yaml' > "$(git rev-parse --git-common-dir)/kpopper-record"
kpopper where
```

This registration is shared by the repository's worktrees. It does not add the record to
Git. Use `where` to confirm what a directory resolves before writing to it.

The recommended fields are `from`, `rests_on`, `wrong_if` and `seen`. The reader infers
dependency, predicate and snapshot roles by shape, so existing vocabularies can work.
Ambiguous roles require an explicit `schema` declaration; the reader refuses to guess.
The [method's record examples](../skills/kpopper/SKILL.md#the-shape) cover the full shape.

Source locations and dates preserve traceability. They do not cause the reader to fetch
documents, query calendars or inspect every linked file. The person or agent records the
relevant reading using the tools and permissions available to that session.

Derived entries store their rules:

```yaml
known:
  workshop.spare_packs:
    rule: "stock.packages - workshop.guests"
```

Here the two inputs refer to the [workshop example](../examples/workshop/PROVENANCE.yaml).
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
| `kpopper add <id> field=value ...` | Add an entry or judgment. Judgment snapshots are filled from the record. |
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

A competing claim can live in `PROVENANCE.d/<name>.yaml` beside the base record. The writer
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

See [the method](../skills/kpopper/SKILL.md) for the full write and consolidation discipline.

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

An entry can name a recipe with `measure: recipe_name`. `PROVENANCE.measure.yaml` beside
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
kpopper page --out record.html
kpopper page --checks record.html
```

The renderer generates a self-contained HTML snapshot. The deterministic `--verify` checks
need no browser. Interactive browser checks additionally need Node 18+, Chrome or Chromium,
and `playwright-core`, which is not bundled:

```sh
npm i --no-save playwright-core
kpopper page --checks record.html
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
| Method and agent guidance | [skills/kpopper](../skills/kpopper/SKILL.md) |
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

The plugin's session opener is quiet in projects without a record. Its stop gate compares
with a session-start baseline and reminds once about new failures. An unchanged judgment
whose falsifier fires after a new scalar reading can remain flagged while recording
finishes; `check` still reports it. Adapters differ in their ability to block, remind or
deliver asynchronously—consult the [capability matrix](../adapters/README.md#capability-matrix).

From a source checkout, run `python3 scripts/cli.py <command>` or `scripts/kpopper <command>`.
When using an installed plugin without a `kpopper` command on `PATH`, use the same dispatcher
under that plugin's `scripts/` directory. Installation paths are versioned; locate the
active installation rather than retaining a path to an older copy.

The method's lasting constraints are traceable grounding, preserving derivation rules,
propagating declared impact without automatically adopting conclusions, and migrations
that keep the checks passing when the record's shape changes.
