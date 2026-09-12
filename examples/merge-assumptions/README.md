# Two green PRs, exercised

These fictional examples reproduce the README's search-cache and download-promise
illustrations. Each starts from one shared base. Two branches change different parts
of the project; their tests pass separately and after a clean Git merge. A recorded
decision's condition fails when the merged inputs are measured.

## Run

With Git, Python 3.9+ and the repository's Python dependencies installed:

```sh
python3 examples/merge-assumptions/run.py
```

The runner creates disposable local Git repositories, commits the base and two
branches, runs their tests, merges them and invokes the real kpopper CLI. It uses
no network and does not change this checkout. Each recipe uses the same Python
interpreter as the runner. The temporary repositories are removed when it finishes.

A successful run exits zero after confirming that both examples have:

1. Passing branch tests and measurement checks in PR A and PR B separately.
2. A Git merge without a text conflict, with both branch test suites still passing.
3. A failing `remeasure --run` result naming the recorded condition.
4. A separate integration probe that detects the same behavioral problem.

That last probe matters: an ordinary integration test can catch these problems too.
The examples expose gaps in their branch tests; they do not establish that tests
cannot express the dependency or that kpopper finds assumptions nobody recorded.

## Read the branches

Each case has a `base/` directory and `pr-a/` and `pr-b/` overlays. An overlay contains
only files that branch changes or adds. Paths inside records and recipes resolve
from the assembled example repository, not from the overlay directory alone.

### Search cache

- [Base search](cache/base/search.py) returns public projects only. In this example,
  everyone making the same query sees the same public results.
- [PR A](cache/pr-a/search.py) enables private projects, filtering them by owner.
  Its tests exercise the search function's owner and non-owner behavior.
- [PR B](cache/pr-b/cache.py) caches the result by query alone. Its test uses public
  results and checks that the same query is served once across users.
- [PR B's record](cache/pr-b/GROUNDING.yaml) preserves why sharing was acceptable:
  `search.results_public`. The condition is `search.results_public == false`.

The [measurement recipe](cache/base/.kpopper/measure.yaml) uses
[measure.py](cache/base/measure.py) to read the explicit visibility switch without
importing application code. That switch controls the example's search behavior;
the recipe is not a general analysis of access control or a proof of privacy.

After the merge, Alice's cached private result can be returned to Bob. The runner
reproduces that leak with a fresh process and the same query for both users. The
branch tests still pass because they exercise access control in the uncached search
function and cache reuse with public fixtures separately. The missing combined
test could be added to the application's suite.

### Download promise

- [The base policy](downloads/base/storage-policy.yaml) retains exports for 30 days;
  [the base email](downloads/base/download-email.html) promises seven days.
- [PR A's policy](downloads/pr-a/storage-policy.yaml) reduces retention to seven days.
- [PR B's email](downloads/pr-b/download-email.html) promises 30-day downloads.
- [PR B's record](downloads/pr-b/GROUNDING.yaml) requires
  `exports.retention_days >= downloads.promised_days`, expressed as the breaking
  condition `exports.retention_days < downloads.promised_days`.

The [recipes](downloads/base/.kpopper/measure.yaml) read both the YAML policy and the
duration written in the HTML email template. After merging, storage removes a file
while its email still promises availability. The runner checks that the file is
unavailable during that promised window. The branch tests validate storage behavior
and the email's link and duration separately; the integration probe connects them.

## Why measurement, rather than check alone?

PR A deliberately leaves the stored reading unchanged. On the merged tree, plain
`kpopper check` therefore still passes: it evaluates the record it has.
`kpopper remeasure --run` runs the declared recipes and tests their changed readings
as a hypothesis. The output includes:

```text
FIRED     search.shared_cache: wrong_if holds (search.results_public == false) - broken by its own condition
FIRED     downloads.availability: wrong_if holds (exports.retention_days < downloads.promised_days) - broken by its own condition
```

Measurement exits nonzero for each combined case. It does not silently refresh the
canonical record or decide how to fix the application. The runner verifies that the
record and checkout remain unchanged by these checks.

The dependency and recipe still have to be chosen and maintained. These are small,
published demonstrations, not blind evaluations of agent performance or reports of
real incidents. See [CI setup](../../docs/coding-and-ci.md), or try a
[Cowork example where the change needs judgment](../cowork-workshop/README.md).
