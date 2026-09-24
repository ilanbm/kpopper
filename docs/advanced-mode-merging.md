# Advanced mode: the open merge problem

**Status: open design question. Advanced mode is experimental.**
Last reviewed: 2026-09-24.

We are keeping the current readable, tracked `GROUNDING.yaml` and its existing history
formats. No storage replacement or automatic merge service has been selected. The goal
is a workflow that is simple for people using kpopper, including people without GitHub
Actions. Moving work from a merge conflict into setup, background automation or a separate
filing step is a cost, not automatically a solution.

Advanced remains the low-level fallback for unconfigured Git projects. First-use onboarding
now offers an explicit choice of Simple or Advanced (experimental); it does not silently
migrate existing records or make the core checks optional. See [project modes](project-modes.md)
for the implemented behavior and [history](history-contract.md) for its preservation rules.

This is a living problem statement. Alternatives below are proposals, not shipped features
or a roadmap commitment. A useful contribution can refine a constraint, provide a small
reproduction or disprove a proposed solution; it need not implement a new storage engine.

## The failure people encounter

Several PRs change unrelated code and each records its findings. Their checks pass. One PR
merges into the target branch. Another PR now has a textual conflict in `GROUNDING.yaml`,
even though its code merges cleanly and its new entries describe different subjects.
Resolving that conflict creates a new commit. The project's CI may run its expensive tests
again because the new commit still contains the PR's code changes.

This happened in this repository on 2026-09-24. At the inspected revisions, [PR #237](https://github.com/ilanbm/kpopper/pull/237)
and [PR #239](https://github.com/ilanbm/kpopper/pull/239) had successful checks and conflicted
only in `GROUNDING.yaml`. Both added different source entries in the same area. The PRs
were subsequently resolved and merged; their earlier state is evidence of the failure,
not a claim that they are still blocked or a measured conflict rate for all users.

For that reproduction, the target was `a30a549d496e5946dc8d0327ef90f1c0bb9670dc`; the
PR heads were `b23ae786ef5551a7cd01e98c1769011eb1df6204` and
`38ebfcadc8fda71b85b42e26c75917793141cd1a`, respectively. With those objects available,
`git merge-tree --write-tree --name-only TARGET HEAD` reports the conflicted paths without
changing the checkout.

There are three distinct problems:

1. **Textual collision:** independent changes cannot be combined by Git's line merge.
2. **Knowledge conflict:** the combined claims, dependencies or recorded conditions need
   review, even if the text merges cleanly. Different IDs can still affect one another.
3. **Repeated validation:** a new commit triggers work that may already have been performed.
   A record-only conflict does not prove that the rest of the combined code is unchanged.

Solving one does not solve the other two. Successful checks on each branch separately do
not establish that their combined graph or code is valid.

## What Git and kpopper do today

Git compares a common base with the two branch versions. Editing the same file does not
by itself cause a conflict. Independent regions can merge, but two different insertions
at the same gap can conflict, including two distinct YAML keys. Sorting helps distribute
insertions but cannot guarantee different gaps. Git's ordinary text merge does not know
which YAML collections represent independent nodes. See [three-way file merging](https://git-scm.com/docs/git-merge-file).

The documented CI integration runs `kpop check` and `kpop consolidate --dry-run`. These
inspect a record and proposed combinations; they do not repair the Git merge, publish a
consolidated commit or silently choose a disputed claim. A `pull_request` workflow does
not run while GitHub cannot create its merge result because of a conflict. See
[CI integration](coding-and-ci.md) and [GitHub workflow events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#pull_request).

A bounded local resolver is available in this source tree as `kpop consolidate --resolve`
(with `--dry-run` for preview). It acts only after a Git merge stops, validates the staged
candidate, preserves supported history and writes the record without staging or committing.
Competing claims and unsupported cases remain explicit refusals. This reduces manual
resolution work; it does not prevent a later target update from causing a conflict or
avoid CI on a new commit. See [the exact command contract](coding-and-ci.md#resolve-a-stopped-git-merge-locally).

`consolidate --from` is a directional proposal check, not a general three-way Git merge
implementation. A general merger needs the common base, both complete records and
their evidence, and the resulting code tree. A file-level driver alone is not a complete
transaction or a validation of the whole graph.

History does not automatically remove file conflicts. Immutable events appended to a
shared per-node file still change that file. Independently named immutable event files
have different merge behavior. Likewise, calling `GROUNDING.yaml` generated or renaming
it to a lockfile does not help if every PR still commits a different version of it.

Shared findings serve another purpose: retaining scoped knowledge independently of a
feature. Reading them alongside a branch does not merge their publication branch into it.
Capture, remote publication and acceptance remain distinct, and feature-dependent claims
must not be routed into shared knowledge merely to avoid a file conflict. A publication
branch or PR is not created just by installing kpopper; see [the publication lifecycle](project-modes.md#one-publication-branch-repeated-review-cycles).

## Alternatives considered

### Keep the current format and make the limitation explicit

Keep the readable file, ordinary Git workflow and current consolidation checks. Resolve
record conflicts when they occur, preserving both sides' sources and historical evidence.

This is the current choice while the question stays open. It has the least new machinery,
but the interruption and possible repeated CI remain real costs. An experimental label
discloses that cost; it does not fix it. Simple mode is a different context model, not a
drop-in way to remove every shared-write or knowledge-conflict problem.

### Make one file easier for Git to merge

Use stable placement, small edits and unchanged formatting. Alphabetical placement can
help, but related IDs such as sources from the same date still cluster together. Hash-based
placement and fixed anchors can spread insertions further without a later sorting step.

The tradeoff is less natural reading order and possibly extra structure. Two additions
can still land in the same gap. A global timestamp, publication pointer or checksum that
every write changes can recreate a common conflict regardless of node placement. Moving
such metadata requires preserving its integrity role, not simply deleting the check.

This can reduce friction and can complement other options; it is not a conflict-free
format guarantee. [The layout probe below](#exploratory-probes) illustrates both effects.

### A local merge driver and one consolidation engine

kpopper could provide a three-way record merger, with a Git driver installed during an
explicit project setup. Updating a branch locally would combine independent knowledge
and preserve disputed alternatives, then validate the complete merged tree before commit.
This retains the full readable file and does not depend on a particular CI provider.

The configuration is local to each relevant clone. GitHub's server-side merge does not
execute a locally configured command. Once the target moves, an open PR can still need
another local update and push, with new checks. The driver reduces resolution effort;
it does not keep every previously green PR continuously mergeable.

A driver receives file versions and may be skipped when no content merge is needed. It
cannot assume that related files have already merged or publish history as a side effect
of each callback. Whole-record validation and atomic publication need a surrounding
operation. [Git's driver contract](https://git-scm.com/docs/gitattributes#_defining_a_custom_merge_driver)
defines that narrower boundary.

Git has no hook for creating a PR. `pre-merge-commit` runs after a successful merge;
`post-merge` does not run after a conflicted merge. `pre-push` receives the commit IDs
already selected for transfer. Creating a new commit there does not retarget that push.
A sync-and-push command could prepare, validate and commit first, but is an additional
workflow operation. A remote merge never wakes a local hook by itself. See [Git hooks](https://git-scm.com/docs/githooks).

### A hosted conflict-resolution service or workflow

On a target-branch update, automation could inspect remaining PRs, prepare each complete
merge, reconcile record-only conflicts and push a normal merge commit after checking that
the PR head has not moved. Code conflicts and unresolved knowledge conflicts would stop
that attempt with an explanation. Only the record merger should be automatic, not an
arbitrary choice of which claim is true.

This saves manual resolution and keeps the full file in Git. It also requires write
authority, race handling, fork support and integration with the hosting service. Trusted
publication must be separated from execution of PR-controlled code. Most importantly,
the repair still creates a new commit and can restart expensive CI. Packaging setup well
does not remove those operating costs or make the integration universal.

### Reuse CI results or validate near the time of integration

A project's CI can reuse a successful result when all relevant test inputs and execution
conditions match, or defer expensive validation until a candidate is prepared against its
integration target. A merge queue can help schedule that validation, but does not itself
teach Git how to merge kpopper records.

This is complementary infrastructure, not a general kpopper fix. Different projects have
different test inputs, services, platforms and policies. The code brought in by another PR
may legitimately require new tests even when the only textual conflict was in the record.
Never treat an older green check as evidence for changed inputs. GitHub's native queue
also has repository eligibility and workflow requirements. See [merge queues](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue).

### Separate current nodes and generate the readable file locally

Store authoritative current values and their history under `.kpopper/`; generate an ignored
`GROUNDING.yaml` or `GROUNDING.lock.yaml` for reading. Different nodes then occupy different
paths. A new node needs a current representation, but need not have a separate history file
until it changes. The proposed filename does not determine the storage semantics.

This removes the shared aggregate from PR diffs. The price is a changed source-of-truth
and editing contract: a fresh clone needs materialization, a local view can become stale,
and GitHub no longer shows the complete aggregate as a tracked file. Hand edits must be
reconciled explicitly or rejected without data loss. A file containing both the current
value and appendable history can still conflict when two branches change that node.

### Immutable change files, snapshots and a disposable local cache

Store each change under a unique content-derived path, grouped by node. Include original
values so the complete record is recoverable without a previously generated view. New
transaction manifests can bind multi-node changes without rewriting one global manifest.
This makes concurrent additions additive at the file level, including changes to the same
node, provided writers do not overwrite shared indexes or current-value files.

The current value is computed from causal heads and acceptance rules, not the last filename,
timestamp or appended line. Several heads can remain in dispute. Textual merge success must
not become silent acceptance of one alternative. Typed deltas can be compact and safe when
bound to an exact base and a verified result; raw text patch guessing is not required.

A cache can retain decoded nodes, rendered fragments and dependency indexes. Changed-input
discovery is an optimization; cache entries must be bound to the exact selected source set
and rule versions. Branch changes must remove out-of-scope events as well as add new ones.
Unknown cache state requires reconstruction. A full YAML rewrite can still cost the size
of the generated view even when graph updates are incremental.

Snapshots can shorten replay. A new hash alone does not justify deleting older evidence:
other branches, pins or judgments may still refer to it. Lossless packing, checkpoints,
knowledge retirement and garbage collection are different operations. Full-history
integrity checks must not silently become checks of only newly arrived files.

There are established patterns to study, including [Automerge's changes and snapshots](https://automerge.org/docs/reference/under-the-hood/storage/)
and [content-addressed build caches](https://bazel.build/remote/caching). Adopting a CRDT
library would not by itself preserve kpopper's acceptance, scope and evidence rules.
[Automerge's conflicting-value behavior](https://automerge.org/docs/reference/documents/conflicts/),
for example, is a policy to examine, not a decision that one knowledge claim is true.

### Publish a generated aggregate only on the target branch

PRs could change separate authoritative files while one publisher refreshes the full
`GROUNDING.yaml` on the target after integration. This preserves a browsable aggregate on
the host without every feature branch rewriting it.

It requires a publisher and a clear freshness contract. The aggregate can lag publication
and does not show a feature branch's latest findings. Adding an already tracked file to
`.gitignore` does not stop local regeneration from appearing in a PR: [Git ignores untracked files](https://git-scm.com/docs/gitignore).
Local materialization and target-only publication would need distinct, enforced behavior.

### A pending area and a periodic consolidator

Accumulate additions, then fold a batch into the main record. A shared pending section
inside the same YAML file still has an insertion hotspot. Separate pending files avoid
that hotspot but defer the aggregate update to another operation.

If every branch independently folds its batch, the shared-file conflicts return. A single
coordinator requires ownership, scheduling and retry rules. Before the fold, the aggregate
is incomplete by itself. Also, "not yet filed in the aggregate" must not be confused with
"not yet accepted as knowledge." This is not currently a simpler replacement workflow.

### Git inside Git

Git's immutable object store and snapshot model are useful reference designs, but a nested
repository adds another set of refs, fetch/push behavior, recovery and retention rules.
A submodule records one commit pointer in the outer repository; divergent pointers can
conflict, and the inner content may need a merge too. See [submodule merging](https://git-scm.com/book/en/v2/Git-Tools-Submodules).

Using Git merely as an internal object backend would still leave knowledge semantics and
portable synchronization to implement. Tracking its mutable repository internals in the
outer repository does not turn them into additive records. This option has not shown an
advantage over the existing history work for the problem described here.

## Exploratory probes

These local probes from 2026-09-24 are narrower than product or user validation. They are
included to keep useful evidence and its limits attached to the proposals, not to select
an architecture by a favorable microbenchmark.

**Text layout.** Using 227 source IDs from this record, 24 synthetic branches each added
one new source from the same naming family. All 276 pairs were passed to local Git 2.54.0
with `merge-file --diff-algorithm=histogram`. The node IDs in clean outputs were checked
against the expected union; this was not a semantic graph check.

| Layout | Pairs with text conflicts |
|---|---:|
| Alphabetical IDs | 276 / 276 |
| IDs ordered by hash | 2 / 276 |
| Hash placement with 256 fixed anchors | 1 / 276 |

The deliberately clustered additions are not a representative workload or a forecast of
user conflict rates. A separate counterexample put two different IDs in the same empty
anchored bucket and conflicted. Updating shared metadata also conflicted even when the
new nodes occupied different buckets. This demonstrates reduction, not elimination.

**Local projection.** A separate Python probe with a C-backed YAML parser stored current
node bodies in separate files and rendered them through a disposable SQLite fragment
cache. Medians below are from three runs on one arm64 Mac with warmed filesystem caches;
the larger catalog repeated representative bodies with distinct synthetic IDs.

| Current nodes | Generated YAML | Build cache and render | Update one known changed node and render the complete YAML |
|---|---:|---:|---:|
| 678 | 0.54 MB | 49 ms | 1 ms |
| 10,000 | 7.7 MB | 757 ms | 10 ms |

These are not kpopper session-start or full-validation timings. The probe omitted process
startup, Git change discovery, history replay, causal reduction, dependency evaluation and
pending contributions. It checked output node count and the changed body, not all product
invariants. The initial build used warm filesystem caches, not a cold disk. Cached fragment
rendering still wrote the complete output through temporary-file replacement and `fsync`.
Any proposal needs reproducible end-to-end measurements before a latency promise.

## What remains open

- Must a fresh clone and the hosting website expose the complete current graph without
  running kpopper, or is a local generated view sufficient?
- What interruption is acceptable after another PR merges: a normal branch update, an
  explicit knowledge decision, or no extra step for independent knowledge changes?
- Can a portable file layout reduce the measured real-world conflict rate enough while
  retaining the current editing and history contracts?
- If immutable files become authoritative, how do creation, scope, pins, deletion, identity
  changes, competing heads and multi-node publication remain complete and atomic?
- How are concurrent snapshots or compaction reconciled without losing evidence required
  by another branch, an old reader or a historical judgment?
- What must every ordinary read freshly verify? What can safely reuse cached verification,
  and how is that coverage distinguished from a complete historical audit?
- Which part belongs in kpopper, and which would require each user's CI or hosting setup?
- Does the shared-findings path work in real installations, with clear setup and publication
  status? It should be validated independently, not assumed to solve branch-record merges.

## How to advance the question

Bring a concrete base and two branch versions, or a minimal reproduction. State which
changes are independent and which require a knowledge decision. Include the history and
evidence needed to check that assertion; do not substitute source names for source bodies.

Evaluate candidates against the same cases: independent additions at the same location,
independent edits, competing changes to one node, delete/edit, old pins after rename,
cross-node invalidation, target movement after green checks, concurrent writers and
recovery, and a clone with no warm cache. Measure text conflicts, user actions, extra commits,
CI work, storage growth and full versus incremental read time separately.

A successful candidate must retain all relevant evidence and make unresolved meaning
visible. It must explain setup, ordinary use, failure recovery and removal for a user who
does not know our implementation. Preserve dated counterexamples and measurement scope
when updating this document. Proposals remain open until those tradeoffs are explicitly
accepted; keeping the current format is preferable to disguising an unresolved cost.
