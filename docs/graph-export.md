# Export a focused knowledge excerpt

`kpopper export` prints selected entries and their recorded relationships. A judgment
shows its historical dependency readings as `at_review`, current selected readings as
`current`, and the ordinary reader's condition result. These are output labels;
`GROUNDING.yaml` and custom snapshot field names are unchanged.

```sh
kpopper export d.choice
kpopper export d.choice --depth 0
kpopper export d.choice --details
kpopper export d.choice --format markdown-mermaid
kpopper export doc.quote --direction impact --depth 2
```

Use `--record path/to/GROUNDING.yaml` for an explicit record; it can be repeated for
multiple inputs. Otherwise normal workspace discovery applies. From a source checkout,
replace `kpopper` with `python3 scripts/cli.py`.

The default is Markdown. `markdown-mermaid` appends an optional diagram to the same text;
`mermaid` emits diagram source only. A destination must support Mermaid to render it.
The diagram includes a visible legend for states present in the selection. Historical
readings and detailed condition results remain in the text output.

## Read the comparison

A dependency table distinguishes:

- A historical value from a current recorded or calculated reading.
- A current body outside the excerpt from a reference missing from the base record.
- An absent historical snapshot from a recorded null, false, zero or empty string.
- A changed value inside a false declared condition from a change requiring review.
- A recorded source/rule reading from a comparison the ordinary reader actually performs.

Comparisons and conditions are evaluated against the full base record before selection.
Omitting a current value does not hide whether it changed, including a change within a
false condition that requires no review. A true condition stays true even if the excerpt
omits its current inputs, or another dependency is missing. A missing current value is
not replaced by its historical value. The exporter
does not refresh snapshots, fetch external sources or run page-only checks.

When a condition reads an existing entry outside the judgment's declared dependencies,
the text names that gap alongside the condition result. Such a result does not establish
that the dependency links are complete. Use `kpopper check` for record diagnostics.

Structured rules retain their input links in both traversal directions. Supported rules and
conditions are displayed as readable expressions; calculated values are labeled separately
from stored readings. Historical computed values remain historical, and a changed formula can
require review even when its result is unchanged. A legacy snapshot containing only a formula
does not supply a historical numeric result. Missing Lean or an unavailable calculation stays
unknown in the excerpt; it does not prevent exporting the other recorded context.

The reader's flags are retained. `MOVED` identifies a changed premise requiring review;
`FALSIFIED` means a declared condition is true on recorded values. They can coexist.
`CONTESTED` compares readable hypotheses and does not establish the absence of every
possible alternative. Hypotheses are not expanded, and unreadable hypotheses are listed.
These checks do not establish source-world truth.

`--details` adds original recorded fields, labeling the snapshot field as historical.
It is available with text formats. To obtain an omitted current entry, use
`kpopper pull ID` or select it in another export.

## Selection and limits

Supply 1–8 exact entry IDs. Entry and dependency IDs must be strings; quote numeric-looking
IDs in YAML. Unsupported IDs produce a diagnostic rather than a partial export.
A selected judgment with a text dependency field is rejected rather than displaying its
characters as missing entries; dependencies must be written as a list.
Depth is 0–4 (default 1); `--max-nodes` is 1–32
(default 12), including seeds. Selection follows recorded links breadth first and handles
cycles once. `support` follows outgoing links; `impact` traverses them backwards while
keeping arrows in their recorded direction.

The output reports nodes outside the selection, links crossing the boundary and internal
links omitted by the 96-edge cap. Dependency tables show up to 12 rows per judgment;
additional rows are explicitly counted. Text fields are clipped at 600 characters,
table readings at 120, and diagram labels at 80. Clipping is marked; the original record
retains the full content. Missing references can occupy a node slot. Metadata is not a
selectable entry.

Links distinguish `rests_on`, `from`, `rule_reads` and historical `refutes`. A rule link
records which entries its expression names; it does not evaluate an arbitrary formula. A
`refutes` link is historical evidence, not a new condition result.

Output goes to stdout. Redirect it to a separate file, leaving the source record intact.
The CLI's `--json` option wraps the text and exit status; it is not a new state or storage
schema. Repeated exports of unchanged input are deterministic.
