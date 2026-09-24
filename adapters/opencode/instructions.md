# Using kpopper in OpenCode

The shared method is loaded separately from `adapters/_shared/method-summary.md`.
Load the relevant canonical kpopper skill with OpenCode's skill tool when needed.
Its location is inside the full checkout; resolve supporting files relative to that
location, without copying or rewriting the method.

This installation supplies no automatic kpopper lifecycle hooks. When beginning
work on the project's knowledge record, run `kpop open` from the target project
unless a current opening was already supplied. Use its resolved record location.
Run `kpop check` after changing the record and report any remaining issues before
finishing. These are instructions to execute commands, not a host-enforced gate.

Use `kpop` from the configured checkout's `scripts/bin/` directory. If it is
unavailable, report the missing runtime; do not guess another installed copy or make a
duplicate record.

Do not treat a discovered skill as evidence that background agents, notifications,
browser tools, or source connectors are available. Use only capabilities this
OpenCode session actually exposes and report a missing capability when it matters.
