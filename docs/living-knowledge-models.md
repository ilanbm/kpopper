# Living knowledge models

A **standalone knowledge model** is the product itself: `GROUNDING.yaml` together with its
complete native history closure and the source materials needed to interpret its entries.
It is one versioned artifact, not a separate model accompanied by a second kpopper record.
Use this framing only when the user explicitly identifies a standalone knowledge model as
the product. A filename or folder layout does not establish that intent. Explain it only
when useful and permitted by the user's guidance preference; `kpop config --guidance off`
suppresses optional explanations.

The model records source-grounded facts, rules and judgments. Keep each fact tied to its
source and location; record the rule that produces a value when that rule can be represented,
instead of only storing its output; and give a judgment its grounds, current reviewed readings
and a meaningful condition or event that would reopen it. Preserve unknowns, scope and
unmodeled cases as unknowns. A passing `kpop check` reports on recorded conditions and
structure; it does not establish that the sources are true or that the model is complete.
See the [record shape](../skills/kpopper/references/shape.md), [reasoning core](reasoning-core.md)
and [command reference](reference.md).

## Keep one exact version

Keep `GROUNDING.yaml` and its complete native history closure, including `.kpopper/` history
sidecars, together at the model's established location. Keep in-project source materials
there too when they are part of the versioned evidence. Identify the exact version being read
or changed. In Git, cite the actual commit; do not imply a different branch or location. The
[history contract](history-contract.md) defines the native closure and its validation.
External sources remain references with enough locator detail to find the cited passage; the
record does not fetch them or make their contents immutable.

The model's purpose as the product does not select Simple or Advanced storage mode. Preserve
the user's existing mode and record location. If the user explicitly asks to change either,
use the documented checked transition and read back its result; do not move the model as a
side effect. See [project modes](project-modes.md).

## Keep case inputs outside the reusable model

Keep person-specific inputs, hypothetical thresholds and scenario results out of the reusable
knowledge model. Bind a case in the authorized private case workspace. If a scenario needs a
copy, keep it separate from the canonical model and identify the exact model version it uses.
Do not treat a missing input, unknown component or failed component as an overall conclusion
about the case. Record only reusable findings that have their own source and scope.

## Update only from a source review

A new reading begins with rereading the relevant source and locating the supporting passage.
Use existing authoring commands: `kpop set` for a changed scalar reading, `kpop add` for a
new sourced fact or rule, and `kpop update --file` only for a prepared report supported by
that command's contract. Inspect the write result and run `kpop check` to see declared
movement or conditions. Reconsider affected judgments against the source and current model
before using `kpop review` to refresh their snapshots. A check does not fetch sources, infer
that they changed, or perform that review. See [write and review](reference.md#write-and-review)
and [background capture](reference.md#background-capture).
