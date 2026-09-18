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
and runs the existing unittest cases with pytest-xdist on up to four CPU-matched processes in one
runner. Tests from a file stay together. JUnit artifacts preserve individual test
outcomes and timings, and the log lists the slowest tests. Dependencies are pinned
to versions supporting Python 3.9; they are CI dependencies, not package dependencies.

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
