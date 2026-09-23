# Image sources

## README hero

`kpopper-canvas-hero.png` opens the README with the robot at its reasoning canvas,
the original lowercase kpopper wordmark and "The canvas your AI didn't know it needed."
The 1774 × 1000 composition reuses the scene and caption from
`canvas-for-reasoning-v2.png` and the wordmark from `kpopper-hero.png`.
The original illustration, blue connections and `GROUNDING.yaml` label are preserved.
`kpopper-canvas-hero-mobile.png` stacks the wordmark and caption below the same scene
at 1536 × 1152 so the main message remains readable at phone width.

## Popper banner

`kpopper-hero.png` appears before the README's **Make it earn its place** invitation,
with its curiosity link beneath it. It keeps the large ink portrait on the left,
a prominent wordmark and one-line quotation on the right, and a separate italic
attribution beneath the quotation, beginning with a hyphen and space. Popper looks inward toward the wording with a
restrained, faintly amused expression; a small Korean speech bubble accompanies the portrait.
The Winamp-inspired slogan is presented as a
fictional humorous quotation, with logic symbols around *lemma*. It is not a
historical quotation or photograph.

The facial reference is the historical portrait reproduced in
[LSE's account of Popper and its philosophy department](https://blogs.lse.ac.uk/lsehistory/2019/03/20/conjectures-and-refutations-karl-popper-and-the-growth-of-lse-philosophy/).
The performer pose, clothing and speech bubble are fictional illustration.

See [the visual identity](brand-guide.md).

## Responsive illustrations

Illustrations with desktop and phone layouts use a `picture` element in the README
and example guides. A `(max-width: 600px)` source selects the phone image; the desktop
`img` remains the fallback. Direct **Phone layout** links stay available below them.
Relative paths keep each checkout's assets together. GitHub browser source selection
is supported; the native iPhone app still needs separate verification.

## Compact icon

`kpopper-icon.svg` is the selected white geometric k on electric blue, supplied
with a 512-pixel `kpopper-icon.png` render. The letterform is made from paths and
does not depend on an installed font. No external organization avatar or favicon
has been changed by adding these files.

## Illustrated stories

The story series lives in `stories/`: two merge cases, a cooking-workshop case in
Claude Cowork and a research synthesis from six dark-matter papers. The README links
to the full download merge case rather than repeating the opening overview's scenario.
Each has a desktop and phone layout. The illustrations use bold
typography and expressive ink scenes; the accompanying examples distinguish a failed
condition from a changed premise that needs judgment, and paper findings from an
agent's synthesis. See [the story index](stories/README.md).

The cache, workshop and research examples each include a record close-up generated
from their complete YAML. Selected entries remain readable while a minimap shows the
actual surrounding file. The research record retains source versions, shared data,
model assumptions, a density calculation and open questions alongside its synthesis.

Explanatory illustrations begin with their panels or process. Omit titles that
repeat the README heading, and place a useful takeaway after the process. Retain
panel labels, data and existing conclusions needed to understand the illustration.

## Process diagrams

The reasoning, conversation and time overviews live in `diagrams/`, with illustrated
desktop and phone PNGs in the story series' ink style. The opening reasoning overview
shows natural-language statements, their explicit YAML relationship and a repeatable
comparison; its exact record is supplied alongside it. Separate editable SVG schematics
preserve compact maps of the conversation and time processes; they are not the
illustration sources.

The current opening overview is `diagrams/reasoning-check-v2.png`, with
`diagrams/reasoning-check-mobile-v2.png` for phones. **Meet GROUNDING.yaml** and the
handwritten **your new friend!** note introduce the file open in a large code editor.
The file is the central object through which the story is told. Earlier and later
session annotations connect the agent's recording actions to the saved verdict and
the changed seven-day reading. The original 30/30 snapshot remains visible.

On desktop, the annotations sit to the left in chronological order. Short captions
say that the agent saves the reason, then updates the record; blue arrows enter the
relevant lines. A restrained teal connector follows the condition down a blank
gutter to the local check, with pink reserved for failure. Natural action verbs and
direction distinguish the operations without read/write badges or an instructional
legend. On phones, matching editor
excerpts labelled as a continuation of the same file preserve the code and reading
order without shrinking the whole desktop image.

The illustration shows the relevant YAML fields and a short diagnostic in an
illustrated editor, rather than a product screenshot. The full values, conclusion, condition and
30/30 review snapshot are in
[`diagrams/reasoning-check.yaml`](diagrams/reasoning-check.yaml).
`FAIL downloads.availability` is a fragment of that example's actual check output;
exit 1 is expected because seven days cannot support its 30-day promise. The agent
chooses whether to change the email or policy. Updating the draft does not deploy it.

The earlier `reasoning-check.png` and phone layout remain available alongside their
[run evidence](diagrams/reasoning-check-run.json). Their 0.24-second badge describes
that dated local sample, not a general latency guarantee. The current illustration
does not carry a timing badge.

## Dependency-check illustration

`diagrams/dependency-checks.png` shows a changed recorded assumption, deterministic
checks over its declared dependencies, and the affected decisions returned to an
agent for review. Amber marks a need for review; it does not assert that the
decisions are false. The graph is schematic. The prominent comparison reads:
"A check that takes an agent 10 seconds runs locally in 4.5 milliseconds."
It describes a possible comparison, not measured benchmark results.
The matching SVG is its editable source. `diagrams/dependency-checks-mobile.png`
and its SVG reflow the same content for phones.

## Reasoning canvas

`canvas-for-reasoning-v2.png` shows an ink-drawn software artist working at a
drafting surface labelled `GROUNDING.yaml`. Evidence and assumptions feed a decision;
the decision connects to what would change it. The agent authors the relationships.
The caption is "The canvas your AI didn't know it needed." It supplies the artwork
for the branded README hero described above.
The earlier PNG and SVG remain as the previous concept, not the source of the new art.

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
Workshop**, an HTML report illustrating the optional, experimental Annotated Documents
application. The document and
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

`lean-logo-readme.png` is a proportionally resized 220-pixel-wide copy for ordinary
Markdown rendering. The original mark, whitespace and trademark symbol are retained.
