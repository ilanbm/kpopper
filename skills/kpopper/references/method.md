# The method, long-form

What the [kpopper skill](../SKILL.md) states in a few lines, argued in full: when structure is bought, what a view is, how the shape may change, and where the reader that ships with the plugin lives. Read a section when its question comes up, not before.

## Add structure only when something forces it

Structure is bought with an observed trigger, never because it would be tidier.

| what appears | only once this has happened |
|---|---|
| `GROUNDING.yaml` | the work will be revisited |
| references instead of restated material, and something that assembles the output from them | the same thing is needed twice |
| checks that **fail the build** — a broken reference, a literal where a reference belongs, a source that does not exist | there is too much for a person to eyeball |
| a named type with required fields | the same shape has appeared a **third** time |
| views as data — an operator pipeline plus a renderer referenced by name | the same question has been asked from two angles |
| judgments with a structured verdict and a gate on re-running them | a judgment started resting on another judgment |

A check that cannot fail the build is decoration. That is the whole difference between a
convention people drift from and a rule they cannot drift from.

These are six independent capabilities, not six stages of one process. They are listed in rough
order of cost, which is what makes them look like a ladder — but each is bought by its own
trigger and they can be acquired in any order. A project can have build-failing checks and no
views, or views and no named types. There is no stage a project is "at".

Stopping at row one or two is a perfectly good outcome. Reaching the bottom when nobody ever asked the same question twice means you over-built.

**The rule that settles most cases:**

> A category is justified by a question someone asked, not by the world containing it.

If nobody has asked "what is still open with Dana?", there is no reason to create a person record for Dana. No question, no structure.

### What a view is, once you reach that rung

Do not hand-write a document that reads the record. A view has a shape:

> **view = a pipeline of operators from a closed set, plus a renderer referenced by name.**

The operators are **data** — `select · filter · group · aggregate · sort · limit`. Closed on
purpose. The renderer is **code**, and the view points at it by name. A new view is a row; a new
renderer is code someone writes. The moment you find yourself expressing layout through
operators, you have invented a home-made programming language — stop and add a renderer instead.

Two invariants make a generated view worth more than a written one: every displayed value is a
placeholder resolved at build time and never typed, and **the build fails when a placeholder has
no entry behind it.** Without the second one, the view is decoration.

The renderer is itself a judgment — someone made it, for a reason, in a particular shape — so it
carries `rests_on` / `verdict` / `wrong_if` like any other. So does a layout decision ("this is
at the top because the payment has no date yet; when it gets one, it moves down"). The operator
pipeline is not a judgment: it recomputes silently and cannot be *wrong*, only badly chosen.
Keeping those apart is what stops every review of the record from marking the queries too.

Views are not a finished deliverable either. They are the projection of what is known onto what
the reader needs *now*, and that changes as the work moves. Change them when a new subject has no
expression in the output, when a prominent item is already resolved, when the same list is
maintained by hand in two places, or when a layout decision gets flagged — and record the
decision, not just the change. Early on the structure moves a lot; as the record matures the
values move and the structure mostly does not. Restructuring a mature view with no new question
behind it is polishing without a trigger.

### Views for an agent, not a person

A view for a person is typeset. A view for an agent is ranked and cut to a budget — two more
operators, `rank` and `budget`, and a different signature:

> **view = f(graph, intent) → projection**

`intent` is a parameter, not a type. "I am about to change the mortgage plan" and "I am writing
the morning brief" pull entirely different subgraphs out of the same record, and neither is the
document a person reads. So context is not a collection someone curates by hand — it is a query
with a budget, and a new session does not "read the project", it runs one.

## Changing the shape

Your first choice of shape will often be wrong. Change it freely; that is cheap. What is expensive is leaving **two** shapes side by side, because every future reader must then learn both and it never resolves on its own.

So: **a change of shape is valid only together with a migration that leaves the build green.**
Not "convert it soon" — in the same pass, with the checks passing at the end of it. That single
rule is what turns "the method can change itself" from philosophy into something runnable: you
cannot add a kind of entry without answering what happens to the existing ones. It does not
prevent change; it prevents *half* a change, which is what actually kills records like this.

## Finding the reader

A reader ships with this plugin at `scripts/provenance.py`, and the `kpop` command line is the
same code with a shorter name. Run it from the directory the record sits in — it needs only
Python and PyYAML:

```bash
kpop open                             # what to read instead of the whole record
kpop check
kpop affects <entry> [entry ...]
kpop pull <entry> [entry ...] [--from <ref>]   # --from: what another branch proposes, beside
kpop set <key> <value> [--why "..."]  # change one value; the reply is the reach
kpop set <key> <value> --source <id> --at "..."  # a new reading with its new citation
kpop add <id> field=value ...         # a new entry or judgment, in id order, seen filled
kpop review <id | "section title">    # it still holds: seen rewritten from the record
kpop consolidate [--dry-run] [NAME ...]   # the union, tested; then folded (--refute NAME "why")
kpop remeasure [--run]                # the entries that name a recipe, taken again from the tree
kpop same <a> <b> | distinct <a> <b> "why"   # one subject under two ids, or two that only look alike
```

Where that command is not on the path, the native reader still ships inside the plugin — but
**do not guess its path.** Resolve the active plugin with its runtime helper:

```bash
R=/absolute/path/to/active/plugin
N=$(sh "$R/scripts/native_runtime.sh" --path)
"$N" open
```

The helper selects the exact installed target and refuses a missing or invalid runtime. A
checkout can also install its matching runtime explicitly with `scripts/install_native.sh`.

Locate it through the active plugin helper rather than relying on `$CLAUDE_PLUGIN_ROOT`: that variable is documented for
hook and MCP configuration, and is **not** set in the shell a skill's commands run in — a command
written against it silently becomes `/scripts/provenance.py` and fails. If neither the command nor
the find turns up anything the plugin is not installed; say so rather than writing your own copy.

Do not rewrite it, and do not write a second one beside a project that already has its own build
doing this work.
