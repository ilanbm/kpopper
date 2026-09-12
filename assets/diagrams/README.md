# README diagrams

The SVG files are the editable sources. Their PNG renders are displayed in the
README so layout does not depend on fonts installed on a reader's device.

The knowledge-source section uses the [detailed source-map illustration](../knowledge-sources.png).

| Diagram | Desktop | Mobile | What it explains |
|---|---|---|---|
| Two PRs | [SVG](ci-merge.svg) · [PNG](ci-merge.png) | [SVG](ci-merge-mobile.svg) · [PNG](ci-merge-mobile.png) | A common base, separate passing records, B's recorded reason and the dependency broken by a clean merge. |
| Conversation | [SVG](conversation-flow.svg) · [PNG](conversation-flow.png) | [SVG](conversation-flow-mobile.svg) · [PNG](conversation-flow-mobile.png) | The main session continues while a separate background process records and checks the update; only important findings return. |
| Across time | [SVG](work-across-time.svg) · [PNG](work-across-time.png) | [SVG](work-across-time-mobile.svg) · [PNG](work-across-time-mobile.png) | Sources, reasons and review snapshots; current connections and changes; followups, daily reviews and triggers. |

## Shared design

- Desktop canvases are 1040 units wide; mobile canvases are 760 units wide, with
  more vertical space. The time diagram changes from columns to stacked panels.
  The README selects mobile PNGs at viewport widths up to 600 pixels using `picture`.
- Arial is the reference typeface, with Helvetica and sans-serif fallbacks in the
  editable source. Text uses a deliberate type hierarchy, with compact eyebrow
  labels, larger section titles and emphasized values.
- Electric blue `#0666ff`, near-black `#101828`, white, pale blue surfaces and thin borders.
- Green status labels mean a passing check; red means a failed condition; amber
  marks attention or review. Labels and symbols accompany colors.
- Thin connectors with small filled arrowheads. `markerUnits="userSpaceOnUse"`
  keeps arrowheads independent of line thickness. Join dots clarify branch merges;
  dashed paths distinguish handoffs and the agent's return step. Paths avoid text.

The diagrams are conceptual illustrations of the documented behavior, not product
screenshots or empirical performance results. The source letters, readings and
assumptions still need an agent or authorized tool to record them. Adjacent README
text states the runtime and interpretation limits.

Render the SVG with Arial available when refreshing the PNG. After editing, check
both layouts against the documented behavior, inspect the normal display size and
a 368-pixel-wide mobile render, and check that arrows do not cross text. Preserve
the semantic content in both layouts: compact presentation must not hide process
boundaries, reasons, review snapshots or return conditions.
