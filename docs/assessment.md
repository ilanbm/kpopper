# Recorded assessment contract

`kpopper assess` returns recorded findings as JSON so tools can use them without interpreting
prose. The ordinary reader's flags and `check` condition results consume the shared assessment;
the focused exporter projects the same findings before limiting displayed values. Reading an
assessment does not update the record, apply proposals, or refresh review snapshots.

```sh
kpopper assess launch.announcement --record examples/launch-party/GROUNDING.yaml
kpopper assess launch.announcement --attention-only \
  --record examples/launch-party/GROUNDING.yaml
kpopper assess launch.announcement --policy falsifiers-only/v1 --attention-only \
  --record examples/launch-party/GROUNDING.yaml
```

Replace `kpopper` with `python3 scripts/cli.py` in a source checkout. Exact IDs select the
returned entries; assessment uses the full supplied base record. `--record` can be repeated.
`--json` is accepted but unnecessary: this command returns JSON directly. Exit 0 means the
assessment was read successfully, including when a condition holds or attention is present.
Use `check` when its validation exit status is needed.

## Findings

The [JSON Schema](../scripts/assessment.schema.json) ships with the package. `schema_version: 1`
identifies the shape. Each returned node preserves `body` alongside computed `state` and derived
`attention`. A `state` field inside the authored body cannot replace the computed state.

| Dimension | Meaning |
|---|---|
| `basis` | Each declared dependency's current reading, historical snapshot, comparison, reasons and formula change. |
| `falsifier` | The declared expression, actual reads for supported syntax, and whether the condition holds. |
| `contention` | Competing readable hypotheses, their witness claims and available alternatives. |
| `integrity` | The checks actually performed and their structural findings. |

A reading has `status: recorded` with a value (including explicit null), `missing` without a
value, or `unavailable` with a reason. An unavailable reading is not a measured null or false.
A computed historical snapshot retains its original value/rule envelope in `at_review.value`;
`historical_calculation` exposes the captured result and formula for consumers. Formula-only
history does not invent a past numeric result. `rule_changed` is separate from a value comparison:
a formula can change while its value stays equal or cannot be computed.
Other snapshot annotations stay in the original `at_review.value` and source body.

`basis.status: assessed` can contain unknown comparisons. A malformed dependency list gives a
basis error while independently evaluable conditions remain visible. A non-judgment has
`not_applicable` basis/falsifier dimensions. A condition can be `holds`, `does_not_hold`,
`not_declared`, `unknown`, or `error`; unsupported syntax has `reads: null`, rather than claiming
that it reads nothing. Human re-openers remain declarations in the body and are not evaluated.

Integrity currently checks dependency and snapshot shape, missing dependencies/snapshots,
malformed predicate types and undeclared condition inputs. `checks` names this coverage.
Non-judgment integrity checks are marked `unassessed`; this contract does not stand in for the
complete `check` command or page verification. Source locators and external source truth are
not verified by assessment.

Contention compares readable hypotheses with one another, following the ordinary reader.
One proposal different from the base is an alternative, not automatically contested. The
scope lists checked and skipped hypotheses. `none_detected` means none detected within that
scope and method; unreadable hypotheses remain disclosed even when other conflicts are found.

In Advanced mode, `scope.knowledge` separately names checked pending contributions, conflict
holders and the locally observed target revision. Complete source, schema and scope differences
can create a knowledge conflict even when scalar claims agree; these findings identify their
method as `readable_hypothesis_pairs_and_knowledge_identity`. A configured target that cannot
be read makes coverage partial and carries a reason. An unassessed target is not verified.
The scope reports the selected live or frozen read mode. Simple aliases resolve to the same
shared record, while frozen reads use only their explicitly selected record files.

## Attention is a separate policy

`focused-review/v1` derives actions from the findings:

- A condition that holds, a changed formula, or a changed premise not covered by a false
  condition produces a `review` reason. Reasons for one review are grouped together.
- Malformed dependency/snapshot/predicate fields or undeclared predicate dependencies produce
  `repair_record` reasons.
- An unsupported declared condition also produces a repair reason. Its lexical mentions can
  help locate the issue, but they are not reported as a complete set of executable reads.
- Missing declared inputs produce `resolve_gap` reasons.
- Detected competing hypotheses produce `inspect_alternatives`.

An unknown condition alone produces no action. A human declaration is not treated as an
observed event. An action names information to consider; it is neither a task-execution block
nor authorization to rewrite a decision. A task unrelated to an affected entry can continue.

`falsifiers-only/v1` selects only conditions that hold. Switching policy changes attention,
not the findings or assessment identity. Applications can also select actions from an existing
report without another record read or evaluator call:

```python
from kpopper.assessment import load, selected_attention

report = load(["GROUNDING.yaml"])
relevant = selected_attention(report, ids=["launch.announcement"], actions=["review"])
```

`--attention-only` applies selection to the requested IDs while preserving scope and revision
metadata. An empty result is empty attention under that policy and selection, not proof that the
whole record or project is correct.
Both full and attention-only reports can be filtered again; IDs outside the report's selection
cannot be introduced by that filtering. Header metadata is not an assessable entry.

## Semantics and revisions

`assessment_profile: ordinary-reader/v1` identifies the existing ordinary evaluator. It retains
its scalar normalization and uses the packaged expression core for structured calculations.
Source read stamps and judgment verdicts can be displayed as readings but remain uncomparable
under this profile where the ordinary comparator skips them. When a comparison is unavailable,
the contract gives a reason instead of labeling it unchanged. This legacy profile continues to use
its existing arithmetic engine. The separate [experimental core profile](reasoning-core.md) has a
versioned result envelope and explicit resource limits; selecting it does not migrate old records.

Existing flags remain a compatibility policy, including their historical lexical coverage of
text predicates. The focused attention policy uses parsed reads, so an identifier inside a
quoted literal does not silence a changed dependency. Neither policy replaces the other silently.
The optional checked-session core still has its own typed comparison and review semantics; this
legacy profile does not claim those semantics have been unified with the ordinary profile.

`record_revision` binds the supplied record and hypothesis contents, complete typed live
conflict bodies, and the observed target revision or its unavailability. Export snapshot identity
uses the same record revision; target citation or scope changes cannot leave it unchanged.
Conflict bodies outside an export selection are bound without displaying their values. `assessment_revision` also
binds the profile, scope and findings, including unavailable computations. This can change when
an evaluator becomes available without any record edit. Policy selection and display clipping
do not change either identity. These revisions are not `record_sha256` byte hashes used to
protect authoring, and are not checked-session revision handles.

Existing `seen` and custom snapshot field names are retained. The contract adds no stored
`state`, timestamps, migration requirement, or fields an agent must fill in by hand.
