# A new offer. An old deadline.

A mortgage plan uses the expiry date in Offer A. Offer B arrives and replaces it,
but gives no expiry date. A later session needs to know which deadline it can rely on.

This is an illustrative reconstruction of a source-and-decision problem. All documents,
dates and identifiers are fictional. It is not a report of an automatically prevented
mistake or a measured improvement over an agent without kpopper.

## Read the example

- [Offer A](sources/offer-a.md) supplies the original deadline.
- [Offer B](sources/offer-b.md) replaces the offer without specifying a deadline.
- [Before](before/GROUNDING.yaml) records why the plan used 18 March.
- [After](after/GROUNDING.yaml) represents an agent having read and recorded Offer B.
  The earlier decision and its review snapshot are unchanged.

The old date remains a correct reading of Offer A. What needs review is its use in
the current plan. The checker can detect that the offer changed; interpreting the
new letter and deciding what to ask the bank is the agent's job.

## Run the checks

From the repository root, with an installed `kpop` on PATH:

```sh
kpop check examples/offer-review/before/GROUNDING.yaml
kpop check examples/offer-review/after/GROUNDING.yaml
kpop open --chars 4000 examples/offer-review/after/GROUNDING.yaml
kpop pull plan.application_deadline examples/offer-review/after/GROUNDING.yaml
```

Both checks exit successfully. The second reports `MOVED plan.application_deadline`:
`offer.current_id` changed from `A` to `B`. Opening the after record surfaces the
changed premise; pulling the decision also exposes its reason and prose review condition.
No `wrong_if` predicate fires, no model is called, and these commands do not rewrite
either record or read the source letters to interpret their meaning.

## Try the continuation with an agent

Give an agent access to this directory and ask:

> Continue the mortgage application plan from the after record. Read the relevant
> source letters. Which deadline can we rely on now, and what is the next step?
> Explain what is documented and what still needs confirmation. Do not change files.

A supported response should distinguish the old offer's documented expiry from
the new offer's unknown expiry, and identify the need for written confirmation of
the new offer's validity and application conditions. It should not invent a date,
assume the old date carries over, or claim the checker proved the plan false.

This is a small, inspectable demonstration, not a blind evaluation: the expected
reasoning is published here. A benefit comparison needs separate, unseen cases and
an agent baseline with the same source access.

## When does the review happen?

`reopened_by` preserves a condition that needs interpretation. It does not schedule
a model call. Here, updating a declared premise creates a review notice; an active
agent can read it when returning to the plan. A configured followup can ask an agent
to revisit the matter on a date or after a recorded change. The optional daily
review can also select a flagged decision, within its work budget.

If the new letter only appears on disk and nobody records it or reads it during
the task, these snapshots do not change. If nothing is scheduled and no agent is
working on the project, the prose condition does not run in the background.
See [followups and scheduling](../../skills/kpopper/FOLLOWUPS.md).
