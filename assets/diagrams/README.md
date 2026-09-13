# Illustrated process overviews

The PNGs are editorial ink illustrations used by the README. They share the opening
[story series](../stories/README.md)'s typography, palette and visual language while
preserving the process details in the earlier diagrams.

The SVGs are separate, compact **editable schematic references** for inspecting the
relationships. They are not the source files from which the illustrated PNGs were
rendered. Exporting a schematic should not overwrite the illustration.

| Overview | Illustration | Supporting reference | What it explains |
|---|---|---|---|
| Conversation | [Desktop](conversation-flow.png) · [Phone](conversation-flow-mobile.png) | [Desktop](conversation-flow.svg) · [Phone](conversation-flow-mobile.svg) | An explicit report is processed separately while the main conversation continues; important findings return. |
| Across time | [Desktop](work-across-time.png) · [Phone](work-across-time-mobile.png) | [Desktop](work-across-time.svg) · [Phone](work-across-time-mobile.svg) | Past grounds, present attention and configured future work stay connected through explicit recording of outcomes. |
| More deterministic reasoning | [Desktop](reasoning-check.png) · [Phone](reasoning-check-mobile.png) | [Illustrated record](reasoning-check.yaml) | An agent makes the premises and condition explicit; the reader repeatedly evaluates that declared comparison. |

## More deterministic reasoning

The opening illustration has two parts: **Conversations** and **Deterministic
checks**. An earlier session establishes the promise and its reason; a later one
changes retention. An **Agent records** arrow connects the dialogue to the YAML.
The comparison and **Download promise fails** result sit directly below the code,
inside the same panel. The phone layout stacks these two parts.

**More** describes the shift of selected reasoning steps into
explicit, repeatable checks. The agent still interprets sources and chooses the
relevant premises and condition; the ordinary reader evaluates supported comparisons
and follows declared dependencies. YAML holds that structure; the checker executes
the comparison. Arbitrary formulas and prose are not automatically executable.

The [pictured record](reasoning-check.yaml) is a fictional, minimal illustration.
At its earlier review, both the file retention and the promised window were 30
days. The recorded retention has since changed to seven; `seen` retains the earlier
values. Source locators are omitted from this teaching excerpt. The
[complete download example](../../examples/merge-assumptions/README.md#premature-file-deletion-consistency)
includes source files, measurement recipes and the two-branch scenario, with longer
entry names.

From the repository root:

```sh
python3 scripts/kpopper check assets/diagrams/reasoning-check.yaml
```

The expected exit status is **1**, with this failed condition:

```text
FAIL download.promise: wrong_if holds (files.days < link.days) - broken by its own condition
```

The comparison is `7 < 30`. Repeating the check against the same record produces
the same result. An accurate comparison still depends on the supplied readings and
the relevance of the chosen condition. This is a demonstration of repeatable checking,
not an evaluation of overall agent reasoning quality.

The conversation bubbles are fictional teaching dialogue, not transcripts of a
live agent run. The visual names the failed promise directly: a true `wrong_if`
condition means failure, so the ambiguous label "Condition holds" is not used.

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

The time and conversation desktop illustrations are 1536×1024; the compact reasoning
overview is 1672×941. Phone illustrations are 1024×1536 and are selected
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
