<a id="new-format-old-assumptions"></a>

# Outdated planning assumptions: one update, several plans to revisit

A cooking workshop was planned onsite around a shared kitchen, six workstations
and ingredients supplied at the venue. A later client brief moves it online.
The saved plan now spans an agenda, equipment, ingredient preparation, groups
and supervision, with the source of each assumption attached.

<p align="center">
  <a href="../../assets/stories/cowork-workshop.png">
    <picture>
      <source media="(max-width: 600px)" srcset="../../assets/stories/cowork-workshop-mobile.png">
      <img src="../../assets/stories/cowork-workshop.png" alt="The client moves a cooking workshop online while the saved plan still assumes one shared kitchen. The changed format prompts review of equipment, ingredients and activities.">
    </picture>
  </a>
</p>

[Phone layout](../../assets/stories/cowork-workshop-mobile.png)

## Inside the planning record

<p align="center">
  <a href="../../assets/stories/cowork-workshop-record.png">
    <picture>
      <source media="(max-width: 600px)" srcset="../../assets/stories/cowork-workshop-record-mobile.png">
      <img src="../../assets/stories/cowork-workshop-record.png" alt="A planning record retains three planning passes, 13 readings, five linked plans and four open questions. The new format is remote while every saved plan still has its onsite review snapshot.">
    </picture>
  </a>
</p>

[Phone layout](../../assets/stories/cowork-workshop-record-mobile.png) · [Before record](before/GROUNDING.yaml) · [After record](after/GROUNDING.yaml)

The current format is `remote`. The five saved plans were reviewed with `onsite`.
The checker reports five `MOVED` notices and exits zero. It does not decide whether
the recipe, group arrangement or activities can work online.

| Planning pass | What it contributes | What the later update changes |
|---|---|---|
| [Original brief](sources/original-brief.md) | Shared kitchen, equipment and ingredients at the venue. | The working format changes to remote. |
| [Session plan](sources/session-plan.md) | 18 participants, 90 minutes, recipe and six groups of three. | The group arrangement and agenda need review; client constraints retain their original source. |
| [Logistics and safety](sources/logistics-plan.md) | Six workstations, ingredient preparation and direct instructor visibility. | Equipment access, distribution and supervision need review. |
| [Updated brief](sources/updated-brief.md) | Participants join online from home. | Home equipment and ingredients remain unspecified. |

## The change reaches the plan

```mermaid
flowchart TD
  B[Updated client brief] --> F[workshop.format: remote]
  F --> A[Agenda]
  F --> E[Equipment plan]
  F --> I[Ingredient preparation]
  F --> G[Group arrangement]
  F --> S[Safety and supervision]
  A --> R[Five plans need judgment]
  E --> R
  I --> R
  G --> R
  S --> R
  C[18 participants and 90-minute slot] -.-> U[Recorded constraints retained]
```

The retained venue facts describe the original venue. They do not become facts
about participants' homes. The record's open questions concern home equipment,
ingredient distribution, dietary restrictions and remote activities.

Selected entries from the after record:

```yaml
known:
  workshop.format:
    v: remote
    from: s.updated_brief
    at: Participants join online from home
  workshop.participants:
    v: 18
    from: s.session_plan
    at: Client constraints
  workshop.duration_minutes:
    v: 90
    from: s.session_plan
    at: Client constraints
  venue.workstations:
    v: 6
    from: s.logistics_plan
    at: Venue provision
judgments:
  workshop.agenda:
    rests_on:
    - workshop.format
    - venue.shared_kitchen
    - workshop.duration_minutes
    - workshop.recipe
    verdict: Use the shared-kitchen agenda and provide ingredients at the venue.
    reopened_by: The format or access to the shared kitchen changes; review activities, equipment
      and preparation.
    seen:
      workshop.format: onsite
      venue.shared_kitchen: true
      workshop.duration_minutes: 90
      workshop.recipe: Fresh pasta with tomato sauce.
  workshop.equipment_plan:
    rests_on:
    - workshop.format
    - venue.equipment_supplied
    - venue.workstations
    verdict: Plan equipment around the six venue workstations.
    reopened_by: The format, supplied equipment or access to the workstations changes.
    seen:
      workshop.format: onsite
      venue.equipment_supplied: true
      venue.workstations: 6
  workshop.ingredient_plan:
    rests_on:
    - workshop.format
    - venue.ingredients_supplied
    - workshop.participants
    verdict: Prepare ingredient portions at the venue for the 18 participants.
    reopened_by: The format, ingredient provision or participant count changes; review purchasing,
      portions and distribution.
    seen:
      workshop.format: onsite
      venue.ingredients_supplied: true
      workshop.participants: 18
```

## Read and run

```sh
kpop check examples/cowork-workshop/before/GROUNDING.yaml
kpop check examples/cowork-workshop/after/GROUNDING.yaml
kpop affects workshop.format examples/cowork-workshop/after/GROUNDING.yaml
```

Both checks exit zero. The after check reports `MOVED` for `workshop.agenda`,
`workshop.equipment_plan`, `workshop.ingredient_plan`, `workshop.group_plan` and
`workshop.safety_plan`. Their earlier review snapshots are preserved.
No model is called and the commands do not interpret the briefs or rewrite the plans.

## Before the first answer

An enabled session-opening hook reads the existing record before work begins.
Its bounded context exposes the changed plans. You can inspect the same context
from the repository root:

```sh
kpop open examples/cowork-workshop/after/GROUNDING.yaml --chars 1300
```

Selected output (other plans and open questions omitted):

```text
Cooking-workshop plan after the client changes the format; the saved plans await review.
holds: workshop (9) · planning (4) · s (4) · venue (4)
26 entries, 5 judgments, 4 open questions, updated 2025-01-02

needs a person (5):
  workshop.ingredient_plan: workshop.format differs from what it last saw: onsite -> remote
```

The user then asks:

> Prepare the ingredient portions for the 18 participants.

The prompt hook compares those words with the record's IDs, names and verdicts.
For this prompt it returned, with Codex skill syntax:

```text
kpopper: the record holds workshop.ingredient_plan on this - $ground workshop.ingredient_plan before answering from memory.
```

That line names an entry; the agent then retrieves its values and sources:

```sh
kpop pull workshop.ingredient_plan examples/cowork-workshop/after/GROUNDING.yaml --budget 2200
```

Selected output (the other affected plans omitted):

```text
venue.ingredients_supplied: True (Venue supplies ingredients) <- s.logistics_plan, at Venue provision
workshop.format: remote (Workshop format) <- s.updated_brief, at Participants join online from home as of 2025 ...
workshop.participants: 18 (Participants in the client brief) <- s.session_plan, at Client constraints
+ workshop.ingredient_plan: Prepare ingredient portions at the venue for the 18 participants.
    holds
    because: The logistics pass assumes ingredients and participants meet in the same place.
    reopened by: The format, ingredient provision or participant count changes; review purchasing, portions an ...
    moved since review: workshop.format onsite -> remote
```

The venue's ingredient provision is an earlier reading. With the format now remote,
it does not establish how participants will receive ingredients at home. The agent
has enough context to revisit that plan before giving preparation instructions.

The prompt hook emits at most three IDs, not the full record or an answer. It can
also ground matching later prompts, with cooldown and read tracking. These outputs
were checked locally against the 1.7 source on 2026-09-19, using this legacy example
record. The prompt-hook probe is not evidence that every host has the integration
enabled; see the [adapter capability matrix](../../adapters/README.md#capability-matrix).

## Continue with an agent

> Continue planning from the after record. Read the original brief, planning notes
> and updated brief. Which decisions need reconsidering? What remains supported,
> and what must we ask before sending participants an agenda? Do not change files.

A supported response should distinguish the venue's provision from unknown home
setups. It should not assume equipment is missing, declare the workshop impossible
or present an adaptation as approved. These are worked planning materials with a
published expected response, not a blind agent evaluation.

The update becomes visible after an agent reads and records it. Scheduled review
requires configuration; see [followups](../../skills/kpopper/FOLLOWUPS.md).
