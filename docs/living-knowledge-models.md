# Living knowledge models

In kpopper, a living knowledge model is a structured, versioned record of facts, assumptions,
rules and judgments, with their sources and dependencies. Its checks can expose affected conclusions when recorded
premises change. "Living" describes a model that can be maintained and checked over time;
external sources still need review. It does not mean a trained language model or automatic
source synchronization.

An ordinary project record is also a living knowledge model. This guide covers the other
purpose: **knowledge as the product**, maintained for reuse across questions or applications.
Both purposes use the same kpopper runtime and record format.

## Start with a model

Choose whether you are creating a model or using one supplied by a publisher.
An author needs a native authoring toolchain. A generated model plugin already
contains its engine and domain skill; its users do not install kpopper separately.

### Build a model

For an installable model, start with the [model skill](../skills/model/SKILL.md)
and the [complete packaging example](model-plugins.md#try-the-complete-example).
That guide identifies the prebuilt tools authors need and the current availability
limits. If you are maintaining an unbundled model directly, use the
[ordinary kpopper installation](../README.md#get-started) and explicit model selectors.

1. Choose a bounded subject and its sources. State what the model will answer and what
   remains outside its coverage.
2. Author its facts, rules and judgments using the existing
   [record commands](reference.md#write-and-review), keeping the complete model together.
3. For rules that need case inputs, provide an application-owned adapter or test harness:
   it makes a disposable complete copy, writes synthetic inputs through native commands,
   and runs the [assessment](assessment.md). Compare the results with independently
   expected outcomes. A shape check alone does not establish that a rule is correct.
4. Version the model with its sources, tests and usage instructions. Explain how another
   person or agent supplies inputs and interprets results, including unknowns and scope.
   Read/query plugins use the shared native reader without custom application code;
   case-specific input binding remains an application adapter's responsibility.

### Use an existing model

For a generated model plugin, follow [plugin installation and use](model-plugins.md#use-a-model-plugin).
Select the publisher's version, install the complete plugin, and ask its domain skill
questions. Its bundled native launcher prepares and verifies the model and engine.
Reading such a plugin requires no separate kpopper, Python or compiler installation.

For an unbundled model, obtain its complete record, history, sources and publisher's
setup instructions, including any runtime or adapter dependencies. A copied skill
or YAML excerpt is not an installable model plugin. To evaluate a private case, use
the model's documented adapter or input interface. kpopper supplies no universal
domain case adapter; personal inputs stay out of the reusable model.

The [synthetic model-plugin example](../examples/model-plugin/README.md) can be packaged
with the qualified prebuilt tools. It checks the packaging path, not usefulness at scale.
The [flight-refund example](../examples/living-travel-rights/README.md) shows a sourced
rule and captured baseline/hypothetical results. It is a read-only excerpt from a private
repository, not a public runnable starter. The remaining sections describe any model's
contents, explicit selection, private case data and maintenance.

## Keep the complete model together

The model can live in its own repository or in a subdirectory beside an optional ordinary project record.
For example, an application can keep its domain model at
`knowledge/us-flight-refunds/GROUNDING.yaml` while the project uses the record selected by its
normal kpopper configuration. These are separate records with separate purposes; the model
does not replace or absorb project knowledge.

To create an installable model plugin, follow [Create a reusable model
plugin](model-plugins.md) or use the `model` skill. The native companion packages
the model and its domain skill with a qualified engine. Read/query models need
no custom adapter; applications that bind private case inputs can supply one.

Treat the model as a complete, versioned bundle: its `GROUNDING.yaml`, its own
`.gitattributes` file carrying its byte-preservation rules, every retained path covered by those
rules (including `.kpopper/**`,
`evidence/**`, and `.kpopper-history-migration/**` when present), and any in-project source
materials it references. Preserve relative paths and bytes, and identify the exact commit being
read or changed. Keep the application's adapter and any person-specific case binding outside
the canonical model. Documented transient state such as `.kpopper/project.lock` and
`.kpopper/.history-local/` does not belong in the portable bundle.

The native history closure is only part of that bundle. Preserve that attributes file, all
covered paths and in-project source materials referenced as evidence. External
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

## Select project and model explicitly

Use the explicit record path for commands that accept one. For example, a project-scoped pull
can name both its workspace and the model record:
`kpop --workspace /path/to/project pull ID /path/to/model/GROUNDING.yaml`.
Other native commands have command-specific target syntax; check their help instead of assuming
that `--record` applies everywhere. Bind ordinary project commands to the application's project
workspace with `--workspace /path/to/project`; do not rely on the current directory to
distinguish the two.
The model folder itself remains a native authoring context: bare commands run there can select
and change the model. A folder boundary does not make it read-only, and native discovery is not
a package registry or an asset-role security boundary.

The project record is optional. When no project record exists, native project discovery can
report absence; do not create a blank placeholder or copy the domain model into the project
record just to make it open. An ordinary configured project record may be external, so a root
`GROUNDING.yaml` must not be assumed to override configuration. These patterns
use existing explicit APIs, not a new storage mode or automatic migration.
The optional [model-plugin companion](model-plugins.md) adds a versioned packaging
and installation contract around them. See [project modes](project-modes.md) and
[node-history storage](node-history-storage.md).

## Keep the product and its storage choice distinct

The model's purpose as the product does not select or change storage mode. Resolve the record
with `kpop where` and inspect `kpop config --json`; do not infer its location from a mode label
or checkout. If the resolved record is external, ordinary storage guidance applies and this
Git-bundle description does not cover it. If the user asks to co-locate or change a record,
use a documented supported command for the actual layout; if none produces the requested
layout, stop and report that limit. The guide triggers no automatic configuration, copying or
migration. See [project modes](project-modes.md) and [node-history storage](node-history-storage.md).

## Keep case inputs in a disposable copy

For a person-specific case or hypothetical, if your application has a case adapter, use it to
make a complete disposable copy of the model bundle, identified by its source commit, and keep
case values and any case-specific native authoring in that private copy. The case binding belongs
to the adapter's ephemeral workspace, not the canonical model or project record. kpopper has no generic
private-case binding service. If no case adapter exists, keep personal values out of the
canonical model; do not write them with `add`, `set`, or `--hypothesis`. The latter writes
retained proposal history, not a temporary case. Do not substitute a blank record that merely
cites the model commit. For supported record layouts, `kpop history migrate --to` creates a
verified compact conversion; see [node-history storage](node-history-storage.md). It is not a
general whole-bundle copy. `kpop knowledge materialize` copies one contribution into a frozen
snapshot, not a complete model ([native command reference](../native/README.md)).

Keep reusable findings separate from person-specific facts. Unknown inputs and failed
components stay visible as such; they do not establish a conclusion about the case.

## Export only application-selected contents

When an application ships a portable bundle, its exporter must select the model closure and
product files from the exact committed tree and verify their committed bytes. A project record,
its history and project-only evidence are not part of the model bundle. The application defines
the portable contents. For native model plugins, use the shared
[model-plugin builder](model-plugins.md) to validate and package that selected
closure; a YAML excerpt is not an installable model.

## Update only from a source review

Model maintenance must explicitly target the model record; ordinary project recording does not
authorize a model change. A changed reading starts with rereading the relevant source and locating
the passage. For a scalar, name the model file as well as its date and citation:

```sh
kpop --workspace /path/to/project set ID VALUE /path/to/model/GROUNDING.yaml \
  --as-of DATE --source SOURCE_ID --at LOCATION
```

`--source` and `--at` are supplied together. `add` also accepts a trailing model-file path
for a new sourced fact or rule. Other operations have command-specific selectors:
`update --file REPORT --record /path/to/model/GROUNDING.yaml` applies a prepared report
under its documented contract. Do not assume selecting a workspace overrides project
configuration; verify the target and use the selector supported by each command.
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

## Package a reusable model plugin

When people need to install and query a model across projects, keep its native record and complete source/history closure distinct from any project record. Use the [model-plugin authoring guide](model-plugins.md) for the versioned package descriptor, standard read skill, native authoring sequence and prebuilt package builder. A descriptor plus GROUNDING.yaml is still source material, not an installable plugin; the built package must bind its exact model closure, skill and complete engine. Case-specific behavior remains a separately declared application adapter, and personal case values stay outside the canonical model.
