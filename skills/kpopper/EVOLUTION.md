# Evolve the representation while preserving what was known

When a write is refused, inspect the affected entry, its source/date, and what
depends on it. Decide whether the new evidence changes a value or exposes a poor
representation. Do not just cast a number to text or repeat the rejected action.

Choose the representation that expresses the same source-grounded meaning:

- A mutable attribute has a stable identity and a value outside its name.
- A dated historical proposition stays true about its own date; becoming historical
  is not evidence that it was false.
- A value determined by other entries is a rule, not another manually synchronized
  copy. A boolean proposition may be calculated from a numeric attribute.

Within the user's existing task authority, choose and execute a supported repair;
routine representation work needs no redundant approval. This does not authorize
deleting evidence, changing what a subject means, overriding a standing judgment,
accepting a contested claim, or changing a record's authority/profile. If the
available operation refuses the required transition, explain the boundary rather
than editing the record files to bypass it.

## Stored scalar to derived value under the same identity

On an active `core/v1` history, `add --reframe` replaces an existing stored scalar
with a structured rule. It keeps the subject ID, writes a new immutable revision,
and preserves old versions and source citations in history. Existing dependents
keep their references and historical `seen`; they are not silently reviewed.

The new rule must read recorded inputs and compute exactly the current scalar's
type and value. `--why` explains why it still describes the same subject. Current
equality is a safety check, not proof that the formula is semantically correct for
every future value; the author must justify that meaning from the sources.
Hosts can also pass `--expected-record-sha256` from the record image they read.
The active-history writer checks that image inside its guarded capture before
preparing the mutation; a changed record must be read again, not silently adopted.

For example, given a recorded numeric `tank.level` of 7 and a stored boolean
`tank.at_seven` of true:

```sh
kpop add tank.at_seven 'rule={expr: "tank.level == 7"}' --reframe \
  --why 'The same level-seven proposition is now derived from the measured level'
```

Changing `tank.level` later changes the computed boolean without setting that
boolean separately. `pull` reads the result; `affects` shows its dependent chain.

If new evidence also changes the truth/value, first record that evidence with its
source and date through the normal guarded writer. Then reframe the now-current
value. This separates a factual change from a representation change, and a failed
reframe leaves the already valid record intact.

The operation refuses unknown results, changed types/values, constants, existing
rules, judgments, hypothesis targets, and legacy records without active core
history. It is not a generic rename, delete, merge, or schema migration. Adding a
numeric field does not establish that two differently named quantities mean the
same thing: a current item index and a count of completed items can differ.

## Finish means checked, not merely written

Read back the changed entries and run `check`. A successful write receipt says the
write was committed, not that the entire task is correct. A validation failure
remains part of the conversation: inspect it, repair within the existing authority,
and validate again. Respect bounded retries; report unresolved failures honestly.
Never replace an executable condition with prose to make a warning disappear,
refresh another judgment's `seen` automatically, or claim an unsupported formula
was computed.
