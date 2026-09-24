# Add reasoning checks to CI

kpopper adds checks for the recorded reasons behind software changes. A branch can change
a premise that another branch's decision depends on, even when Git can combine the files
without a text conflict. The [README example](../README.md#coding-check-the-reasoning-behind-a-merge)
illustrates this with a shared search cache and private results, followed by a download
promise that outlasts file retention. [Run both examples](../examples/merge-assumptions/README.md)
to see passing branch tests, clean Git merges and failed recorded conditions together.

Advanced mode is [experimental](advanced-mode-merging.md). These checks validate a
combined record; they do not resolve Git's textual conflicts. Parallel PRs can still
conflict on `GROUNDING.yaml` after one merges, and updating a PR can start its checks again.
The open design note explains this limitation and the alternatives under consideration.

## Check the proposed merge result

Keep `GROUNDING.yaml` and any referenced record files available in the checkout. For a
separate project using the released command, a GitHub Actions workflow can start with:

```yaml
name: reasoning-check

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  reasoning:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install kpopper
        run: |
          curl -fsSL -o install.sh \
            https://github.com/ilanbm/kpopper/releases/download/v0.10.0/install.sh
          sh install.sh --version 0.10.0 --prefix "$HOME/.local"
          echo "$HOME/.local/bin" >> "$GITHUB_PATH"
      - run: kpop check
      - run: kpop consolidate --dry-run
```

The installer downloads the archive matching the runner's platform and verifies its
SHA-256; nothing is built on the runner. Choose the version deliberately when updating
this workflow, and pin it so a new release cannot change a check's meaning without a
commit. The ordinary
`pull_request` checkout uses
[GitHub's proposed merge result](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#how-the-merge-branch-affects-your-workflow);
keep that behavior when the purpose is to test the combined record. A push check also checks the resulting `main`
after changes land. Configure the CI job as a required status check if it should block
merges; adding a workflow alone does not configure branch protection.

`check` is necessary even when consolidation is present: it checks the base record,
whereas consolidation tests proposed hypotheses and can have nothing to combine.
Neither command folds hypotheses or rewrites conclusions in this configuration.

| Finding | CI result |
|---|---|
| A supported `wrong_if` condition fires. | Failure. |
| The checked record has an undeclared structural gap or invalid reference. | Failure. |
| Consolidation finds competing claims for the same ID or a reading refused by the writer. | Failure. |
| A comparable premise moved and needs review, without a fired condition. | Reported; does not by itself fail CI. It blocks folding an affected hypothesis. |
| A condition is explicitly declared uncheckable or needs human interpretation. | Reported according to the record's declarations; not a proof that the decision holds. |

The repository's own [check workflow](../.github/workflows/check.yml) always runs measurement
recipes and page verification, and selects test families from the changed paths. Its
`ci-required` job checks that every selected family succeeded. See the
[contribution guide](../CONTRIBUTING.md#make-and-submit-a-change) for selection and main-branch coverage.
Those steps are specific to this repository; the two-command example above is the basic
integration for another project.

## Inspect another branch before merging

With the other branch or ref already available in the local repository:

```sh
kpop consolidate --dry-run --from other-branch
```

This reads the branch's committed record and its hypotheses, compares claims by ID, and
tests proposed changes over the current record. It shows affected decisions, changed
premises, broken conditions, disputed readings and possible duplicates. It neither merges
Git branches nor applies the proposed record changes.

This is a directional proposal check against the current record, not a general three-way
semantic merge algorithm. Record dates and the writer's replacement rules matter. A claim
missing from the other branch is not automatically a deletion request, and similarity
between IDs is a review prompt, not proof that they denote the same subject. Check the
actual merged tree in CI as well.

## Resolve a stopped Git merge locally

When an ordinary Git merge has stopped on the record, first resolve and stage any other
conflicted paths. Then inspect and apply the record resolution:

```sh
kpop consolidate --resolve --dry-run
kpop consolidate --resolve
# Review the result, then use git add and finish the existing Git merge.
```

An optional record path selects a different in-tree entry. This is an explicit local
command, not an installed hook, merge driver or hosted service. It never fetches, stages,
commits, pushes, changes the configured project mode or folds pending findings. Check
`kpop consolidate --help` before using it with an older installation.

For ordinary records, it compares complete entries against Git's stage-1 common base.
Independent additions, edits and deletions are combined while selected source blocks,
comments and line endings remain intact. Two different changes to the same entry,
delete/edit, or competing metadata changes require an explicit decision. It does not
combine fields inside a claim or choose a newer-looking date. For supported active-history
records, it rebuilds from the complete staged history only when both input views are known
generated views and the history reducer reports no contested subject. Immutable objects
and manifests remain untouched; unknown hand edits require explicit reconciliation.

The candidate is checked in a private copy of the staged tree with frozen `check` and
`consolidate --dry-run`. Nothing is written if those checks fail. On success, only the
working-tree record is replaced; the index still requires `git add` after review. This
checks recorded knowledge, not code behavior: it runs no measurement recipes, hooks,
project scripts or tests. Run the relevant validation on the final merge as usual.

This first resolver is deliberately bounded. It requires a normal two-parent merge with
Git's `AUTO_MERGE` snapshot and three regular-file index stages for the record. It refuses
manual edits made to the conflict file after that snapshot, add/add or delete/edit of the
whole record, pointer records, unsupported history formats, symbolic links or submodules
in the staged snapshot, and an alternate Git index. Ordinary entries need block collections
with plain keys. Snapshot capture is bounded to 100,000 index entries, 256 MiB of blob
output and 256 MiB of materialized files. Concurrent changes to the index, merge heads or record cause refusal before the
write. These limits are reported, never silently bypassed.

The [broader merge question](advanced-mode-merging.md) remains open: resolving a conflict
still creates work for the user, and a subsequent commit may trigger CI again.

## Connect recorded premises to the code

A code change that never updates a relevant reading may be invisible to the record. For
facts that can be measured reproducibly, attach a named `measure` recipe and define its
argument list in `.kpopper/measure.yaml`:

```sh
kpop remeasure       # Inspect what would run
kpop remeasure --run # Execute the reviewed recipes and test the resulting readings
```

Add the second command to CI only after those recipes are in place and reviewed as code.
It tests differences through the hypothesis mechanism and does not silently refresh the
canonical record. See [measurement details](reference.md#measurement) and
[the repository's contribution guide](../CONTRIBUTING.md).

Keep behavioral tests for the software itself. kpopper checks declared dependencies and
conditions; it does not infer intentions from code or decide arbitrary logical conflicts
between prose descriptions. The optional Lean session core is a separate mechanism and
is not required for these merge checks.
