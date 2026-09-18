# Experimental native history CLI

`kpop-native` is an opt-in Rust executable for a bounded subset of kpopper history.
It does not replace `kpop`, modify installed plugins, or activate existing records.
Use disposable workspaces only. macOS arm64 is the initial verified target;
other platforms are not yet verified, and non-POSIX operations currently refuse.

## Build and run

With Rust installed, from this directory:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

The resulting executable does not require Python, Node, Cargo or Rust on the runtime
PATH. Rust and downloaded build dependencies are required only when building it.

Supply an absolute path to an existing **empty** directory:

```sh
kpop-native --workspace /absolute/disposable/workspace init --record-id example
kpop-native --workspace /absolute/disposable/workspace add p.hours \
  --value 10 --source calendar --operation observation-1 --on 2026-09-19T12:00:00Z
kpop-native --workspace /absolute/disposable/workspace set p.hours \
  --value 12 --operation observation-2 --on 2026-09-19T13:00:00Z
kpop-native --workspace /absolute/disposable/workspace open
kpop-native --workspace /absolute/disposable/workspace history p.hours
```

Values use JSON syntax. `null`, booleans, arbitrary-size integers, finite floating
point values, strings, lists and string-keyed maps remain distinct. A repeated
operation with the exact same request is idempotent; different content under that
operation refuses. Optional `--expected-revision` compares the revision returned
by `open`. Each new observation requires a new operation and an explicit recorded time.

`session-start` consumes a host JSON payload containing an absolute `cwd` on stdin.
It reports supported readings and an executable `KPOPPER_AGENT_CONTEXT.command`.
It ignores subagent payloads, and reports failed opening visibly without blocking
the host. This command does not register hooks or establish host trust.

## Supported history boundary

The executable retains `history/v1` authority/manifest envelopes,
`typed-history/v2` immutable object IDs and `subject-paths/v2` storage names.
It handles a linear chain of explicit reading/accept operations, preserving
original objects and the open disposition acts on historical claims.

Writes stage immutable objects, publish a manifest as the commit point, then
replace the derived `GROUNDING.yaml` view. `recover` recreates a stale view from
committed evidence. `KPOP_NATIVE_FAIL_AFTER=objects` or `commit` injects an error
at either boundary for disposable fault tests. This is a native-private recovery
protocol; Python's prepared mutation journals are unsupported and block opening.

The existing POSIX directory-flock protocol is shared with Python history writers.
Cross-language authoring interoperability remains unverified. The opt-in marker
prevents accidental use, not malicious modification by another local process.
Symlinks, missing/corrupt committed evidence, unknown authority/capabilities,
incomplete or branching commit graphs, duplicate mapping keys and oversized data
refuse. Parsing has explicit depth, node and byte limits; YAML aliases, merge keys
and unsupported tags are not accepted. Writes use JSON, which is valid YAML.

## Deliberate limits

- No judgments, dependency pins, formulas, competing histories, review/refutation,
  temporal semantics, Lean evaluation, MCP, UI, migration or production activation.
- No authority v2 cancellation metadata, legacy 40-hex objects or typed YAML
  date/datetime support. This is not full PyYAML compatibility.
- Receipts explicitly say `semantic_assessment: not_performed`; accepted history
  is not a claim that a condition was checked or that source content is true.
- Python's history contract/store and explicit Snapshot capture can validate this
  subset. Its public legacy CLI refuses this ordinary-reader history profile;
  the executable is not a drop-in replacement for the active core/history product.
- At most 1,000 commits, 1 MiB per serialized file and 16 MiB of selected store
  data. This bounded implementation rereads history and is not a scale benchmark.
