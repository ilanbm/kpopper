# CI selection and execution

Every pull request runs the record, skill/release contracts and selector tests.
`.github/scripts/ci_selection.py` examines the complete PR diff, including deletions
and both endpoints of a rename. Unknown inputs or unavailable history select all
families. The final `ci-required` job requires successful completion of every
selected family and rejects missing output, incomplete test plans and unexpected
skips.

The selector emits an explicit list of Python suites: core, documents, reasoning,
session and other. Shared runtime changes retain all suites; document-only changes
select documents. New tests enter the other suite until classified. The execution
helper discovers importable test files, rejects an empty or unknown selection,
and splits individual unittest cases by measured duration with pytest-split.
Full PRs use eight Linux machines with two processes each for Python 3.9, and four
machines with four processes each for Python 3.13. A push to main checks 3.13 only.
Document-only selections use one machine per interpreter. At most twelve Python
jobs run concurrently, leaving capacity for the native and session checks.
pytest-xdist uses work stealing inside each machine so a slow file cannot hold an
entire group on one process. Dependencies are pinned to versions supporting Python
3.9; they are CI dependencies, not package dependencies.

`.github/test-durations.json` contains the maximum observed per-test durations
across both interpreters from [run 35345933376](https://github.com/ilanbm/kpopper/actions/runs/35345933376).
The 3.9 sample includes two CLI timeout failures; these are scheduling weights,
not claims of successful execution. New tests receive the average duration and
are still selected. Refresh weights from complete JUnit reports when the suite
changes materially; weights never decide whether a test is required.

JUnit artifacts preserve outcomes and timings. The log names individual tests
and reports active tests with elapsed time every 30 seconds. Each worker reports
its full and selected collection, and each machine retains an execution manifest.
After all jobs succeed, `ci-required` checks that workers agreed, every expected
group exists, and the groups executed the complete collection exactly once for
each interpreter. Missing manifests, inconsistent discovery, duplicates and
unexecuted tests fail the gate.

Before provisioning expensive jobs, `changes` validates the committed native
bundles, their source identity and the corresponding-source archive. All expensive
families also require the inexpensive record and contract job to succeed. The native
workflow repeats that inexpensive gate for direct callers. It dispatches one
reusable workflow per target: its build and installed tests depend only on that
target. All five native targets and the existing nine Python/platform installation
combinations remain covered. Installed wheel, sdist and plugin checks still use
the committed payload, cold runtime caches and no compiler PATH.

Native builds cache pinned download archives and successfully tested GMP prefixes.
Cache keys include the target, source and recipe hashes, compiler/build tools,
Lean version, runner image and relevant build environment. Production and modified
replacement GMP have separate receipts. A prefix is reused only if its complete
file/link inventory matches; missing or corrupt receipts rebuild. The cache has no
partial-key fallback. Runtime compilation, proof/link audits, archive conformance
and installed replacement-library tests still execute. A cold cache therefore
retains the original GMP `make check` work; warm-run savings must be measured
separately from cold-run timings.

To generate candidates before updating committed bundles, dispatch
`reasoning-runtime` with `candidate-only=true`. This explicit maintainer mode
builds all target candidates without claiming installed validation of the old
committed payload. Normal PR, main and release calls keep the integrity gate and
installed matrix. Changes to either native workflow or the builder must regenerate
`scripts/reasoning/native/gmp-source-and-build.tar.gz`, which carries their exact
source and build instructions.
