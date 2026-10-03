# Living travel-rights model example

This generated, read-only excerpt and its results come from **private source commit**
`39932a684c25cc87851ccf313ea3ea8919ad10b9` of `living-travel-rights`. The complete source
repository is not public. `55a95eae…` is the **`GROUNDING.yaml` SHA-256**, not a hash of the whole
Git bundle. Results carry `baseline_authority_digest` as an opaque native-history provenance
receipt. The manifest and results include other source/runtime/input digests as provenance
receipts too. Because their contents are omitted, those values cannot be independently checked
from this example. Only hashes for shipped files can be verified against `manifest.files`; see
the [manifest](manifest.json).

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

The manifest's `case_sha256` is the adapter's canonical-input digest (also reported as
`input_digest`); this adapter-defined representation is not specified here. It is not the raw
`case.json` byte hash. That raw file hash is under
`files["case.json"]`. The private export also generated `docs/scope.md`, a transformed
`docs/sources.md`, and the full source ZIP; those are omitted here. This directory contains
the synthetic case, excerpt, manifest and result rows, not the complete source bundle or its
captured source bodies.

In the captured case, `flight_cancelled` and `renumbering_only` are both `false`. Both baseline
components fail; under the hypothetical 120-minute domestic threshold, arrival-change passes.
The baseline cancellation component fails with `flight_cancelled: false`. Its `scope_reason` is
general component metadata about renumbering-only coverage, not an active exclusion in this case;
the case sets `renumbering_only: false`. That metadata also says the scope limit is not a denial
of a right. See the [captured rows](results.json) and [case](case.json). Both results preserve
`other_rights_not_ruled_out: true` and the coverage notice. These are outputs for the captured
inputs, not a complete rights finding.

Files: [synthetic case](case.json) · [read-only model excerpt](model-excerpt.yaml) ·
[captured results](results.json) · [source/export manifest](manifest.json).
