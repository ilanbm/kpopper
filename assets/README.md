# Image sources

## Work across time

`work-across-time.png` is a conceptual illustration for the README's Past, Present and
Future overview. It shows sources and earlier reasoning, current dependency checks, and
deferred work. The return arrow describes the agent's separate evidence-recording step;
it does not represent automatic rewriting of the graph. The diagram is not a product
screenshot or a release roadmap, and its document icons contain no research citations.

## Product screenshots

`standalone-document-reasoning.png` is an unaltered browser capture of **The Autumn Garden
Workshop**, an HTML report produced by the standalone document workflow. The document and
the focused explanation for its 16-day registration window appear together. The card marks
that interpretation as **Not checked**, explains its basis in two dates, and links to the
project notes offered as context. It does not present the interpretation as an automatically
verified source fact.

### Earlier record-page captures

`document-hover.png` and `document-drilldown.png` are browser captures of the actual
kpopper renderer's **Now** tab, cropped to the report excerpt. They use the included
[Greenhouse report record](../examples/greenhouse-report/GROUNDING.yaml) and
[document layout](../examples/greenhouse-report/.kpopper/view.yaml). The example adapts
the Greenhouse test data into a short report with one heating judgment.

The first capture shows a hover over `c.boiler_short`. The second follows two dependency
links: `c.boiler_short` → `heat.deficit_kw` → `heat.boiler_kw`. The resulting card shows
the reading and its source identifier. The screenshots preserve the rendered UI; no live
personal project data appears in them.

## Lean logo

`lean-logo.png` is the unmodified white-background PNG from the
[Lean logo downloads](https://lean-lang.org/logos/), including its trademark symbol.
The [original image](https://lean-lang.org/static/png/lean-logo-official-TM-white-2400x900.png)
identifies the Lean programming language and proof assistant used by kpopper's optional
session core.

The Lean name and logo are trademarks of Lean Focused Research Organization (FRO).
Their use follows the [Lean Trademark Policy](https://lean-lang.org/trademark-policy/)
and does not imply endorsement of kpopper by Lean FRO. The logo is not covered by this
repository's MIT license.
