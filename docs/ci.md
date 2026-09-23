# CI selection and execution

Every pull request runs the record, skill/release contracts and selector tests. The other
checks are grouped into lanes, and `.github/scripts/ci_selection.py` declares what each lane
reads: the tracked files whose contents its checks open, and the directories whose entries
they list. A pull request runs a lane when its complete diff, including deletions and both
endpoints of a rename, changes one of those files, or adds or removes a file in one of those
directories.

| Lane | Jobs | Reads |
|---|---|---|
| `python` | Python suite, 3.9 and 3.13 shards | The package, tests, fixtures, skills, adapters, hooks and plugin manifests |
| `documents` | Document tests and the offline DOM suite | The package and the standalone-document sources and tests |
| `session` | Checked session with the reviewed Lean kernel | The package, its session tests and the Lean kernel |
| `installed` | Wheel, sdist and plugin installs on each native target | The package, everything the plugin carries, the committed runtimes |
| `runtime` | Rebuilding the reasoning runtime on each target | Lean sources, build recipe, bundled runtimes and notices |
| `rust` | Native command on each platform | `native/`, the assets it compiles in, the runtime resources and the setup that builds its Lean test program |

The declarations are checked, not trusted. On Linux, `.github/scripts/ci_audit.py` watches
every file the audited lanes open, through fanotify, which costs no measurable time. It maps
installed copies and bytecode back to the tracked source and fails the job if a lane read a
file, or listed a directory, that its declaration leaves out. A new dependency changes a file
the lane already reads, so the lane runs on that pull request and the audit reports the new
edge there, naming the file to declare. The recorder fails closed: it must see a canary read
before stopping, and a lost-event overflow fails the audit. A lane declared with
`enforced=False` reports undeclared reads as warnings without failing. The `installed` and `runtime`
lanes run reusable workflows that are recorded in the corresponding-source archive; they are
declared broadly and are not audited.

Every tracked file must be read by a lane or declared unread; the contract tests fail
otherwise, so a new directory is classified in the pull request that adds it. A path nothing
claims, a change to the CI selection or audit itself, an empty diff or unavailable history
selects every lane. The record job's own test modules and release scripts select no lane,
because the record job runs them on every pull request; the Python shards leave them out.

Every push to main runs every lane on every platform, recompiling the reasoning runtime only
when its sources changed, so a dependency that escaped the audit is still caught after merge.
A pull request leaves out Intel macOS (`darwin-x86_64`), the slowest leg of both the native
and the installed-package matrices. A pull request that changes what decides platform
behaviour - Cargo manifests, the build script, installers, packaging scripts, the committed
runtimes or the platform workflows - takes every target. The final `ci-required` job requires
successful completion of every selected lane and rejects missing output, incomplete test
plans, an unexpected platform scope and unexpected skips.

The selector emits an explicit list of retained Python compatibility suites: core,
documents, reasoning, session and other. The `python` lane runs all suites; the `documents`
lane alone runs the documents suite. New tests enter the other suite until classified. The execution
helper discovers importable test files, rejects an empty or unknown selection,
and splits individual unittest cases by measured duration with pytest-split.
Full PRs use eight Linux machines with two processes each for Python 3.9, and four
machines with four processes each for Python 3.13. A push to main uses 3.13 for these shards.
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
lanes also require the inexpensive record and contract job to succeed. That job first checks
the copied native/shared inputs, then reads this repository's record with the native `kpop`
built from the same tree. A push to main caches its build by the hash of `native/`; a pull
request restores that build and saves none, so only a pull request that changes the native
sources builds one, and that pull request checks the record with the reader it changes.
`check.yml` runs
`reasoning-target.yml` once per selected target, with the matrix of `reasoning-runtime.yml`;
selection lives in `check.yml` because both reasoning workflows are recorded in the
corresponding-source archive. Each target's build and installed tests depend only on that
target. On main all five native targets and the nine Python/platform compatibility
combinations are covered. Installed wheel, sdist and plugin checks still exercise
the retained Python distribution; native installed checks use the committed bundles,
cold runtime caches and no compiler PATH.

Native builds cache pinned download archives and successfully tested GMP prefixes.
Cache keys include the target, source and recipe hashes, compiler/build tools,
Lean version, runner image and relevant build environment. Production and modified
replacement GMP have separate receipts. A prefix is reused only if its complete
file/link inventory matches; missing or corrupt receipts rebuild. The cache has no
partial-key fallback. Runtime compilation, proof/link audits, archive conformance
and installed replacement-library tests still execute. A cold cache therefore
retains the original GMP `make check` work; warm-run savings must be measured
separately from cold-run timings.

The native lane runs two jobs side by side, each with a leg per target. `tests` builds and
runs the native test suite. `release` builds the release program, then accepts, packages
and uploads it in the same job, so a publish run ships only bytes that the job which built
them accepted. A pull request that changes no platform input builds and accepts the release
on `linux-x86_64` alone, while its tests keep every target but Intel macOS; main, a publish
run and a pull request that changes a platform input build and accept it on all five
targets. A final `verdict` job fails when either job failed, was cancelled, or was skipped
where the validation requires it: a publish run skips the tests and requires the release,
and every other validation requires both. Without it, a job skipped inside the native
workflow would leave `ci-required` green.

The native command's compiled Rust dependencies are cached per target and build profile.
The `tests` job restores and saves the test build's entry, and the `release` job the
release build's, so each job of a check run or a publish run restores only what its own
build uses. The key's prefix hashes the toolchain file and the native workflow, whose
changes make every artifact stale, and a restore never crosses it; its suffix hashes the
Cargo manifests, so a lockfile change starts from the nearest entry and rebuilds only what
changed. Only a job that succeeds on main saves its entry, after removing the package's own
artifacts, which every checkout rebuilds. Pull requests read main's entries.

To generate candidates before updating committed bundles, dispatch
`reasoning-runtime` with `candidate-only=true`. This explicit maintainer mode
builds all target candidates without claiming installed validation of the old
committed payload. Normal PR, main and release calls keep the integrity gate and
installed matrix. Changes to either native workflow or the builder must regenerate
`scripts/reasoning/native/gmp-source-and-build.tar.gz`, which carries their exact
source and build instructions.
