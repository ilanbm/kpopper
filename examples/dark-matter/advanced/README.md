# The collection changes; the answer stays five

Continue the [six-paper research example](../README.md) with a finite collection
query, exact arithmetic and retained history. This is a separate `core/v1`
fixture; the legacy record and its captured walkthrough remain readable.

## What enters the query

The initial `studies` collection has five members: Rubin rotation, SPARC, the
radial acceleration relation (RAR), Bullet Cluster lensing and Planck. The rule
selects a member exactly when `in_review` is true **and** `domain` is `astronomy`.
Those two fields are authored review classifications, not quotations from the
papers or classifications inferred by the runtime. Every member names its
existing paper source and passage. The six paper URLs retain the versions used
by the original example; `read` dates are inherited from that prepared record,
not evidence that this exercise fetched the sources again.

The exercise later adds the existing LZ first-search paper as `study.lz`, with
`in_review: true` and `domain: laboratory`. It is now part of the captured scope,
but fails the astronomical selection criterion. The count remains **5** and the
selected IDs remain equal. The input count becomes **6** and the query's
computational basis changes: it binds the whole scope, including nonmatches.
The complete typed query response therefore changes even though its numeric
`result` does not.

The script compares the query response's exact numeric `result` separately from
its basis and scan counts. A query is a complete rule in this version; this
display projection is Python code, not a nested query expression. The recorded
judgment keeps the earlier value **and** basis in `seen`; adding a study does
not silently review it again.

```yaml
collection_scope:
  collection: studies
  fields: [domain, in_review, particle_identity]
```

`m.particle_identities` projects the last field over the whole collection. No
member supplies it, so the result stays `unknown`, with `value: null` and
diagnosed missing values. Missing evidence becomes neither an empty list nor a
claim that dark matter does not exist. An unknown membership would likewise
prevent `count` or `filter` from returning a complete answer.

## Run it

Use macOS or Linux with Python and a kpopper build that includes `query/v1`
(this checkout is 1.7.0). The installed package supplies the native evaluator;
no separate Lean setup, model call or paper download is needed.

From the repository root, in a virtual environment:

```sh
python -m pip install .
python examples/dark-matter/advanced/exercise.py
```

To retain the generated record, immutable history and complete captures in a
**new** directory:

```sh
python examples/dark-matter/advanced/exercise.py --output /tmp/dark-matter-query
python -m kpopper.cli assess m.astronomy_count m.particle_identities \
  --record /tmp/dark-matter-query/history/GROUNDING.yaml --history
```

For the compact agent/CLI exchange shown in the main README, install `jq` and run
this from the repository root after the exercise has created that directory:

```sh
kpop assess m.astronomy_count m.particle_identities d.review_scope \
  --record /tmp/dark-matter-query/history/GROUNDING.yaml --history |
  jq -f examples/dark-matter/advanced/assessment-summary.jq
```

The filter selects and names four fields from the CLI response. For a completed count,
`astronomy_studies_selected` shows the definite matches; otherwise it preserves the
reported unknown or error status. It performs no additional research reasoning.
The complete response retains the diagnostics and evidence. The full exercise runs
without `jq`.

The exercise first uses the supported `history migrate` CLI to import a copy of
the explicit core fixture. It records a qualitative judgment through `add`, then
captures a Snapshot. The later `add` publishes the LZ member through the same
history writer. Each write is a separate committed operation. It never migrates
the legacy example or changes the user's project routing.

New CLI processes read both stages through `assess --history` (schema v3).
The Python API calls `Snapshot.capture`, `to_json`, `from_json` and `Evaluator`.
After the later write, the exercise replays the saved earlier Snapshot with its
source directory temporarily unavailable. The earlier scope, result and basis
must all be recovered from retained data.

The printed JSON is a **script-produced projection** of those CLI and public API
responses, with checked comparisons; it is not raw CLI output. With `--output`,
`captures/` also retains the unabridged CLI responses, command arguments,
Snapshots and evaluator results. The retained Snapshot is sufficient for this
computation replay; keep `history/GROUNDING.yaml` and its `.kpopper/` together
for the complete record history. A Snapshot is not a complete history-store
backup.

### Captured result

[captured-output.json](captured-output.json) is the actual printed summary from
a successful run on macOS arm64 with kpopper 1.7.0 installed from this checkout.
Selected lines from that output:

```json
  "same_selected_members": true,
  "count_basis_changed": true,
  "qualitative_falsifier": "not_declared",
  "review_snapshot_unchanged": true,
```

The initial and replayed count basis is
`b230d6d0679ea7b64fcbd26c047ce42fd4351092657157a2d642317707b16ec9`;
after LZ it is
`93cdf31523c200e9606934f60e7261960e52c8b3b136649f9bbc20530cb378a0`.
The CLI assessment also reports `basis_comparison: changed` for the judgment's
two query premises. Its Planck ratio, framework and shared-data premises remain
`same`. This is a review signal, not a scientific verdict.

## Scientific limits

- **Five selected papers is a coverage count.** RAR uses SPARC; the count is not
  a count of independent datasets, confirmations or votes. The catalog, galaxy
  analysis, lensing interpretation and cosmological fit play different roles.
- **The framework remains explicit.** Standard gravitational mass inference,
  stellar mass-to-light choices and base Lambda-CDM shape the interpretations.
  The connected example retains Planck's lensing-amplitude tension.
- **The density ratio stays `75/14`.** It uses the same rounded Planck central
  values, with no propagated uncertainty and no new independent observation.
- **LZ constrains particular interactions.** The selected v4 first-search result
  is not a survey of later limits, a particle identification, or a test of every
  dark-matter explanation.
- **The judgment stays qualitative.** `d.review_scope` has a prose reason to
  revisit it. A changed basis calls for review; it neither rewrites that
  judgment nor proves or refutes the scientific synthesis.

See [record.yaml](record.yaml), the [later LZ member](later-study.yaml), the
[authored judgment](judgment.yaml), [query semantics](../../../docs/query.md)
and the [history contract](../../../docs/history-contract.md).
