---
name: ground
description: "Read the project's knowledge record before answering from memory or from the files. Use before answering about the project's state, a number, a date, a decision, a source, a deadline or what was agreed - even a casual 'what's the situation with X' or 'is that quote still current' - because the recorded answer, with its source, beats a fresh search of the tree; before changing a value the record may hold; and whenever the session opener or a grounding line names entries. Runs pull, affects and check, reads what hypotheses and other branches propose, and searches when a question maps to no name. A missing id is not absence. Not for showing the record's page (the page skill). Requests come in any language."
---

# Ground

When mentioning this skill to the user, include the plugin name: `kpopper:ground` or "ground from the kpopper plugin". Use the user's language and fold it into the explanation of the action; no extra announcement is needed.

The record answers two questions a session cannot answer reliably in its head: what is known here now, with its sources, and what a change reaches. Ask it before restating a value, a date or a decision from memory, and before changing one. The command line is `kpopper` where it is on PATH; otherwise the `command` in the `KPOPPER_AGENT_CONTEXT` line the session opener printed (`python3 <plugin>/scripts/cli.py`) runs the same code. Do not guess a path and do not write a second reader - [the method's reference](../kpopper/references/method.md#finding-the-reader) says how to find the installed copy when neither is at hand.

This method owns one known entry point: **`GROUNDING.yaml` in the project's working directory** - or
`PROVENANCE.yaml`, the name records were born under before, read wherever it already is and never
created again. What the record keeps beside itself - hypotheses, the brief, the recipes - sits in
`.kpopper/` next to the entry file, a dot-directory a plain `ls` or `rg --files` skips: list it as
`ls -a .kpopper` or `rg --files --hidden`, and never conclude from a listing that skipped it that
there are no hypotheses. A record moved by half, its files left under the earlier names, fails
`check` and is named in the opener's head.
Git is optional. Use the location resolved by the opener or `kpopper open --json`: it checks
the current directory and ancestors within the workspace boundary, then Git's registered
external location. Do not start a duplicate because the file is outside the current directory.

**If it exists:** use it as the durable record of claims, grounds and review state. Check each
claim's source and scope; the record can itself be stale, and newer evidence can overturn it.
Add to it in the shape it already uses.

**Ask it what moved; do not read it whole.** Reading the record whole costs its full size on
every session, and almost none of it moved since the last one.

**One command opens the session:** `provenance.py open`. It prints the record's own head —
what this project is, where the record actually lives if this file is a pointer — and then
only what needs a person: ranked, cut to a budget, and honest about how many it left out.
On a clean record that is a few lines. The plugin runs this by itself at session start; if
a project with a record shows no such report, run it by hand.

**When the hook already supplied a checked view with `revision=...`, reuse it.** Expand the
relevant branch and read the claim through `kpopper_read` or the `session read` command named
by the hook. Do not replace that view with a second legacy opening. Keep current recorded
values separate from `seen`, preserve declared conflicts, and distinguish an executable
condition from unevaluated prose. A checked record field is not a proof of world truth or
permission to act. The optional transport is described in `docs/checked-sessions.md` at the
plugin root.

**What opening shows about the project is not the record's work.** Opening can surface a
branch behind its base, a red build, a stale environment. Say so in one line and write the
record with the world as it is — then offer the repair as its own piece of work, in the open.
Folding a repair into the turn that records it hides both from the person watching: they
cannot tell bookkeeping from a change to their project, and the repair's minutes read as
the method's cost.

`open` also prints **what the record holds** — the namespace, not the values: `mtg (11) ·
pay (4) · c50 (9) · …`. Use a known namespace directly; otherwise discover candidates
with the checked-session search below or local source search.
A record kept under letters says what they stand for in its head (`meta.prefixes`), and the
opener prints that legend beside the namespace - `prefixes: d=decision · m=measurement` - as
the page's namespace bar shows the words.

**A second command runs when the work starts, not before:** `provenance.py affects <seed>` for
what a change reaches, or `provenance.py pull <seed>` to ground yourself on the subject itself —
where the seed is the prefix or entry the question maps to. Their first message is the seed,
which is why this cannot be folded into the first command — it does not exist yet when the
session opens. If they only said hello, it never runs at all.

**If a question does not map to a name, the lookup is incomplete.** Names and topic labels
do not establish that the subject is absent. When checked sessions are configured, use
`kpopper_search` at the opening revision to find candidate references, then read their original
fields and declared premises. Search is discovery, not evidence or permission. Optional local
embeddings and the CLI equivalent are described in `docs/retrieval.md` at the plugin root.
When evidence is still missing, continue a returned `next_cursor` with the same search arguments;
one page is not a coverage boundary. Use `kpopper_context` for explicit support or impact reads
around relevant IDs. Read its omitted values and incident-edge frontier as needed, and keep
global search available for unlinked qualifications. Declared paths do not prove claims.
For retained reports and local UTF-8 source passages, use `kpopper search "terms" --chars 4000`;
[RETRIEVAL.md](../kpopper/RETRIEVAL.md) covers scopes and exact source reads. Its `search-corpus`
revision is distinct from a checked-session revision. Preserve source-language names and
try alternate terms when needed; the search does not translate or infer relationships.
Expand relevant branches or inspect source
content before claiming that the record does not cover it. A missing exact ID establishes
only that the ID is absent. If relevant evidence remains unfound, say so and continue the
discovery without inventing a matching claim.

While the record is small enough that you would happily re-read it every session, just read
it; this buys nothing. From the moment you would not, it is the difference between an opening
cost that grows with the record and one that grows with what moved — and that difference is
the only thing this method promises to measure.

## The read commands

Read in the project's mode: Simple shares one graph and named hypotheses; Advanced preserves the branch's code-world and shows project contributions alongside it, including with one checkout.
Ordinary and checked reads show pending source, scope and publication state. Pending does not mean accepted, and another branch's measurement does not automatically describe this branch.
Use frozen reads for committed PR/CI evidence, without moving pending refs or private files. Explicit materialization retains the complete closure and evidence.
Compare the shared subset: extra branch entries are allowed; different source bodies or schemas can make apparently equal readings differ. Cached acceptance names last verified versions, not fresh remote evidence.
See [project modes](../../docs/project-modes.md).

```bash
kpopper open                             # what to read instead of the whole record
kpopper check                            # does the record still hold together
kpopper affects <entry> [entry ...]      # what a change reaches
kpopper pull <entry|prefix> [...]        # values with sources, and what rests on them
kpopper pull <seed> --from <ref>         # what another branch's record proposes, beside
kpopper watch shared                     # shared external observations, from any branch
```

`check` enforces every invariant of the shape and **exits non-zero** when one fails: a dependency that is
not an entry (unless the judgment declares it missing), a dependency with no snapshot, a predicate
naming something the judgment does not declare, a predicate field holding prose, and a
plain-comparison predicate that currently holds — a judgment broken by its own condition. The
prose case matters most — prose in a predicate field is worse than an empty field, because it
reads like a predicate while nothing evaluates it and nobody notices. Say `blocked_on` instead.

It also reports, as `MOVED` lines that do not fail the build, every dependency whose value
differs from the judgment's snapshot — except one the predicate names and still evaluates
false, which moved without crossing the line the judgment drew. Movement is a question; a
crossed line is the answer. That comparison is only as good as the snapshot: `seen` must
hold the dependency's value as recorded, never a paraphrase of it — the reader cannot tell
a paraphrase from a move, so it reports both, and a person has to look.

Where the two readings are long and share an opening, each is clipped to **where they part**
rather than to its first few words, and the dropped opening is marked with an ellipsis — so a
line reads `…before the first cold night -> …after the first frost` instead of printing the
same unchanged head twice. Every surface that sets two readings against each other reads them
that way: `check`, `open`, what a write answers with, what a fork refusal asks you to choose
by, and what `review` says it rewrote.

`open` is the session opener: how large the record is, and then only what needs a person —
a dependency that is not an entry, a judgment broken by its own condition, a dependency that
moved since the judgment last looked, a judgment nothing was ever checked against, a declared
hole still waiting, a judgment nothing evaluable would falsify. Ranked, budgeted, and it
says how many it left out. What still stands is listed after that, and a judgment listed as
needing a person is not repeated there. On a clean record it prints one line, which is the
point; hypotheses beside the record add one to the head - how many wait, each with its age and
how many judgments rest on it.

`affects <entry>` answers the question the whole method exists for: something moved, what does it
reach — including through intermediate judgments — and for each one, whether its predicate can now
be evaluated or whether it is only flagged. That traversal is the part a session cannot do
reliably in its head, and the part the next session cannot do at all.

`pull <entry>` answers the other question — not what a change reaches but what is known here now:
that subject's own values with their sources, and the judgments resting on them, cut to a budget.
A judgment seed pulls in what it rests on, so pulling a conclusion also grounds it. Both `pull`
and `open` print a judgment's reasoning with its `{{references}}` resolved to what the record
holds now, under the judgment's own flag — so the re-reading a flag asks for can start there.

## What a hypothesis proposes is read beside the base

Every command reads the hypotheses over the base and evaluates the base alone: `pull` shows what
each proposes beside the value; `check` and `open` say CONTESTED where two hold one id with
different claims - a rival claim a person must decide between, and nothing else is ever contested;
the opener's count line says how many wait, with each one's age and how many judgments rest on it,
and draws no line - when one has waited too long is your reading (`q.hypothesis_lifetime` is open).
A hypothesis ends **folded** into the base by `consolidate`, or **refuted** by `consolidate --refute
NAME "why"` - one negative finding stays, `hyp.<name>: refuted`, its claim as the name, the why as
its place and the judgments it held in `refutes:`, and nothing else of it - or it stays
**untested**, counted at every open until someone does one.

Shared external observations captured through the [watch skill](../watch/SKILL.md) are read with `kpopper watch shared` from any branch; absence from the branch record alone does not establish absence. Comparison never changes `main` or a judgment's `seen`.

## Then reference, never retype

What the record holds is referenced in the output, not restated: a number retyped into prose goes stale while the recorded one is updated. The [record skill](../record/SKILL.md) carries the rule; the reading side of it is that a value quoted in an answer names the entry it came from, so the next session can pull it again.
