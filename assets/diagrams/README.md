# Illustrated process overviews

The PNGs are editorial ink illustrations used by the README. They share the opening
[story series](../stories/README.md)'s typography, palette and visual language while
preserving the process details in the earlier diagrams.

The SVGs are separate, compact **editable schematic references** for inspecting the
relationships. They are not the source files from which the illustrated PNGs were
rendered. Exporting a schematic should not overwrite the illustration.

| Overview | Illustration | Editable schematic | What it explains |
|---|---|---|---|
| Conversation | [Desktop](conversation-flow.png) · [Phone](conversation-flow-mobile.png) | [Desktop](conversation-flow.svg) · [Phone](conversation-flow-mobile.svg) | An explicit report is processed separately while the main conversation continues; important findings return. |
| Across time | [Desktop](work-across-time.png) · [Phone](work-across-time-mobile.png) | [Desktop](work-across-time.svg) · [Phone](work-across-time-mobile.svg) | Past grounds, present attention and configured future work stay connected through explicit recording of outcomes. |

## What the illustrations preserve

The time overview keeps all three perspectives:

- **Past:** sources and evidence, decisions and reasons, and snapshots of inputs at
  the last review.
- **Present:** connected claims, changes and contradictions, and new information.
- **Future:** followups and actions, daily reviews, and time or event triggers when
  configured. The return loop is the agent recording outcomes as evidence; finishing
  a task alone does not rewrite the record.

The conversation overview keeps two visibly parallel tracks. The main agent captures
an explicit source and update, then continues work that does not depend on that result.
The separate worker saves the source, records the specified scalar change and checks
its declared dependencies. An important finding can return through the configured host
route; routine updates stay quiet. The compact SVG groups source retention and the
update in one box, while the illustration shows them as separate numbered steps.
The illustration's headline is **Save in the background. Return when needed.**
A person may appear in the main-session scene; the software worker has no human
avatar, so the diagram does not imply a second person doing the background work.

These are conceptual illustrations, not product screenshots or evidence of automatic
interpretation, a delivery guarantee or unconfigured background monitoring. See
[capture and write limits](../../skills/kpopper/INGESTION.md),
[host delivery](../../skills/kpopper/DELIVERY.md) and
[followups](../../skills/kpopper/FOLLOWUPS.md).

## Layout and maintenance

Desktop illustrations are 1536×1024. Phone illustrations are 1024×1536 and are selected
by the README's `picture` element at viewport widths up to 600 pixels. The time panels
stack on a phone; the conversation keeps parallel lanes. Important wording also remains
in the README text and image descriptions.

Keep the ink scenes expressive, labels readable and connectors clear of text. Preserve
the original concepts and the distinction between recording, checking and deciding.
Blue identifies the principal connections; amber marks attention. Use no arbitrary
calendar dates to imply an actual scheduled run.

The schematic references use Arial with Helvetica and sans-serif fallbacks. Their
smaller canvases and simple shapes are intended for editing the logic. Keep source
and update handling, parallel-process labels, selective return and explicit outcome
recording accurate when either representation changes.
