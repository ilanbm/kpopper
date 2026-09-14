# Find evidence inside the current conversation

Use a known ID with `pull` or the checked reader. Otherwise:

Checked sessions also expose `kpopper_search` / `kpopper session search` for resumable
record discovery and `kpopper_context` for support/impact reads. Their session revision
belongs to that protocol; see [checked-session retrieval](../../docs/retrieval.md).

```sh
kpopper search "משלוח Cedar" --chars 4000 --limit 5
kpopper search --read source:s.contract --revision REV --offset 0 --length 2400
```

`search` returns JSON, including in ordinary CLI mode. Each result carries `scope`,
`status`, its recorded ID, declared source/dependency IDs, an excerpt, offset and read
reference. `line` is a line of the indexed source text or serialized entry body, not a
line of the whole YAML file. Search results locate evidence; they do not establish truth,
semantic equivalence, authorization, or that a quoted report remains current.

The primary agent supplies useful search terms from its existing understanding. Terms
are matched literally, with Unicode tokenization and OR matching; there is no separate
LLM call, translation or automatic synonym expansion. IDs and names receive more weight
than body text. Preserve original names and quotations in every language. Try alternate
terms when the first query misses; a missed query is not proof that no evidence exists.

Search covers record entries, native hypotheses, retained ingestion reports and referenced
local UTF-8 text sources up to 1 MiB each. Supported suffixes are `.txt`, `.md`, `.markdown`,
`.rst`, `.csv`, `.tsv`, `.json`, `.yaml`, `.yml`. It does not fetch URLs, extract PDFs or
read the host's whole conversation history. Missing, unsupported and changed captured
sources appear in `unindexed`; `unindexed_count` and `unindexed_omitted` remain explicit
when the output budget cannot carry every diagnostic. Relative source files resolve
beside the record file that defined the winning entry. Sources declared in a hypothesis
resolve beside the base record, so folding the hypothesis does not relocate them.

Source contents are read only inside the primary record's directory tree, or from an
integrity-checked ingestion capture. For an authorized source directory elsewhere, pass
`--source-root /path/to/sources`; repeat it for multiple directories and on every `--read`.
The record and its hypotheses cannot grant this permission. Parent traversal and symlinks
are checked against the resolved roots. Outside sources appear in `unindexed` until a root
is explicitly supplied. Changing those grants invalidates the search revision.

`omitted` counts matching results not returned because of `--limit` or `--chars`.
Excerpts declare truncation. Use `--read` with the returned reference and revision to
recover exact text; use `next_offset` to continue a partial read. The `search-corpus`
revision binds the project, source text, capture state and graph. It is **not** a checked
session revision: never pass it to `kpopper_read` or `kpopper_propose`. If the corpus changes,
search again; do not reuse a stale excerpt as the current source.

The separate `record_sha256` field is the hash of the primary record file. `open --json`
also returns it with the record view, so no extra search is needed just to obtain a hash.
Pass the hash from your prior read with batches that add entries. It protects recorded
premises against intervening changes; it does not bind external source content or replace
the search-corpus revision used for exact source reads.

Hypotheses remain labeled hypotheses. Captured reports retain their processing state,
including `needs_primary`, and do not become facts merely by matching a query. A canonical
claim's state comes from the existing reader; unevaluated falsifiers remain unknown.
Rule references are followed as dependencies. Structured rules are computed by the shared
Lean core; legacy textual rules remain unevaluated. Search does not infer formulas from prose.

The SQLite FTS5 index exists only in the command's memory and is rebuilt from authoritative
inputs. The shared reader may maintain its derived parse cache outside the project; use
`kpopper --no-cache search ...` to bypass it. Search does not change records or ingestion
state and makes no network/model calls.
The tool call and returned text still occupy model context. Keep reads focused, batch
related updates, and surface only findings that matter to the user's work.
