# Checked sessions

To read records with their declared dependencies, start with `kpop context <id>`.
It captures the current revision automatically; `--revision` can require an existing
view. [Graph context](retrieval.md#read-a-declared-neighborhood) documents its bounds
and recovery routes. The `session` commands below remain the transport and setup API.

The optional session transport opens a complete navigable view of the record, keeps exact field references, and computes assessment fields with a local Lean core. It exposes the same operations through the command line and MCP. It does not apply pending proposals or certify source-world truth.

Native 0.9.0 bundles include the checked-session resources. Enable the native
session path with:

```sh
kpop session setup
kpop session enable --tokens 1000
```

The bundled path needs no Lean installation at runtime. `kpop session status`
reports whether the core is ready.

Setup prepares the verified programs bundled with the native distribution. It does not download or compile a toolchain, or execute record-supplied commands. A valid local runtime cache is reused; reads never build a program. `session setup --rebuild` retains the previous cache and extracts a fresh verified copy. Running session servers must restart after a program replacement.

## Open and read

```sh
kpop session open --input GROUNDING.yaml --project example
kpop session read --input GROUNDING.yaml --project example \
  --revision REVISION_FROM_OPEN --ref node:some.claim
```

The default input is the current project's root or registered record, including when the command runs in a repository subdirectory. `--input`, `--project`, `--state` and `--profile` can bind a service explicitly. An explicit input resolves project settings against that record's location, independently of the process directory. `--normalized` accepts a normalized JSON graph (`nodes`, `topics`, and optional `edges`/`node_order`) for replay, including with `context`. The graph is structurally validated and evaluated again; embedded findings are not accepted as verified results. Revisions bind the input path, bytes, project, navigation and executing program. Pass `--normalized` on follow-up commands too; it uses `checked-reader/v1` and cannot be combined with `--assessment-profile core/v1`. Every read uses the revision returned by open; a changed record rejects an old revision.

References include:

| Reference | Content |
|---|---|
| `/topic/path` | A complete branch view, folded by depth when needed |
| `node:ID` or `ID` | An entry; judgments include their original body alongside an assessment card |
| `node:ID#/body/FIELD` | The original source field spelling and value |
| `checked:ID` | Structured assessment, including declared conflict and recorded status |
| `links:ID` or `links:/topic` | Directed outgoing links; `rests_on` is not implication |
| `events:/` and `event:e1` | Checked changes and uncheckable conditions; event IDs belong to the revision |
| `conditions:/` | Executable conditions and unevaluated prose declarations, including unreadable judgments |
| `source:record` | Captured source text and its content hash |
| `pending`, `native` | Pending session proposals and native hypotheses, respectively |

Large values remain addressable by JSON pointer. Large scalar text can be read with `--offset` using labeled fragments, offsets and a hash. Output never silently stops mid-sentence. A budget too small to carry a complete root is rejected explicitly. Token budgets use a named reference tokenizer; provider overhead and billing can differ.

Opening and branch views may mix depths: a large group can remain folded while smaller
siblings expand into readable entries. Every group expansion retains all its children
and all other branches. Remaining space is allocated by a deterministic structural
heuristic, preferring additional leaf names per token, then additional branches; it
does not infer which topic answers a future question. Displayed entries use explicit
`node:ID` handles. `source_count` and `dependency_count` are link counts, not source IDs
or proof. Unknown node and topic routes point back to `/`; a source entry requested with the file
prefix points to its actual `node:ID` handle.

A judgment's complete source body stays with its computed card so rationale, recorded status
and revision history remain available. If both exceed the read budget, the response gives
exact field routes. Checks and field absence apply only to the named ID; a premise's value
does not disclose all of that premise's fields. Derived state tags are distinct from the
status text recorded in the source body.

IDs are printable names; `#`, `:` and `/` are reserved for the reference protocol and are
rejected in IDs instead of emitting unreachable references. Dotted names and Unicode names
are supported. Use `node:orientation`, for example, for an entry whose name matches a metadata
reference; bare `orientation` reads the view's editorial orientation.

## Declared navigation

Without a profile, navigation follows the record's sections and dotted ID namespaces. Names and grouping do not establish claims. An optional `.kpopper/session.json` beside the record can declare more useful topic routes:

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

The installed plugin's native hook uses the bundled session runtime when checked mode
is explicitly enabled:

```sh
kpop session enable --tokens 1000
kpop session disable
```

Enable stores the native executable and project-scoped settings outside the record, so
the hook and later CLI reads use the same runtime. Optional `--profile`, `--project`
and `--state` values are project-scoped. `--global` enables or disables the default
for this machine without binding all projects to one profile or proposal store. A
project setting overrides the global default. `KPOPPER_SESSION_DISABLE=1` temporarily
disables inherited settings.

Checked CLI output, hook transport and the Lean JSON protocol use UTF-8 independently
of the host code page. Captured source text retains its original line endings and
content hash. The printed command hint targets a POSIX shell, including Git Bash on
Windows; it is labeled accordingly. MCP requests do not require that shell syntax.

The checked hook includes an executable read hint and budgets that hint together with the view. That command carries its resolved project, input, state and profile with `--no-settings`, so replaying it from another directory cannot pick a different interpreter or project configuration. If enabled checked mode cannot run, the hook reports that limitation and points to the record; it does not silently substitute a truncated result while claiming a checked view. Disabling does not delete the record, proposals or core cache. Prompt diagnostics retain the session-start baseline comparison without a Stop continuation.

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
      "command": "/path/to/kpop",
      "args": [
        "session", "serve",
        "--input", "/path/to/project/GROUNDING.yaml",
        "--project", "example"
      ]
    }
  }
}
```

A `kpop` on PATH can be named instead of an absolute path. `serve` uses the runtime that launched it; it does not switch executables based on project settings. The server offers `kpopper_open`, `kpopper_read`, `kpopper_propose` and `kpopper_verify_claims`. The verifier checks only listed structured assertions against the supplied snapshot, never accompanying prose, source reliability or action authority. Proposals remain pending and do not overwrite the canonical record.

## Guarantees and limits

The checked predicate parser accepts one whitespace-separated reference, comparison
operator (`==`, `!=`, `<`, `<=`, `>`, `>=`) and right-hand operand. The operand can be a
complete JSON number, Boolean or string, a single-quoted string, or another declared
reference. For example:

```text
kbd.gate.prod != 'unset'
release.state == "awaiting review"
metrics.failures > 0
```

JSON strings retain JSON escaping. Single-quoted strings support only `\\` and `\'`
escapes. Spaces inside quotes are significant; the original expression is retained in
the assessment. Malformed quotes, trailing expressions, undeclared references, missing
values and incompatible types remain unknown. Null, arrays, objects and operators
without surrounding whitespace are outside this predicate subset. Null remains
readable as source data; comparisons involving null are uncheckable.

The core separates current values from review snapshots, unknown from false, executable conditions from prose, and premise confidence from judgment confidence. The view guard checks complete ID/link accounting, conflict signals, exact recovery references, event values and topic bindings. Native inferred field roles are passed separately from original source spelling; stale normalization is rejected. Unsupported predicates remain uncheckable, and malformed predicate types remain explicit assessment errors.

The router, adapter and text renderer are tested but not formally proved end to end. The compiler and locally managed cache are part of the trusted runtime. Source-world truth, human re-openers and arbitrary logical languages remain outside the checks. Platform CI builds the pinned source on Linux, macOS and Windows; local validation alone does not establish that every host integration behaves identically.

## Formal guarantees

The [Lean source](../scripts/session/lean/Main.lean) proves specific properties of
the checked-session core:

| Theorem | Property |
|---|---|
| `changed_does_not_falsify` | A changed premise alone does not count as falsification when the falsifier is false. |
| `unknown_is_not_a_negative_result` | An unknown falsifier cannot satisfy an assertion that it is false. |
| `scan_accounts_for_every_row` | Every scan row contributes to either the success or error count. |
| `accepted_view_preserves_conflict_signal` | A view accepted by the core's acceptance predicate preserves declared conflict signals. |
| `accepted_view_accounts_for_every_link` | An accepted view accounts for every link index. |

The [proof audit](../scripts/session/lean/ProofAudit.lean) names these theorems and
prints their axiom dependencies. Other runtime checks cover exact recovery references,
topic bindings and event values. The [arithmetic evaluator's proof scope](reasoning-core.md#runtime-licensing-and-assurance)
is documented separately.

These guarantees concern defined data structures and checks. They do not establish
source accuracy, logical entailment of an agent's verdict or permission to act.

## Finding a record from a question

Use [checked-session search](retrieval.md) for exact IDs, whole-record lexical search, and optional pinned local E5 ranking. Results are candidate references at the current revision; read them for evidence. The complete opening and Lean assertion checks retain their existing boundaries.
