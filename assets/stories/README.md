# Illustrated README stories

The series uses editorial ink illustration, strong black and electric-blue
typography, and independent source or decision panels that meet in a larger
consequence or synthesis scene. The PNGs are illustration assets, not product
screenshots or editable SVGs.

| Story | Desktop | Phone | What the result means |
|---|---|---|---|
| Private data exposure (assumption checks) | [Image](cache-privacy.png) | [Image](cache-privacy-mobile.png) | A recorded condition fails when results cease to be public. |
| Premature file deletion (consistency) | [Image](download-promise.png) | [Image](download-promise-mobile.png) | A recorded condition fails when retention is shorter than the promised window. |
| Outdated planning assumptions (freshness) | [Image](cowork-workshop.png) | [Image](cowork-workshop-mobile.png) | The recorded format changes; the agent needs to review the saved plan. |
| Dark matter across studies (evidence synthesis) | [Image](dark-matter.png) | [Image](dark-matter-mobile.png) | Three sourced findings support a synthesis under stated models; open questions remain visible. |

Parenthetical labels identify each story's emphasis. Consistency compares the storage
policy with the download promise; freshness asks whether the saved plan's premises
still match the updated brief. These descriptions can overlap across examples.

Desktop images are 1536×1024. Phone images are 1024×1536 and stack the input panels,
with connectors that collect independent inputs rather than making them sequential.
The README selects phone layouts through `picture` at widths up to 600 pixels.
Essential explanations also appear as text and image descriptions.

The first two stories use green for the branch tests and clean merge, and red for
the failed recorded condition. Their consequence scenes are labelled as risks;
the [executable examples](../../examples/merge-assumptions/README.md) reproduce the
specific problems and also show that integration tests can catch them.

The third uses amber for review. Its
[before and after records](../../examples/cowork-workshop/README.md) leave the saved
plan intact and report the changed premise. They do not prove the plan false or
claim the software interprets client briefs automatically.

The fourth story uses three real papers in a reconstructed research workflow.
Its astronomical drawings are schematics, not paper figures or measured data.
Research tasks are labelled as agents; the synthesis is a judgment under explicit
assumptions. Amber marks an open question or reason to revisit it, never a certified
scientific result. The [runnable replay](../../examples/dark-matter/README.md) checks
concurrent record writes, not the research ability of live models.

The first three scenarios are fictional. Follow the [brand guide](../brand-guide.md) when
extending the series, and retain both the phone reading order and the distinction
between a failed condition and a request for judgment.
