# The record's shape

One file, three sections, and the two ways a record lives somewhere other than the project root. Read this when a record is born or when something does not fit the fields it has.

## Where the record lives

The entry point is `GROUNDING.yaml` in the project's working directory, born by the first `kpop add`. Everything the record keeps beside itself lives in `.kpopper/` next to it - `hypotheses/`, `view.yaml`, `measure.yaml`, `session.json`, and `build/` for what is rebuilt from it, which ignores itself. A record born under the earlier name, `PROVENANCE.yaml`, is read as it is, with `PROVENANCE.d/`, `PROVENANCE.view.yaml`, `PROVENANCE.measure.yaml` and `PROVENANCE.session.json` beside it; to bring one over, rename the file and move those four into `.kpopper/` in the same commit - a record moved by half fails `check` and is named by the opener, since nothing reads what was left under the other name. The directory is a dot-directory: a plain `ls` or `rg --files` skips it, so list it as `ls -a .kpopper` or `rg --files --hidden` when looking for the brief or a hypothesis.

**If the project already keeps a record somewhere else** — a `facts.yaml` in a subfolder, a table someone maintains — do not move it and do not duplicate it. Create `GROUNDING.yaml` as a pointer:

```yaml
record: analysis/facts.yaml     # the real record lives here
also:   analysis/claims.yaml
```

One known location, wherever the content actually sits. **Two parallel records is the most expensive mistake available here** — from then on every reader must know both, and it never resolves on its own.

**If the root cannot hold it** — the repository's own rules keep such files out of the tree, or
the checkout is disposable and untracked files die with it — keep the record wherever the
project keeps its untracked material, and register the path with the checkout:

```bash
echo "/abs/path/to/GROUNDING.yaml" > "$(git rev-parse --git-common-dir)/kpopper-record"
```

The opener and every command below then find it from any checkout or worktree of that
repository (`kpop where` prints what they found), and nothing enters the tree. Say
where it went in your reply and in whatever memory the project keeps — a record nobody can
find is a record nobody updates.

## The shape

Three sections, mirroring the three things worth keeping. Grow it freely; do not rename it.

```yaml
meta:
  updated: 2026-08-31

known:        # taken from a source, or worked out from other entries
  acme.seat_price: {v: 42, unit: USD/mo, from: pricing, at: "Enterprise tier"}
  acme.seats:      {v: 180, from: "Dana, in the 28/08 planning call"}
  acme.annual:     {rule: "acme.seat_price * acme.seats * 12"}
  contract.exit:
    quoted: "Either party may terminate for convenience on ninety (90) days' written notice."
    from: msa
    at: "§12.2"

judgments:    # concluded or composed — each says what would make it wrong
  why_acme:
    rests_on: [acme.annual, beta.annual, migration.weeks, contract.exit]
    verdict:  "prefer Acme above ~150 seats"
    because:  "Cheaper only above ~150 seats, the migration cost is one-time, and the
               90-day exit keeps the downside bounded."
    wrong_if: "acme.seats < 150"
    seen:     {acme.annual: 90720, beta.annual: 84000, migration.weeks: 6,
               contract.exit: "90 days"}   # the dependency values at the moment it was written

sources:      # documents, pages, people, messages, a session's prior — things nothing else produced
  msa:      {name: "the master agreement", file: "contracts/acme-msa-2026.pdf", of: "2026-04-02"}
  pricing:  {name: "Acme's pricing page", url: "https://…/pricing", read: "2026-08-30"}
```

These field names are a recommendation, not a requirement — a record that already uses others
is fine. Classify each entry honestly; a role with no content yet needs no placeholder.

**Put source sections last.** Keep knowledge, judgments and open questions before sections
devoted to sources, sessions or conversations. Apply this by the section's role, whatever its
key is called: `sources`, `sessions`, `conversations`, or a name the project already uses.
For a new record, recommend `sources`. Preserve existing names and grouping; a section that
mixes documents, runs and conversations moves together. This authoring convention puts the
findings before their origins. Update commands preserve the existing section order; they do
not infer a section's role and reorder it automatically.

**`seen` is not optional and not decoration.** It is the snapshot of what every dependency held
when the judgment was written. Without it there is no before, so nothing can be compared and
**no drift can ever be detected** — the record still says what things rest on, but it can never
tell you that one of them moved. Write it when you write the judgment; a later reviewer refreshes
it when they confirm the wording still holds. If a dependency is missing from `seen`, that
judgment has never been checked against it, and that is itself worth reporting.

It is also what lets a threshold measure *accumulated* drift. Three changes of 200 each are all
below a 500 threshold individually, but together they cross it — and only a snapshot taken at the
last review can see that. Propagating a dirty flag at write time cannot.

**If the work is a genuine one-off** — a single question, a throwaway script, something nobody will return to — do not create the file. There is nothing to keep.

## When something does not fit

A fixed set of fields creates a pressure to jam things into it. Resist that: a judgment invented
to satisfy a checker is worse than a gap, because it looks right. Three honest ways out, and
using one is never a failure:

```yaml
open:         # a live question. Not a fact, not yet a judgment. No verdict is owed.
  which_base: "does the bank measure the ratio against price or against its own valuation?"
  rate_lock:  # closed by `answer rate_lock mtg.lock_policy`; the tool writes answered:
    question: "does the lock survive a change of lender?"
    answered:
      by: mtg.lock_policy
      said: "the lock is the lender's, not the borrower's"
      of: "2026-09-18"

judgments:
  blocked_thing:
    rests_on: […]
    verdict:  "…"
    wrong_if: "the request from his device returns an explicit not-found body"
    blocked_on: "could not reproduce past the login gate, so the predicate is un-evaluable"
```

`blocked_on` says a predicate cannot be evaluated *and why*. That is a real state and it should
be visible — what must never happen is a predicate that reads like one and is actually prose,
because nothing evaluates it and nobody notices. Say it is blocked instead. A judgment that is
*decided* — on a session's prior, or on taste — and names only the sign that would re-open it
is a third state, and says `reopened_by`: not a hole, and not waiting (see *An agent's prior as
a source* in [falsifiers.md](falsifiers.md)).

## Field names are read by shape

**It does not know your field names.** Each role has a distinctive *shape*, and the shape is
enough: dependencies are a list of strings that are all ids of other entries; a snapshot is a
mapping whose keys are a subset of that entry's dependencies; a predicate is a string containing
entry ids *and* something else — an operator, a number, a bracket, so a bare reference is not one.
So it reads a record that says `known:`/`judgments:`/`from:` exactly as well as one that says
`facts:`/`claims:`/`src:`.
