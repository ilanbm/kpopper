# Illustrated README stories

The series uses editorial ink illustration, strong black and electric-blue
typography, and independent source or decision panels that meet in a larger
consequence or synthesis scene. Start with those panels rather than an umbrella
title. The two merge stories close with their punchlines; the workshop and research
stories retain their existing conclusions without another headline. The PNGs are
illustration assets, not product screenshots or editable SVGs.

| Story | Desktop | Phone | What the result means |
|---|---|---|---|
| Private data exposure (assumption checks) | [Image](cache-privacy.png) | [Image](cache-privacy-mobile.png) | A recorded condition fails when results cease to be public. |
| Premature file deletion (consistency) | [Image](download-promise.png) | [Image](download-promise-mobile.png) | A recorded condition fails when retention is shorter than the promised window. |
| Outdated planning assumptions (freshness) | [Image](cowork-workshop.png) | [Image](cowork-workshop-mobile.png) | The recorded format changes; the agent needs to review the saved plan. |
| Dark matter across studies (evidence synthesis) | [Image](dark-matter.png) | [Image](dark-matter-mobile.png) | Six papers contribute detailed readings, model constraints and a connected synthesis with open questions. |
| Three-paper research introduction | [Image](dark-matter-intro.png) | [Image](dark-matter-intro-mobile.png) | The earlier ink illustration introduces rotation, lensing and CMB evidence before the expanded six-paper view. |

Each main example also has a close-up drawn from its complete record:

| Record | Desktop | Phone | Retained context |
|---|---|---|---|
| Search service | [Image](cache-privacy-record.png) | [Image](cache-privacy-record-mobile.png) | 13 readings, four judgments and two open questions. |
| Cooking workshop | [Image](cowork-workshop-record.png) | [Image](cowork-workshop-record-mobile.png) | 13 readings, five saved plans and four open questions. |
| Dark-matter research | [Image](dark-matter-record.png) | [Image](dark-matter-record-mobile.png) | 38 readings, one calculation, seven judgments and five open questions. |

The close-ups select actual entries; their desktop minimaps are generated from all
lines of the linked YAML files. They do not invent extra background content.

Parenthetical labels identify each story's emphasis. Consistency compares the storage
policy with the download promise; freshness asks whether the saved plan's premises
still match the updated brief. These descriptions can overlap across examples.

Desktop overviews and close-ups use widths suited to their content. Phone images are 1024 pixels wide and stack
the input panels, with connectors that collect independent inputs rather than
making them sequential.
Heights follow the content, including any closing statement.
README images use ordinary Markdown for compatibility with repository clients.
Each illustration links to its dedicated phone layout. Essential explanations
also appear as text and image descriptions.

The first two stories use green for the branch tests and clean merge, and red for
the failed recorded condition. Their consequence scenes are labelled as risks;
the [executable examples](../../examples/merge-assumptions/README.md) reproduce the
specific problems and also show that integration tests can catch them.
The cache story closes with **Two green PRs. One data leak.** to name the privacy
consequence directly.

The third uses amber for review. Its
[before and after records](../../examples/cowork-workshop/README.md) leave the saved
plan intact and report the changed premise. They do not prove the plan false or
claim the software interprets client briefs automatically.

The research story uses six real papers in a worked literature example.
Its astronomical drawings are schematics, not paper figures or measured data.
Sources, shared data and modeling assumptions remain distinct; the synthesis is an
authored judgment. Amber marks an open question or reason to revisit it, never a certified
scientific result. The [runnable replay](../../examples/dark-matter/README.md) checks
concurrent record writes, the exact density calculation and review propagation,
not the research ability of live models.

The software and workshop stories are worked examples. Follow the [brand guide](../brand-guide.md) when
extending the series, and retain both the phone reading order and the distinction
between a failed condition and a request for judgment.
