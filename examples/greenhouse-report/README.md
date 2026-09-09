# Greenhouse report

This example presents the Greenhouse data as an ordinary report with linked reasoning.
The data is adapted from the repository's page fixture; the document layout is kept in
`PROVENANCE.view.yaml`.

With the kpopper CLI installed, run from this directory:

```sh
kpopper check
kpopper page --verify
kpopper page --open
```

On the **Now** tab, hover over the heating conclusion to see its premises, condition and
reasoning. Click the conclusion to pin the card, then follow `heat.deficit_kw` and
`heat.boiler_kw` to reach the boiler-output reading in two steps. The card names
`doc.boiler_sheet` as its source; its supporting document is
[the boiler service sheet](sources/boiler-service.md).

Use the back button or Escape to return through the cards.
