# Revisit a walking-club plan

This invented example has three routes, a group-size calculation, a missing
forecast and a qualitative route preference. It runs against your installed
`kpopper` package and changes only a temporary copy of the included record.

The writing exercise requires POSIX file locks (macOS or Linux). From this
repository, with the package installed:

```sh
python examples/scoped-query/exercise.py
```

The first reading finds 30 permitted walking minutes and room for 16 people.
The exercise then records three guides and six walkers per guide as two explicit
writes, and adds a permitted five-minute garden route. A new CLI process reads
35 minutes and capacity for 18 people. The query's scope basis changes along
with its membership. Replaying the earlier captured Snapshot still returns 30.

The forecast stays `unknown` because its reading was never supplied. The route
preference stays qualitative, with a prose reason to revisit it and no invented
`wrong_if`. The exercise does not treat that preference as a proved consequence
of the number of guides.

Expected output:

```json
{
  "initial_minutes": 30,
  "revised_minutes": 35,
  "initial_people": 16,
  "revised_people": 18,
  "forecast": "unknown",
  "qualitative_falsifier": "not_declared",
  "scope_basis_changed": true,
  "retained_snapshot_minutes": 30
}
```

The assertions check recorded behavior through the installed evaluator, writer
and CLI. This is a scripted exercise, not a measurement of an uncoached agent's
ability to author a record. For atomic multi-entry updates, use the
[recording report protocol](../../skills/record/SKILL.md); these two explicit
writes intentionally make no batch-atomicity claim.

The fixture selects `core/v1` and `query/v1` explicitly. A normal package install
supplies the native runtime; it needs no separate Lean compiler or session setup.
See [query semantics](../../docs/query.md) and the
[contribution guide](../../CONTRIBUTING.md) for extension and conformance work.
