# HTML documents with evidence

Ask the agent for the document you want, for example: “Create an HTML project update
from these notes” or “Make an HTML report from these results.” With kpopper active,
the authoring workflow creates the content, design and source/check mapping together.
You do not need to prepare a mapping or ask for a second provenance operation.

The result is one HTML file containing the authored document, styles, scripts, required
assets, selected evidence snapshots and review state. Open it in a browser with
JavaScript enabled. It needs no server, network connection or neighboring files to
show the document, inspect its evidence or choose a prepared update.

A marked passage opens a floating explanation beside the pointer; hover gives a preview,
and clicking keeps it open. The card explains the result in ordinary language, with
technical identifiers, locations and raw check details collapsed by default. Source
links open a source view in the same panel, with Back to the preceding explanation.
Only one view appears at a time. Internal source navigation and Back keep the same
outer frame; longer content scrolls inside it. Click outside to close it; the close control remains
visible while its content scrolls. The Evidence button opens the full document overview. A match means the displayed value or exact
quotation agrees with a captured reading, or that a supported calculation agrees at the
stated precision. Missing sources remain unavailable. Interpretations remain unchecked,
even when their inputs are available. Unmarked text is not checked; the layer reports
unmarked passages and possible unmarked values without claiming semantic completeness.
Citation links can open original sources, but those originals are not fetched on opening.

## Updating a document

Give the agent the saved HTML and the new source material. It rereads the selected
sources and prepares a new copy with precise before/after proposals. Values that share
inputs are reviewed together, so a changed denominator and its percentage are one choice.
Other sources retain their older snapshot and are labeled as not reread. A changed
source can flag an interpretation for review without pretending to verify or rewrite it.

Accept or reject a proposed group, then use Save document copy to save the decision.
This control is in the overview and correction views, not inside a source reading.
It saves the HTML and its current evidence/choices; it does not certify the whole document. The
saved file retains its chosen content, evidence and decision when reopened. Reloading
the original file discards unsaved choices. Acceptance changes the copy only: it does
not update the source, contact a service or certify unsupported reasoning.

The author can use ordinary inline styles and scripts. These run in an isolated frame;
source excerpts are rendered as text in the evidence layer. Accepting/resetting an edit
restarts the authored interactions. Transient app state is not persisted as a document
save format. Network-dependent apps, Word/Google Docs, application-screen overlays and
viewing without JavaScript are outside this version. Opening an old file never detects
later source changes by itself.

## Author and developer entry points

```sh
kpopper document guide
kpopper document build --html draft.html --manifest evidence.json --out report.html
kpopper document inspect report.html
kpopper document refresh report.html --sources sources.json --out report-updated.html
```

The [author guide](../scripts/document-guide.md) describes exact source selectors,
numeric formats, supported checks and the read-only integration with existing ingestion
outcomes. Input manifests are internal authoring work; the delivered file does not
need them. A document does not require a new knowledge record or workspace survey.

The Python package and editor plugins carry this runtime. The npm package continues to
provide the existing record-page browser checker, not the document command. Runtime
checks cover HTML structure, finite source snapshots, portable packaging and copy
updates. The Node document tests exercise real layer scripts in a DOM model; they are
not a claim of native browser layout or download verification.
