---
name: kpopper
description: "Maintain a grounded knowledge record for work that gets revisited: planning, research, coordination, decisions and software. Use when starting or resuming such work, including when PROVENANCE.yaml is absent, or when the user asks to map existing materials and their history. Also use when creating or refreshing an HTML document, so the document carries its source and check layer."
---

# kpopper

Start with the work the user wants to move forward. A project may span documents,
conversations, calendars, task systems and earlier sessions; code and repositories are one
important setting among these. Keep the goals, commitments, constraints, decisions, sources
and open questions that help later sessions continue. The connection to the user's goal
determines what belongs, rather than the mere availability of a tool or folder.

*Named for Popper: nothing here is ever verified, only exposed to refutation. Every judgment
must say what would make it wrong, and one that cannot be wrong is an opinion.*

This is **epistemic** bookkeeping. The record's subject is not the work but the state of
knowledge about the work: what is known, what each conclusion rests on, what the world looked
like when someone last checked, and where a hole was *declared* rather than filled. Do not
oversell that — it is not modal logic and there are no K operators here. Its nearest formal
relative is a truth-maintenance system, where `rests_on` is a justification and a changed input
invalidates everything downstream of it; what this adds is Popper's asymmetry, that a judgment
is never confirmed, only left standing.

That makes the record **self-evolving**, in one exact sense worth being precise about: nothing
here rewrites your conclusions. What the record does on its own is re-check every judgment
against what it was last checked against, and put the ones that no longer hold in front of you.
Nobody maintains a list of what went stale. Staleness is derived, so it cannot be forgotten,
cleared by accident, or survive a revert — and the page's sections fill themselves from the
same derivation, which is why an arrangement written last week still shows this week's problem.

Work that gets revisited needs one thing sessions usually throw away: **where each piece came from.** Keep that while you work. This is not a data-modeling exercise, and it should not change what you produce — only what you keep.

**New information is useful when you can see what it changes.** When ongoing record maintenance
is authorized, capture a material correction or source report while your understanding is fresh.
For an explicit update that can be checked in the background, use `kpopper ingest capture` and
continue unrelated work. Supply what you already know; leave an unknown target or meaning open.
The worker uses the existing writer and returns an attention signal only when a declared
condition fires or a new review question needs focus. Routine completion requires no waiting or
ACK. [INGESTION.md](INGESTION.md) gives the small input contract, supported record layouts, and
the different idle-delivery behavior in Claude Code and Codex. Notification content is a result,
not a new user report: never capture it again as fresh evidence.

In Codex hosts with native background agents and `send_message_to_thread`, use
`capture --notify-task` and dispatch its returned delivery job so an important finding can also
reach a primary that has finished answering. [DELIVERY.md](DELIVERY.md) gives the short native
worker contract. Plain hooks alone do not provide that idle delivery.

For opted-in worktree compatibility and shared external facts, use
[watch](../watch/SKILL.md). Keep code observations on their branch. An external observation
needs an explicit environment, source and date before shared capture; confidence alone
never selects its destination. Continue unrelated work while checks run, and inspect the
current result before relying on it. Shared facts can be read from any branch with
`kpopper watch shared`; comparison never changes main or a judgment's `seen`.

## When the requested output is an HTML document

For an ordinary HTML report, summary, brief or other document, use the standalone
[document authoring workflow](DOCUMENTS.md). Create the user's content and design,
and create the source/check mapping while writing it; the user does not prepare that
mapping or ask for a separate evidence step. Deliver the single HTML produced by
`kpopper document build`, with its inline evidence and review controls. Read the short
author guide before authoring, use actual available sources, and keep missing evidence
and inferred prose explicit. A one-off document does not require a new PROVENANCE.yaml,
a workspace map, or unrelated record setup. Existing record-page requests still use PAGE.md.

## Step 1 — `PROVENANCE.yaml`

**Keep deferred work connected.** When a later check or action is authorized, use
`kpopper followups` to link it to the relevant knowledge and explicit activation conditions.
Prefer the user's existing task destination; a private fallback is available. Strongly recommend
a short daily review when this first becomes useful, alongside event checks, and respect the
user's scheduling choice. [FOLLOWUPS.md](FOLLOWUPS.md) covers capture, external owners, claims,
evidenced outcomes and connecting a real host schedule. A completed check can rearm a followup;
it does not by itself complete the work or refresh a judgment's `seen`.

This method owns one known entry point: **`PROVENANCE.yaml` in the project's working directory.**
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
pay (4) · c50 (9) · …`. That one line is the whole link between a question in plain language
and the graph. "What is happening with the mortgage" has no meaning to a file; `mtg` does.

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
Expand relevant branches or inspect source
content before claiming that the record does not cover it. A missing exact ID establishes
only that the ID is absent. If relevant evidence remains unfound, say so and continue the
discovery without inventing a matching claim.

While the record is small enough that you would happily re-read it every session, just read
it; this buys nothing. From the moment you would not, it is the difference between an opening
cost that grows with the record and one that grows with what moved — and that difference is
the only thing this method promises to measure.

**If it does not exist:** there is no record here yet; knowledge may already live in the
materials. Run `kpopper open` if the hook did not supply `KPOPPER_START`, and follow
`kpopper _agent guide` for the optional first offer, source discovery and contextual explanations.
An unavailable registered record is a location problem, not a new project.

Default to learning during the user's actual task. Create the record with the first useful
finding worth revisiting, within the user's write authorization. A record containing only
`sources`, `known` and `open` is a valid start; add judgments when there are actual conclusions.
The reader fills `seen` when `add` creates one. Installation alone creates no file. A one-off
can finish with no record.

**Mapping is available by choice.** Offer learning while working, an initial map of existing
materials, or a deeper investigation within agreed subjects, sources and dates. An explicit
mapping request already chooses that work; no extra confirmation is needed. Use the guide
to locate where decisions happen and to keep historical accounts separate from present
findings. Do not expand into unrelated private sources or infer permission from silence.
For an explicit mapping request, call `kpopper map --json` or `kpopper map --deep --json`
with the host session environment, accept and execute the returned task, and report its actual
result through the supplied internal protocol. Do not stop after announcing a ready task.
Learning during ordinary work needs no configuration. Public operations are `open`, `map`,
and `config --guidance on|off`; request IDs and receipt calls are internal.

**Teach through actual events.** A compact card can explain the first saved finding, linked
source, grounded decision, conflict, reuse, or changed premise. Show real links where available
and use the host's native card or a Markdown block. `kpopper _agent status` lists unseen concepts;
`kpopper _agent shown EVENT` acknowledges an explanation only after displaying it. The introduction
is remembered per user, the starting offer per project. Respect `kpopper config --guidance off` and do
not turn an onboarding step into a requirement for finishing the user's task.

**If the project already keeps a record somewhere else** — a `facts.yaml` in a subfolder, a table someone maintains — do not move it and do not duplicate it. Create `PROVENANCE.yaml` as a pointer:

```yaml
record: analysis/facts.yaml     # the real record lives here
also:   analysis/claims.yaml
```

One known location, wherever the content actually sits. **Two parallel records is the most expensive mistake available here** — from then on every reader must know both, and it never resolves on its own.

**If the root cannot hold it** — the repository's own rules keep such files out of the tree, or
the checkout is disposable and untracked files die with it — keep the record wherever the
project keeps its untracked material, and register the path with the checkout:

```bash
echo "/abs/path/to/PROVENANCE.yaml" > "$(git rev-parse --git-common-dir)/kpopper-record"
```

The opener and every command below then find it from any checkout or worktree of that
repository (`provenance.py where` prints what they found), and nothing enters the tree. Say
where it went in your reply and in whatever memory the project keeps — a record nobody can
find is a record nobody updates.

### The shape

Three sections, mirroring the three things worth keeping. Grow it freely; do not rename it.

```yaml
meta:
  updated: 2026-08-31

sources:      # documents, pages, people, messages, a session's prior — things nothing else produced
  msa:      {name: "the master agreement", file: "contracts/acme-msa-2026.pdf", of: "2026-04-02"}
  pricing:  {name: "Acme's pricing page", url: "https://…/pricing", read: "2026-08-30"}

known:        # taken from a source, or worked out from other entries
  acme.seat_price: {v: 42, unit: USD/mo, from: pricing, at: "Enterprise tier"}
  acme.seats:      {v: 180, from: "Dana, in the 28/08 planning call"}
  acme.annual:     {rule: "acme.seat_price * acme.seats * 12"}
  contract.exit:
    quoted: "Either party may terminate for convenience on ninety (90) days' written notice."
    from: msa
    at: "§12.2"

judgments:    # concluded or composed — each says what would make it wrong
  why_acme:
    rests_on: [acme.annual, beta.annual, migration.weeks, contract.exit]
    verdict:  "prefer Acme above ~150 seats"
    because:  "Cheaper only above ~150 seats, the migration cost is one-time, and the
               90-day exit keeps the downside bounded."
    wrong_if: "acme.seats < 150"
    seen:     {acme.annual: 90720, beta.annual: 84000, migration.weeks: 6,
               contract.exit: "90 days"}   # the dependency values at the moment it was written
```

These field names are a recommendation, not a requirement — a record that already uses others
is fine. Classify each entry honestly; a role with no content yet needs no placeholder.

**`seen` is not optional and not decoration.** It is the snapshot of what every dependency held
when the judgment was written. Without it there is no before, so nothing can be compared and
**no drift can ever be detected** — the record still says what things rest on, but it can never
tell you that one of them moved. Write it when you write the judgment; a later reviewer refreshes
it when they confirm the wording still holds. If a dependency is missing from `seen`, that
judgment has never been checked against it, and that is itself worth reporting.

It is also what lets a threshold measure *accumulated* drift. Three changes of 200 each are all
below a 500 threshold individually, but together they cross it — and only a snapshot taken at the
last review can see that. Propagating a dirty flag at write time cannot.

**If the work is a genuine one-off** — a single question, a throwaway script, something nobody will return to — do not create the file. There is nothing to keep.

### When something does not fit

A fixed set of fields creates a pressure to jam things into it. Resist that: a judgment invented
to satisfy a checker is worse than a gap, because it looks right. Three honest ways out, and
using one is never a failure:

```yaml
open:         # a live question. Not a fact, not yet a judgment. No verdict is owed.
  which_base: "does the bank measure the ratio against price or against its own valuation?"

judgments:
  blocked_thing:
    rests_on: […]
    verdict:  "…"
    wrong_if: "the request from his device returns an explicit not-found body"
    blocked_on: "could not reproduce past the login gate, so the predicate is un-evaluable"
```

`blocked_on` says a predicate cannot be evaluated *and why*. That is a real state and it should
be visible — what must never happen is a predicate that reads like one and is actually prose,
because nothing evaluates it and nobody notices. Say it is blocked instead. A judgment that is
*decided* — on a session's prior, or on taste — and names only the sign that would re-open it
is a third state, and says `reopened_by`: not a hole, and not waiting (see *An agent's prior as
a source*, under Step 5).

## Step 2 — Record what the work calls for

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
on — a `prior.*` claim whose value is the confidence; see *An agent's prior as a source*.

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

Before recording a significant judgment, [look for a failure the current check would miss](#look-for-a-failure-the-current-check-would-miss).
Ask whether its recorded premises could all be correct while the conclusion still fails.
Give more attention to decisions with costly consequences or many dependent conclusions.

### The trap worth naming

**A characterization of a source drifts into being treated as a reading of it.** Someone writes "the contract restricts resale", a later reader takes it as quoted fact, and by the third session it has hardened into something nobody traces and nobody re-checks — while the clause it came from may say something much narrower. If you are describing what a source *means* rather than reproducing what it *says*, that is a judgment and it needs `wrong_if` like any other.

## Step 3 — Never restate from memory what you recorded

The moment something is needed in a second place, reference the record instead of typing it again. A number retyped into prose goes quietly stale while the recorded one is updated — and a quotation restated from memory is worse, because it drifts *and* still looks like a quotation.

If the document is generated, that means a placeholder resolved at build time. If it is hand-written, it means re-reading the record before restating anything, every time.

Watch for the failure where a reference gets swallowed by surrounding text — a literal `~1` typed
in front of a reference to an `80,000` entry silently renders as `180,000` and then moves whenever
that unrelated entry does. If a rendered number is not exactly one reference, it is a bug.

## Step 4 — Add structure only when something forces it

Structure is bought with an observed trigger, never because it would be tidier.

| what appears | only once this has happened |
|---|---|
| `PROVENANCE.yaml` | the work will be revisited |
| references instead of restated material, and something that assembles the output from them | the same thing is needed twice |
| checks that **fail the build** — a broken reference, a literal where a reference belongs, a source that does not exist | there is too much for a person to eyeball |
| a named type with required fields | the same shape has appeared a **third** time |
| views as data — an operator pipeline plus a renderer referenced by name | the same question has been asked from two angles |
| judgments with a structured verdict and a gate on re-running them | a judgment started resting on another judgment |

A check that cannot fail the build is decoration. That is the whole difference between a
convention people drift from and a rule they cannot drift from.

These are six independent capabilities, not six stages of one process. They are listed in rough
order of cost, which is what makes them look like a ladder — but each is bought by its own
trigger and they can be acquired in any order. A project can have build-failing checks and no
views, or views and no named types. There is no stage a project is "at".

Stopping at row one or two is a perfectly good outcome. Reaching the bottom when nobody ever asked the same question twice means you over-built.

**The rule that settles most cases:**

> A category is justified by a question someone asked, not by the world containing it.

If nobody has asked "what is still open with Dana?", there is no reason to create a person record for Dana. No question, no structure.

### What a view is, once you reach that rung

Do not hand-write a document that reads the record. A view has a shape:

> **view = a pipeline of operators from a closed set, plus a renderer referenced by name.**

The operators are **data** — `select · filter · group · aggregate · sort · limit`. Closed on
purpose. The renderer is **code**, and the view points at it by name. A new view is a row; a new
renderer is code someone writes. The moment you find yourself expressing layout through
operators, you have invented a home-made programming language — stop and add a renderer instead.

Two invariants make a generated view worth more than a written one: every displayed value is a
placeholder resolved at build time and never typed, and **the build fails when a placeholder has
no entry behind it.** Without the second one, the view is decoration.

The renderer is itself a judgment — someone made it, for a reason, in a particular shape — so it
carries `rests_on` / `verdict` / `wrong_if` like any other. So does a layout decision ("this is
at the top because the payment has no date yet; when it gets one, it moves down"). The operator
pipeline is not a judgment: it recomputes silently and cannot be *wrong*, only badly chosen.
Keeping those apart is what stops every review of the record from marking the queries too.

Views are not a finished deliverable either. They are the projection of what is known onto what
the reader needs *now*, and that changes as the work moves. Change them when a new subject has no
expression in the output, when a prominent item is already resolved, when the same list is
maintained by hand in two places, or when a layout decision gets flagged — and record the
decision, not just the change. Early on the structure moves a lot; as the record matures the
values move and the structure mostly does not. Restructuring a mature view with no new question
behind it is polishing without a trigger.

### Views for an agent, not a person

A view for a person is typeset. A view for an agent is ranked and cut to a budget — two more
operators, `rank` and `budget`, and a different signature:

> **view = f(graph, intent) → projection**

`intent` is a parameter, not a type. "I am about to change the mortgage plan" and "I am writing
the morning brief" pull entirely different subgraphs out of the same record, and neither is the
document a person reads. So context is not a collection someone curates by hand — it is a query
with a budget, and a new session does not "read the project", it runs one.

## Step 5 — When something a judgment rests on changes

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

### Look for a failure the current check would miss

For a significant judgment, make a focused attempt to find a plausible failure that its current
checks would leave unflagged. Consider an assumption about a source, a gap between the premises
and the conclusion, or an interaction with another decision. For example, passing tests may
leave a migration's effect on existing data untested.

When you find an independent failure mode, record the assumption it exposes and the observation
that would reveal it: a source correction, test result, measurement or other concrete evidence.
Say whether that observation contradicts the conclusion, removes its support, or blocks the
proposed action. Add relevant dependencies and snapshots so later changes can reach the judgment.
An imagined scenario is a reason to investigate, not evidence that the failure occurred.

Preserve useful existing checks. Follow the encoding rules below for additional conditions;
do not imply that a condition recorded in prose runs automatically. Use `reopened_by` for the
prior or taste decisions described below when the additional sign calls for interpretation.

Keep the search proportionate to the consequences. Do not invent thresholds, duplicate a check
in different words, or fill a quota of conditions. If no credible additional failure emerges,
keep the existing conditions without claiming they cover every possible failure. New evidence
can still reopen the judgment even when none of its declared predicates fires.

### A falsifier your own decision can suppress is no falsifier

Before you keep a `wrong_if`, ask one question of it: *under this decision, could that still
happen?* A decision shapes the world it is judged in. If the sign you named can only appear
when people do something the decision itself stops offering, the sign will never appear, and
the judgment is sealed rather than falsifiable. "This page draws one grouping; wrong if a
brief ever declares a second" is sealed — nobody declares what nothing draws. "Wrong if the
briefs rewritten under the writing guidance still declare one each" is not: the option is
open, and the sign is a choice people can make against you.

The same test catches the quieter forms: a threshold measured only by the code the decision
governs, a survey the decision's author runs, a "nobody asked" over a channel the decision
closed. Name a sign that lives outside the decision's reach — a person's request, a record
written by someone else, a measurement the change cannot touch — and say when it will be
read. A reviewer reading falsifiers as a foreign agent should find at least one that could
fire.

### Make `wrong_if` a predicate where you honestly can

`wrong_if: "acme.seats < 150"` can be evaluated; `wrong_if: "if the numbers move materially"` cannot. When it is a predicate:

- **false** ⇒ drift in the values the predicate references is **muted** — they moved but did not cross the declared threshold. This reports no violation of that condition; separate evidence or an uncovered failure still needs review. Values the predicate does *not* reference still flag when comparable values have moved.
- **true** ⇒ the judgment is **broken**, not merely flagged.
- **references something with no anchor** ⇒ report it as un-evaluable. That is a feature: it turns "we should verify that number someday" into something blocking.

A predicate may only name things the judgment declares it rests on. Otherwise it reads an entry
the graph does not connect it to, and a change to that entry never reaches the judgment at all —
a silent hole that no amount of reading will reveal.

Measure thresholds as change since the last review rather than absolute level where you can — it
keeps a literal out of the predicate and survives the value moving for unrelated reasons.

**One comparison, or it is not evaluated.** `wrong_if` is a name, an operator and one value —
`acme.seats < 150`, `flue.clear == false`, `signed_on < "2027-01-01"`. A right-hand side carrying a
second comparison is not richer, it is unread: the shape takes everything after the operator as the
value, so `a == false, or b == false` is compared against the text after the first operator and is
false in every state of the record. Use separate judgments for distinct claims. An additional
condition on the same judgment lives in `because` with `blocked_on` saying the reader cannot
decide it; keep the existing executable condition. A truth value is matched (`== false`), never
ordered.

**Never invent a threshold to make a predicate evaluable.** A vague quantifier is a signal that the threshold lives in someone's head and was never stated — surface it and ask. If it cannot honestly be made evaluable, say so with `blocked_on` rather than writing prose in the predicate field — and if the judgment is decided and the prose is what would re-open it, say `reopened_by`.

### An agent's prior as a source

What a session already knows is a source. It enters the record as a claim that says where it came
from, with one difference: **the value is the confidence.**

```yaml
known:
  prior.macros_bind_by_position:
    v: 0.9                        # the confidence, and nothing else
    unit: confidence
    reach: general
    name: "spreadsheet macros written against an export bind to column positions, not names"
    from: s.2026_09_03_export     # the session source - s.<date>_<slug>, with asked: verbatim
```

`from:` names the session source, never a model: which model answered is a fact about the run, and
calibrating models is not the record's job. `reach` says what kind of claim it is. **General** is
how things usually work anywhere; the session has seen the pattern countless times and its
confidence is earned. **Local** is this project right now, which training cannot know: the session
is guessing from what is typical, and the specific case is specific. Scored on one day, a
session's general claims held 3 of 3, its local claims checked first 6 of 6, its local claims
assumed 0 of 4, each of which felt like 0.85. So `reach: general` may rest on the prior; `reach:
local` wants a `run.` or a `doc.` beside it — and a judgment resting on a prior also rests on at
least one measured local fact.

**Who decides is confidence × the cost of being wrong** — the lane policy applied to a judgment:
confidence attaches to the claim, never to the decision; cost is reversibility and blast radius.

| | high confidence (≥ ~0.8) | low confidence |
|---|---|---|
| **cheap to reverse** | decided: one line in the record, no person | try it: the reversal is the test; record what was tried |
| **costly to reverse** | decided, recorded, and the person sees it — to confirm, not to wait on | the person decides — or buys information with the cheapest experiment |

Only the last cell waits for an event, and there by choice: 0.7 behind a reversible choice is a
decision, not a hypothesis. **Always write what would re-open the judgment; pursue it only when
being wrong is costly.** Three fields, three meanings:

| field | meaning | read by |
|---|---|---|
| `wrong_if` | a predicate over entries the judgment rests on | the reader, at every `check` |
| `blocked_on` | the predicate cannot be evaluated, and why | nobody — a declared hole |
| `reopened_by` | the prose sign that re-opens a judgment decided on a prior, or on taste | a person, when the sign appears |

```yaml
  c.order_is_the_contract:
    rests_on: [prior.macros_bind_by_position, export.header_changes]
    verdict: "the column order of the export is a contract: a column is added at the end, never between"
    reopened_by: "a macro that breaks on an export whose column order did not change"
    seen: {prior.macros_bind_by_position: 0.9, export.header_changes: 0}
```

A judgment with `reopened_by` and an empty `wrong_if` is decided, not waiting: `check` counts it
among the declared, nothing lists it as needing a person, and its card shows the sign in a row of
its own. Write `wrong_if` beside it where the record holds what it reads: `prior.x < 0.8` re-opens
the decision when the confidence is re-stated below the line. Rating everything 0.9 buys nothing:
the sign still has to be written, and a bad one is visible — "wrong if it turns out wrong" reads
as what it is. The kind carries its own falsifier: if judgments resting on priors at 0.8 or above
keep reversing, the confidences carry no information and `prior.*` is demoted to a note. How many
they are is counted rather than guessed at: `check` says how many judgments rest on `prior.*`
claims and how many of those on a prior at 0.8 or above, and the opener carries the same line
where a record has any; `graph.prior_reversal_rate` is the share of those high-confidence
judgments refuted by consolidation, whose findings keep the judgment IDs the hypothesis
held in `refutes: [id, ...]` (identities, not premises), with older findings still linked
by a whole judgment ID or `{{judgment.id}}` in `name`, counted once per judgment and read
as 0.0 when none qualify, with the demotion line only in a judgment's `wrong_if`.

## Step 6 — Changing the shape

Your first choice of shape will often be wrong. Change it freely; that is cheap. What is expensive is leaving **two** shapes side by side, because every future reader must then learn both and it never resolves on its own.

So: **a change of shape is valid only together with a migration that leaves the build green.**
Not "convert it soon" — in the same pass, with the checks passing at the end of it. That single
rule is what turns "the method can change itself" from philosophy into something runnable: you
cannot add a kind of entry without answering what happens to the existing ones. It does not
prevent change; it prevents *half* a change, which is what actually kills records like this.

## Forks and hypotheses

One record is written by everyone, from any session or branch, as long as the write is consistent
with it. **A contradiction opens a hypothesis**, and the reader tells one by the id and the day.
`set` of a reading no newer than the base's (its `of:`, else its source's read date) that differs is
refused - two readings of one day that disagree are two writers, not the world moving; a newer
reading updates the base and flags what rests on it. `add` of an id the base holds is refused, and
with a different value or verdict it is a contradiction: a standing judgment is replaced in the base
only when its own `wrong_if` holds now, and otherwise the rewrite waits beside the record until a
person folds it. Nothing written into the body opens that door - every session's first write is a
source carrying what it was asked, so a field read as a person's authority would be a key every
session already holds. `request: s.<date>_<slug>` - a session source whose `asked:` is the person's
request verbatim, rested on - still names whose asking the change was taken from, said *on the word
of* on every surface: provenance the person weighs at the fold, and permission for nothing.
A write resting on what only a hypothesis holds belongs in that hypothesis. Each refusal names the
command that writes the same thing into `PROVENANCE.d/<name>.yaml` beside the record - the base
untouched - named after the id contradicted and a mark of the claim written, unless `--hypothesis
NAME` on `set`, `add` or `review` names it. The first write stamps `born` in the head; `claim:`, a
head `wrong_if:` and `folds: never` are the one thing a hand writes there. Three uses: a concurrent
writer whose reading was refused; a proposal not yet approved - a branch's record *is* this, and
travels with the branch; a what-if, `folds: never`, evaluated at every dry run and never written.

Every command reads the hypotheses over the base and evaluates the base alone: `pull` shows what
each proposes beside the value; `check` and `open` say CONTESTED where two hold one id with
different claims - a rival claim a person must decide between, and nothing else is ever contested;
the opener's count line says how many wait, with each one's age and how many judgments rest on it,
and draws no line - when one has waited too long is your reading (`q.hypothesis_lifetime` is open).
A hypothesis ends **folded** into the base by `consolidate`, or **refuted** by `consolidate --refute
NAME "why"` - one negative finding stays, `hyp.<name>: refuted`, its claim as the name, the why as
its place and the judgments it held in `refutes:`, and nothing else of it - or it stays
**untested**, counted at every open until someone does one.

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

**The consolidation walk.** `consolidate --dry-run` lays the hypotheses named - every one, when none
is - over the base by id and runs the reader's own `check` on it, reported in a fixed order:
arrived, updates (what a hypothesis replaces, and what rests on it), moved / falsified, contested,
candidates, new subjects. Three lists ask three answers, each recorded by a command so the question
never returns: a candidate pair is the **same** subject (`same a b`) or **different** (`distinct a b
"why"`); a **contradictory** id - two hypotheses on one, or a reading the door refuses - is read
again on a later day, `set` in the base, or in the hypothesis that read it when the newer reading
bears its claim out but not its number, so the fold still carries what else it brought; or it is
refuted. The run exits non-zero on a contested id, a falsifier that holds, a hole, or a head
`wrong_if` it cannot decide; a premise that moved under a judgment leaves it green and blocks only
the fold, and `review <id> --hypothesis NAME` refreshes the snapshot against the record as it stands
under the hypothesis. `consolidate` runs the same test and, only when it is clean, writes the union
through the write path: every replacement passes the one door a `set` passes, so a reading born of a
same-day refusal waits until someone reads again on a later day - while a verdict over a standing
judgment the record's own sign has not broken passes here and nowhere else, because the fold is the
person's act and says so in the line above it; the result is read back and undone whole if `check`
then says anything new; the folded files go, and what to commit is printed.

Before accepting a material consolidation, examine affected judgments for a failure that arises
only when the changes are combined, even if each change passes separately. Use the same
[failure search](#look-for-a-failure-the-current-check-would-miss) to expose shared assumptions
or newly incompatible decisions. Record any resulting objection or additional condition, and
repeat the dry run after changing the record; a passing dry run covers the declared checks.

**One rule, two containers.** What stands is contested only by something recorded: a rival claim, in
a hypothesis; or a doubt with no rival value yet, an open question that names the id. The page says
*a hypothesis contests this arrangement*, or *a question* does, `check` notes the same, and both
stand until a person consolidates or answers. Deleting the file is neither.

**How a merge goes.** git merges the files: additions in id order rarely meet, and hypothesis files
meet only when two branches claim the same thing under one id, since the name carries both - two
readings that agree, and either head is the whole of them; two that disagree are two files git
merges, and which of them stands is the dry run's question for a person. The dry run tests the
result: the pull request runs it on the merged tree, and the push to `main` runs it again as the
second net, for two pull requests each consistent alone that contradict together. Fold a hypothesis
the dry run proves *before* the pull request, so `main` receives base changes; let an unproven one
merge as a file, and `main` carries an open hypothesis the opener counts. Nothing crosses branches
unasked: `pull <seed> --from <ref>` lays what another branch committed beside your pull, and
`consolidate --from <ref> --dry-run` tests its record as one more hypothesis named after the ref - a
pull, never a push, and how a dead branch's facts are harvested. Every write - `set`, `add`,
`review`, `same`, `distinct`, the fold, the refutation - takes an exclusive lock on the record's
directory where the platform has one - Windows has none - so two sessions on one file take turns
instead of the last one discarding the first. In the tree each worktree writes its own copy and git
merges them; out of the tree every worktree writes the one file, and only the lock stands between
them.

## The reader

A reader ships with this plugin at `scripts/provenance.py`, and the `kpopper` command line is the
same code with a shorter name. Run it from the directory the record sits in — it needs only
Python and PyYAML:

```bash
kpopper open                             # what to read instead of the whole record
kpopper check
kpopper affects <entry> [entry ...]
kpopper pull <entry> [entry ...] [--from <ref>]   # --from: what another branch proposes, beside
kpopper set <key> <value> [--why "..."]  # change one value; the reply is the reach
kpopper set <key> <value> --source <id> --at "..."  # a new reading with its new citation
kpopper add <id> field=value ...         # a new entry or judgment, in id order, seen filled
kpopper review <id | "section title">    # it still holds: seen rewritten from the record
kpopper consolidate [--dry-run] [NAME ...]   # the union, tested; then folded (--refute NAME "why")
kpopper remeasure [--run]                # the entries that name a recipe, taken again from the tree
kpopper same <a> <b> | distinct <a> <b> "why"   # one subject under two ids, or two that only look alike
```

Where that command is not on the path, the reader still ships inside the plugin — but **do not
guess its path.** An installed plugin sits under a *version* directory, and a plugin checkout can
hold worktrees of its own that carry a second copy of every script. So match the name loosely,
skip the nested checkouts, and take the highest version:

```bash
R=$(find ~/.claude -name worktrees -prune -o -path '*kpopper*/scripts/provenance.py' -print 2>/dev/null | sort -V | tail -1)
python3 "$R" open
```

Each piece earns its place: `-name worktrees -prune` drops the copies inside a checkout's own
worktrees, `sort -V` orders `0.9.0` *below* `0.16.0` where a plain `sort` puts it on top, and the
loose `*kpopper*` survives the version segment that a fixed `*kpopper/scripts/…` pattern cannot
match. Anchoring on the exact installed layout is what breaks; this matches the shape instead.

Locate it that way rather than relying on `$CLAUDE_PLUGIN_ROOT`: that variable is documented for
hook and MCP configuration, and is **not** set in the shell a skill's commands run in — a command
written against it silently becomes `/scripts/provenance.py` and fails. If neither the command nor
the find turns up anything the plugin is not installed; say so rather than writing your own copy.

Do not rewrite it, and do not write a second one beside a project that already has its own build
doing this work.

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

**It does not know your field names.** Each role has a distinctive *shape*, and the shape is
enough: dependencies are a list of strings that are all ids of other entries; a snapshot is a
mapping whose keys are a subset of that entry's dependencies; a predicate is a string containing
entry ids *and* something else — an operator, a number, a bracket, so a bare reference is not one.
So it reads a record that says `known:`/`judgments:`/`from:` exactly as well as one that says
`facts:`/`claims:`/`src:`.

`check` enforces every invariant above and **exits non-zero** when one fails: a dependency that is
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

`set`, `add` and `review` are how the record changes from the command line - `same`, `distinct`
and the fold write through the same path, under the same lock - and each answers with the reach: what is worked out from what it wrote, every judgment resting on it and its
state now — MOVED, MUTED, FIRED — and the texts of the brief that saw the old value. `set` changes
one value and stamps its date; `add` inserts a new entry or judgment in id order beside its
siblings and fills `seen` from what the dependencies hold; `review` says "I read it, it still
holds" and rewrites `seen` from the record — a judgment's, or a section text's by its title.
**Never type `seen` by hand once these exist.** The one field the method says no hand writes is
the one the tool writes, and a hand-typed snapshot is the paraphrase the reader cannot tell from
a move.

`remeasure` is how the tree answers for the record. `check` compares `seen` against the record's
own stored value and never against the tree, so a count that is wrong about the tree passes as
long as it agrees with itself. An entry whose value is a fact about the tree - lines of a file,
files a package ships, places in the code where something is decided - names the recipe that
takes it, `measure: <name>`, and `PROVENANCE.measure.yaml` beside the record holds that name's
argument list. The name is all the record carries: a bare name, refused by `add` and failed by
`check` when it is not one or stands on anything but a stored scalar reading. `kpopper remeasure`
prints the plan and runs nothing; `--run` runs each cited recipe once, from the checkout's root,
without a shell, and lays what differs over the record as one more hypothesis, `tree/<commit>`,
through the same dry run that tests any hypothesis - red on a falsifier that holds on the measured
value, a hole, or a reading the tree contests; green, with the `set` command that refreshes it, on
an older reading that moved without crossing a line. A hypothesis that replaces a measured entry
carries the line with it, since the fold takes its block over whole. The pull request runs it; nothing that reads
the record does. Name a recipe only where a command honestly takes the count the entry's `at:`
describes - a survey, a hand-scored run, a prior has none, and their honest form is the reading
with its date.

### The page

`render_page.py` turns any record into one self-contained HTML file, in two tabs: **Record**,
which nobody writes, and **Now**, the arrangement this session chose. What makes it worth
opening is the provenance layer - hover anything for where it came from, click to walk to a
dependency, and see the graph around it outlined in place.

```bash
python3 "$(dirname "$R")/render_page.py" > record.html      # the page
python3 "$(dirname "$R")/render_page.py" --verify           # deterministic, no browser
node    "$(dirname "$R")/verify_page.js" record.html        # what only looking catches
```

**Read `PAGE.md` beside this file before writing a brief or choosing a layout** - the brief
format, the closed set of renderers and what each one requires, and the rules that keep an
opinionated arrangement honest all live there. It is a reference, not part of the opening
cost: skip it entirely on a session that never builds a page.

**Make a new record useful to its reader.** Give a short explanation and link to the saved
finding when the `record` explanation is due. For a mapping or a record that benefits from a
visual view, render and show the shipped page when the surface supports it. A simple first
finding can be met in the conversation; creating a page is not an onboarding requirement.

```bash
sed -n '1,400p' "$(dirname "$R")/../skills/kpopper/PAGE.md"
```

**Do not write this layer yourself.** It ships here for the same reason the reader does: it
took a browser and six bugs to get right, and a session rebuilding it will produce something
worse and not know. Write layout if you need layout; call this for the mechanism.

## The part that does not change

Everything above is open to revision — the sections, the field names, the operators, the views.
Four things are not, because they are what the rest is built on:

1. **Every entry declares what it rests on.** Something with no declaration does not go in.
2. **Do not store an output when you can store what produces it.**
3. **Invalidation spreads automatically; re-running a judgment never applies itself.**
4. **A change to the shape is valid only with a migration that leaves the build green.**

A record that can rewrite its own rules will drift — slowly, unnoticed — unless something in it
is not up for revision. These four are that floor.

## Never do these

Each is recognizable while you are doing it. If you catch yourself, stop.

- Designing a structure before meeting the evidence, including during a selected mapping.
- Starting a second record, or renaming `PROVENANCE.yaml` to something you like better.
- Creating a category with one member. One is a case, two a coincidence, three a category.
- Recording things nobody asked about, for completeness. Completeness is not the goal.
- Renaming or reorganizing because it would be tidier, with no question behind it.
- Building something general when only two specific cases exist.
- Recording anything a person would have to maintain by hand afterwards. That is the definition of bureaucracy, and it is how this fails.
- **Asking the user how to structure their data.** That is your job, not theirs, and asking makes the method cost them something on the very first turn.
- Spending a whole turn on the record when nobody asked for it. It is a byproduct of the work; if it becomes the work, something has gone wrong.
- Repairing the project inside the turn that records it, without saying so first. Record, say what you found, then fix — as work the person can see.
- Presenting your reading of a source as the source's own words.
- Inventing a number, a date or a threshold so that something becomes computable.
- **Filing something as a judgment because that is the only slot available.** A live question goes
  in `open:`, an un-evaluable predicate says `blocked_on`. Shaping the record to pass a checker
  produces records that look right and are not.
- **Declaring which of the capabilities above a project has.** Which ones exist is visible in the
  files — does a views file exist, does the build fail on a broken reference, do judgments carry
  verdicts. So derive it, and if it is worth surfacing, have the build write it down beside the
  evidence and the trigger that bought it. A hand-declared stage can contradict reality with
  nobody noticing, and a number at the top of a file turns into something to increment.

## Check yourself

- **Keep first use proportionate.** Offer a short optional choice at a suitable moment;
  preserve progress on the task and never repeat an explanation just because a session restarted.
  A chosen mapping is work in its own right, with scope and a useful result.
- Pick three specific claims from what you produced — a figure, a date, a statement about what some document says. Each should trace to a recorded origin, a rule, or a stated judgment. If one cannot, it was invented.
- Take one thing you presented as quoted and find it in its source. If you cannot, it was a paraphrase wearing quotation marks.
- Search your output for material that also sits in the record. If it appears as literal text in both places, it will drift — reference it instead.
- Every judgment says what would make it wrong, and carries `seen` so that "wrong" can actually be noticed.
- Anything you could not ground is **written down as unverified**, never given an invented source.
- Nothing you recorded requires a human to keep it up to date.

## What would show this was not worth it

The method promises exactly one measurable thing: **the opening cost of session number N.**
Today that cost is flat — every session re-reads, re-verifies, and sometimes re-invents. If it
is working, a later session reads only what is marked as moved.

An optional introduction should earn its small cost through useful work; a selected mapping
should return the picture the user asked for. If it takes more than the ladder above to explain when to add
structure, the ladder failed. And if after five sessions the sixth does not open cheaper, the
method did not return what it cost, and you can drop it with a clear conscience.

There is no target state here. Refinement is driven by use, not by steps: what gets asked about
gets sharper, what nobody asks stays rough forever, and that is correct. An agent that believes
there is a finished version to converge on will keep polishing when nobody asked.
