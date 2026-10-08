# Create and use a reusable model plugin

A model plugin contains a versioned knowledge model, its sources and native history,
a domain skill, and a pinned native engine. It can be used from different projects.
The reusable model stays separate from project records and private case data.

Initial support is **local Codex on macOS ARM64**. The companion is currently a source
candidate; a public prebuilt companion download has not been released. The author
walkthrough requires a qualified prebuilt candidate supplied by a maintainer.

## What you need

| Your task | Requirements |
| --- | --- |
| Use a model plugin | Codex and a complete generated plugin from its publisher. The engine is included; no separate kpopper, Python or compiler installation. |
| Create and package a model | Git, a qualified prebuilt `kpop-model` companion and the complete pinned engine archive. Authoring a new record also uses the qualified native `kpop` executable. |
| Maintain the companion or a custom case adapter | Rust and the component's tests. This is the maintainer's build task; read/query model authors need no custom Rust adapter. |

If you only want to ask questions, go to [Use a model plugin](#use-a-model-plugin).
To create your own model, the [model skill](../skills/model/SKILL.md) guides the work.
The walkthrough below first packages a complete synthetic example, so its commands
can be followed without authoring a record or inventing a descriptor.

## Try the complete example

Use a checkout containing [examples/model-plugin](../examples/model-plugin/README.md),
the prebuilt companion, and the public [kpopper 0.15.1 macOS ARM64 engine archive](https://github.com/ilanbm/kpopper/releases/download/v0.15.1/kpopper-0.15.1-darwin-arm64.tar.gz)
named by the [descriptor](../examples/model-plugin/model-package.json). The builder
verifies the archive's SHA-256. Git must have your author identity configured.

Set these four absolute paths. `MODEL_WORK` must be a new directory in a writable
location outside any existing Git repository. Keep these variables in the same shell
for the following steps.

```sh
KPOPPER_SOURCE="/ABS/PATH/TO/kpopper"
KPOP_MODEL="/ABS/PATH/TO/kpop-model"
ENGINE_ARCHIVE="/ABS/PATH/TO/kpopper-0.15.1-darwin-arm64.tar.gz"
MODEL_WORK="/ABS/WRITABLE/new-model-example"
```

Create a source repository from the complete example, including its hidden native
history and attributes. The model is already authored; do not recreate or edit its
`GROUNDING.yaml` or history by hand.

```sh
mkdir "$MODEL_WORK"
MODEL_SOURCE="$MODEL_WORK/source"
MODEL_PLUGIN="$MODEL_WORK/venue-rates-0.1.0"
MODEL_CACHE="$MODEL_WORK/cache"
mkdir "$MODEL_SOURCE"
cp -R "$KPOPPER_SOURCE/examples/model-plugin/." "$MODEL_SOURCE/"
git -C "$MODEL_SOURCE" init
git -C "$MODEL_SOURCE" add -- .
git -C "$MODEL_SOURCE" commit -m "Add the synthetic venue-rates model"
```

Build into the absent output directory. The builder checks the committed source
bytes and validates the native model before producing the plugin.

```sh
"$KPOP_MODEL" build --source "$MODEL_SOURCE" --descriptor model-package.json \
  --engine-archive "$ENGINE_ARCHIVE" --output "$MODEL_PLUGIN"
```

Read through the generated plugin's own launcher, using a private cache:

```sh
"$MODEL_PLUGIN/bin/kpop-model" --bundle "$MODEL_PLUGIN" verify
"$MODEL_PLUGIN/bin/kpop-model" --bundle "$MODEL_PLUGIN" --cache "$MODEL_CACHE" setup
"$MODEL_PLUGIN/bin/kpop-model" --bundle "$MODEL_PLUGIN" --cache "$MODEL_CACHE" \
  pull pricing.standard_hourly_rate pricing.weekend_rate_status
```

The source states USD 10 per hour for the standard room and leaves weekend pricing
unspecified. Read the returned source references and keep that unknown visible.
The same entry IDs appear in the model, descriptor, example README and commands.
This small fixture exercises packaging and installation; it is not a demonstration
of usefulness at scale or comparative model performance.

## Use a model plugin

Obtain a complete generated plugin and its version/marketplace instructions from
the publisher. A source checkout or a copied skill does not contain the built
launcher and engine. Review the plugin's coverage and sources before using its answers.

In local Codex, add the publisher's marketplace and install the named plugin. These
forms are supported by Codex CLI 0.153.3; use `codex plugin --help` for your version.
The publisher supplies the marketplace source, plugin name and marketplace name.

```sh
codex plugin marketplace add /ABS/PUBLISHER-MARKETPLACE
codex plugin add PLUGIN-NAME@MARKETPLACE-NAME
```

Start a new Codex task and ask the installed domain skill a question. For the example:

> Use venue-rates to explain the standard room rate and whether weekend pricing is
> known. Cite the source and identify the model version.

The domain skill locates its installed bundle and runs the bundled launcher. First
use runs `setup` to validate and activate a private generation; later reads use that
installation. The creator's `model` skill is for building models, not for answering
questions from an installed domain plugin.

For direct use, set `MODEL_PLUGIN` to the actual installed bundle directory and
`MODEL_CACHE` to a private writable cache. Use the same explicit selection on every call:

```sh
"$MODEL_PLUGIN/bin/kpop-model" --bundle "$MODEL_PLUGIN" --cache "$MODEL_CACHE" doctor
"$MODEL_PLUGIN/bin/kpop-model" --bundle "$MODEL_PLUGIN" --cache "$MODEL_CACHE" \
  pull pricing.standard_hourly_rate
"$MODEL_PLUGIN/bin/kpop-model" --bundle "$MODEL_PLUGIN" --cache "$MODEL_CACHE" versions
```

The entry above belongs to the example; other plugins declare their own IDs and
instructions. Install an update explicitly and run its setup. When a new bundle
replaces an earlier version at the same path, `activate --digest DIGEST` can select
a retained generation. When Codex installs versions at different paths, select the
older plugin through the host. The selected version remains visible in results.

### Install the local example

For the example built above, create a local marketplace **beside** the generated
bundle. This file is host configuration and must not be added inside the locked bundle.
The commands below install the example into your Codex configuration.

```sh
mkdir -p "$MODEL_WORK/.agents/plugins"
cat > "$MODEL_WORK/.agents/plugins/marketplace.json" <<'JSON'
{
  "name": "local-model-example",
  "plugins": [{
    "name": "venue-rates",
    "source": {"source": "local", "path": "./venue-rates-0.1.0"},
    "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
    "category": "Productivity"
  }]
}
JSON
codex plugin marketplace add "$MODEL_WORK"
codex plugin add venue-rates@local-model-example
```

Generated bundles use the supported `.codex-plugin/plugin.json` compatibility
manifest. See [OpenAI's plugin packaging documentation](https://developers.openai.com/plugins/build/plugins)
for local marketplace setup and host differences. These commands do not publish a
plugin to a public directory or install generic project-record hooks.

## Create your own model

Choose a bounded subject, sources and the questions the model should answer. State
its source edition, assumptions, coverage and unknowns. Derive expected answers
independently from the sources and keep maintenance tests in the source repository.
They need not ship in the consumer plugin.

Author a new native record in a fresh private directory outside Git, with cited local
source files beneath that root. A first `kpop add` inside a repository can resolve to
its project record; `--workspace` alone does not select a separate nested model.
Set `XDG_CACHE_HOME` to a writable private directory outside the model, within the
task's existing allowed paths. The native engine needs its cache even with `--no-cache`.

Use the qualified native engine and the [native authoring reference](../skills/model/references/authoring.md).
Record sources before claims that cite them. After the first add creates the record,
select its absolute `GROUNDING.yaml` path for each applicable authoring command.
Run `kpop check` and `kpop history-capture` against it. A clean check establishes
structure and declared conditions, not source truth or completeness.

Copy the complete authored model into its chosen source directory: `GROUNDING.yaml`,
`.gitattributes`, native history and all cited local files. Exclude transient
`.kpopper/project.lock` and `.kpopper/.history-local/`. Do not copy the example's model
or history over your own record. Use its descriptor and skill only as format examples:
set your own ID/version, model path, skill path, product-file allowlist and smoke entry.
Every smoke/read ID must exist in your model. Commit that selected closure before
running `build` as above. Generated bundles and caches stay outside the source tree.

A read/query model uses the common native reader, including native rules over
recorded inputs. An application that binds private case inputs or projects results
into a domain-specific format declares `capability: application` and supplies a
separately tested native adapter. Its publisher owns the adapter's compatibility and
build. Case binding uses disposable model copies; personal values never enter the
canonical model or project record. Missing inputs and failed components remain
visible and do not imply a conclusion about every part of a case.

## Maintain the native companion

The companion in `tools/model-package` is a separate Rust crate. Its dedicated
[Native model plugins workflow](../.github/workflows/model-plugins.yml) tests and
builds it on macOS ARM64 with the pinned complete engine archive. The core engine's
CI lane does not exercise this crate. Maintainers can run the same gate locally:

```sh
export KPOP_MODEL_ENGINE_ARCHIVE=/ABS/kpopper-0.15.1-darwin-arm64.tar.gz
cargo +1.98.1 fmt --check --manifest-path tools/model-package/Cargo.toml
cargo +1.98.1 test --release --locked --manifest-path tools/model-package/Cargo.toml
cargo +1.98.1 clippy --locked --manifest-path tools/model-package/Cargo.toml --all-targets -- -D warnings
cargo +1.98.1 build --release --locked --manifest-path tools/model-package/Cargo.toml
```

The gate exercises actual package creation, installation, native reads, updates,
rollback and failure recovery. Its candidate executable is an artifact for review;
a successful build does not publish or qualify a public release.
