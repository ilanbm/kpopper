# Find evidence in a checked session

`kpopper session search` finds candidate records when a question does not map cleanly to a name in the opening. It returns references to the original records at the opening's revision. Read those references and their declared premises before relying on them. Search does not create relationships, verify claims or change the record.

Exact identifiers are preferred over approximate matches. Lexical search covers the whole record without embedding dependencies. Optional local E5 adds semantic or hybrid ranking. A branch hint only breaks equal scores; it cannot exclude a stronger result from another branch.

## Command line

First [set up checked sessions](checked-sessions.md) and open the project:

```sh
kpopper session open
kpopper session search --revision REV_FROM_OPEN --query 'editable script copies and performance measurements'
kpopper session search --revision REV_FROM_OPEN --id p.script_copies --search-mode lexical
kpopper session read --revision REV_FROM_OPEN --ref node:p.script_copies
kpopper session read --revision REV_FROM_OPEN --ref edges:p.script_copies
```

Use IDs actually supplied by the project; `--id` also accepts a returned `node:ID` reference. Source handles and field selectors remain read operations. An exact ID selects a record, not a guarantee of relevance: prefer the original question or a faithful translation when its source is not yet known. An unresolved ID means only that the ID was not found; a question may still be covered under a different name. `--branch /known/p` is an optional hint from the current map. `--limit` accepts1–32 candidates (default8); `--tokens` budgets the returned text, including metadata and its final newline (default1,000 reference tokens). Results identify the candidate subset, any omitted whole hits and the root recovery route. If the budget cannot fit a complete response, raise it; records and identifiers are never clipped to fit.

Search results contain references, not evidence excerpts. Existing reads preserve complete original fields, source handles, current values and review snapshots; oversized reads return field routes or explicitly labeled exact-text fragments. A changed record or navigation profile invalidates the revision. Reopen before another search or read.

## Optional local E5

Install the optional runtime in the same Python environment as checked sessions:

```sh
python -m pip install 'kpopper[session,retrieval]'
```

Obtain these two files from the [pinned Xenova multilingual-e5-small revision](https://huggingface.co/Xenova/multilingual-e5-small/tree/761b726dd34fb83930e26aab4e9ac3899aa1fa78):

- `onnx/model_quantized.onnx`
- `tokenizer.json`

Place both directly in one local directory as `model_quantized.onnx` and `tokenizer.json`. Their pinned hashes and sizes are verified before loading. Other model variants require an explicit implementation change; a similarly named file is not accepted. Search performs no downloads or remote embedding requests.

```sh
kpopper session search --revision REV_FROM_OPEN --query 'recorded observations and unresolved conditions' --embedding-dir /path/to/e5 --search-mode hybrid
kpopper session serve --embedding-dir /path/to/e5
```

`semantic` ranks by E5 similarity; `hybrid` combines semantic and lexical ranks. Both prefer exact IDs. Without a configured/usable encoder, the response explicitly reports complete-record lexical fallback. Ordinary query text is enough: E5 prefixes are added internally. Translation or project-specific query wording belongs to the calling agent; it must preserve the request's constraints and uncertainty. The server itself never calls a language model or needs its credentials.

The encoder uses local CPU inference. Index-only overlapping chunks cover long documents through their final token, and the highest chunk similarity supplies the document's score. Chunks do not replace the original evidence returned by `read`. The current bounded scorer supports at most512 records,128KiB per indexed document,2MiB total indexed text and1,024 chunks. Exceeding a bound falls back explicitly for the entire record; no oversized record is silently excluded. Queries over512 encoder tokens also fall back without being truncated.

The derived index is cached in memory by document IDs and content. A persistent MCP server reuses it across searches; a new CLI process builds it again. Changed documents rebuild the complete index. No index files are written into the project, and no canonical field or relationship comes from similarity.

## MCP

The same bound server exposes:

```text
kpopper_search(query, revision, ids=[], limit=8, tokens=1000, branch=null, mode="hybrid")
kpopper_read(ref, revision, tokens=1600)
```

Configure the server with `--embedding-dir` to make E5 available. The search tool has the same behavior and source boundaries as the CLI. It is an optional discovery path alongside the complete map and exact reader. Lean continues to check the existing map and structured assertions; search similarity and an agent's prose are outside that proof boundary.
