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

Ordinary pull requests default to `linux-x86_64` for both tests and installed acceptance.
Windows inputs add Windows; macOS inputs add both macOS architectures. Shared platform
inputs (toolchains, dependencies, installers and packaging), unknown files,
and unavailable history select all five targets. Changed Rust files are read at both ends
of the diff: existing platform-conditional code keeps its platform coverage even if the
conditional itself was not edited. Unix or architecture-specific code takes every target.
A manifest whose only change is the package version keeps the Linux default; a simultaneous
dependency or configuration change still selects all platforms. The release PR needs no
special title or label to qualify for this rule.

Changes to CI selection, auditing and the Linux control workflow exercise the selected lanes
on Linux. A native workflow change also stays on Linux when only routing, scheduling or
artifact upload changes: the selector compares the platform jobs' execution inputs and
complete set of runner mappings at both revisions. Changed build commands, step conditions,
shells, environment, actions or runner mappings still select every target. An unreadable or
unrecognized native workflow also keeps the full matrix. Workflow syntax and routing
contracts are tested separately from executing the product on every platform.

Ordinary pushes to main run the record and contract checks alone. A commit that changes
`VERSION` runs every lane and platform, using the same release planner as publication.
The final `ci-required` job rejects failed or unexpectedly skipped checks, missing output,
and a release selection that omits any lane or platform. Manual full checks remain available.

Before provisioning expensive jobs, `changes` validates the committed native bundles,
their source identity and the corresponding-source archive. Native checks then run beside
the record job, without waiting for it; both must succeed. The record job runs the skill,
release, selection and native-plumbing contracts, then reads this repository's record with
the `kpop` built from the same tree. A push to main caches its build by the hash of `native/`;
a pull request restores that build and saves none. Installed acceptance uses the committed
bundles, cold runtime caches and no compiler on PATH.

Native builds cache pinned download archives and successfully tested GMP prefixes.
Cache keys include the target, source and recipe hashes, compiler/build tools,
Lean version, runner image and relevant build environment. Production and modified
replacement GMP have separate receipts. A prefix is reused only if its complete
file/link inventory matches; missing or corrupt receipts rebuild. The cache has no
partial-key fallback. Runtime compilation, proof/link audits, archive conformance
and installed replacement-library tests still execute. A cold cache therefore
retains the original GMP `make check` work; warm-run savings must be measured
separately from cold-run timings.

The native lane runs two jobs side by side, each with a leg per selected target. `tests`
builds and runs the native suite. `release` builds, accepts, packages and uploads the
program. Its final `verdict` requires both matrices to succeed, rejecting failure,
cancellation and unexpected skips. The standalone manual `distribution` mode remains
available for packaging diagnostics; it does not authorize publication.

Publication starts only after a successful `kpopper check` run triggered by a push to main
in this repository. It checks out that run's exact commit and downloads its distribution
artifacts and verified crate by run ID. It does not rebuild them. The native asset verifier
requires all five targets and checks their version, source commit, contents and hashes.
Thus a failed test, record check or installation prevents publication. Later main pushes do
not cancel a release being checked. If publication fails, rerun that publish run; if checks
fail, rerun the original check run. Both retain the original source commit. Artifacts are
kept for seven days; an expired artifact requires rerunning its check run before publication.

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
release build's, so each job of a check run restores only what its own
build uses. The key's prefix hashes the toolchain file and the native workflow, whose
changes make every artifact stale, and a restore never crosses it; its suffix hashes the
Cargo manifests, so a lockfile change starts from the nearest entry and rebuilds only what
changed. Only a job that succeeds on main saves its entry, after removing the package's own
artifacts, which every checkout rebuilds. Pull requests read main's entries.

To generate candidates before updating committed bundles, dispatch
`reasoning-runtime` with `candidate-only=true`. This explicit maintainer mode
builds all target candidates without claiming installed validation of the old
committed payload. Normal PR, main and release calls keep the integrity gate and
installed matrix. Changes to `reasoning-runtime.yml`, `reasoning-target.yml` or the builder must regenerate
`scripts/reasoning/native/gmp-source-and-build.tar.gz`, which carries their exact
source and build instructions.


The Linux native test job also executes the offline Annotated Documents DOM
suite with pinned Node dependencies. Its synthetic HTML and export validation
come from a test-only bridge compiled into the same native library test binary;
no Python reader is installed. The recovered interaction assertions cover review
choices, source navigation, focus, layout and saved copies. They model a DOM,
not a browser sandbox or pixel rendering. Run locally after `npm ci` in
`tests/document-support` with `python3 native/ci/document_ui.py`.
