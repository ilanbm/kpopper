# Scalar arithmetic kernel

`Kernel.lean` executes the arithmetic/v1 scalar fragment with exact rational
numbers, booleans, text, explicit null, references, and explicit unavailable
inputs. Every recursive evaluator and graph/type preflight has deterministic
fuel. Numeric growth, parser nesting, graph edges, node counts, and steps are
bounded. The expression type makes binary arity explicit. Unknown operators
return `unsupported_capability` at the native registry boundary.

`Protocol.lean` validates the ASCII KP1 framing, lowercase UTF-8 hex strings,
canonical counts, decimal syntax, duplicate IDs, complete input consumption,
potential read closure, declared read grant, and static scalar types before
execution. The KR1 response is ordered canonically. Each line is independent;
`Main.lean` accepts multiple requests until EOF. Public JSON values cross this
boundary through `../transport.py`, which does not execute arithmetic.

For maintainers with the pinned Lean 4.33.1 toolchain:

```sh
lake build kpopper-reasoning Proof
lake env lean Audit.lean
```

Runtime source imports only Init/Std. Build through the standard generated
entrypoint and upstream runtime initializer. Proof modules are separate from the
runtime dependency graph. Users execute the packaged native artifact; evaluation
does not compile or download a runtime.

## Checked theorem boundary

`Kpopper.Proof.evaluate_closedRat_sound` is about `Kpopper.evaluate`, the same
function called by the native handler. For closed numeric literals and binary
add/subtract/multiply/divide expressions, any successful numeric result has the
independent `Denotes` meaning. Its division constructor requires a nonzero
right-hand denotation. The final memo and executed-read sets equal their initial
values. Fuel, limits, graph, depth, and initial state are arbitrary; steps may
increase.

`Kpopper.Proof.evaluate_literal_success` proves successful execution of any
rational literal whose digits fit, with positive fuel, remaining step budget,
and permitted entry depth. This supplies a general non-vacuous success case.

These statements do not prove completeness for general closed expressions,
reference semantics, potential reads, preflight/parser behavior, all state
fields, Python adapters, native compilation, distribution, or source-world
truth. `Audit.lean` prints the exact theorem types and their axiom dependencies.
The public tests exercise the unproved boundaries with deterministic oracle and
malformed-input cases; they do not extend the formal scope.
