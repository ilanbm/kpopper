# Delivery-policy conversation

This small fictional record supports the [conversation walkthrough](../../docs/session-lifecycle.md).
It demonstrates original bodies, sources, exact reads, query anchors and a changed
revision. It does not require a model API, an external service or delta mode.

Use a published native `kpop` with its bundled resources. This is an ordinary
record, so every session command uses `checked-reader/v1`. From this directory:

```sh
kpop session open --no-settings --project delivery-example \
  --assessment-profile checked-reader/v1 --tokens 16000
```

Copy the returned `revision=` value into `REV`; it is neither `snapshot_id` nor
`findings_revision`. Keep the same directory, project and assessment profile:

```sh
REV='PASTE_THE_RETURNED_REVISION'

# The complete small graph can fit in this view.
kpop session view --no-settings --project delivery-example \
  --assessment-profile checked-reader/v1 --revision "$REV" --tokens 16000

# A question and a follow-up with an explicit original-ID ranking hint.
kpop session view --no-settings --project delivery-example \
  --assessment-profile checked-reader/v1 --revision "$REV" --tokens 16000 \
  --query "standard delivery"
kpop session view --no-settings --project delivery-example \
  --assessment-profile checked-reader/v1 --revision "$REV" --tokens 16000 \
  --query "express" --anchor policy.standard_days

# Request the exact body, then just its original value field.
kpop session view --no-settings --project delivery-example \
  --assessment-profile checked-reader/v1 --revision "$REV" --id policy.standard_days
kpop session read --no-settings --project delivery-example \
  --assessment-profile checked-reader/v1 --revision "$REV" \
  --ref 'node:policy.standard_days#/body/v'
```

Expected readings: standard is five working days, express is two and restricted
to the local region, weekend dispatch is false. No dispatch receipt exists here.
The view dictionary maps display aliases back to original IDs. Because the record
is small, a focused view may still contain every body; an anchor need not change
the resulting selection.

In an installed Codex session, follow the issued startup route instead. Eligible
answer citations can supply automatic anchors there. The standalone commands above
return full selected views and use only the explicit anchor you provide.

## Demonstrate an update in a disposable copy

Copy this entire directory to a scratch workspace before writing. Update the
fictional source's section 1 there to seven working days, then run:

```sh
kpop set policy.standard_days 7 --as-of 2026-09-29 \
  --why "Fictional revised delivery policy, section 1"
kpop affects policy.standard_days
kpop check
```

The old session revision must now refuse a read. Run `session open` again and
replace `REV` to read seven. Writing a changed policy does not establish that any
parcel was sent, and reopening does not approve affected judgments. This context
upgrade does not require migrating the record's persistent format.
