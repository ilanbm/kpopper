# Checked sessions

The optional session transport opens a complete navigable view of the record, keeps exact field references, and computes assessment fields with a local Lean core. It exposes the same operations through the command line and MCP. It does not apply pending proposals or certify source-world truth.

Install the optional dependencies with Python 3.10 or newer:

```sh
python -m pip install 'kpopper[session]'
```

Use Lean 4.33.1, either selected by `elan` on the path or supplied as a toolchain directory:

```sh
kpopper session setup --lean-root /path/to/lean-4.33.1
kpopper session status
```

Setup compiles only the Lean source shipped with the package. It does not download a toolchain or execute record-supplied commands. The executable and its manifest are published together in a local cache keyed by source hash, operating system and architecture. A valid cache is reused; reads never build it. `session setup --rebuild` quarantines an invalid or existing cache before replacing it. Running session servers must restart after a program replacement.

## Open and read

```sh
kpopper session open --input PROVENANCE.yaml --project example
kpopper session read --input PROVENANCE.yaml --project example \
  --revision REVISION_FROM_OPEN --ref node:some.claim
```

The default input is the current project's root or registered record, including when the command runs in a repository subdirectory. `--input`, `--project`, `--state` and `--profile` can bind a service explicitly. An explicit input resolves project settings against that record's location, independently of the process directory. `--normalized` accepts a normalized JSON snapshot for replay. Every read uses the revision returned by open; a changed record rejects an old revision.

References include:

| Reference | Content |
|---|---|
| `/topic/path` | A complete branch view, folded by depth when needed |
| `node:ID` or `ID` | An entry; judgments default to an assessment card |
| `node:ID#/body/FIELD` | The original source field spelling and value |
| `checked:ID` | Structured assessment, including declared conflict and recorded status |
| `links:ID` or `links:/topic` | Directed outgoing links; `rests_on` is not implication |
| `events:/` and `event:e1` | Checked changes and uncheckable conditions; event IDs belong to the revision |
| `conditions:/` | Executable conditions and unevaluated prose declarations, including unreadable judgments |
| `source:record` | Captured source text and its content hash |
| `pending`, `native` | Pending session proposals and native hypotheses, respectively |

Large values remain addressable by JSON pointer. Large scalar text can be read with `--offset` using labeled fragments, offsets and a hash. Output never silently stops mid-sentence. A budget too small to carry a complete root is rejected explicitly. Token budgets use a named reference tokenizer; provider overhead and billing can differ.

IDs are printable names; `#`, `:` and `/` are reserved for the reference protocol and are
rejected in IDs instead of emitting unreachable references. Dotted names and Unicode names
are supported. Use `node:orientation`, for example, for an entry whose name matches a metadata
reference; bare `orientation` reads the view's editorial orientation.

## Declared navigation

Without a profile, navigation follows the record's sections and dotted ID namespaces. Names and grouping do not establish claims. An optional `PROVENANCE.session.json` beside the record can declare more useful topic routes:

```json
{
  "description": "Declared navigation; no logical implications.",
  "opening_depth": 2,
  "groups": {
    "delivery/reliability": ["d.delivery_policy", "m.delivery_failures"]
  },
  "orientation": {
    "text": "Recorded evidence and decisions about delivery reliability.",
    "basis": ["d.delivery_policy"],
    "status": "editorial synopsis; read its basis"
  }
}
```

Unassigned IDs remain visible under their namespace. Profiles cannot run code. A synopsis is an editorial declaration with basis references; the core does not prove its meaning from those references.

## Session hooks and rollback

The installed plugin's normal hook keeps its legacy behavior until checked mode is explicitly enabled:

```sh
kpopper session enable --tokens 1000
kpopper session disable
```

Enable stores the current Python executable in a project-scoped settings file outside the record, so the hook and later CLI reads use the environment containing the session dependencies. Optional `--profile`, `--project` and `--state` values are project-scoped. `--global` enables or disables the default for this machine without binding all projects to one profile or proposal store. A project setting overrides the global default. `KPOPPER_SESSION_DISABLE=1` temporarily selects the legacy hook.

The checked hook includes an executable read hint and budgets that hint together with the view. That command carries its resolved project, input, state and profile with `--no-settings`, so replaying it from another directory cannot pick a different interpreter or project configuration. If enabled checked mode cannot run, the hook reports that limitation and points to the record; it does not silently substitute a truncated result while claiming a checked view. Disabling does not delete the record, proposals or core cache. The Stop gate retains its existing baseline behavior.

Codex also reviews and trusts plugin hook definitions separately from installation. If the
host asks for that review, inspect the hook definition there before expecting automatic
SessionStart output. Reinstalling a plugin does not itself grant hook trust. The CLI and
explicitly configured MCP reader can be tested independently of that host step.

## MCP

Bind each stdio server to one project explicitly. This avoids depending on whether a host shares server processes or which directory it gives them:

```json
{
  "mcpServers": {
    "kpopper-example": {
      "command": "/path/to/session-python",
      "args": [
        "/path/to/kpopper/scripts/session_cli.py", "serve",
        "--input", "/path/to/project/PROVENANCE.yaml",
        "--project", "example"
      ]
    }
  }
}
```

An installed console entry can also launch `kpopper session serve` with the same arguments. `serve` uses the explicitly launched Python environment; it does not switch interpreters based on project settings. The server offers `kpopper_open`, `kpopper_read`, `kpopper_propose` and `kpopper_verify_claims`. The verifier checks only listed structured assertions against the supplied snapshot, never accompanying prose, source reliability or action authority. Proposals remain pending and do not overwrite the canonical record.

## Guarantees and limits

The core separates current values from review snapshots, unknown from false, executable conditions from prose, and premise confidence from judgment confidence. The view guard checks complete ID/link accounting, conflict signals, exact recovery references, event values and topic bindings. Native inferred field roles are passed separately from original source spelling; stale normalization is rejected. Unsupported predicates remain uncheckable, and malformed predicate types remain explicit assessment errors.

The Python router, native adapter and text renderer are tested but not formally proved end to end. The compiler and locally managed cache are part of the trusted runtime. Source-world truth, human re-openers and arbitrary logical languages remain outside the checks. Platform CI builds the pinned source on Linux, macOS and Windows; local validation alone does not establish that every host integration behaves identically.
