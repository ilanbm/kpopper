# Native model authoring

A model is a dedicated native record plus its history, byte-preservation rules and cited local sources. Keep it separate from any ordinary project record. When adding a model inside an existing repository, first author in a new private directory outside all Git repositories, then copy the complete generated model closure into its selected model directory. A first add from a nested repository path can resolve to the repository's ordinary project record. The global workspace selector alone does not select a separate nested model file.

In a new empty private model directory, place cited source files beneath the model root and make the first kpop add with workspace set to that exact directory. The first add creates its GROUNDING.yaml there. Do not call kpop init for an ordinary model; that command initializes the separate native feasibility store. After the first add, pass the absolute model GROUNDING.yaml path as the final positional selector on every add that supports it.

Record the source itself before claims that cite it. A source entry is a mapping with a name, a local file or URL, a read date, a location and the source text or a bounded quotation. A fact carries v, from and at; use --as-of for its date. Use exact source IDs and locations. A passing kpop check is structural evidence, not proof that the source is true or the model is complete.

Set `KPOP_BIN` to the absolute qualified native engine path, `MODEL_ROOT` to a fresh
private directory outside Git, and `XDG_CACHE_HOME` to a separate writable private
cache within the task's allowed paths. Create the model root and copy the cited
source under it before using these forms:

    "$KPOP_BIN" --workspace "$MODEL_ROOT" add source.venue_quote '{"name":"Venue quote","file":"sources/venue-quote.md","read":"2026-10-06","at":"quoted rate line","fidelity":"direct quotation","v":"..."}' --json

    "$KPOP_BIN" --workspace "$MODEL_ROOT" add pricing.standard_hourly_rate v=10 from=source.venue_quote 'at=quoted hourly rate' --as-of 2026-10-06 "$MODEL_ROOT/GROUNDING.yaml" --json

Replace the illustrative source text, value and dates with what your source actually
supports. These forms explain native authoring; the [packaging walkthrough](../../../docs/model-plugins.md#try-the-complete-example)
starts from an already-authored fixture. Never overlay that fixture's history on your own model.

Set KPOPPER_AGENT_SESSION only from an actual host-provided session identity, when available. Keep native caches and read state outside the portable model closure. Run native check and history capture against the model before packaging. Copy the complete authored model root, including .gitattributes, .kpopper/history.yaml, all required history members and cited source files; exclude transient .kpopper/project.lock and .kpopper/.history-local/.

Use examples/model-plugin/model-package.json as a descriptor example. Keep independent expected questions and answers in evaluator-only files; do not copy them into the model package or creator-visible prompt. Build only with the release-qualified prebuilt kpop-model tool and exact complete engine archive. Never substitute a locally compiled binary or a global kpop executable for the declared toolchain.
