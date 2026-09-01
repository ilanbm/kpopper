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

- **Every judgment states, in advance, what would make it wrong.** `wrong_if` is a predicate
  over the things the judgment declares it rests on, and the reader re-checks it —
  mechanically, not by remembering to.
- **Staleness fires on reality, not on the calendar.** Every judgment carries `seen`, a
  snapshot of what its dependencies held when it was last reviewed. Nothing stores a stale
  flag; drift is *derived* by comparison, so it cannot be forgotten, cleared by accident, or
  survive a revert. A date passing tells you nothing; the thing you relied on changing tells
  you everything.

And one boundary: **invalidation spreads automatically, but a re-derivation never applies
itself.** A change marks everything downstream of it; deciding what to do about that waits
for a person. A stale recommendation someone read beats a current one nobody did.

## What it installs

| | |
|---|---|
| `skills/kpopper` | The method. Loads when work will be revisited, or when resuming such work. |
| `skills/kpopper/PAGE.md` | The page reference — briefs, renderers, the tree. Read only when building a page. |
| `scripts/kpopper` | One entry point: `open · check · affects · pull · page`. |
| `scripts/provenance.py` | The reader underneath. Field names are inferred by shape, so it reads records written in any vocabulary. |
| `scripts/render_page.py` | The record as one self-contained page, three tabs, no dependencies beyond the reader. |
| `scripts/verify_page.js` | Browser checks for that page, in both themes. Playwright. |
| `hooks/` | A session opener and a stop gate. The opener runs `provenance.py open` when the project keeps a record, silent everywhere else; the gate bounces a session once, with the failures, if it tries to finish having left `check` worse than it found it. |

## The record

One file, one place: `PROVENANCE.yaml` at the project root — or a pointer to wherever the
content actually sits, including a mapping of several files; the reader follows it. It holds
three things: what was taken from a source (and where within it), what was worked out (the
rule, never the result), and what was concluded (with what would make it wrong).

```yaml
sources:
  msa:      {file: "contracts/acme-msa-2026.pdf", of: "2026-04-02"}

known:
  acme.seat_price: {v: 42, from: msa, at: "Enterprise tier", name: "Acme seat price"}
  acme.annual:     {rule: "acme.seat_price * acme.seats * 12"}

judgments:
  why_acme:
    rests_on: [acme.annual, beta.annual, acme.seats]
    verdict:  "prefer Acme above ~150 seats"
    wrong_if: "acme.seats < 150"
    seen:     {acme.annual: 90720, beta.annual: 84000, acme.seats: 180}
```

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
K=$(find ~/.claude -path '*kpopper/scripts/kpopper' | head -1)
"$K" open                        # what a session reads instead of the whole record
"$K" check                       # does the record still hold together
"$K" affects <entry>             # what a change reaches, through intermediate judgments
"$K" pull <entry|prefix>         # a subject's values with their sources - and what moved
                                 # since each judgment last looked
```

`check` exits non-zero on an undeclared gap: a dependency that is not an entry (unless the
judgment declares it missing with `blocked_on`), a dependency with no snapshot, a predicate
naming something undeclared, or prose sitting in a predicate field. A declared hole is a
note, not a failure — a build that stays red over an honest declaration teaches records to
stop declaring.

## The page — and the tree

```bash
"$K" page --open                 # the record as one page, in the browser
"$K" page --open --tree          # landing on the tree
"$K" page --verify               # deterministic checks, no browser
```

Three tabs. **Now** is the arrangement this session chose, written in a brief the page keeps
honest: it may order, it cannot drop. **Record** is everything, arranged by nothing. **Tree**
is the whole record as one growing thing: what was read from the world is the root fan below
the ground line, worked-out values branch above it, and conclusions blossom in a canopy —
the three node kinds in their three colors. Everything on every tab is the same live card:
hover or tap for where a value came from, click a dependency to walk to it, and on the tree,
opening a node lights the sap — the full chain of evidence feeding it — while its neighbours
stir like branches in wind. A card's `tree` button prunes the tree to that node's world; one
chip brings the whole tree back.

## The part that does not change

1. **Every entry declares what it rests on.** Something with no declaration does not go in.
2. **Store what produces an output, never the output.**
3. **Invalidation spreads automatically; re-running a judgment never applies itself.**
4. **A change to the shape is valid only with a migration that leaves the build green.**

Everything else — field names, sections, renderers — is open to revision. A record that can
rewrite its own rules will drift unless something in it is not up for revision. These four
are that floor.

## Installing into Claude Code

```bash
/plugin marketplace add /path/to/kpopper
/plugin install kpopper@kpopper
```

or non-interactively:

```bash
claude plugin install kpopper@kpopper --scope user
```

`--scope user` makes it available in every project on the machine; `--scope project` commits
it to the repo you are in. This repository is the only place the plugin is edited; every
installed copy is a read-only distribution.

## Requirements

Python 3 and PyYAML (`pip3 install pyyaml`). The browser checks additionally want Node with
`playwright-core` and a Chrome/Chromium binary (`CHROME=/path/to/chrome` when it is not on a
known path). Nothing else.

## What would show this was not worth it

The method promises exactly one measurable thing: the opening cost of session number N. If
after five sessions the sixth does not open cheaper, the method did not return what it cost,
and you can drop it with a clear conscience.
