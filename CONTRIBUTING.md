# Working on kpopper

`main` is what ships. Changes reach it through a pull request.

## The loop

```
git switch -c my-change
# work
python3 -m unittest discover -s tests
git push -u origin HEAD
gh pr create
```

Every pull request runs the skill/release contracts, CI selection tests, `kpop check`,
`kpop consolidate --dry-run`, `kpop remeasure --run` and `kpop page --verify` on
Python 3.13. It fails if `.kpopper/view.yaml` no longer matches the record it renders from.
The checkout remains GitHub's proposed merge result.

The `changes` job selects the other checks from the entire PR's diff against its merge
base, including deleted files and both sides of renames. The selection and changed files
are printed in its log; the job summary lists the selected families.

| Change | Additional checks |
|---|---|
| Skills, Markdown documentation or the project's record | None: their contracts and record checks already run. |
| Standalone-document code, assets or tests | Document Python tests on 3.9 and 3.13, and the offline DOM suite. |
| Shared Python code, other tests/fixtures, adapters, hooks, packaging metadata, or the check/session workflows | Full Python suite on 3.9 and 3.13, document tests, session checks on three operating systems, and installed-distribution checks. |
| Native sources, build recipes, bundled runtimes, loader, notices or the distribution probe | All checks, including native compilation and modified-GMP replacement checks on five targets. |
| The CI selector, native workflow, other CI configuration, an unclassified path, an empty diff or unavailable Git history | All checks. |

The selector lives in `.github/scripts/ci_selection.py`. Keep shared inputs broad and add a
regression case to `tests/test_ci_selection.py` when changing a classification. It does not
infer Python dependencies. A change to `pyproject.toml` verifies packaging and installation;
it does not by itself rebuild unchanged native sources.

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

Every push to `main` refreshes one pull request, **Release x.y.z**, holding the version files
(`pyproject.toml`, `package.json`, `plugin.json`, `marketplace.json`) and a changelog entry: what merged
since the last release, the bump each declared, and the decisions the record gained. The
version is the largest declared bump. Merging that pull request is the release; several merges
in a day fold into one release if nobody merges it in between. After it lands:

```
claude plugin update kpopper@kpopper
```

That same push tags the commit `vx.y.z` and opens a GitHub release carrying the changelog
entry, with the `.whl` and `.tar.gz` built from that very commit attached. Those two files
are what an upload to PyPI should use: the tag, the text and the files all come from one
tree, which is not true of anything built by hand afterwards. Only the commit that moves the
version publishes, so a release that failed is made by rerunning its own run rather than by
pushing again.

The pull request is opened by the workflow's own token, which runs no checks of its own, so
the script runs `kpop check` and `kpop page --verify` on the release tree before pushing
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
kpop page
npm i --no-save playwright-core
kpop page --checks .kpopper/build/page.html
```

`--checks` runs the copy of the checker that came with the reader, so it works the same from a
checkout and from an installed command. The driver is found in the `node_modules` beside the
page; point `NODE_PATH` at another one to use a project's own.

None of that is a substitute for looking. `kpop page --open` renders the page and opens it in
your own browser, and `--tree` lands on the tree. It has to be a real browser: the provenance
layer is all JavaScript, so a preview pane or a viewer that does not run the page's scripts shows
every word of it and none of its behaviour.
