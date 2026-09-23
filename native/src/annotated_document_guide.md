# Annotated Documents author guide

When explicitly asked for a kpopper HTML document with evidence (or under a standing preference), write the requested content and design, and create its
internal evidence mapping while writing. Deliver the output of `kpop experimental annotated-doc build`. The
user supplies neither anchors nor mapping. A one-off document needs no record setup.
The document contains all display resources, source excerpts and decisions. JavaScript
is required for viewing and review; no server, CDN, neighboring files or network is needed.

## Author once, package once

1. Read the relevant authorized sources. Keep their exact excerpts/fields and original
   citations. A local extraction made from a connector or original document must be
   labeled `representation: "extraction"`; its arithmetic can be checked, but extraction
   accuracy is the author's responsibility. Never invent an unavailable source value.
2. Write complete UTF-8 HTML with explicit balanced html/head/body tags. Keep the design,
   inline CSS/JavaScript, inline SVG, and data-URI images/fonts. Avoid external styles,
   imports, srcset, frames, base or CSP/refresh meta elements. The packager refuses these,
   rather than deleting them. Obtain any required assets through authorized tools and
   inline them first. Self-contained author interactions run inside a sandbox; network,
   forms, popups, workers and access to the evidence shell are blocked. Origin-bound
   storage such as localStorage is unavailable; keep transient controls in memory. Citation links
   in the evidence layer may open the original source explicitly.
3. Anchor each substantive fact or inference with a stable ID, for example
   `<span data-kpopper-claim="registered">80</span>`. A mechanical claim must cover a
   text-only element. Keep anchors outside controls and templates; do not nest them or
   reuse IDs. Reveal mapped passages inside a disclosure or tab before accepting their
   update. Do not split one atomic group across mutually exclusive views. Preserve the source's exact wording for a quotation; paraphrases
   and conclusions use `inference` unless supported by a specific mechanical check.
4. Write the small manifest described below and run the build command. Inspect its
   results. Fix accidental missing anchors/incorrect formats; keep real missing evidence
   and unsupported reasoning visible. Unmarked content is not checked. The displayed
   coverage counts are heuristics, not proof that every assertion was discovered.
5. Return the one final HTML file. The internal draft/manifest/source files are authoring
   inputs only. Show the document using the host's permitted artifact/browser tools and
   check its actual layout and interactions when available.

```sh
kpop experimental annotated-doc build --html draft.html --manifest evidence.json --out report.html
kpop experimental annotated-doc inspect report.html
```

`--root` defaults to the manifest's directory. Source paths must stay inside it and
must not traverse symlinks. Output is a separate copy; `--overwrite` can replace a prior
output, never an input/source. If `kpop` is not on PATH, run the command the active
plugin installed under its own `scripts/runtime` directory; do not modify global settings.

## Manifest v1

Given `counts.json` containing `{"registered":80,"finished":60}` and authored HTML
with text-only anchors `registered` = `80` and `rate` = `75%`:

```json
{
  "version": 1,
  "title": "Reading challenge",
  "language": "en",
  "sources": {
    "counts": {"name":"August counts", "path":"counts.json", "format":"json"}
  },
  "claims": [
    {"id":"registered", "label":"Registered readers", "kind":"value",
     "inputs":[{"source":"counts", "pointer":"/registered"}]},
    {"id":"rate", "label":"Completion rate", "kind":"ratio",
     "inputs":[{"source":"counts", "pointer":"/finished"},
               {"source":"counts", "pointer":"/registered"}],
     "format":{"scale":100,"decimals":0,"suffix":"%"}}
  ]
}
```

The source map accepts `format: "json"`, `"text"` or `"record"`, plus an optional HTTP(S)
`uri` identifying the original citation. Only selected scalar fields/quotes are embedded,
with the file revision and actual read time. Entire unrelated sources are not copied.
A missing source is explicit: `{"name":"Accessibility approval",
"unavailable":"No written approval was provided"}`. Reference it from a claim as usual.
A failed read, missing field, null value, ambiguous quote or invalid arithmetic remains
unavailable. It never becomes a match or an actionable correction.

Every claim has an `id`, human `label`, `kind`, and `inputs`. IDs start with a letter
and contain only letters, digits, `.`, `_` or `-` (at most 128 characters).

- `value`: one JSON pointer (RFC 6901) or stored-record reading; the exact rendered
  string/boolean/number is compared to the captured scalar.
- `quote`: one input `{"source":"notes","quote":"The exact source sentence."}`
  from a text source. The quote must occur exactly once and equal the anchor's text.
- `sum`, `product`: one or more numeric inputs; `difference`, `ratio`: exactly two
  inputs in order. Values must be numeric source fields; numeric-looking prose is not
  silently interpreted. A zero divisor cannot produce a ratio.
- `inference`: `reason` explains the interpretation or missing support. Inputs can cite
  exact fields/quotes or just `{"source":"notes"}`. It is always shown as unchecked;
  changed sources flag it for review. It is never automatically rewritten.

Numeric `format` supports `decimals` (0–8), `scale` (for example 100 for percentages),
`prefix`, `suffix`, `thousands` (`,`, `.`, space or nonbreaking space) and `decimal`
(`.` or `,`). Separators must differ. Rounding is decimal half-up at the declared
precision. Without a format, a number is rendered without grouping or trailing zeroes.
Use an explicit format for a ratio. The original anchor text must exactly match the
expected formatted reading to earn a match; an unrelated label/unit belongs outside
that anchor unless it is part of the prefix/suffix.

Claims that share input fields, the same paragraph/cell, or an optional `group` ID are
reviewed atomically. Related source values and calculations should all be anchored.
The packager proposes only text changes, shows complete containing context, rechecks
its candidate, and preserves unrelated author markup. If a member lacks required evidence,
the group has no actionable update. Unchecked inference stays separate and visible.

## Refresh and review

Read the new sources first. Create `sources.json`, an object from existing source IDs
to new input definitions (the same shape as the manifest's `sources` object), then run:

```sh
kpop experimental annotated-doc refresh report.html --sources sources.json --out report-updated.html
```

Only supplied sources are reread. Others retain their old snapshots and are labeled
not reread. A changed revision is distinct from a failed value check. Decide any pending
proposal and download its copy before another refresh; do not silently discard a choice.

In the document, accept or reject a complete proposed group. Acceptance changes only the
working document; it does not read sources, certify reasoning or write to originals.
Download the copy to save the decision, evidence and chosen content. Reopening that copy
retains them. Reloading the original file discards unsaved choices. Accept/reset restarts
the author's embedded interactions; transient app state is not a document save format.
If a claim was changed by author code after the proposal, the guard refuses a stale edit.

## Existing record and ingestion

For a stored scalar, use `format: "record"`, a local PROVENANCE.yaml `path`, and a
selector such as `/reading.capacity/v`. The canonical reader carries its recorded
source identity, location and date into the snapshot. This is a check against recorded
values, not an optional Lean proof or an independent read of the cited original.
Multi-file/pointer/hypothesis records, judgments and computed entries are not projected
as scalar evidence; extract the relevant authorized reading explicitly when appropriate.

For an explicit core record source, add `profile: "core/v1"`:

```json
{"name":"Current record","path":"GROUNDING.yaml","format":"record","profile":"core/v1"}
```

The builder captures one frozen Snapshot and one history-aware assessment, then embeds only the
selected node/history findings with their `snapshot_id` and `findings_revision`. Offline replay
does not read the record, Git or an evaluator. Refresh rereads only sources named in the refresh
request; retained sources are marked `reread: false`. This source profile does not activate or
migrate the record, and document-local decimal checks keep their existing contract.

A record source can include the canonical 32-character `event_id` returned by capture
(and an optional source-root-relative `state_dir`)
to require a matching durably applied ingestion event. Use the existing ingestion tools
to capture/process an authorized update. Keep the exact envelope/date across retries;
read `kpop ingest status --event-id ...` even when a repeated processing pass returns
no items. The document command reads the outcome; it never processes or writes the record.
Only a matching current value AND citation establish the event binding.

The HTML has no watcher and cannot know about a later external change on its own.
Word/Google Docs, app-screen overlays, arbitrary online applications, non-JavaScript
viewing and cryptographic attestation of a malicious author are outside this version.
