# Falsifiers: what would make a judgment wrong

The long-form of the one rule the [record skill](../../record/SKILL.md) states in a paragraph: how to look for a failure the current check would miss, which falsifiers are sealed by the decision they judge, what the reader can evaluate, and how a session's own prior enters the record as a source.

## Look for a failure the current check would miss

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

## A falsifier your own decision can suppress is no falsifier

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

## Make `wrong_if` a predicate where you honestly can

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

## An agent's prior as a source

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
