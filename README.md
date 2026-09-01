# kpopper

*Named for Karl Popper: nothing here is ever verified, only exposed to refutation. Every
judgment must say what would make it wrong — one that cannot be wrong is an opinion.*

Work that gets revisited needs one thing sessions usually throw away: **where each piece came
from.** This plugin keeps that while the work happens — not as a separate modeling step, and
without changing what gets produced.

It is epistemic bookkeeping: the subject is not the work but the state of knowledge about the
work. Its nearest formal relative is a truth-maintenance system — `rests_on` is a justification,
and a changed input invalidates everything downstream of it — and what it adds is Popper's
asymmetry, that a judgment is never confirmed, only left standing. That makes the record
self-evolving in one exact sense: nothing rewrites your conclusions, but every judgment
re-checks itself against what it was last checked against, so staleness is derived rather than
remembered.

## What it installs

| | |
|---|---|
| `skills/kpopper` | The method. Loads when work will be revisited, or when resuming such work. |
| `scripts/provenance.py` | A reader that checks the record and walks it. Python 3 + PyYAML. |
| `scripts/render_page.py` | The record as one self-contained HTML page, in two tabs. |
| `scripts/verify_page.js` | Browser checks for that page, in both themes. Playwright. |
| `hooks/` | A session opener. Runs `provenance.py open` when the project keeps a record; silent everywhere else. |

## The record

One file, one place: `PROVENANCE.yaml` at the project root — or a pointer to wherever the content
actually sits. It holds three things: what was taken from a source (and where within it), what was
worked out (the rule, never the result), and what was concluded (with what would make it wrong).

Nothing is created on the first turn. The file appears when there is a first thing to put in it,
and structure is added only when something observable forces it. A project that never grows past
a single file is a correct outcome.

## The reader

```bash
R=$(find ~/.claude -path '*kpopper/scripts/provenance.py' | head -1)
python3 "$R" open                # what a session reads instead of the whole record
python3 "$R" check
python3 "$R" affects <entry>
```

`open` is what a new session runs first: how big the record is, then only what needs a
person — ranked, cut to a budget, and honest about what it left out. On a clean record it
prints one line. That, and not the file, is the opening cost the method is trying to lower.
Once the plugin is installed this runs by itself at session start, following a root file
that is a pointer to wherever the record actually lives.

`check` fails the build on a dependency that is not an entry (unless the judgment declares it
missing), a dependency with no snapshot, a predicate naming something undeclared, or prose
sitting in a predicate field. `affects` reports
what a change reaches, including through intermediate judgments, and whether each one can now be
re-evaluated or is only flagged.

It infers roles from **shape**, not field names, so it reads records written in any vocabulary.
Where two fields genuinely fit one role it refuses to guess and asks for a three-line `schema:`
block rather than silently picking one.

## The page

```bash
python3 "$(dirname "$R")/render_page.py" > record.html
python3 "$(dirname "$R")/render_page.py" --verify
```

Hover any key for where the value came from, click to pin, click a dependency to walk to it,
Escape to step back. Reading up toward a source and down into what a judgment rests on are the
same gesture, so one mechanism serves both.

**Record** is the tab nobody writes: everything, arranged by nothing but the record's own shape.
**Now** is the tab the session writes — an arrangement aimed at what this session is for, in a
brief beside the record (`<record>.view.yaml`). Intent is the one input that is not in the record
and dies with the conversation, which is why it cannot ship and cannot be derived.

Two properties keep that arrangement honest, mechanically rather than by memory: it may reorder
but it cannot drop — anything flagged that no section claimed lands in a trailing section the
brief cannot switch off — and its sections hold selectors rather than frozen lists, so they fill
themselves as the record moves. The brief also records the record's *shape* when it was written,
and a shape that moved raises a banner: a date changing does not make a layout wrong, but a
fourth blocked judgment might.

## Requirements

Python 3 and PyYAML (`pip3 install pyyaml`). Nothing else.

## Installing into Claude Code

Installing to a Claude account does not put the plugin into the Claude Code CLI. For that, add
this directory as a marketplace and install from it:

```bash
/plugin marketplace add /path/to/kpopper
/plugin install kpopper@kpopper
```

or non-interactively:

```bash
claude plugin install kpopper@kpopper --scope user
```

`--scope user` makes it available in every project on the machine; `--scope project` commits it
to the repo you are in.
