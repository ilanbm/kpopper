# Contributing to kpopper

Contributions are welcome: bug reports, clearer documentation, reproducible examples,
host compatibility reports and code changes. You can contribute without installing an
agent plugin. Community participation follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## Find the right place

- Search [existing issues](https://github.com/ilanbm/kpopper/issues) before opening a new one.
- Use the [issue chooser](https://github.com/ilanbm/kpopper/issues/new/choose) for bugs,
  improvements and usage questions. Include a small example with invented data, the
  installed version and the host where it happened. Remove private records, source
  documents, credentials and personal information from attachments and logs.
- Follow [SECURITY.md](SECURITY.md) for a suspected vulnerability.
- Small fixes can go straight to a pull request. Discuss new commands, record fields,
  dependencies or substantial behavior changes in an issue first.

For a first contribution, look at
[good first issues](https://github.com/ilanbm/kpopper/issues?q=is%3Aissue%20is%3Aopen%20label%3A%22good%20first%20issue%22)
or improve an example you tried. Maintainers review scope and compatibility before merging;
there is no guaranteed response time.

## Local setup

The native Rust runtime is the primary implementation and release artifact. Install
Rust using the pinned toolchain, then build from `native/`. The Python environment
below is retained for the source-only compatibility adapter and its test suite.
Fork the repository on GitHub, then clone your fork (replace `YOUR-USERNAME`):

```sh
git clone https://github.com/YOUR-USERNAME/kpopper.git
cd kpopper
git remote add upstream https://github.com/ilanbm/kpopper.git
git switch -c my-change
rustup show
cd native
cargo build --locked --release
cd ..
python3 -m venv .venv
source .venv/bin/activate
python -m pip install -e ".[html]"
```

On Windows, create the retained Python environment with `py -m venv .venv` and activate
it in PowerShell with `.venv\Scripts\Activate.ps1`. Native Rust CLI and durable report
batching support Windows; the legacy Python compatibility path retains its POSIX file
locking limitation. Python followup/watch hooks also require POSIX; native Windows
delivery tests check their behavior directly.

| If you are changing… | Start here |
|---|---|
| The native CLI, reader or record writes | `native/src/`, `native/tests/` and the [reference](docs/reference.md) |
| kpopper Hub or Annotated Documents | Native application modules in `native/src/`; Python compatibility modules and assets in `scripts/page/` and `scripts/document/` |
| Agent guidance or integration | `skills/`, `hooks/` and [adapters](adapters/README.md) |
| An example or explanation | `examples/`, `docs/` and `README.md` |
| The optional checked-session runtime | `scripts/session/` and [checked sessions](docs/checked-sessions.md) |
| The experimental deterministic core | `scripts/reasoning/` and [core/v1](docs/reasoning-core.md) |

## Make and submit a change

`main` is what ships. Changes reach it through a pull request from your fork or a branch.
Keep changes focused and preserve compatibility with existing records. Add a regression
test when a bug fix or behavior change needs one; documentation-only changes need accurate
examples and working links.

For native changes, run the relevant Rust test target, then the native suite:

```sh
cd native
cargo test --locked
cd ..
```

See [native setup and resources](native/README.md) for tests that require the packaged
reasoning engines. The required CI matrix also installs and exercises each platform archive.

Run the relevant retained Python compatibility test module while working, for example:

```sh
KPOPPER_RUNTIME=python python -m unittest discover -s tests -p 'test_release.py'
```

Before submitting a code change, run the ordinary test suite and record checks:

```sh
KPOPPER_RUNTIME=python python -m unittest discover -s tests
# PowerShell: $env:KPOPPER_RUNTIME = 'python'; python -m unittest discover -s tests
kpop --frozen check
kpop --frozen consolidate --dry-run
kpop --frozen experimental hub --verify
```

If `.kpopper/view.yaml` changes because you updated the record, include that generated view.
Read the measurement recipes before running `kpop --frozen remeasure --run`; the
[measurement checks](#record-and-measurement-checks) below explain what executes.
Optional runtimes may skip tests locally. Changes to them need their documented setup and
the corresponding CI job; skips are not proof that the integration passes.

The retained Python compatibility suite exercises Python hooks and the legacy checked
session adapter. Set `KPOPPER_RUNTIME=python` for those runs. If a matching kernel is
cached from `kpop session setup`, some tests exercise the checked-session runtime.
Install its optional dependencies in the same environment before running that suite:

```sh
python -m pip install -e '.[session]'
```

Without a matching cached kernel, the integration tests skip. Having Lean on `PATH`
alone does not enable them. Set `KPOPPER_REQUIRE_CORE_TESTS=1` when testing this runtime
to make an unavailable kernel fail instead of silently skipping, as CI does.

For changes to that retained Python runtime, follow the full [checked-session setup](docs/checked-sessions.md)
and use the toolchain pinned in `scripts/session/lean/lean-toolchain`. Python 3.13 matches
the checked-session CI job. The base package's Python minimum does not imply that every
optional dependency supports that version.

The experimental `core/v1` profile uses a separate packaged native runtime. Its normal
execution needs no Lean compiler or checked-session setup. For changes to that core,
follow the [runtime build and validation guide](scripts/reasoning/native/README.md),
including the platform checks and third-party notices required when changing a bundle.

The finite-scope query extension is an additive module (`query/v1`) with KP4/KR4
transport and `resources/v4`; see [Query operations](docs/query.md). An additive
module change should preserve KP2/KP3 behavior and register its capability, protocol
and resource contract. A revision to an existing core module changes its compatibility
contract and needs the corresponding review. Keep authored facts, computed results,
proof claims and declared support separate in documentation. For query changes, the
focused conformance boundary is `python -m unittest tests.test_reasoning_query_runtime`;
run the relevant native validation from `scripts/reasoning/native/README.md` when the
packaged runtime changes. These checks do not activate query support on real records.

Then push and open a pull request using GitHub or the optional `gh` CLI:

```sh
git push -u origin HEAD
gh pr create
```

Describe the problem, the resulting behavior and how you checked it. Include a
`Bump: patch`, `Bump: minor` or `Bump: major` line; [Releasing](#releasing) explains the choices.
Feature pull requests leave version numbers and generated release notes to the release process.

When a change makes a lasting design decision, add it to `GROUNDING.yaml` with its reasons
and what would prompt reconsideration. Routine fixes need no new decision entry. The
[recording guide](skills/record/SKILL.md) describes the format; a maintainer can help with it.

## Record and measurement checks

Every pull request runs the skill/release contracts, CI selection tests, `kpop check`,
`kpop consolidate --dry-run`, `kpop remeasure --run` and `kpop experimental hub --verify` on
Python 3.13. It fails if `.kpopper/view.yaml` no longer matches the record it renders from.
The checkout remains GitHub's proposed merge result.

The mandatory contracts also check local file links and heading anchors in the four
root community documents, plus the structure and documentation links of the issue forms.
These checks run offline; remote-page availability and GitHub reporting settings still
need verification when preparing a public release.

The `changes` job selects the other checks from the entire PR's diff against its merge
base, including deleted files and both sides of renames. The selection and changed files
are printed in its log; the job summary lists the selected families.

| Change | Additional checks |
|---|---|
| Skills, Markdown documentation, the project's record, root security/community policies, issue/PR templates or `.gitignore` | None: mandatory contracts and record checks still run. |
| The CI selector or its regression tests | Selector and workflow-contract tests in the mandatory job; no runtime or native rebuild just for classification changes. |
| Standalone-document code, assets or tests | Document Python tests on 3.9 and 3.13, and the offline DOM suite. |
| Shared Python code, other tests/fixtures, adapters, hooks, packaging metadata, or the check/session workflows | Full Python suite on 3.9 and 3.13, document tests, session checks on three operating systems, and installed-distribution checks. |
| Native sources, build recipes, bundled runtimes, loader, notices or the distribution probe | All checks, including native compilation and modified-GMP replacement checks on five targets. |
| The native workflow, other CI configuration, an unclassified path, an empty diff or unavailable Git history | All checks. |

The selector lives in `.github/scripts/ci_selection.py`. Keep shared inputs broad and add a
regression case to `tests/test_ci_selection.py` when changing a classification. It does not
infer Python dependencies. A change to `pyproject.toml` verifies packaging and installation;
it does not by itself rebuild unchanged native sources.

Changes to the selector rely on maintainer review as well as its mandatory regression
tests. Reviewers must inspect every newly skipped family and the complete PR diff,
especially when the same PR narrows a classification and changes the affected files.
The tests validate declared cases; they cannot prove that a new classification covers all
dependencies. Keep uncertain paths on the full checks, or use manual dispatch for a full
audit before merging. Main's consumer checks run after merge and do not replace this review.

Every push to `main` runs all test families, with native compilation selected from the push
diff. Manual dispatch forces the complete native audit. New commits cancel older checks for
the same ref. Release and publish workflows keep their own cancellation policies.

`ci-required` runs even if another job fails or is skipped. It requires every selected job
to succeed and accepts skips only for unselected jobs. This is the aggregate status to use
when configuring branch protection; changing a workflow does not change repository rules.

The full Python suite uses the fixture record in `tests/fixtures/page`, which exercises
every field the reader and the page accept; a new field goes there first.

Standalone-document changes also run the offline UI suite with Node 22 or later:

```sh
pip install .
npm ci --prefix tests/document-support --ignore-scripts --no-audit --no-fund
npm run test:documents
```

Set `PYTHON` to the Python interpreter with the package dependencies when it is not
`python3`. These tests build real synthetic artifacts and execute their scripts in a
DOM model. They cover review/export behavior; native browser layout, sandbox/CSP
behavior and downloads require a separate permitted browser check.

The dry run lays the hypotheses beside the record over it and checks the result: red on a
contested id, a falsifier that holds, or a hole; a premise that moved under a judgment is green
and blocks only the fold. On a pull request the tree it runs on is the merge commit — the merged
tree — and the same steps run on every push to `main`, the second net for two pull requests that
were each consistent and contradict together.

`remeasure` takes every entry that names a recipe again from that tree. An entry whose value is
a fact about the tree says `measure: <name>`, and `.kpopper/measure.yaml` beside the record maps the
name to an argument list — `[python3, -I, -c, "..."]`, `[sed, -n, '...', a/file]` — the one file
whose content ever runs, reviewed as code in the pull request that edits it. A recipe runs once,
without a shell, with sixty seconds and 64 KiB of output, from the root of the checkout the
record sits in — for a record the tree cannot hold, from the record's own directory, beside the
allowlist; the plan says which, every time. The one line it prints is the value, and it must be
of the recorded value's kind: a plain number where the record holds a number, and text compared
exactly, so `001` is not `1`.

What differs from the record is laid over it as the hypothesis `tree/<commit>` through the same
dry run: red on a falsifier that holds on the measured value, a hole (a recipe the file lacks, or
one that fails), or a reading the tree contests — a reading of the same day that disagrees, which
the author corrects in the pull request; an older reading that moved is green, and the log carries
the `kpop set … --why "measured by …" --as-of <the day it was measured>` that refreshes it, or
says to edit the value by hand where no command carries it as it was measured. Nothing writes the
record but that command, run by a person or a session.

A hypothesis that replaces a measured entry carries the `measure:` line with it: the fold takes
the hypothesis's block over whole, so a replacement that says nothing about the recipe would drop
it and nothing would take that reading again — the step refuses that rather than going quiet.
Locally `kpop remeasure` prints the plan and runs nothing; add `--run` to reproduce a red step
at your keyboard. Name a recipe only where a command honestly takes the count the entry's `at:`
describes — `python3 -I` for the Python ones, so a file in the checkout cannot stand in for a
module they import.

Count what the `at:` scopes and no more: a recipe that swept a whole module for numbers
would answer a different question and correct a value against the judgment that reads it. A
recipe whose subject is the record reads the record's own text and imports nothing — a count
taken through the reader would agree with the reader by construction, and the isolation that
keeps a checkout file from standing in for a module also hides a package installed for you
rather than for the interpreter, so an importing recipe passes here and fails at your
keyboard.

Merges are squashed: one commit on `main` per pull request, subject taken from the
pull request title. The branch is deleted once it lands.

## Releasing

A feature pull request never touches the version. It says what it asks of the version instead,
in one line of its body that the template carries and the checks require:

```
Bump: minor
```

`patch` is a fix, a doc, tooling — nothing a user of the reader, the page or the hooks has to
learn. `minor` adds something — a command, a field the reader accepts, a computed name, a page
behaviour. `major` removes something or changes its meaning.

Every push to `main` refreshes one pull request, **Release x.y.z**, holding the native
version and plugin manifests plus a changelog entry. The legacy Python and npm version
files remain compatibility metadata; they do not define the native release number.
The release pull request records what merged since the last release, the bump each
declared, and the decisions the record gained. Merging it is the release; several merges
in a day fold into one release if nobody merges it in between. After it lands:

```
claude plugin update kpopper@kpopper
```

After updating plugin files, install the matching native runtime into the active plugin
copy using its printed installer command.

That same push tags the commit `vx.y.z` and opens a GitHub release carrying the changelog
entry and native bundles for all five targets, plus `SHA256SUMS`, built from that very
commit. The release assets are the supported user installation route. Existing PyPI/npm
artifacts remain legacy distributions; this workflow does not claim a native registry
publication. Only the commit that moves the version publishes, so a failed release is
made by rerunning its own run rather than by pushing again.

The pull request is opened by the workflow's own token, which runs no checks of its own, so
the script runs `kpop check` and `kpop experimental hub --verify` on the release tree before pushing
it. One repository setting must allow it, once: *Settings → Actions → General → Workflow
permissions → Allow GitHub Actions to create and approve pull requests*. Until then the branch
is pushed and the run says which command opens the pull request by hand.

## Keep main behind the guard

The rule lives in the checkout rather than on the remote, so it needs turning on once
per clone:

```
git config core.hooksPath .githooks
```

After that, `git push` aimed at `main` stops and hands back the branch commands. When a
direct push really is the right move — undoing a bad merge, say — `git push --no-verify`
goes through.

## The browser pass

`scripts/verify_page.js` opens a rendered page in Chrome and exercises the provenance
layer in both themes and under reduced motion — what `--verify` cannot reach. It installs with
every channel, but the driver that drives the browser ships with none of them, so it stays a
local step:

```
kpop experimental hub
npm i --no-save playwright-core
kpop experimental hub --checks .kpopper/build/page.html
```

`--checks` runs the copy of the checker that came with the reader, so it works the same from a
checkout and from an installed command. The driver is found in the `node_modules` beside the
page; point `NODE_PATH` at another one to use a project's own.

None of that is a substitute for looking. `kpop experimental hub --open` renders the page and opens it in
your own browser, and `--tree` lands on the tree. It has to be a real browser: the provenance
layer is all JavaScript, so a preview pane or a viewer that does not run the page's scripts shows
every word of it and none of its behaviour.
