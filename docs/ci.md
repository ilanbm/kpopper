# CI selection and execution

Every pull request runs the record, skill/release contracts and selector tests. The other
checks are grouped into lanes, and `.github/scripts/ci_selection.py` declares what each lane
reads: the tracked files whose contents its checks open, and the directories whose entries
they list. A pull request runs a lane when its complete diff, including deletions and both
endpoints of a rename, changes one of those files, or adds or removes a file in one of those
directories.

| Lane | Jobs | Reads |
|---|---|---|
| `rust` | The command on each platform: compilation, its tests, the release build and installed acceptance | `native/`, the plugin's shell plumbing and host wrappers, the packaging tools, the installers, the runtime resources and the Lean sources |
| `runtime` | Rebuild the Lean/GMP reasoning runtime and exercise the fresh archive plus a replacement GMP library | Lean sources, runtime recipe and corresponding-source archive, native runtime integration test |

The declarations are checked, not trusted. On Linux, `.github/scripts/ci_audit.py` watches
every file the audited lanes open, through fanotify, which costs no measurable time. It maps
installed copies and bytecode back to the tracked source and fails the job if a lane read a
file, or listed a directory, that its declaration leaves out. A new dependency changes a file
the lane already reads, so the lane runs on that pull request and the audit reports the new
edge there, naming the file to declare. The recorder fails closed: it must see a canary read
before stopping, and a lost-event overflow fails the audit. A lane declared with
`enforced=False` reports undeclared reads as warnings without failing.

Every tracked file must be read by a lane or declared unread; the contract tests fail
otherwise, so a new directory is classified in the pull request that adds it. A path nothing
claims, a change to the CI selection or audit itself, an empty diff or unavailable history
selects every lane. The record job's own test modules and release scripts select no lane,
because the record job runs them on every pull request anyway.

Every push to main runs every lane on every platform, so a dependency that escaped the audit
is still caught after merge. A pull request leaves out Intel macOS (`darwin-x86_64`), the
slowest leg of the platform matrix. A pull request that changes what decides platform
behaviour - Cargo manifests, the build script, installers, packaging scripts, the committed
runtimes or the platform workflows - takes every target. The final `ci-required` job requires
successful completion of every selected lane and rejects missing output, an unexpected
platform scope and unexpected skips.

Before provisioning expensive jobs, `changes` validates the committed native
bundles, their source identity and the corresponding-source archive. All expensive
lanes also require the inexpensive record and contract job to succeed. That job runs the
skill, release, selection and native-plumbing contracts, then reads this repository's record
with the `kpop` built from the same tree. A push to main caches its build by the hash of
`native/`; a pull request restores that build and saves none, so only a pull request that
changes the native sources builds one, and that pull request checks the record with the
reader it changes. On main, all five targets are covered. The reasoning-runtime job checks
committed-bundle integrity and source correspondence, rebuilds fresh candidates, then validates
both the build-tree archive and the downloaded artifacts. Pull requests use the same platform
scope as the native lane; changes to runtime
platform inputs and main pushes cover all five targets; manual dispatch defaults
to all five and exposes the pull-request subset as an explicit choice.

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

The `tests` job runs every native test in one pool with nextest, which schedules the tests of
all the test binaries together, where `cargo test` runs the binaries one after another and a
binary holding one slow test leaves the other cores idle. nextest is pinned to one version in
the workflow, and each platform's archive is checked against the sha256 written there before
it is unpacked. The run never retries a test, because a test that passes on a second attempt
would turn a failing run green; `native/.config/nextest.toml` reports a test still running
after a minute and stops one after ten, so a hang fails under its own name. The job keeps the
run's JUnit report, with each test's outcome and duration, in its evidence. A standing step
compares nextest's listing with cargo's own, so the run covers exactly the tests that
`cargo test` runs. nextest does not run doc-tests: the crate has none, and a contract test
fails when one appears until the workflow also runs `cargo test --doc`.

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
builds and validates the candidate on each selected target while skipping the
committed-bundle preflight and the second validation after artifact download.
Full validation keeps the integrity gate and checks the matching downloaded
candidate again. The candidate tests run the native scalar, composition and query
corpus, then replace the extracted GMP with a separately built probe library and
check both its load marker and a large-integer arithmetic result. Changes to either
runtime workflow or the builder must regenerate
`scripts/reasoning/native/gmp-source-and-build.tar.gz`, which carries their exact
source and build instructions.
