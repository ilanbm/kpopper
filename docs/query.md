# Finite collection queries

`query/v1` is an optional compiled module for explicit `core/v1` records.
It evaluates one captured collection through the packaged native runtime.

```yaml
meta:
  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1, query/v1]}
routes:
  route.park: {minutes: 20, permitted: true}
  route.ridge: {minutes: 15, permitted: false}
scopes:
  scope.routes:
    collection_scope: {collection: routes, fields: [minutes, permitted]}
calculations:
  m.minutes:
    rule:
      query:
        version: 1
        scope: scope.routes
        op: sum
        where: {column: permitted}
        value: {column: minutes}
```

Save this as `routes.yaml`, then run:

```sh
kpop assess m.minutes --profile core/v1 --record routes.yaml
```

The result is a typed record whose `result` is the exact number 20. It also
carries five counts: two input rows, one definite match, and zero unknown
memberships, unknown values or errors.

## Operations

| Operation | Required expressions | Result | Empty scope |
| --- | --- | --- | --- |
| `filter` | `where` | Matching member IDs | `[]` |
| `project` | `value` | One value per member | `[]` |
| `select` | `where`, `value` | Values for definite matches | `[]` |
| `count` | `where` | Exact match count | `0` |
| `sum` | `value`; optional `where` | Exact numeric sum | `0` |
| `all` | `where` | Every predicate true | `true` |
| `any` | `where` | At least one predicate true | `false` |

A query is a whole rule or direct operation; it is not nested inside a scalar
or composition expression. Rows are ordered by member ID. A row expression uses `{"column": "minutes"}`
and the existing typed literals, arithmetic, comparisons, boolean operators,
conditionals, containers and literal-key field access. Required arithmetic or
composition modules must be declared. Row expressions cannot use `ref`, another
query, an assessment, a historical snapshot, a file or a network lookup.

Only the scope's declared authored fields are inputs. A missing field remains
missing even when a member formula could compute it. `rule`, `computed` and
reserved assessment fields cannot become columns. Unsupported authored values
are unavailable rather than coerced. Active history requires complete accepted
membership; incomplete coverage refuses the query instead of scanning a subset.

## Unknowns, errors and evidence

`filter`, `select`, `count` and conditional `sum` cannot return a complete answer
when a row's membership is unknown. `project` and `sum` also need every selected
value. A missing or contested field that is read remains visible in diagnostics.

For `all`, one definite false may determine the result; for `any`, one definite
true may determine it. The scan still counts and diagnoses uncertain or erroneous
rows. This dominance never overrides invalid syntax, unsupported capabilities,
resource limits or operational failure. Non-numeric selected values make `sum`
an error. An unknown or erroneous result has `value: null`, not a partial list or
zero disguised as a complete answer.

A completed scan retains `input_count`, `definite_match_count`,
`unknown_membership_count`, `unknown_value_count` and `error_count` in
`query_counts` and its finalized `query-inputs/v1` basis. A refusal before scan
completion has no finalized counts or query basis. Depending on its cause it
reports an error, unsupported capability, resource limit or operational failure.

The basis binds the operation, scope definition, membership, projected field
observations, required modules, limits and counts. Adding a nonmatching member
or changing a nonmatching field changes this basis even when the result is equal.
An inline query reads one scope witness; a stored query also reads its root node.
Member IDs stay inside the scope evidence rather than expanding authored
`rests_on` into a changing list of members. A declared dependency is not proof
that a conclusion follows, and successful computation does not verify the world.

## Bounds and contribution

KP4/KR4 uses canonical length-framed JSON and `resources/v4`. Fixed maxima are
10,000 candidates and 100,000 captured field reads, together with the versioned
step, depth, digit and typed-value bounds. KP2/KR2 and KP3/KR3 retain their
existing bytes and semantics. Manifest version 3 advertises the complete
protocol and module sets.

The module registry is closed compiled code inside the command. A module
prepares a detached bounded request, validates the runtime's response and finalizes
the evidence basis; the Lean kernel owns row evaluation. Consumers render common typed
values, witnesses and findings without query-specific branches.

A scoped module's four steps are preparation, validation of that detached
envelope, decoding of the already decoded KR4 result against it, and a basis
finalized with verified scan counts. Preparation reads a bounded capture of the
query scope and binds the normalized operation, the runtime request, required
modules, potential witnesses and the basis template. Arithmetic and composition
retain their older preparation shape; this is a reviewed source extension point,
not a dynamic plugin API. A new scoped module also needs explicit evaluator
dispatch and transport/schema registration. It cannot expand its own captured
authority.

For an additive module, register a new versioned capability and its protocol,
resources, basis recipe, preparation and response validation. Supply independent
semantic cases, malformed-input tests, scope invalidation tests and installed
conformance. Changing an existing module's meaning requires a versioned core
compatibility decision. Records cannot register callbacks or load module code.

From a source checkout, the query, extension-contract, runtime and consumer
cases run with the rest of the suite:

```sh
cd native && cargo test --locked
```

The suite runs the committed host archive and fails if it is unavailable
or stale. The [runtime guide](../scripts/reasoning/native/README.md) covers builds,
axiom audits and runtime acceptance. Query conformance is executable
evidence; no theorem of complete query correctness is claimed. See
[core assurance](reasoning-core.md#runtime-licensing-and-assurance) for the exact
proved fragment and remaining trusted components.
