---
name: map
description: "Map existing materials into a first knowledge record, or investigate a subject's history in depth. Use when the user asks for the map itself: to map, survey, inventory, reconstruct or investigate what already exists - folders of documents, decision records, meeting notes, emails, exports, code - or to build a starting record from existing materials, even when the word 'record' is never said. Not for an ordinary task that uses supplied files for something else: collecting them into a plan, a note or a summary is learning while working, kept by the record skill as findings arise, and is never turned into a mapping on the user's behalf. The starting choices - learn while working, map, investigate - are put to the user once by the record skill; a chosen map or investigation comes here. Requests come in any language."
---

# Map

A map is work in its own right, with a scope and a result the user asked for. Learning while working - the default - needs none of this: the [record skill](../record/SKILL.md) keeps findings as they arise, and the first useful one creates the record. The command line is `kpopper` where it is on PATH; otherwise the `command` in the `KPOPPER_AGENT_CONTEXT` line the session opener printed (`python3 <plugin>/scripts/cli.py`) runs the same code. Do not guess a path and do not write a second reader - [the method's reference](../kpopper/references/method.md#finding-the-reader) says how to find the installed copy when neither is at hand.

**If it does not exist:** there is no record here yet; knowledge may already live in the
materials. Run `kpopper open` if the hook did not supply `KPOPPER_START`, and follow
`kpopper _agent guide` for the optional first offer, source discovery and contextual explanations.
An unavailable registered record is a location problem, not a new project.

**A mapping request names the map as what the user wants**: survey these folders, reconstruct
what was decided, build a starting record from the archive, find out how a subject came to be
where it is. A request that uses the materials for something else - a plan from the quotes, a
release note from the checks, a migration plan from the notes - is ordinary work: read the files,
do the task, keep findings through the [record skill](../record/SKILL.md), and never run
`kpopper map` for it on the user's behalf. Where such work is clearly going to be revisited, the
starting offer in the [record skill](../record/SKILL.md) is the one place the choice is put to the user.

## The starting offer is the record skill's

The three starting choices are put to the user once, at the first finding worth keeping in work
that will be revisited, by the [record skill](../record/SKILL.md) - never on a greeting - and
`kpopper _agent shown welcome` records that they were shown, so no later session repeats them. A
map or an investigation chosen there, or asked for by name, is what this skill does.

**Mapping is available by choice.** An initial map of existing materials, or a deeper
investigation within agreed subjects, sources and dates, is chosen by the user - at the starting
offer or by asking for it by name; no extra confirmation is needed then, and nothing short of
naming it is read as a request. Use the guide
to locate where decisions happen and to keep historical accounts separate from present
findings. Do not expand into unrelated private sources or infer permission from silence.
For a mapping request, call `kpopper map --json` or `kpopper map --deep --json`
with the host session environment, accept and execute the returned task, and report its actual
result through the supplied internal protocol. Do not stop after announcing a ready task.
Learning during ordinary work needs no configuration. Public operations are `open`, `map`,
and `config --guidance on|off`; request IDs and receipt calls are internal.

**Teach through actual events.** A compact card can explain the first saved finding, linked
source, grounded decision, conflict, reuse, or changed premise. Show real links where available
and use the host's native card or a Markdown block. `kpopper _agent status` lists unseen concepts;
`kpopper _agent shown EVENT` acknowledges an explanation only after displaying it. The introduction
is remembered per user, the starting offer per project. Respect `kpopper config --guidance off` and do
not turn an onboarding step into a requirement for finishing the user's task.

A selected mapping or investigation makes discovery the task: follow `kpopper _agent guide` (the packaged [start guide](../../scripts/start-guide.md)), stay inside the agreed subjects, sources and dates, and report its limits. The availability of more material is not a reason to survey it. The result is a report the user can read and a record whose first entries carry their sources; on a surface that can publish, render and show the record's page with the [page skill](../page/SKILL.md).
