# Find evidence in a checked session

`kpop session search` finds candidate records when a question does not map cleanly to a name in the opening. It returns references to the original records at the opening's revision. Read those references and their declared premises before relying on them. Search does not create relationships, verify claims or change the record.

For retained ingestion reports and passages in local text source files, the separate
`kpop search` command provides source-content discovery and exact source reads. Its
`search-corpus` revision is separate from this checked-session protocol. See the
[local source retrieval guide](../skills/kpopper/RETRIEVAL.md).

Exact identifiers are preferred over approximate matches. Lexical search covers the whole record without embedding dependencies. Optional local E5 adds semantic or hybrid ranking. A branch hint only breaks equal scores; it cannot exclude a stronger result from another branch.

## Command line

First [set up checked sessions](checked-sessions.md) and open the project:

```sh
kpop session open
kpop session search --revision REV_FROM_OPEN --query 'editable script copies and performance measurements'
kpop session search --revision REV_FROM_OPEN --id p.script_copies --search-mode lexical
kpop session read --revision REV_FROM_OPEN --ref node:p.script_copies
kpop session read --revision REV_FROM_OPEN --ref edges:p.script_copies
```

Use IDs actually supplied by the project; `--id` also accepts a returned `node:ID` reference. Source handles and field selectors remain read operations. An exact ID selects a record, not a guarantee of relevance: prefer the original question or a faithful translation when its source is not yet known. An unresolved ID means only that the ID was not found; a question may still be covered under a different name. `--branch /known/p` is an optional hint from the current map. `--limit` accepts1–32 candidates (default8); `--tokens` budgets the returned text, including metadata and its final newline (default1,000 reference tokens). Results identify the candidate subset, any omitted whole hits and the root recovery route. If the budget cannot fit a complete response, raise it; records and identifiers are never clipped to fit.

When more candidates are needed, pass the returned `next_cursor` and repeat the same query, ordered IDs, branch and mode:

```sh
kpop session search --revision REV_FROM_OPEN --query 'recorded performance limits' --limit 8
kpop session search --revision REV_FROM_OPEN --query 'recorded performance limits' --limit 16 --cursor CURSOR_FROM_SEARCH
```

Page size and token budget may change. `offset` is the page's starting position; `matched_candidates` counts the entire ranked result, and `next_offset`/`next_cursor` describe the next page. Continuation advances past emitted whole hits only, so a smaller text budget does not skip candidates. If even the next whole hit cannot fit, the search fails explicitly instead of skipping it. Pagination metadata consumes part of the same budget.

The cursor is a validated, search-bound read position, not an authenticated credential. Treat it as opaque. A fresh process can continue the same ranking. Changing the revision, search arguments, actual backend or ranked result rejects it; restart search in that case. Keep the optional embedding configuration available on later pages. A null cursor means this ranking is exhausted. In lexical mode that covers positive matches for this query, not every potentially relevant record; reformulate or inspect the map if evidence remains missing.

Search results contain references, not evidence excerpts. Existing reads preserve complete original fields, source handles, current values and review snapshots; oversized reads return field routes or explicitly labeled exact-text fragments. A changed record or navigation profile invalidates the revision. Reopen before another search or read.

## Read a declared neighborhood

After identifying relevant records, use `kpop context` to read their declared neighborhood:

```sh
kpop context d.choice
kpop context d.choice --depth 2 --tokens 2000
kpop context m.reading --direction impact --max-nodes 16
```

This captures the current record and defaults to support depth 1 and 2,000 reference
tokens. It needs no AI conversation, known task object, or preceding `session open`.
Supply 1–8 exact IDs or `node:ID` handles; discover them with the map or search first.
`--revision REV_FROM_OPEN` preserves an explicit checked view and rejects stale inputs.
`--input`, `--project`, `--state`, `--profile`, `--assessment-profile`, `--encoding` and
`--no-settings` retain the existing reader's routing. It does not enable a runtime,
change a record's interpretation, apply proposals, or certify source truth.
The private cache must be writable. In a restricted agent sandbox, use
`--state /path/to/allowed/private-directory` or an allowed `XDG_STATE_HOME`;
the record itself remains unchanged.

The same checked-reader prerequisites apply. If unavailable, use `pull`/`affects` and
state that limitation. The existing transport spelling remains supported:

```sh
kpop session context --revision REV_FROM_OPEN --id d.choice --direction support --depth 2 --tokens 2000
kpop session context --revision REV_FROM_OPEN --id m.reading --direction impact --depth 1 --max-nodes 16
```

Replace these example IDs with IDs from the project. Supply1–8 known IDs or exact `node:ID` references. `support` follows declared `rests_on`, `from` and `rule_reads` links; `impact` follows those links in reverse to recorded dependents. The displayed edges retain their original orientation. Names, hierarchy, similarity and prose mentions do not create new relationships.

Depth defaults to1 and accepts0–4; zero reads seeds only. `--max-nodes` defaults to16 and accepts1–64 candidates, including the unique requested seeds. A cap smaller than the seed count is rejected. Candidates are visited breadth first with deterministic edge ordering, one original body per ID and the first exact path to each neighbor. Cycles are deduplicated.

The response contains selected complete original reads and existing Lean cards, their `via` paths, and `omitted_for_budget` node references. `unread_seed_refs` identifies requested bodies that did not fit. `candidates` counts the bounded schedule, while `candidate_limit_reached` identifies traversal stopped by that cap. These counts do not describe all relevant evidence.

`frontier` gives exact incident-edge routes for seeds and returned reads with unread endpoints in the requested direction. `unread_edges` includes unresolved endpoints; `missing_targets` separately counts distinct IDs absent from the record. Read `edges:ID` to see their exact names, including incoming edges for impact. Depth, candidate and token limits can all leave known bodies unread. Further branches remain accessible through the full map.

The default budget is2,000 reference tokens for the entire response, including cards, paths, omissions, frontier metadata and newline. Whole bodies are skipped when they do not fit; read `node:ID` or its exact `#/body` fields for them. If even the required boundaries cannot fit, raise the budget or reduce depth/candidate count. Read errors remain errors and do not establish absence.

Context reads are opt-in; ordinary search never expands automatically. Continue global search for lower-ranked evidence and unlinked qualifications. A declared path is not logical entailment, and reading a source locator does not fetch its external document. Lean continues to check the existing structured record assertions and current-versus-reviewed values; traversal and packing do not add a new formal proof or evaluate hypothetical updates.

## Optional local E5

The encoder ships with the command; ranking needs only the two model files below.

Obtain these two files from the [pinned Xenova multilingual-e5-small revision](https://huggingface.co/Xenova/multilingual-e5-small/tree/761b726dd34fb83930e26aab4e9ac3899aa1fa78):

- `onnx/model_quantized.onnx`
- `tokenizer.json`

Place both directly in one local directory as `model_quantized.onnx` and `tokenizer.json`. Their pinned hashes and sizes are verified before loading. Other model variants require an explicit implementation change; a similarly named file is not accepted. Search performs no downloads or remote embedding requests.

```sh
kpop session search --revision REV_FROM_OPEN --query 'recorded observations and unresolved conditions' --embedding-dir /path/to/e5 --search-mode hybrid
kpop session serve --embedding-dir /path/to/e5
```

`semantic` ranks by E5 similarity; `hybrid` combines semantic and lexical ranks. Both prefer exact IDs. Without a configured/usable encoder, the response explicitly reports complete-record lexical fallback. Ordinary query text is enough: E5 prefixes are added internally. Translation or project-specific query wording belongs to the calling agent; it must preserve the request's constraints and uncertainty. The server itself never calls a language model or needs its credentials.

The encoder uses local CPU inference. Index-only overlapping chunks cover long documents through their final token, and the highest chunk similarity supplies the document's score. Chunks do not replace the original evidence returned by `read`. The current bounded scorer supports at most512 records,128KiB per indexed document,2MiB total indexed text and1,024 chunks. Exceeding a bound falls back explicitly for the entire record; no oversized record is silently excluded. Queries over512 encoder tokens also fall back without being truncated.

The derived index is cached in memory by document IDs and content. A persistent MCP server reuses it across searches; a new CLI process builds it again. Changed documents rebuild the complete index. No index files are written into the project, and no canonical field or relationship comes from similarity.

## MCP

The same bound server exposes:

```text
kpopper_search(query, revision, ids=[], limit=8, tokens=1000, branch=null, mode="hybrid", cursor=null)
kpopper_context(ids, revision, direction, tokens=2000, depth=1, max_nodes=16)
kpopper_read(ref, revision, tokens=1600)
```

Configure the server with `--embedding-dir` to make E5 available. The search tool has the same behavior and source boundaries as the CLI. It is an optional discovery path alongside the complete map and exact reader. Lean continues to check the existing map and structured assertions; search similarity and an agent's prose are outside that proof boundary.
