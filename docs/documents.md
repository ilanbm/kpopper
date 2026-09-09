# HTML documents with evidence

Ask the agent for the document you want, for example: “Create an HTML project update
from these notes” or “Make an HTML report from these results.” With kpopper active,
the authoring workflow creates the content, design and source/check mapping together.
You do not need to prepare a mapping or ask for a second provenance operation.

The result is one HTML file containing the authored document, styles, scripts, required
assets, selected evidence snapshots and review state. Open it in a browser with
JavaScript enabled. It needs no server, network connection or neighboring files to
show the document, inspect its evidence or choose a prepared update.

The Evidence button explains marked claims. A match means the displayed value or exact
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

Accept or reject a proposed group, then download the copy to save the decision. The
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
