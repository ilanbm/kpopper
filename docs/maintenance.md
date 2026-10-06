# Declaring clock and source maintenance

Maintenance intent is an explicit local followup declaration. It names recorded subjects,
a clock deadline or source inspection, an interval in whole calendar days, and the evidence
required for its intended use. Ordinary source text and `wrong_if` prose do not create a
policy or grant permission.

`kpop followups compile --file DECLARATION.json` reads intent and returns a proposal. Missing
choices appear in `unresolved`; an incomplete declaration has no executable specification.
A complete proposal still has unresolved authorization and no local registration marker.
Compilation does not write a record, create a followup, fetch a source, or install a host job.
When compiling a local file, the proposal links that file as its task. Its bytes remain
pinned: even an innocuous edit needs an explicit `followups refresh` before inspection.

A source declaration looks like this. Values illustrate selected policy, not defaults:

```json
{
  "schema": "kpopper.maintenance-declaration/v1",
  "kind": "source",
  "id": "vendor-status",
  "title": "Inspect the vendor status",
  "why": "The active migration depends on this source",
  "how": "Read the selected status and retain evidence of the result",
  "scope": "Read the selected status and propose changes; no publication",
  "related": ["service.endpoint"],
  "cadence_days": 7,
  "timezone": "Europe/Prague",
  "check_time": "09:00",
  "use_policy": "allow_cached_until_expiry",
  "evidence_requirement": "host_attested",
  "first_due_at": "2026-10-12T07:00:00Z",
  "source": {
    "source_id": "vendor-status",
    "locator": "https://vendor.example.test/status",
    "publisher": "Selected vendor",
    "selection": "Current migration status",
    "adapter": "selected-host-read/v1",
    "tool_policy": "Use the existing authorized read tool",
    "allowed_roots": ["https://vendor.example.test/"],
    "evidence_format": "selected-text-and-tool-output",
    "max_age_hours": 48
  }
}
```

Source checks use a local task and an `at` trigger. They do not depend on their own external
observation, so missing or expired evidence cannot prevent the inspection needed to renew it.
Source identity includes locator, publisher, selection, adapter, tool policy, allowed roots
and evidence format. Changing those semantics creates a different observation reference;
changing cadence preserves compatible source evidence. Age is explicit for these declarations;
legacy external triggers that omit age retain their existing 24-hour behavior.

For `kind: clock`, omit `source` and `first_due_at`, set `use_policy: require_live` and
`evidence_requirement: trusted_clock`, and provide either `deadline: {"utc": "...Z"}` or
`deadline: {"date": "2026-10-12", "timezone": "Europe/Prague", "boundary": "midnight"}`.
`end_of_day` selects the final microsecond before the following local midnight. Ambiguous or
nonexistent local boundaries remain unresolved; select an explicit UTC instant. No source
fetch receipt is invented for clock checks.

After the user has selected scope and authorized local capture, submit the proposal's `spec`
to `followups add`. The registration digest is local to its workspace and record. It is an
audit marker, not proof of source-access permission, a host binding, or successful refresh.
Copying a declaration never copies that marker. Host execution still requires the actual
user authorization, supported tools, and separately verified owner/configuration.

## Recording an inspection

The host uses its existing authorized tools and retains actual output and selected passage.
`followups inspect --file REPORT.json` accepts the followup `id`, `policy_digest`, `source_ref`,
matching `inspection` descriptor, bounded `value`, `inspected_at` with explicit offset, and
an `evidence` reference. These are host attestations; the command performs no HTTP request
and authenticates no origin, redirect or transport claim. `trusted_origin` is retained as an
intended evidence requirement; this host-attested route cannot establish it.

The store creates its own `receipt_at`, separately from the attested inspection instant.
Missing value/time, mismatched selection, future inspection time, or older/conflicting
observations become unavailable attempts while preserving the previous success. A failure
without a usable observation uses `followups attempt` with the same identities, evidence
and bounded `reason`. Invalid typed input is refused and can be reported through that
unavailable-attempt path. Successful attempts retain an observation digest and their native
receipt, so a later success is distinguishable from a previous failed attempt.

Each attempt explicitly records host-attested recording, the declared requirement, whether
that recording meets the requirement, and `current_use_adequacy: unassessed`. An `observed`
outcome means a guarded observation was recorded. It does not establish current freshness,
applicability, truth, or permission. Assess adequacy with current time at each material use;
a receipt never renews an old inspection time. Legacy `observe` cannot write a maintenance
source reference, including a retained maintenance observation after its policy changes.

Evidence references and unavailable reasons are at most 512 bytes. Link full output and
diagnostics instead of copying their bodies. At 67 attempts, the ledger compacts to 32 recent attempts plus the latest successful
inspection and refresh when older. Hot history is bounded at 66 attempts. Older attempts move automatically to
immutable, digest-verified linked segments in its `attempt-history/` directory. Each successful
observation is retained by digest in `observation-history/`; its attempt names that digest.
`followups show` includes the archive head and recent attempts. Normal loads verify only the
current archive head to keep recurring work bounded. Retried old finishes verify every visited
segment; older-chain and observation-directory completeness is not certified by a normal load.
Keep full backups and treat a traversal/recovery error as incomplete retained evidence. Preserve the ledger and both
history directories together for backup, relocation of state, or recovery. To recover missing
or damaged history, restore the exact digest-named segment from that backup; never delete or
rewrite an archive reference to bypass the failure. Completed request idempotence includes
retained segments. Retention writes use atomic replacement. An interrupted or damaged
unreferenced file can be reconstructed from complete retained/report evidence; damaged bytes
are preserved in a `.corrupt-...json` file first. Referenced history that cannot be reconstructed
still requires the exact backup; a missing receipt is never invented. Local retention needs available disk space; a write/storage failure is an
explicit failure, not a successful refresh. No cumulative attempt cap disables an obligation.

Authorized edits within the existing inspection scope use `followups refresh --evidence ...`.
A change to source identity, inspection scope, executor ownership, use/evidence policy,
an increased maximum age or cadence, delayed due time, changed check time/timezone,
or removal of maintenance additionally requires `--authorization-evidence` referencing actual
existing or new user authorization. Reusing an applicable standing grant does not require
renewed permission. Same-scope descriptive edits and stronger age/cadence bounds need no
new authorization reference. That reference records the caller's basis; source prose does
not establish it. Keep unchanged inspections local. Changed evidence enters ordinary
proposal/affects/review work; inspection does not accept semantic changes or publish a model.
Rebinding maintenance to another record with `followups relocate` likewise needs
`--authorization-evidence`; relocation evidence and authorization are separate references.
Cancelling/closing a maintenance obligation with `finish cancelled` or `resolve` requires
`--authorization-evidence`, separately from the outcome narrative. Routine lease release
cannot unpark a check stopped after repeated source failures; explicit reconciliation/resume
remains necessary.
Recurring checks finish `checked` with `next_at` computed from the admitted check's local
calendar day plus `cadence_days`, at the declared `check_time`; an arbitrary later date is
refused. An unavailable inspection releases its lease into the native bounded retry, or parks
for reconciliation after repeated failures. It cannot finish as a successful checked inspection; host scheduling and
execution receipts remain separate from the source observation.

## Runtime compatibility

Capturing maintenance promotes only that local followup ledger to version 2. Ledgers that
have never captured maintenance or selected a new execution mode remain version 1. A historically promoted ledger remains
version 2 even after its last maintenance declaration is removed; it never auto-demotes.
Metadata version 1 pins the policy digest's v1 field set. Future metadata versions must use
a separate schema while preserving validation of recorded v1 digests. Older coordinators must refuse the new ledger instead of treating
required maintenance semantics as ordinary work. Preserve the full ledger and its evidence;
do not lower its version or delete metadata to make a downgrade appear compatible. A prior
consumer can still read the unchanged knowledge record, but reading or operating a version 2
maintenance ledger requires a supporting runtime. Host runtime reconciliation and rollback
therefore need their own receipt.


## One workspace wake and local execution intent

`followups daily mode paused --authorization-evidence REF` pauses local automatic execution;
it changes no host schedule and creates no host readback. `manual` selects current-session
work only; use `daily start --owner SESSION --manual-evidence REF` for an explicitly requested
manual review. Source access still uses its own actual authorization. Mode/adoption metadata
promotes the local ledger to version2 so older coordinators cannot silently ignore it.

For N greater than one, a newly selected daily fallback requires `daily mode daily_fallback
--authorization-evidence REF --acknowledge-empty-run-cost`. This records the selection and
acknowledgement that otherwise empty agent wakes can spend tokens; it installs nothing.
An already owned daily wake can serve due work alongside its existing authorized tasks.
Maintenance compatibility requires a read-back daily time in the policy timezone at or after
the recurring `check_time`. An earlier time is incompatible; absent phase or a different
timezone is unproven. Repair requires explicit host authorization or select manual mode;
selection never changes the owner or policy. The first due time can differ and be caught up
once without proving the recurring phase.

Native mode records an actual normalized host readback with `daily mode native --file FILE
--authorization-evidence REF`. The file contains host/id, positive whole `cadence_days`,
`anchor_at`, timezone, `interval_semantics: calendar_days`, actual host-attested `observed_at`
and an evidence reference. Unknown semantics, absent/expired phase data and incompatible
ordinary obligations stay unknown. Attested compatibility is configuration evidence, not a
host execution receipt or a scheduling guarantee. With no owner, installation returns a
native-admission inspection request; supported host tools and actual readback remain necessary.

A fixed Monday native N=7 wake caught up on Tuesday cannot serve Tuesday+7 exactly. Status
exposes the incompatible phase and requires a separately selected daily fallback or manual
mode. It never silently changes or replaces the old owner. Paused, missing, incompatible and
unverified execution remain visible independently of source age and guidance preference.


## Scoped current use and source/model review

`followups assess --ids SUBJECT...` reads the actual declared transitive closure and current
native time. It separates evidence/alignment adequacy from applicability, truth, authority and
the consumer artifact/version. Cycles, missing required policies, uninspectable scope, pending
alignment and expired evidence remain explicit; unknown takes precedence without hiding stale
reasons. Historical facts do not become false merely because current-use evidence expires.

A fresh observation does not align an old canonical decision automatically. Perform the actual
ordinary `affects`/proposal/hypothesis/review/history work first. `followups review-source ID
--outcome candidate_pending --evidence REF --authorization-evidence REF` records that the
candidate remains unaccepted. After the actual authorized review, `no_model_change_needed` or
`reviewed_model_update` correlates that review with the native captured model-scope identity
and exact guarded observation. This command writes no knowledge/model acceptance or publication,
and its references are not proof of authority. Changing the selected observation or declared
model scope invalidates the correlation. An unchanged selected state with an unchanged model
scope can carry the existing correlation to a later guarded inspection; no recurring semantic
approval or date bump is invented.

Require-live use needs a current owned source claim and matching successful inspection after
its native start, supplied with `assess --claim-token TOKEN`. A separately authorized current
user request can take a source lease before its periodic due time using `claim ...
--current-use-authority REF`; this is manual current-use work, not a background budget bypass
or source-access permission. Release this lease after use; it cannot finish checked or silently
shift the periodic due time. Periodic sources cannot finish checked without a successful
current-claim inspection. A cached policy can retain finite-window evidence after a failed
attempt while explicitly disclosing that failure; exactly at expiry it is stale. No receipt
renews an old inspection timestamp, and a current clock/attestation does not certify current law.


## Daily execution evidence

A local daily completion records work performed. Manual and legacy or unattested completions
leave the host configuration unverified; owner text does not identify a scheduled invocation.
`daily start --manual-evidence REF` retains the current-user authorization and manual origin.
These receipts do not establish automatic host execution.

When an authorized host tool supplies an actual readback of the current scheduled invocation,
pass it with `daily start --host-execution FILE`. Its exact fields are `schema:
kpopper.host-execution/v1`, `trigger: scheduled`, `host`, `id`, `executed_at`, `observed_at`,
and `evidence`. The owner must match the active binding, execution must follow the binding identity epoch, and the report must describe an invocation within ten minutes, with ordered current
timestamps. Manual and host-execution flags cannot be combined. A matching completed receipt
records `scheduled_execution_reported`: caller-supplied execution information, not authenticated
host proof. A local binding epoch separates owner/configuration changes from repeat readbacks;
refreshing the same owner does not discard the current report. Reports have their own finite
liveness window and establish no future scheduler availability, source truth or semantic
acceptance. This command changes no host job.


Routine shown, decline, snooze and authorization choices live in private first-use state,
including the actual unexpired snooze time. A routine acknowledgement does not upgrade an
ordinary ledger. Actual maintenance capture or an execution-mode choice remains a version2
boundary; do not lower an existing ledger version to make rollback appear supported.

A received authorized native wake may run due catchup work even when phase readback is old,
unknown or incompatible. Its packet preserves the unknown/repair status; this does not select
a daily fallback, acknowledge empty-run cost, change the owner or rewrite a schedule. Phase
compatibility describes calendar phase only, not action capacity, maximum source age, host
authentication or guaranteed execution. Ordinary obligations require their own compatibility.

Calendar recurrence starts from the admitted periodic claim's local day. On a DST overlap,
the check uses the earliest valid instant; in a gap it uses the next valid instant. The declared
policy time and timezone stay intact. Closed obligations remain visible as history but do not
create ongoing degradation or repeated health fingerprints.

No-policy or no-success snapshots describe local maintenance assurance as unknown; they do
not prove that recorded facts are invalid. Relevant ongoing clock/source work needs a scoped
proposal before closing the response, with unresolved choices explicit. Material current use
must disclose actual failure reasons and known last successful observation and due timestamps.
Stable, historical, closed or unrelated answers stay quiet, and saved suppression never becomes
permission or evidence of freshness.

A missing required attempt-history segment makes the validated ledger unavailable across
ledger-backed followups. Startup keeps its workspace and ordinary guidance and reports that
scope honestly. Restore the required retained segments or reconcile an intact backup; never
fabricate missing history or claim unrelated ledger rows were validated.
