---
name: record
description: "Write what the work found into the project's knowledge record, so the next session inherits it instead of re-deriving it. Use the moment something worth keeping exists: a fact taken from a source, a rule worked out, a decision or conclusion, a measurement, a correction to a recorded value, or a question left open - and before finishing a session that produced any of these. The first write creates PROVENANCE.yaml. Covers add, set, review, same and distinct, judgments with a falsifier, and background capture."
---

# Record

Keep what the work produced while it is still in your hands: the source with its location, the rule rather than its result, the conclusion with what it rests on and what would make it wrong. It is a byproduct of the work, written when it exists and not at a ceremony. The command line is `kpopper` where it is on PATH; otherwise the `command` in the `KPOPPER_AGENT_CONTEXT` line the session opener printed (`python3 <plugin>/scripts/cli.py`) runs the same code. Do not guess a path and do not write a second reader - [the method's reference](../kpopper/references/method.md#finding-the-reader) says how to find the installed copy when neither is at hand.

## The first write

Default to learning during the user's actual task. Create the record with the first useful
finding worth revisiting, within the user's write authorization. A record containing only
`sources`, `known` and `open` is a valid start; add judgments when there are actual conclusions.
The reader fills `seen` when `add` creates one. Installation alone creates no file. A one-off
can finish with no record.

Where no record resolves for the workspace, `kpopper add` creates `PROVENANCE.yaml` at the repository root (the working directory outside git) with that first entry, and a later `add` extends it. A registered record that is unavailable is a location problem: restore it rather than starting another. The [shape reference](../kpopper/references/shape.md) shows the sections, pointer records and how a record lives outside a tree that cannot hold it.

**Make a new record useful to its reader.** Give a short explanation and link to the saved
finding when the `record` explanation is due. For a mapping or a record that benefits from a
visual view, render and show the shipped page when the surface supports it. A simple first
finding can be met in the conversation; creating a page is not an onboarding requirement.

## Record what the work calls for

During ordinary work, keep the findings and sources you actually use. A selected mapping or
investigation makes discovery the task: follow `kpopper _agent guide`, stay inside the agreed
scope and report its limits. The availability of more material is not a reason to survey it.

### 1. Something you took from a source

Not only numbers. A date, a name, a deadline, a quoted clause, a definition, a term someone agreed to, a list you extracted, a position a person stated. In most domains this is mostly **text**.

Record where it came from, precisely enough to go back: the source *and the location within it*. "The contract" is not a location; "the contract, §4.1" is.

Record negative findings the same way. "Absent from all three registry files" is a result with a
source, and the next session should not have to look again.

**A session is a source when the evidence was made in it** — a measurement, a probe, a run
that produced the number. Record it as one: what was done, when, and in which session,
precisely enough to go back to the transcript. The judgments a session writes need no
session field; if two sessions ever disagree about one, that is the question that earns it,
and not before. A session is also a source when what it already knew is what a judgment rests
on — a `prior.*` claim whose value is the confidence; see *An agent's prior as a source* in [falsifiers.md](../kpopper/references/falsifiers.md).

**And record how faithful it is.** This matters far more for text than for numbers, because a number is either read correctly or not, while text degrades in stages:

| | what it is | how to treat it |
|---|---|---|
| **quoted** | the source's own words | the strongest thing you can hold; keep it verbatim |
| **paraphrase** | their substance, your words | fine, but say so — someone may need the original wording |
| **characterization** | your reading of what a source means or implies | **this is a judgment, not a reading** — see below |

**Give it a name.** One line per entry, in the record's own language, saying what the thing *is*:

```yaml
  d.rate_lock:
    name: סוף שמירת הריבית באישור העקרוני
    v: 2026-09-18
    src: ...
```

`name` (or `title`/`label`/`what`) is the only field this method asks for by name rather
than inferring by shape — a sentence has no distinctive shape. It costs one line at the
moment you already know what you are writing down, and without it every surface that is not
about keys has to fall back to `rate lock`, which is the system's name for the thing and not
the reader's. `render_page.py --verify` reports how many entries are missing one. Sources
need one as much as entries do: `pr352: {url: …}` is a key, and `name: "the pull request"`
is what a reader sees in every hover that cites it.

That is the general rule for the whole page: **name things the way the reader would, never
the way the record is built.** A blocked line says *"the bank's position was never put in
writing"*, not *"blocked on mtg.clause7_reading"* — so a `blocked_on:` mapping should carry
its reason in prose, and the page shows the reason and keeps the key for the hover. Keys,
rules, predicates, state names: all of that is the machinery, and the machinery lives one
hover away. A derived entry says it was *worked out*; its formula is in the hover, never on
the surface.

### 2. Something you worked out

Record the rule, never the result. `subtotal * 0.17`, not `4,250`. The same applies to non-numeric work: if you derived a list by filtering another, record the filter.

### 3. Something you concluded or composed

Recommendations, assessments, comparisons — **and summaries, overviews, and any passage you wrote from several sources.** A summary belongs here and not in category 1: it is not something you took, it is something you made, and what you left out is invisible to every later reader.

> **The test:** if everything it rests on is correct and it could still be wrong, it is a judgment.

"The annual cost is $90,720" is not — check the arithmetic and you are done. "Acme is the better choice" is, and no amount of checking settles it. "The agreement is restrictive about early termination" is one too, even though it sounds like a report of what a document says — which is exactly the trap.

Before recording a significant judgment, [look for a failure the current check would miss](../kpopper/references/falsifiers.md#look-for-a-failure-the-current-check-would-miss).
Ask whether its recorded premises could all be correct while the conclusion still fails.
Give more attention to decisions with costly consequences or many dependent conclusions.

### The trap worth naming

**A characterization of a source drifts into being treated as a reading of it.** Someone writes "the contract restricts resale", a later reader takes it as quoted fact, and by the third session it has hardened into something nobody traces and nobody re-checks — while the clause it came from may say something much narrower. If you are describing what a source *means* rather than reproducing what it *says*, that is a judgment and it needs `wrong_if` like any other.

## Never restate from memory what you recorded

The moment something is needed in a second place, reference the record instead of typing it again. A number retyped into prose goes quietly stale while the recorded one is updated — and a quotation restated from memory is worse, because it drifts *and* still looks like a quotation.

If the document is generated, that means a placeholder resolved at build time. If it is hand-written, it means re-reading the record before restating anything, every time.

Watch for the failure where a reference gets swallowed by surrounding text — a literal `~1` typed
in front of a reference to an `80,000` entry silently renders as `180,000` and then moves whenever
that unrelated entry does. If a rendered number is not exactly one reference, it is a bug.

## The sign that would make it wrong

Every judgment says what would make it wrong, and the reader evaluates it where it honestly can:
`wrong_if` is one comparison over an entry the judgment rests on - `acme.seats < 150`,
`flue.clear == false` - and nothing richer; a second comparison on the right-hand side is unread.
A predicate that cannot be evaluated says so with `blocked_on` and why; a decision taken on a
session's prior, or on taste, names the prose sign that would re-open it in `reopened_by`. Never
invent a threshold to make a predicate evaluable, and never write prose in a predicate field.
Before recording a significant judgment, look for a failure the current check would miss, and
ask of every falsifier whether the decision itself could suppress the sign. All of it, with the
table of who decides by confidence and the cost of being wrong, is in
[falsifiers.md](../kpopper/references/falsifiers.md).

## When something a judgment rests on changes

**Flag it. Do not rewrite it.**

Say so — in your reply, or as a marker in the record: *"seat count moved 180 → 140; the Acme recommendation rests on it."* Then let a person decide, or re-examine it deliberately and say what you changed.

During review, ask whether the new evidence exposes a hidden assumption or a way the existing
check could miss a failure. Revisit the affected reasoning before refreshing `seen`; a false
`wrong_if` does not settle a new objection to the conclusion.

Do not silently regenerate the wording. Rephrasing produces a different text even when the reasoning is unchanged, so nobody can tell what actually moved; judgments that survived review get quietly replaced by fresh ones nobody read; and where judgments build on each other, the damage compounds. A stale recommendation someone read beats a current one nobody did.

**Re-running is allowed. Applying is not.** The re-run should usually write nothing at all — it
answers one cheap question, *does this still hold?* "Yes" refreshes the review without touching a
word. "No" produces a proposal that waits for a person. That way the document never moves under
the feet of someone reading it.

### Make the report a consequence of the write

An instruction to check after changing something is an instruction that will be forgotten, because
a skill is read once at the start and the session gets long. So wherever you can, **attach the
invalidation report to the action that causes it**: make the write path print what it broke, so
nothing has to be remembered. A separate "remember to run the check" command is the weakest
possible version of this.

That report is a push, and it is not a substitute for comparing against `seen` at build time. The
comparison is what survives edits made by hand, by another session, or by a refreshed source —
and it is derived rather than stored, so unlike a saved dirty flag it cannot itself go stale, get
cleared by accident, or survive a change that was reverted. The reader's `set` is this write
path: its reply is what the write reached, and `check` still makes the comparison afterwards.

### Verdict separate from explanation

Write each judgment's **verdict** — a short line stating what it concludes — alongside the prose that argues for it. **Anything downstream points at the verdict, not the prose.**

Without this, re-examining any judgment marks everything beneath it as needing review, including the many cases where the reasoning did not change and only the wording did. Within days the marks mean nothing and people stop reading them. `verdict: "prefer Acme above ~150 seats"` can stay fixed through three rewrites of the paragraph explaining it — and while it stays fixed, nothing downstream needs a second look.

## Tasks, subjects and contradictions

**A task is a judgment** whose truth is pending, and the fork is where it is written: the problem as
a judgment about today's state, with its source; `rests_on`, the premises taken as true; `verdict`,
the claim; `wrong_if`, what would refute the task's success - measurable where honest, else
`blocked_on: observational`; `seen`, the premises at the fork, filled by `add`. Asked at the fork,
never on a session's first turn: draft it from the task, and let the person confirm.

**Pull before add.** If the subject exists, extend it; do not create a sibling. `add` names the
nearest existing entries from declared fields alone - the same source and place is certain, the same
source or rule less so, the same premises a pair to judge - and a name only orders that list. Two
ids that are one subject fold with `same <a> <b>`: every reference rewritten, `also:` left on the
survivor; two that only look alike are kept apart with `distinct <a> <b> "why"`, and the pair never
returns. Neither shows on `pull` or the page yet - the file holds `also:` and `distinct_from`.

A write that contradicts what the base holds - the same id with a different value or verdict, a reading no newer than the base's that differs, a judgment resting on what only a hypothesis holds - is refused into the base and named a hypothesis: the refusal prints the `--hypothesis NAME` command that writes the same thing into `PROVENANCE.d/NAME.yaml` beside the record. Nothing written into the body opens that door. Testing, folding and refuting hypotheses is the [consolidate skill](../consolidate/SKILL.md).

## The write commands

```bash
kpopper add <id> field=value ...         # a new entry or judgment, in id order, seen filled
kpopper set <key> <value> [--why "..."]  # change one value; the reply is the reach
kpopper set <key> <value> --source <id> --at "..."  # a new reading with its new citation
kpopper review <id | "section title">    # it still holds: seen rewritten from the record
kpopper same <a> <b> | distinct <a> <b> "why"   # one subject under two ids, or two that only look alike
kpopper ingest capture --file report.json   # retain a source report, processed in the background
kpopper followups add ...                # deferred work, linked to the knowledge it waits on
```

`set`, `add` and `review` are how the record changes from the command line - `same`, `distinct`
and the fold write through the same path, under the same lock - and each answers with the reach: what is worked out from what it wrote, every judgment resting on it and its
state now — MOVED, MUTED, FIRED — and the texts of the brief that saw the old value. `set` changes
one value and stamps its date; `add` inserts a new entry or judgment in id order beside its
siblings and fills `seen` from what the dependencies hold; `review` says "I read it, it still
holds" and rewrites `seen` from the record — a judgment's, or a section text's by its title.
**Never type `seen` by hand once these exist.** The one field the method says no hand writes is
the one the tool writes, and a hand-typed snapshot is the paraphrase the reader cannot tell from
a move.

**A correction from a new source changes the citation with the value.** Add the source first,
then use `set --source <id> --at "<location>"` with the reading's `--as-of` date. Both citation
options are required together, so a page or clause from the old source cannot survive by accident.
`--why` is a comment about the change; `pull` retrieves the `from` / `at` citation. Without the
new options, `set` retains that citation. A source must be a recorded mapping with
`asked` / `file` / `url` / `of` / `read`, with no value or rule; this validates the reference,
not whether an external source can currently be read. The new options use `from` / `at`;
entries with `src` / `source` fields must reconcile those fields before using them.

**Recording can finish with an old judgment still flagged.** A new session mark records the
judgments and their current inputs as well as check failures. When updated readings make an
unchanged, pre-existing judgment's explicit condition fire, the stop gate lets the recording
finish. It does not refresh `seen`, rewrite the verdict or make `check` green. Review that
judgment before relying on it. New or edited broken judgments, structural errors and page
arrangement failures still receive the gate's reminder. Older count-only marks retain their
conservative behavior until the next session opens.

## Background capture, shared facts and deferred work

**New information is useful when you can see what it changes.** When ongoing record maintenance
is authorized, capture a material correction or source report while your understanding is fresh.
For an explicit update that can be checked in the background, use `kpopper ingest capture` and
continue unrelated work. Supply what you already know; leave an unknown target or meaning open.
The worker uses the existing writer and returns an attention signal only when a declared
condition fires or a new review question needs focus. Routine completion requires no waiting or
ACK. [INGESTION.md](../kpopper/INGESTION.md) gives the small input contract, supported record layouts, and
the different idle-delivery behavior in Claude Code and Codex. Notification content is a result,
not a new user report: never capture it again as fresh evidence.

In Codex hosts with native background agents and `send_message_to_thread`, use
`capture --notify-task` and dispatch its returned delivery job so an important finding can also
reach a primary that has finished answering. [DELIVERY.md](../kpopper/DELIVERY.md) gives the short native
worker contract. Plain hooks alone do not provide that idle delivery.

For opted-in worktree compatibility and shared external facts, use
[watch](../watch/SKILL.md). Keep code observations on their branch. An external observation
needs an explicit environment, source and date before shared capture; confidence alone
never selects its destination. Continue unrelated work while checks run, and inspect the
current result before relying on it. Shared facts can be read from any branch with
`kpopper watch shared`; comparison never changes main or a judgment's `seen`.

**Keep deferred work connected.** When a later check or action is authorized, use
`kpopper followups` to link it to the relevant knowledge and explicit activation conditions.
Prefer the user's existing task destination; a private fallback is available. Strongly recommend
a short daily review when this first becomes useful, alongside event checks, and respect the
user's scheduling choice. [FOLLOWUPS.md](../kpopper/FOLLOWUPS.md) covers capture, external owners, claims,
evidenced outcomes and connecting a real host schedule. A completed check can rearm a followup;
it does not by itself complete the work or refresh a judgment's `seen`.
