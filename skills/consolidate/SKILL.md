---
name: consolidate
description: "Test, fold or refute hypotheses beside the knowledge record, and reconcile records across branches. Use when open or check say hypotheses wait or an id is CONTESTED, when a write was refused into a hypothesis, before merging a branch whose record changed, and when a pull request's dry run or remeasure is red. Covers consolidate --dry-run, the fold, --refute, --from, same and distinct at the fold, and remeasure."
---

# Consolidate

A hypothesis is a claim the base does not yet hold: a refused reading, a proposal not yet approved, a branch's record, a what-if. Nothing here decides for a person; the dry run tests, the fold and the refutation are the person's acts, recorded so the question never returns. The command line is `kpopper` where it is on PATH; otherwise the `command` in the `KPOPPER_AGENT_CONTEXT` line the session opener printed (`python3 <plugin>/scripts/cli.py`) runs the same code. Do not guess a path and do not write a second reader - [the method's reference](../kpopper/references/method.md#finding-the-reader) says how to find the installed copy when neither is at hand.

One record is written by everyone, from any session or branch, as long as the write is consistent
with it. **A contradiction opens a hypothesis**, and the reader tells one by the id and the day.
`set` of a reading no newer than the base's (its `of:`, else its source's read date) that differs is
refused - two readings of one day that disagree are two writers, not the world moving; a newer
reading updates the base and flags what rests on it. `add` of an id the base holds is refused, and
with a different value or verdict it is a contradiction: a standing judgment is replaced in the base
only when its own `wrong_if` holds now, and otherwise the rewrite waits beside the record until a
person folds it. Nothing written into the body opens that door - every session's first write is a
source carrying what it was asked, so a field read as a person's authority would be a key every
session already holds. `request: s.<date>_<slug>` - a session source whose `asked:` is the person's
request verbatim, rested on - still names whose asking the change was taken from, said *on the word
of* on every surface: provenance the person weighs at the fold, and permission for nothing.
A write resting on what only a hypothesis holds belongs in that hypothesis. Each refusal names the
command that writes the same thing into `.kpopper/hypotheses/<name>.yaml` beside the record - the base
untouched - named after the id contradicted and a mark of the claim written, unless `--hypothesis
NAME` on `set`, `add` or `review` names it. The first write stamps `born` in the head; `claim:`, a
head `wrong_if:` and `folds: never` are the one thing a hand writes there. Three uses: a concurrent
writer whose reading was refused; a proposal not yet approved - a branch's record *is* this, and
travels with the branch; a what-if, `folds: never`, evaluated at every dry run and never written.

**The consolidation walk.** `consolidate --dry-run` lays the hypotheses named - every one, when none
is - over the base by id and runs the reader's own `check` on it, reported in a fixed order:
arrived, updates (what a hypothesis replaces, and what rests on it), moved / falsified, contested,
candidates, new subjects. Three lists ask three answers, each recorded by a command so the question
never returns: a candidate pair is the **same** subject (`same a b`) or **different** (`distinct a b
"why"`); a **contradictory** id - two hypotheses on one, or a reading the door refuses - is read
again on a later day, `set` in the base, or in the hypothesis that read it when the newer reading
bears its claim out but not its number, so the fold still carries what else it brought; or it is
refuted. The run exits non-zero on a contested id, a falsifier that holds, a hole, or a head
`wrong_if` it cannot decide; a premise that moved under a judgment leaves it green and blocks only
the fold, and `review <id> --hypothesis NAME` refreshes the snapshot against the record as it stands
under the hypothesis. `consolidate` runs the same test and, only when it is clean, writes the union
through the write path: every replacement passes the one door a `set` passes, so a reading born of a
same-day refusal waits until someone reads again on a later day - while a verdict over a standing
judgment the record's own sign has not broken passes here and nowhere else, because the fold is the
person's act and says so in the line above it; the result is read back and undone whole if `check`
then says anything new; the folded files go, and what to commit is printed.

Before accepting a material consolidation, examine affected judgments for a failure that arises
only when the changes are combined, even if each change passes separately. Use the same
[failure search](../kpopper/references/falsifiers.md#look-for-a-failure-the-current-check-would-miss) to expose shared assumptions
or newly incompatible decisions. Record any resulting objection or additional condition, and
repeat the dry run after changing the record; a passing dry run covers the declared checks.

**One rule, two containers.** What stands is contested only by something recorded: a rival claim, in
a hypothesis; or a doubt with no rival value yet, an open question that names the id. The page says
*a hypothesis contests this arrangement*, or *a question* does, `check` notes the same, and both
stand until a person consolidates or answers. Deleting the file is neither.

**How a merge goes.** git merges the files: additions in id order rarely meet, and hypothesis files
meet only when two branches claim the same thing under one id, since the name carries both - two
readings that agree, and either head is the whole of them; two that disagree are two files git
merges, and which of them stands is the dry run's question for a person. The dry run tests the
result: the pull request runs it on the merged tree, and the push to `main` runs it again as the
second net, for two pull requests each consistent alone that contradict together. Fold a hypothesis
the dry run proves *before* the pull request, so `main` receives base changes; let an unproven one
merge as a file, and `main` carries an open hypothesis the opener counts. Nothing crosses branches
unasked: `pull <seed> --from <ref>` lays what another branch committed beside your pull, and
`consolidate --from <ref> --dry-run` tests its record as one more hypothesis named after the ref - a
pull, never a push, and how a dead branch's facts are harvested. Every write - `set`, `add`,
`review`, `same`, `distinct`, the fold, the refutation - takes an exclusive lock on the record's
directory where the platform has one - Windows has none - so two sessions on one file take turns
instead of the last one discarding the first. In the tree each worktree writes its own copy and git
merges them; out of the tree every worktree writes the one file, and only the lock stands between
them.

## The tree, measured against the record

`remeasure` is how the tree answers for the record. `check` compares `seen` against the record's
own stored value and never against the tree, so a count that is wrong about the tree passes as
long as it agrees with itself. An entry whose value is a fact about the tree - lines of a file,
files a package ships, places in the code where something is decided - names the recipe that
takes it, `measure: <name>`, and `.kpopper/measure.yaml` beside the record holds that name's
argument list. The name is all the record carries: a bare name, refused by `add` and failed by
`check` when it is not one or stands on anything but a stored scalar reading. `kpopper remeasure`
prints the plan and runs nothing; `--run` runs each cited recipe once, from the checkout's root,
without a shell, and lays what differs over the record as one more hypothesis, `tree/<commit>`,
through the same dry run that tests any hypothesis - red on a falsifier that holds on the measured
value, a hole, or a reading the tree contests; green, with the `set` command that refreshes it, on
an older reading that moved without crossing a line. A hypothesis that replaces a measured entry
carries the line with it, since the fold takes its block over whole. The pull request runs it; nothing that reads
the record does. Name a recipe only where a command honestly takes the count the entry's `at:`
describes - a survey, a hand-scored run, a prior has none, and their honest form is the reading
with its date.

## The commands

```bash
kpopper consolidate --dry-run [NAME ...]     # the union, tested; every hypothesis when none is named
kpopper consolidate [NAME ...]               # folded into the base when the test is clean
kpopper consolidate --refute NAME "why"      # one negative finding stays; the file goes
kpopper consolidate --from <ref> --dry-run   # another branch's committed record as one more hypothesis
kpopper same <a> <b> | distinct <a> <b> "why"   # the two answers the candidates list asks for
kpopper remeasure [--run]                    # the entries that name a recipe, taken again from the tree
```

The pull request runs the dry run and the remeasure on the merged tree, and the push to `main`
runs them again; [coding-and-ci.md](../../docs/coding-and-ci.md) shows the steps and how to
inspect another branch's record before merging.
