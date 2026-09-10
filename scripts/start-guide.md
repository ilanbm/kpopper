# Starting with the user's work

Use this guide when a workspace has no knowledge record, when the user asks to map existing
materials, or when a contextual explanation is due. `kpopper open` gives the current knowledge context;
`kpopper _agent status` reports saved choices and unseen explanations. Neither scans materials
nor writes state. The session carries out this workflow with its available tools.
If `kpopper` is not on PATH, the hook's `agent_command` gives an argument array for the
installed dispatcher; append `guide`, `status`, or another subcommand to that array.

A project is work around a goal: planning a week, coordinating people, preparing a launch,
buying a home, researching a question, or developing software. Learn its boundaries from the
user's task and the links between materials. Documents, calendars, task lists, conversations,
files and prior agent sessions can all be sources. Code, commits and pull requests are useful
sources for software projects. An account or folder is a clue, not a complete definition of
the project. Use the user's language and their names for the work.

## The first offer

An absent file does not mean an empty body of knowledge or a new user. Respect the opener's
record location, including a record elsewhere or in a parent directory. An unavailable
registered record needs its location restored; it must not trigger a replacement. If existing
materials already maintain a compatible record, use the pointer mechanism in the method's shape reference (skills/kpopper/references/shape.md).

At the first suitable moment in meaningful work, offer three choices in a short conversational
card. Skip the introductory explanation if `introduced` is true. Defer the offer during urgent
work; a greeting or an automated/background task is not a reason for onboarding.

> **kpopper · Getting to know your work**
>
> I can keep what we learn, the sources behind it, and what would need another look so a later
> session can continue from here.
>
> We can learn while working, map the existing materials, or investigate the current situation
> and its history more deeply. You can skip this and continue your task.

Use the host's native choice UI when available: **Learn while working** (default), **Initial
map**, and **Deeper investigation**. Always allow a free-text response or skipping. Give the
user a reasonable opportunity to reply while continuing independent work. No answer means
continue the task; silence does not authorize scanning or additional writes.

After actually displaying the offer, run `kpopper _agent shown welcome`. This remembers the offer
for this project and the introduction for this user. Do not acknowledge it merely because the
hook supplied it. Learning while working needs no configuration. For an explicit mapping request, run `kpopper map --json`
or `kpopper map --deep --json` and execute the returned task. Do not ask again if the request is already clear.
`kpopper config --guidance off` remembers a request to skip explanations; `kpopper config --guidance on` restores
them. These preferences live locally outside the record; they are not project evidence.

## Learn while working

Continue the requested task. With authorization to maintain its record, save the first finding
worth carrying into a later session, together with its actual source. Installation alone does
not authorize changing a project whose instructions or current task forbid writes. A one-off
question or throwaway task can finish without any record.

Create the file with real material, not an empty template: the request that brought the session
here, a source, and a supported finding or open question. Keep only roles with something to hold.
For a small record with no judgments yet, use `sources`, `known` and `open` with their ordinary
source/value/question fields; the reader accepts this starting shape without a `schema` block.
Use `kpopper add` for judgments so the tool fills their `seen` snapshots. Do not invent a conclusion,
date, threshold or confidence to make the record look complete. Run `kpopper check` before relying
on the new entries.

For significant conclusions, make a focused attempt to find a plausible failure that the current
checks would miss, including one where the recorded premises remain correct. Keep any exposed
assumption and the observable evidence that would undermine it. Preserve useful existing checks:
the reader evaluates one comparison per judgment. Record an additional unevaluated condition
in `because` with `blocked_on` explaining the evaluation gap; a separate judgment needs a distinct
claim. Scale the effort to the consequences; do not invent thresholds or fill a quota of conditions.


## Execute a returned mapping task

Mapping uses the calling host agent; it does not launch an additional model process. The hook's
`KPOPPER_AGENT_CONTEXT` supplies the session environment and CLI argument array. Carry that
environment into mapping and receipt calls. Codex tool calls can also use their `CODEX_THREAD_ID`.
When neither identity is available, `map` returns an explicit error and saves no task. Do not
fabricate an identity to claim an agent is connected from a plain terminal.

Use `map --json` to receive the workflow and the internal argument arrays. A `ready` task has
been returned to the caller but has not started. Run its `accept` protocol, perform the mapping
with this session's tools, and continue until the agreed scope is handled. Do not stop after
saving or announcing the request. Once the real result exists, invoke `complete` with the
actual report or record path. It records completion and the record check result, including
any findings; completion is not a claim that all recorded conclusions are true. If execution
cannot finish, invoke `fail` with the reason and explain the remaining work.

These are internal protocol calls. Do not teach request IDs or acknowledgements as ordinary
user commands. Use `open`, `map [--deep]`, and `config --guidance on|off` in user-facing help.

## Map existing materials

Mapping is an explicit task the user selected. It can also extend an existing record: pull
relevant entries first and preserve current judgments when historical accounts differ.

1. **Locate the work.** Start with the task, supplied materials, workspace instructions and
   explicit links. Make a small inventory of where information and decisions appear to live.
   Prefer metadata before reading a large body of content. Do not crawl the user's home
   directory, unrelated conversations, or whole connected accounts.
2. **Make the scope concrete.** Say which goals, subjects and sources you propose to cover,
   what can actually be accessed, and where information seems missing. Infer routine boundaries
   from the request. Ask only where a missing choice materially changes the work. An explicit
   selection of supplied materials is sufficient; do not ask for the same access again. Obtain
   a choice before a material expansion into additional private sources.
3. **Build a useful picture.** Answer: what are we trying to achieve; what is known now
   (including commitments, deadlines and constraints); what was decided and why; where is the
   evidence; and what needs investigation or another look. Let real subjects shape the record.
   These questions are a guide, not five mandatory categories to fill.
4. **Ground each claim.** Keep the source and exact location, date read, and event or validity
   date when provided. Distinguish quotations, paraphrases and conclusions. An unopened linked
   source remains unreviewed. Unavailable material is not evidence of absence. Preserve
   disagreements and identify their effects on decisions.
   Apply the same failure search to significant conclusions drawn from the materials. Preserve
   additional review conditions without presenting unevaluated prose as an automatic check.
5. **Return the map and its limits.** Show useful findings, open questions, sources reviewed and
   scope not covered. Link the knowledge view, record and sources where supported. Use the
   shipped page renderer when a page helps, following PAGE.md; keep the outcome accessible in
   the conversation too. Run `kpopper check` and use the returned completion protocol with the real report or record
   path when the agreed mapping is finished. A completed map is not a standing instruction to survey again.

The initial map prioritizes the present situation and evidence behind important decisions.
Do not catalog everything because it is accessible. A short map with honest gaps is a finished
result when it answers the agreed scope. Preserve scope and progress in the task context and,
once a record exists, its source entry. A saved task identity alone does not reconstruct
an interrupted task's agreed scope; recover that context before expanding the work.

## Investigate more deeply

Follow the same steps with an explicit set of subjects, sources and time period. Investigate
the current situation and relevant historical decisions within those limits. Follow citations
and related discussions when they answer the agreed questions; ask before extending scope or
effort materially. Report coverage, not a claim of total knowledge.

Match each question to evidence capable of answering it. A calendar entry records a planned
meeting; it does not prove attendance. A task marked done is a report of completion. A message
establishes what someone said; the current agreement may be in a later document. A commit shows
a change; its rationale may be in a discussion. An approved proposal is distinct from
implementation. An agent's claim that it ran a check needs the actual result to establish what
was checked. Do not reconstruct an undocumented reason from the outcome.

Store historical accounts and present findings together, with dates and scope. Today's
reconstruction date differs from the event date. An old explanation does not automatically
remain a current constraint. A new measurement establishes the present state without proving
a historical account. Conflicting accounts can both be recorded as accounts; the most recent
text does not automatically win. Conclusions need their own grounds and reconsideration conditions.

## Explain an event when it happens

When guidance is enabled, consult `pending_tips` in `kpopper _agent status`. Show a compact card
when an unseen concept actually becomes useful, then run `kpopper _agent shown EVENT`. This
acknowledges display; it does not check that the event occurred. That evidence must be in the
work. Never manufacture an event to finish a tutorial, or refresh a judgment just to clear a
step. Explanations can span sessions and projects.

| Event | When to explain it | What the user should learn |
|---|---|---|
| `record` | A useful record is actually saved | What was kept and where a later session can find it |
| `source` | A material finding is linked to its origin | How to inspect the source supporting the finding |
| `decision` | A conclusion has grounds and a reconsideration condition | Why it stands and what could change the answer |
| `conflict` | Relevant sources or interpretations disagree | What disagrees and which decisions depend on resolving it |
| `reuse` | Earlier recorded knowledge saves repeated discovery | What it supplied and what was checked before reuse |
| `review` | An updated premise flags a conclusion | What changed and why the conclusion needs another look |

Use one short card at a natural pause; combine related concepts when one event teaches them
together. Say what happened, why it helps, and link the finding and its source when those links
exist. Never invent a link or show success before a write/check completes. Keep commands, IDs
and storage details out of the card unless needed for action. Use a native rich card where
supported and a consistent Markdown block elsewhere. Example, adapted to the user's language:

> **kpopper · The reason is saved**
>
> We kept why the workshop was scheduled for Thursday, with the message confirming the room's
> availability. A later session can revisit the date if that availability changes.
>
> Include real links to the decision and the confirming message here.

These explanations describe recorded knowledge and observed work. Source changes are detected
only when the relevant readings are updated; onboarding adds no background monitoring.
