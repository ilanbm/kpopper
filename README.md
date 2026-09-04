# kpopper

*Named for Karl Popper: nothing here is ever verified, only exposed to refutation. Every
judgment must say what would make it wrong — one that cannot be wrong is an opinion.*

The stores remember. This remembers **how you know.**

kpopper keeps an epistemic record for work that gets revisited: what was read from the world,
what was worked out from it, what was concluded — and what every conclusion is still standing
on. It is not another knowledge store, and it does not model what exists. A fact from
anywhere — a document, a database, another knowledge system — enters as an entry that says
where it came from; the record governs its life: who vouches for it, when it was last
checked, and what falls when it moves.

Two mechanics carry the whole method:

- **Every judgment states, in advance, what would make it wrong — or declares out loud why
  it cannot yet.** `wrong_if` is a predicate over the things the judgment declares it rests
  on. The reader refuses prose in its place, rejects a predicate that reads anything
  undeclared — and when the predicate is a plain comparison, evaluates it: a judgment whose
  own falsifier holds **fails the build**. Anything richer is surfaced beside exactly what
  moved, never guessed at.
- **Staleness fires on reality, not on the calendar.** Every judgment carries `seen`, a
  snapshot of what its dependencies held when it was last reviewed. Nothing stores a stale
  flag; drift is *derived* by comparison, so it cannot be forgotten, cleared by accident, or
  survive a revert. What it compares is the value the judgment used — re-reading a source is
  still a human act, and a value the record never re-read cannot drift.

And one boundary: **invalidation spreads — along what each judgment declared it rests on,
and through the rules of worked-out values — but a re-derivation never applies itself.** A
change marks everything downstream of it; deciding what to do about that waits for a person.
A stale recommendation someone read beats a current one nobody did.

A content hash over a source answers a different question than the one a conclusion needs:
it fires when a file is reformatted and stays silent when the number you relied on moves
somewhere the hash never covered. Matching bytes prove a file is unchanged — never that what
you concluded from it still holds. So kpopper snapshots the *value the judgment used*, not
the bytes it came from, and states the breaking condition in advance instead of waiting for
a checksum to notice.

## What it installs

The package is the command line: `kpopper`, with the reader and the renderer behind it. The
plugin is all of that plus the method and the session hooks:

| | |
|---|---|
| `skills/kpopper` | The method. Loads when work will be revisited, or when resuming such work. |
| `skills/kpopper/PAGE.md` | The page reference — briefs, renderers, the tree. Read only when building a page. |
| `scripts/kpopper` | One entry point: `open · check · affects · pull · set · add · review · consolidate · remeasure · same · distinct · page`. The dispatcher itself is `scripts/cli.py` — the same code the installed `kpopper` command runs. |
| `scripts/provenance.py` | The reader underneath. Field names are inferred by shape, so it reads records written in any vocabulary. |
| `scripts/render_page.py` | The record as one self-contained page, three tabs, no dependencies beyond the reader. |
| `scripts/verify_page.js` | Browser checks for that page, in both themes and under reduced motion. Playwright. Reached as `kpopper page --checks`. |
| `tests/` | The contract the reader and the page keep, run as `python3 -m unittest discover -s tests` against the fixture record in `tests/fixtures/page` — every field they accept, exercised once. |
| `PROVENANCE.measure.yaml` | The recipes the pull request takes this record's tree-facts with again — an argument list per name an entry cites with `measure:`, run by `kpopper remeasure --run` and by nothing else; what differs from the record is one more hypothesis through the dry run. |
| `hooks/` | A session opener and a stop gate. The opener runs `provenance.py open` when the project keeps a record — at its root, or registered with the checkout — and is silent everywhere else; the gate bounces a session once, with the failures, if it tries to finish having left `check` worse than it found it. |

## The record

One file, one place: `PROVENANCE.yaml` at the project root — or a pointer to wherever the
content actually sits, including a mapping of several files; the reader follows it. A project
whose tree cannot hold the file keeps it elsewhere and registers the path with the checkout
(one line in the git common dir; `kpopper where` prints it), and the opener and every command
find it from any worktree. It holds three things: what was taken from a source (and where
within it), what was worked out (the rule, never the result), and what was concluded (with
what would make it wrong).

```yaml
sources:
  msa: {file: "contracts/acme-msa-2026.pdf", of: "2026-04-02"}

known:
  acme.seat_price: {v: 42, from: msa, at: "Enterprise tier", name: "Acme seat price"}
  acme.seats:      {v: 180, from: msa, name: "Committed seats"}
  acme.annual:     {rule: "acme.seat_price * acme.seats * 12", name: "Annual Acme cost"}
  beta.annual:     {v: 84000, from: msa, name: "Annual Beta cost"}

judgments:
  why_acme:
    rests_on: [acme.annual, beta.annual, acme.seats]
    verdict:  "prefer Acme above ~150 seats"
    wrong_if: "acme.seats < 150"
    seen:     {acme.annual: 90720, beta.annual: 84000, acme.seats: 180}
  seats_fit:
    rests_on: [acme.seats]
    verdict:  "the committed seats fit one contract tier"
    wrong_if: "acme.seats > 500"
    seen:     {acme.seats: 180}
```

Drop the seat count to 120 and `check` fails with `why_acme: wrong_if holds (acme.seats <
150) - broken by its own condition`; `affects acme.seat_price` reaches `why_acme` through
the rule that computes the annual figure. The record does not wait to be asked.

Nothing is created on the first turn. The file appears when there is a first thing to put in
it, and structure is added only when something observable forces it. A project that never
grows past a single file is a correct outcome.

## Opening a session costs what moved, not what exists

With the plugin installed, every session in a project that keeps a record opens with the
record's own head — its name, its namespace, what needs a person, the open questions — inside
a fixed character budget. Measured on a live record of 107 entries: reading it whole costs
39,777 characters; the opener costs 948 on a clean record. The opener also re-runs after a
context compaction, which is exactly when a session most needs to be re-grounded.

The rest is pulled, never preloaded:

```bash
kpopper open                     # what a session reads instead of the whole record
kpopper check                    # does the record still hold together
kpopper affects <entry>          # what a change reaches, through intermediate judgments
kpopper pull <entry|prefix>      # a subject's values with their sources - and what moved
                                 # since each judgment last looked
kpopper where                    # the record this directory answers for
kpopper set | add | review       # change the record; the reply is what the write reached
                                 # (--hypothesis NAME writes beside the record instead)
kpopper consolidate [--dry-run]  # the record with its hypotheses laid over it, tested - then folded
kpopper same <a> <b>             # one subject under two ids: b retired into a
kpopper distinct <a> <b> "why"   # two that only look alike, kept apart for good
```

Each command reads the record in the current directory, or the files you name. Without the
command line on your path, the same dispatcher ships inside the plugin — find it once, then run
`"$K" open`. An installed plugin sits under a version directory and may hold worktrees of its
own, so the search skips those and takes the highest version rather than the first hit:

```bash
K=$(find ~/.claude -name worktrees -prune -o -path '*kpopper*/scripts/kpopper' -print 2>/dev/null | sort -V | tail -1)
```

`check` exits non-zero on an undeclared gap: a dependency that is not an entry (unless the
judgment declares it missing with `blocked_on`), a dependency with no snapshot, a predicate
naming something undeclared, prose sitting in a predicate field, or a plain-comparison
predicate that currently holds — a judgment broken by its own condition. A declared hole is a
note, not a failure — a build that stays red over an honest declaration teaches records to
stop declaring. So is a judgment decided on the session's own prior — a `prior.*` claim whose
value is the confidence — that names in `reopened_by` the sign a person would read to re-open
it. A dependency that moved since a judgment's snapshot is reported as `MOVED`
and does not fail the build either: it puts the judgment in front of a person, and it is
muted when the predicate names it and still evaluates false — moved, not across the line.

Several sessions and branches write the one record, and a write that contradicts what it holds —
the same id with a reading no newer than the base's, or a different verdict — is refused into a
**hypothesis**: `PROVENANCE.d/<name>.yaml` beside the record, in the record's own shape, which
every command reads over the base and nothing applies. `consolidate --dry-run` tests the record
with its hypotheses laid over it — premises re-checked, falsifiers evaluated, an id two of them
hold with different claims stopped, near-duplicates named for a person to judge — and
`consolidate` folds what the test leaves clean, `--refute` keeps a failed one as a negative
finding, and `--from <ref>` reads another branch's committed record the same way, pulled and
never pushed. Sameness is judged, not guessed: `add` names the nearest existing entries, and
`same` or `distinct` records the answer so the pair never returns.

## The page — and the tree

```bash
kpopper page --open              # the record as one page, in the browser
kpopper page --open --tree       # landing on the tree
kpopper page --verify            # deterministic checks, no browser
```

Three tabs. **Now** is the arrangement this session chose, written in a brief the page keeps
honest: it may order, it cannot drop. **Record** is everything, arranged by nothing. **Tree**
is the whole record as one growing thing: what was read from the world is the root fan below
the ground line, worked-out values branch above it, and conclusions blossom in a canopy —
the three node kinds in their three colors. Everything on every tab is the same live card:
hover or tap for where a value came from, click a dependency to walk to it, and on the tree,
opening a node lights the sap — the full chain of evidence feeding it — while its neighbours
stir like branches in wind. A card's `tree` button prunes the tree to that node's world; one
chip brings the whole tree back. The tab grows in the first time it is opened, and any node can
be pulled — it resists like a branch, stretches its limbs, and springs back when you let go,
because the page keeps nothing.

## The part that does not change

1. **Every entry declares what it rests on.** Something with no declaration does not go in.
2. **Store what produces an output, never the output.**
3. **Invalidation spreads automatically; re-running a judgment never applies itself.**
4. **A change to the shape is valid only with a migration that leaves the build green.**

Everything else — field names, sections, renderers — is open to revision. A record that can
rewrite its own rules will drift unless something in it is not up for revision. These four
are that floor.

## Installing

The command line on its own, for any project and any editor:

```bash
pipx install kpopper             # or: pip install kpopper
```

The browser checks come with it — they are Node, but they install where everything else does:

```bash
kpopper page --out record.html   # then: kpopper page --checks record.html
```

The Claude Code plugin — the method as a skill, the session opener, the stop gate, and the
same commands:

```bash
/plugin marketplace add /path/to/kpopper
/plugin install kpopper@kpopper
```

or non-interactively:

```bash
claude plugin install kpopper@kpopper --scope user
```

`--scope user` makes it available in every project on the machine; `--scope project` commits
it to the repo you are in. Other editors are wired up from `adapters/`.

Where the channels overlap they are the same files rather than copies of them: the packages are
mapped onto the plugin's `scripts/`, so the reader, the renderer, the dispatcher and the browser
checks have nothing to keep in step, and one version number covers all of them. There is a third
package, on npm, and it is that same mapping: it holds the name while an open question in the
record — whether the renderer, the command line and the checks move to TypeScript — is still open,
and it carries the browser checks rather than nothing, because they are the part that is already
Node. This repository is the only place any of it is edited; every installed copy is a read-only
distribution.

## Requirements

Python 3.9+ and PyYAML — `pipx` brings it along; a plugin-only install wants
`pip3 install pyyaml`. The browser checks additionally want Node 18+, a Chrome/Chromium
binary (`CHROME=/path/to/chrome` when it is not on a known path), and `playwright-core` — the
driver, which none of the packages ship: it is found beside the page when the project already
uses Playwright, and otherwise `npm i --no-save playwright-core` next to the page is enough.
Nothing else.

## What would show this was not worth it

The method promises exactly one measurable thing: the opening cost of session number N. If
after five sessions the sixth does not open cheaper, the method did not return what it cost,
and you can drop it with a clear conscience.
