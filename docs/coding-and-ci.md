# Add reasoning checks to CI

kpopper adds checks for the recorded reasons behind software changes. A branch can change
a premise that another branch's decision depends on, even when Git can combine the files
without a text conflict. The [README example](../README.md#coding-check-the-reasoning-behind-a-merge)
illustrates this with a request timeout and a checkout budget.

## Check the proposed merge result

Keep `GROUNDING.yaml` and any referenced record files available in the checkout. For a
separate project using the published package, a GitHub Actions workflow can start with:

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
      - uses: actions/setup-python@v5
        with:
          python-version: "3.13"
      - run: python -m pip install kpopper==1.2.0
      - run: kpopper check
      - run: kpopper consolidate --dry-run
```

Choose the package version deliberately when updating this workflow. The ordinary
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

The repository's own [check workflow](../.github/workflows/check.yml) also runs the test
suite, measurement recipes and page verification. Those steps are specific to its record
and page configuration; the two-command example above is the basic integration for another
project.

## Inspect another branch before merging

With the other branch or ref already available in the local repository:

```sh
kpopper consolidate --dry-run --from other-branch
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

## Connect recorded premises to the code

A code change that never updates a relevant reading may be invisible to the record. For
facts that can be measured reproducibly, attach a named `measure` recipe and define its
argument list in `.kpopper/measure.yaml`:

```sh
kpopper remeasure       # Inspect what would run
kpopper remeasure --run # Execute the reviewed recipes and test the resulting readings
```

Add the second command to CI only after those recipes are in place and reviewed as code.
It tests differences through the hypothesis mechanism and does not silently refresh the
canonical record. See [measurement details](reference.md#measurement) and
[the repository's contribution guide](../CONTRIBUTING.md).

Keep behavioral tests for the software itself. kpopper checks declared dependencies and
conditions; it does not infer intentions from code or decide arbitrary logical conflicts
between prose descriptions. The optional Lean session core is a separate mechanism and
is not required for these merge checks.
