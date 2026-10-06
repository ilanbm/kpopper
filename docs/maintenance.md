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
`followups show` includes the archive head and recent attempts. Preserve the ledger and both
history directories together for backup, relocation of state, or recovery. To recover missing
or damaged history, restore the exact digest-named segment from that backup; never delete or
rewrite an archive reference to bypass the failure. Completed request idempotence includes
retained segments. Local retention needs available disk space; a write/storage failure is an
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
Recurring checks finish `checked` with a justified future `next_at`; host scheduling and
execution receipts remain separate from the source observation.

## Runtime compatibility

Capturing maintenance promotes only that local followup ledger to version 2. Ledgers that
have never captured maintenance remain version 1. A historically promoted ledger remains
version 2 even after its last maintenance declaration is removed; it never auto-demotes.
Metadata version 1 pins the policy digest's v1 field set. Future metadata versions must use
a separate schema while preserving validation of recorded v1 digests. Older coordinators must refuse the new ledger instead of treating
required maintenance semantics as ordinary work. Preserve the full ledger and its evidence;
do not lower its version or delete metadata to make a downgrade appear compatible. A prior
consumer can still read the unchanged knowledge record, but reading or operating a version 2
maintenance ledger requires a supporting runtime. Host runtime reconciliation and rollback
therefore need their own receipt.
