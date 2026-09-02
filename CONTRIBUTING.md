# Working on kpopper

`main` is what ships. Changes reach it through a pull request.

## The loop

```
git switch -c my-change
# work
git push -u origin HEAD
gh pr create
```

Every pull request runs `kpopper check` and `kpopper page --verify` on the two Python
versions the package claims to support, and fails if `PROVENANCE.view.yaml` no longer
matches the record it renders from — the record moved, the view did not.

Merges are squashed: one commit on `main` per pull request, subject taken from the
pull request title. The branch is deleted once it lands.

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
layer in both themes — what `--verify` cannot reach. It needs a driver the plugin does
not ship, so it stays a local step:

```
kpopper page --out record.html
npm i --no-save playwright-core
NODE_PATH="$PWD/node_modules" node scripts/verify_page.js record.html
```
