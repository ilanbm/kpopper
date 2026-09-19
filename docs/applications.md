# Experimental applications

kpopper's core keeps claims, sources, dependencies, history and review conditions.
Applications use those records and assessments to produce another experience.
Layer and maturity are separate: an application can become supported without
becoming part of the core, and a core profile can remain experimental.

**kpopper Hub** (`hub`) presents the project record. **Annotated Documents**
(`annotated-doc`) creates standalone HTML with evidence. Both are optional, experimental
applications.
Their command options, layout and artifact formats may change. Experimental status
does not relax evidence, isolation or record-integrity checks.

## Installation and use

Install the core CLI normally. Add the shared HTML application dependencies explicitly:

```sh
python -m pip install 'kpopper[html]'
kpop experimental --help
kpop experimental hub --open
kpop experimental hub --verify
kpop experimental annotated-doc guide
kpop experimental annotated-doc build --html draft.html --manifest evidence.json --out report.html
```

For a source checkout, install `'.[html]'`. The same distribution includes application
code and assets; the extra installs their runtime dependencies. Applications are not
separate packages or independently versioned releases.

For a Claude Code or Codex plugin, use the active plugin's runtime setup:

```sh
python3 /absolute/path/to/kpopper/scripts/plugin_runtime.py setup --applications html
```

Ordinary setup installs the core dependencies and timezone data for followups; hooks
check only PyYAML before opening the record. Installing an application
does not activate it on every task. Request the record page or kpopper's evidence-bearing
HTML document explicitly, or give the agent a standing preference to use it. A generic
HTML request and completion of a map do not automatically select these applications.

The canonical commands are `kpop experimental hub` and `kpop experimental annotated-doc`.
The old `page` and `document` names still work both directly (`kpop page`) and under
`experimental` (`kpop experimental page`), with a compatibility notice on stderr.
All aliases use the same implementation and optional dependencies. The canonical skills
are `$hub` and `$annotated-doc` (Claude: `/kpopper:hub` and `/kpopper:annotated-doc`);
`$page` and `$document` remain explicit compatibility aliases.
Help works without installing HTML dependencies; commands never install them implicitly.
`kpop experimental --json` lists application layer, maturity and installation metadata.

## Boundaries

- Record checks and ordinary session hooks do not render HTML or validate page layouts.
- `kpop experimental hub --verify` owns coverage, arrangement and presentation checks.
  Projects using a page should run that explicit check in addition to `kpop check`.
- Page measurements are not ordinary recorded facts. A condition depending on an
  unavailable `page.*` value remains unevaluated; it is not reported as having passed.
- Explicit legacy arrangement and section authoring uses the optional page application.
  Page-specific writes and consolidation of layout proposals preserve their snapshot
  and validation guarantees; they require the optional runtime.
- The page renders a snapshot of the record. Standalone document refresh and review
  affect a document copy, never the source record. Core findings retain their original
  identity when displayed by either application.

The application entry points are `scripts/applications/hub.py` and
`scripts/applications/annotated_doc.py`. Existing renderer and
document modules remain implementation modules at their compatible paths. Existing
`page.*` record fields, artifact schemas and output paths retain their meanings. Core reader
operations do not depend on those entry points; explicit presentation authoring crosses
the application boundary deliberately.

## Maturity

| Capability | Layer | Status |
|---|---|---|
| kpopper Hub: record, layouts and interactive graph | Application | Experimental, optional HTML runtime |
| Annotated Documents: HTML evidence and copy updates | Application | Experimental, optional HTML runtime |
| Deterministic `core/v1` and immutable history | Core profile | Default for new records; existing legacy records require explicit adoption |
| Checked sessions and their MCP transport | Integration | Experimental, opt-in |

Promotion of an HTML application requires repeated independent use, a documented
compatibility/migration policy for its inputs and artifacts, and browser verification
of the supported open, refresh, save and reopen workflows. General-purpose record
commands do not become experimental merely because applications call them.

See [Annotated Documents](documents.md), the [Hub reference](../skills/kpopper/PAGE.md),
and [plugin runtime setup](plugin-runtime.md).
