# Living knowledge models

This guide describes a **standalone knowledge model already kept in Git** and identified by
its owner as the product. The product is one versioned knowledge model, not a separate model
with a second kpopper record. Its Git bundle consists of `GROUNDING.yaml`, the generated
`.gitattributes`, every extant path covered by its rules (including `.kpopper/**`,
`evidence/**`, and `.kpopper-history-migration/**` when present), and any in-project source
materials it references. A fresh model need not have the migration directory. Keep the bundle
at its established Git location and identify the exact commit being read or changed.

The native history closure is only part of that bundle. Preserve the generated attributes
file, all covered paths and in-project source materials referenced as evidence. External
sources remain locators to passages; their contents are not made immutable by the model.
See [node-history storage](node-history-storage.md) for supported file layouts and
[history evidence](history-contract.md) for the narrower native closure definition. Optional
explanations respect `kpop config --guidance off`.

The model records source-grounded facts, rules and judgments. Keep each fact tied to its
source and location; record the rule that produces a value when that rule can be represented,
instead of only storing its output; and give a judgment its grounds, current reviewed readings
and a meaningful condition or event that would reopen it. Preserve unknowns, scope and
unmodeled cases as unknowns. A passing `kpop check` reports on recorded conditions and
structure; it does not establish that the sources are true or that the model is complete.
See the [record shape](../skills/kpopper/references/shape.md), [reasoning core](reasoning-core.md)
and [command reference](reference.md).

## Keep the product and its storage choice distinct

The model's purpose as the product does not select Simple or Advanced storage mode. Preserve
the user's existing mode and record location. If the user explicitly asks to change either,
use the documented checked transition and read back its result; do not move the model as a
side effect. See [project modes](project-modes.md).

## Keep case inputs in a disposable copy

For a person-specific case or hypothetical, if your application has a case adapter, use it to
make a complete disposable copy of the model bundle, identified by its source commit, and keep
case values and any case-specific native authoring in that private copy. kpopper has no generic
private-case binding service. If no case adapter exists, keep personal values out of the
canonical model; do not write them with `add`, `set`, or `--hypothesis`. The latter writes
retained proposal history, not a temporary case. Do not substitute a blank record that merely
cites the model commit. For supported record layouts, `kpop history migrate --to` creates a
verified compact conversion; see [node-history storage](node-history-storage.md). It is not a
general whole-bundle copy. `kpop knowledge materialize` copies one contribution into a frozen
snapshot, not a complete model ([native command reference](../native/README.md)).

Keep reusable findings separate from person-specific facts. Unknown inputs and failed
components stay visible as such; they do not establish a conclusion about the case.

## Update only from a source review

A changed reading starts with rereading the relevant source and locating the passage. For a
scalar, `kpop set ID VALUE --as-of DATE --source SOURCE_ID --at LOCATION` records its own date
and citation; `--source` and `--at` are supplied together. Use `kpop add` for a new sourced
fact or rule. `kpop update --file` applies a prepared report under its documented contract.
Inspect the write result and run `kpop check` for declared movement and conditions; it does
not fetch sources or decide whether an interpretation is true.

If new evidence overturns a judgment, do not run `kpop review` on its old verdict to clear
movement. Preserve it and prepare a proposed replacement with its own source, location and
date. A contradictory `add` can be recorded in a named `--hypothesis`. `kpop consolidate --dry-run`
tests the proposal; a judgment replacement folds only under the documented consolidation
rules, including an explicit `--take ID` when its existing `wrong_if` has not fired. For a
proposed judgment, `kpop add` supports its own `--as-of DATE --source SOURCE_ID --at LOCATION`
provenance. In history-backed records, if the accepted version has a recorded author, its
replacement remains reserved until a different recorded actor reviews that exact version and
current inputs. `--by` records provenance, not authenticated identity. When author provenance
is missing, the record flags `review_provenance_missing`; an exact review acknowledges that gap
without backfilling the author or proving independence. `review` records an assessment and does
not establish truth. See [consolidation](../skills/consolidate/SKILL.md),
[write and review](reference.md#write-and-review), and [judgment review semantics](history-contract.md#reviewing-a-changed-judgment).
