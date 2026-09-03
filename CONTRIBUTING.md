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

Every pull request runs the tests, then `kpopper check` and `kpopper page --verify` on the
two Python versions the package claims to support, and fails if `PROVENANCE.view.yaml` no
longer matches the record it renders from — the record moved, the view did not. The tests
run against the fixture record in `tests/fixtures/page`, which exercises every field the
reader and the page accept; a new field goes there first.

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

The pull request is opened by the workflow's own token, which runs no checks of its own, so
the script runs `kpopper check` and `kpopper page --verify` on the release tree before pushing
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
kpopper page --out record.html
npm i --no-save playwright-core
kpopper page --checks record.html
```

`--checks` runs the copy of the checker that came with the reader, so it works the same from a
checkout and from an installed command. The driver is found in the `node_modules` beside the
page; point `NODE_PATH` at another one to use a project's own.

None of that is a substitute for looking. `kpopper page --open` renders the page and opens it in
your own browser, and `--tree` lands on the tree. It has to be a real browser: the provenance
layer is all JavaScript, so a preview pane or a viewer that does not run the page's scripts shows
every word of it and none of its behaviour.
