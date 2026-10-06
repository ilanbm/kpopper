# Create a reusable model plugin

A model plugin is for a reusable, sourced knowledge model that people can install and query from different projects. It is separate from an ordinary project record, a personal case, and an application adapter. Start with a dedicated native record and keep its source and history closure intact. The minimal read-model source fixture shows the descriptor and skill shape; its source is synthetic.

## Decide what the model covers

Write down the questions the model is meant to answer, the source edition or retrieval date, the facts and rules it can support, and the boundaries it leaves unknown or outside scope. Make each claim traceable to a source passage. When a rule can be represented, record the rule and its inputs instead of only a computed result.

Derive expected questions and answers independently from the source, then use them to check the authored model. Keep these maintenance tests in the source repository; they are not evidence for a claim and need not ship in the consumer plugin.

## Author the native record

Use a dedicated model root. For a model that is also kept inside a larger project repository, create the record first in a fresh private directory outside all Git repositories, then copy its complete authored closure into the dedicated model directory. This prevents the first kpop add from resolving to the repository's ordinary project record: --workspace binds a project context and does not by itself select a nested model entry.

Put cited local source files beneath the model root. In an empty private model directory, make the first native add with --workspace set to that same directory. It creates GROUNDING.yaml at the workspace root. Do not use kpop init for an ordinary model; that initializes a different native feasibility store. Once the record exists, use the explicit absolute GROUNDING.yaml selector on each applicable authoring command.

Set `XDG_CACHE_HOME` to a separate private directory that the task can already
write. Native execution needs its runtime cache even with `--no-cache`; a
sandboxed task may not be allowed to create the default `$HOME/.cache`.
Keep the cache outside the model directory and source export. This is a
task-local environment setting, not a global permission or installation change.

For example, with a fictional quoted rate:

    export XDG_CACHE_HOME=/ABS/WRITABLE/model-authoring-cache
    mkdir -p "$XDG_CACHE_HOME"
    mkdir -p "$MODEL_ROOT/sources"
    cp /ABS/SOURCE/rate-quote.md "$MODEL_ROOT/sources/rate-quote.md"

    kpop --workspace "$MODEL_ROOT" add source.rate_quote '{"name":"Venue rate quote","file":"sources/rate-quote.md","read":"2026-10-06","at":"quoted rate sentence","fidelity":"direct quotation","v":"The standard meeting room costs $10 per hour. This quote states no weekend rate or weekend surcharge."}' --json

    kpop --workspace "$MODEL_ROOT" add pricing.hourly_rate v=10 from=source.rate_quote 'at=quoted hourly rate' --as-of 2026-10-06 "$MODEL_ROOT/GROUNDING.yaml" --json

The source mapping should name a local file or URL, a read date, and a specific location; preserve enough quoted material to support the claim. A fact uses v, from and at; add a meaningful --as-of date. When available, pass the actual host-provided KPOPPER_AGENT_SESSION identity to native authoring commands. Do not invent identity values.

Run native kpop check and kpop history-capture against the authored entry. Read the source and model back through the native readers. A clean check establishes record structure and declared conditions; it does not verify the source's truth or prove model completeness.

Keep the whole model directory together: GROUNDING.yaml, its generated .gitattributes, compact native history and required history members, and every cited local source file. Do not hand-edit generated history. Exclude transient .kpopper/project.lock and .kpopper/.history-local/ from the committed or packaged model. Do not copy a project record or project-only evidence into it.

## Describe and package the model

Add a domain skill that tells the consumer how to ask in-scope questions, read the model and source, cite results, and preserve unknowns. Keep the descriptor's model and skill paths inside committed source files. Use model-package.json with format kpop-model-package/v1; keep the ID and version explicit, declare capability read for models using only the standard reader, include exact target and engine identity, and choose a source-backed smoke entry. See the example descriptor for the full field shape.

Copy the complete examples/model-plugin directory contents, including hidden native history and attributes, into the root of a dedicated Git source repository. The descriptor's model and skill paths are relative to that repository root. Commit the source closure before building; the builder refuses selected files that are not committed or whose bytes differ from HEAD.

Build from that exact source tree with the release-qualified prebuilt common kpop-model tool and complete engine archive:

    kpop-model build --source /ABS/SOURCE_REPO --descriptor model-package.json --engine-archive /ABS/kpopper-0.15.1-darwin-arm64.tar.gz --output /ABSENT/OUTPUT_DIR

The output must be absent. The builder verifies committed source bytes, the model closure and declared engine before producing the generated lock, launcher, bundled engine, Codex plugin manifest and skill. Do not hand-create generated lock data, copy only GROUNDING.yaml, or label a descriptor and model source folder an installable plugin. The release-qualified common tool and engine are supplied to authors as prebuilt artifacts; model authors do not need Rust/Cargo, Python, Node or another runtime. If the qualified builder or complete engine archive is unavailable, keep the authored source as a candidate and report that it is not a runnable bundle.

Verify and prepare the generated bundle in an isolated package cache, then read through its bundled native reader:

    kpop-model --bundle /ABS/BUNDLE verify
    kpop-model --bundle /ABS/BUNDLE --cache /ABS/PRIVATE-CACHE setup
    kpop-model --bundle /ABS/BUNDLE --cache /ABS/PRIVATE-CACHE pull pricing.hourly_rate

Install only a generated plugin using Codex's supported plugin route. The local Codex layout uses `.codex-plugin/plugin.json` in the plugin directory and `.claude-plugin/marketplace.json` in the marketplace root. Installing plugin code still requires the user's host trust decision. After installation, test the domain skill in a fresh task and confirm that it invokes the installed native reader.

## Keep application behavior separate

A read model uses the common native reader and needs no custom Rust code. Existing native rules can derive values from recorded inputs without a custom adapter. If an application additionally supplies case-specific inputs, private binding or domain output projection, declare capability application and maintain its adapter, independent cases, build, compatibility and release separately. Do not put domain-specific case fields into the generic descriptor or write case data into the reusable model.

For case adapters, bind each case in a disposable copy of the complete model; keep personal inputs outside the canonical model and project. A failed model component or missing field is unknown for that component, not an overall denial. Publishing, remote marketplace setup and consumer-machine installation are separate decisions beyond building a local candidate.
