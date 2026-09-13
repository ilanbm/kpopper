# Image sources

## Brand hero

`kpopper-hero.png` is the current README hero: a large ink portrait on the left,
a prominent wordmark and one-line quotation on the right, and a separate italic
attribution beneath the quotation. Popper looks inward toward the wording with a
restrained, faintly amused expression; a small Korean speech bubble accompanies the portrait.
The Winamp-inspired slogan is presented as a
fictional humorous quotation, with logic symbols around *lemma*. It is not a
historical quotation or photograph.

The facial reference is the historical portrait reproduced in
[LSE's account of Popper and its philosophy department](https://blogs.lse.ac.uk/lsehistory/2019/03/20/conjectures-and-refutations-karl-popper-and-the-growth-of-lse-philosophy/).
The performer pose, clothing and speech bubble are fictional illustration.

See [the visual identity](brand-guide.md).

## Compact icon

`kpopper-icon.svg` is the selected white geometric k on electric blue, supplied
with a 512-pixel `kpopper-icon.png` render. The letterform is made from paths and
does not depend on an installed font. No external organization avatar or favicon
has been changed by adding these files.

## Illustrated stories

The opening series lives in `stories/`: two merge cases followed by a cooking-workshop
case in Claude Cowork and a research synthesis from three dark-matter papers.
Each has a desktop and phone layout. The illustrations use bold
typography and expressive ink scenes; the accompanying examples distinguish a failed
condition from a changed premise that needs judgment, and paper findings from an
agent's synthesis. See [the story index](stories/README.md).

## Process diagrams

The conversation and time overviews live in `diagrams/`, with desktop and phone
PNG illustrations and separate editable SVG schematics. The schematics describe
relationships; they are not the source files for the illustrations.

- [Background capture](../README.md#keep-the-conversation-moving): recording a report
  while other work continues, with selective return of important findings.
- [Past, present and future](../README.md#past-present-future): sources, review state
  and configured followups connected through the record.

## Knowledge-source illustration

`knowledge-sources.png` keeps the detailed, playful source-map treatment: crowded
groups of familiar tools and materials on the left, and an ordered reasoning layer
on the right. It uses the series's black ink linework, electric-blue emphasis and
bold editorial typography while retaining the five source categories and their
many individual objects. The reasoning record is labelled `GROUNDING.yaml`.

The agent selects relevant evidence; the source files and tools remain in place.
This is a conceptual illustration, not a screenshot or a claim to automatically
organize every source.

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
