# Living travel-rights model example

This generated, read-only excerpt and its results come from the **unpublished local source
commit** `cccc9c674293a075cdb88d6c40c639ffdf37336a` of `living-travel-rights`. There is no
public source URL. `55a95eae…` is the SHA-256 of that commit's `GROUNDING.yaml`, not the
whole Git bundle. Each result also carries a `baseline_authority_digest` for native history;
the source bundle is omitted here, so that digest cannot be independently verified from this
example. The [manifest](manifest.json) records source and generated-file hashes.

The excerpt is explicitly incomplete and cannot execute the model by itself. It retains source
IDs and passage locators, but the complete record and source bodies are omitted. The table
below identifies the referenced source entries and their recorded dates. It does not reproduce
or validate source text; URL endpoints may show content newer than the recorded edition.

| ID | Recorded source | Locator | Edition as recorded; read date |
|---|---|---|---|
| `source.definitions` | 14 CFR 260.2 | [eCFR](https://www.ecfr.gov/current/title-14/chapter-II/subchapter-A/part-260/section-260.2) | Edition as-of 2026-09-23; read 2026-09-25 |
| `source.refunds` | 14 CFR 260.6 | [eCFR](https://www.ecfr.gov/current/title-14/chapter-II/subchapter-A/part-260/section-260.6) | Edition as-of 2026-09-23; read 2026-09-25 |
| `source.renumbering` | DOT notice, 91 FR 41556–41557 | [GovInfo](https://www.govinfo.gov/content/pkg/FR-2026-07-07/html/2026-13675.htm) | Edition as-of 2026-07-07; read 2026-09-25 |
| `source.model_scope` | Authored scope interpretation | `docs/scope.md` in the source commit | of/read 2026-09-25 |
| `source.case` | Synthetic input | [case.json](case.json) | Captured example input; no source-edition claim |

The manifest's `case_sha256` is the adapter's canonical case-input digest (also reported as
`input_digest`), not the raw `case.json` byte hash. That raw file hash is under
`files["case.json"]`. The private export also generated `docs/scope.md`, a transformed
`docs/sources.md`, and the full source ZIP; those are omitted here. This directory contains
the synthetic case, excerpt, manifest and result rows, not the complete source bundle or its
captured source bodies.

In the captured case, `flight_cancelled` and `renumbering_only` are both `false`. Both baseline
components fail; under the hypothetical 120-minute domestic threshold, arrival-change passes
while cancellation still fails because this case supplies no cancelled flight. The cancellation
row's `scope_reason` describes the model's general renumbering-only coverage limit; it is not
the active reason for this case and explicitly does not deny a right. See the [captured rows](results.json)
and [case](case.json). Both results keep `other_rights_not_ruled_out: true` and the coverage
notice. These are outputs of the identified adapter and runtime for the captured inputs, not a
complete rights finding.

Files: [synthetic case](case.json) · [read-only model excerpt](model-excerpt.yaml) ·
[captured results](results.json) · [source/export manifest](manifest.json).
