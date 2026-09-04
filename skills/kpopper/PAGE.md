# The page

*Reference for `render_page.py`, which ships with the kpopper plugin. Read this when you
are about to build or change a page; it is not part of what a session reads to start.*

`render_page.py` turns any record into one self-contained HTML file — no domain knowledge,
nothing typed twice. The first two are what `kpopper page` runs for you:

```bash
kpopper page --out record.html                              # the page
kpopper page --verify                                       # deterministic, no browser
```

The third has no wrapper, so it needs the directory the scripts sit in. Locate that the same way
the skill locates the reader — skipping a checkout's own worktrees, highest version wins — rather
than assuming where an installed plugin unpacks:

```bash
S=$(dirname "$(find ~/.claude -name worktrees -prune -o -path '*kpopper*/scripts/render_page.py' -print 2>/dev/null | sort -V | tail -1)")
node "$S/verify_page.js" record.html                        # what only looking catches
```

What makes it worth opening is not the layout — it is the provenance layer. Hover any key to
see where the value came from, click to pin, click a dependency to walk to it, back out with
the button or Escape. Reading **up** toward a source and **down** into what a judgment rests on
are the same gesture, which is why one mechanism serves both.

**Do not write this layer yourself.** It ships here for the same reason the reader does: it took
a browser and six bugs to get right, and a session rebuilding it will produce something worse
and not know. Write layout if you need layout; call this for the mechanism.

## The tabs, and who writes each

**Record** is the tab nobody writes: everything in the record, grouped by nothing but the
record's own prefixes. It is the fallback when an arrangement is wrong, and it is the only tab
when you supply no brief.

When hypotheses wait beside the record (`PROVENANCE.d/`), one line under the heading says how
many, and how many are contested; the page draws the base alone - what a hypothesis proposes is
read with `kpopper pull`.

**Now** is the tab *you* write, and you are the only one who can: the arrangement of this record
aimed at what this session is for. Intent is the one input the record cannot derive — it lives
in the conversation, and a session records it as a source (`s.*`, below) so the page can be held
against it. The arrangement itself goes in a brief beside the record (`<record>.view.yaml`,
picked up automatically):

```yaml
title: "The loan, this week"
intent: "Close the loan documents before the rate lock expires on the 18th"
sections:
  - title: What is blocking now
    why: each of these is a judgment that cannot close for want of a fact nobody has recorded
    pick: blocked
  - title: The dates that are running
    pick: d.
  - title: The mortgage
    pick: [mtg., c.equity_10pct]
    as: lines
shape: {entries: 89, judgments: 12, flagged: 9, blocked: 4}
```

`pick` takes a state (`blocked`, `unchecked`, `broken`, `falsified`, `moved`, `no_predicate`,
`flagged`, `judgments`, `all`), a prefix (`d.`), or an exact id — evaluated at render time,
never a frozen list. That is what keeps a section current: a judgment that becomes blocked
tomorrow appears under *What is blocking now* with no edit to the brief.

`title:` names the page — the browser tab, and the heading when the record carries no name of
its own. It is presentation, which is why it lives in the brief; without it the page is named
after the record, and a record without a name is called `record`.

`as` picks the shape. This is where intent actually shows, so the set is deliberate and closed —
each renderer states what data it can carry, and asking one to hold data it cannot express is a
build failure rather than a page that looks arranged and is not:

| `as` | for | requires |
|---|---|---|
| `timeline` | anything that happens on a date — deadlines, validity windows | every pick has a date value; today is marked in place |
| `grouped` | separate groups read side by side (a grid on a wide screen); `fronts` still reads as this shape | two or more groups among the picks, in the scheme the section reads by |
| `headline` | the one to four numbers everything else turns on | one to four picks, each with a literal value |
| `alerts` | what needs a person, worst first | picks are judgments |
| `cards` | judgments read for their reasoning | — (default for judgments) |
| `table` | the record's own key/value shape | — (default for entries) |
| `lines` | a bare set of names | — |

A run of key/value rows is not an arrangement, it is a dump with a heading. If the only shape
that fits a section is `table`, that is a signal the section is not about anything in
particular.

**Groupings are declared, not inferred.** The record's prefixes say what *kind* a thing is —
`d.` is a date — and that is a different question from which thread it belongs to. `d.prg_out`
is a date and it is Prague; nothing in the record says so. So the brief declares its groupings
once, under whatever names the project reads by — fronts, fields, subsystems, environments —
and every renderer can then say what a row is under:

```yaml
groups:
  The mortgage: [mtg., equity., d.rate_lock, c.equity_10pct]
  Prague:       [prg., d.prg_out, d.prg_deadline]
```

A group then appears as a small tag beside each item wherever a section mixes more than one —
on a timeline row, an alert, a headline caption. Without it a reader looking at a list of
eleven dates has no way to tell which of them are even about the same thing.

A grouping is a scheme, and a project rarely reads by one. The same entries group by thread
on one section and by counterpart on another, and a tag is nothing but a scheme whose groups
overlap. So `groups:` may declare several schemes, each under its own name, and a section says
which one it reads by:

```yaml
groups:
  threads:
    The mortgage: [mtg., equity., d.rate_lock, c.equity_10pct]
    Prague:       [prg., d.prg_out, d.prg_deadline]
  counterparts:
    Adi:   [mtg.adi_fee, q.adi_gift, q.cond7]
    Dolev: [crypto., q.dolev_filed]
sections:
  - title: Who is holding what
    pick: [mtg., crypto., q.]
    as: grouped
    by: counterparts
```

A flat `groups:` is one scheme. What the record already carries needs no declaration: `by:
prefix` reads the id's namespace, and `by: from`, `by: unit`, `by: kind` — any field the entries
carry — read that field, naming the group by the value's own name when the value is an entry.
Declare a scheme only for a reading the record does not know; where membership is a fact about
the world, record it on the entry and read by the field. A section draws by the scheme it reads by, and an id
under two groups of that scheme is drawn under both — which is all a tag is: a scheme whose
groups overlap. A section that reads by a scheme nobody declared fails `--verify`; a group that
picks nothing is said. `fronts:` and `as: fronts` are the older names for one scheme and this
shape; they still read, until the briefs that use them are renamed.

Give sections a `why:` as well as a `title:`. A title names a section; the `why` says what the
reader is supposed to do with it, and it is the difference between a heading and a hand-off.
Individual rows carry their own one-line explanation when the record has one — a `via`, `note`
or `why` on the entry — which is where "why is *this* the date" gets answered.

`labels:` in the brief renames entries for the surfaces that are not about keys. A label is a
presentation choice, which is exactly why it lives in the brief and not in the record. Without
one the page falls back to a name the record carries, then to the key's last segment.

The page's own direction follows the record's: a record written in Hebrew or Arabic is laid out
right to left. Like every other layout decision here, it reads the record's shape and never its
values, so it cannot flip when a number changes.

## The tree

A third tab draws the whole record as one growing thing: what was read from the world is the
root fan below the ground line, worked-out values branch above it, and judgments blossom in a
canopy dome — the three node kinds in their three colors. It answers the question a reader has
once, at first meeting — what is the shape of the whole — so the reading surfaces stay flat.
Every node is the same live card as everywhere else; clicking one lights the sap: the full
chain of limbs and roots that feeds it in one color, everything it feeds in another, and its
direct neighbours stir like branches in wind. A record too large to draw keeps its canopy and
says how many roots stayed below the grass.

The first time the tab is opened the tree grows into place — roots, then trunk, then limbs,
then blossoms, over about a second. Once per page load, because the question it answers is
asked once; a reader who has asked for less motion gets the finished tree and no wait at all.

A node can also be pulled, and it answers the way a branch does. Against a pull of `d` the
applied displacement is `R * tanh(d/R)`, with `R` an eighth of the drawing's width: it follows
at first, gives less the further you go, and past `R` it visibly stops following the hand. Its
limbs stretch with it and the neighbours it crowds are elbowed aside; let go and everything
springs back to where the record put it. Nothing is kept — the position is written nowhere,
and it could not be, because the page has no write path at all. Pulling is for feeling out how
a part of the tree is attached, not for arranging it.

## References belong in the sentence

A row of monospace ids under a card is the graph leaking onto the reading surface. So the page
does not draw one. Instead it binds what the prose already says: an id written out, or a
dependency's value quoted in the text, becomes hoverable in place. Candidates are limited to
*that judgment's own dependencies* — anchoring against the whole record invents links, and a
false link is worse than a missing one. Restatements that are not exact ("~180,000" for 179,842)
are deliberately not matched.

While a card is open the page also outlines the graph around it, in place: a solid outline on
everything that rests on the open thing, a dashed one on everything it rests on. That is the
dependency display, and it is better than a diagram of the whole graph for the same reason the
namespace line beats a table of contents — it answers *what does this reach*, which is the
question people actually have, instead of *what is the shape of the whole thing*, which is the
question nobody asks.

Two things stay visible as keys, because both are about something the reader has to go and get:
a dependency that is **missing**, and the key a blocked line is **waiting on**. Everything
present is reached by hovering the words that mention it, and whatever the prose did not name is
still one hover away on the judgment itself — `--verify` reports how much of each kind there is.

A derived entry has no value in the record, it has a rule; the reader detects that by shape (an
expression naming other entries) and shows the rule, with the ids inside it live. It never shows
an empty cell where a number belongs, and it never lets `headline` carry one.

A card and a section's text carry the same warning when what they rest on has moved since they
were reviewed: a tint, the moved value marked in place with what it was, and one line naming
what moved — a judgment's dependencies against its `seen`, a text's references against its own.
It is in the markup, not applied by script, so it shows in a host that strips scripts too, and
it is a warning rather than a correction: nothing on the page rewrites the sentence. `kpopper
review <judgment>` or `kpopper review "<section title>"` clears it, after someone has read the
sentence against the new value.

Two properties keep an opinionated tab honest, and both are mechanical rather than remembered:

- **It may order. It may not drop.** Anything flagged that no section picked up lands in a
  trailing section written by the page, which the brief cannot switch off - and so does whatever
  a session wrote for an intent no tab serves. It is counted once for the page and drawn on
  every tab, because a reader opens a tab and not the page. An arrangement that hides what it
  did not anticipate is worth less than no arrangement.
- **The arrangement itself can be wrong.** `shape:` is the brief's own `seen` — over the
  record's *shape*, not its values, because a date moving does not make a layout wrong but a
  fourth blocked judgment might. Render with no `shape:` and the command prints the block to
  paste; render after the shape moved and the tab says so at the top. `kpopper review` of the
  arrangement judgment rewrites it from the record — the one tab's shape, or the shape of the
  tab serving a session source the arrangement rests on — and nothing refreshes it by itself.

Rewrite the brief freely. It is the cheapest file in the method — derived from a moment, not
from the record — and an arrangement nobody chose to keep is not one worth maintaining.

## Intents, tabs, coverage

A session records what it was for as a source - `s.<date>_<slug>` with `asked:` verbatim and
frozen, and `name:` as the session's own reading, revisable until it stops. What the session
writes carries `from:` that source, and a judgment the request itself is a premise of rests on
it; those two are how the record knows what a session recorded. A session that turns out to
have done two things splits into two sources.

A tab is a reading occasion, not an intent: `occasion:` says when the reader opens it and for
what, and `serves:` lists the intents it answers - several can share one occasion. Every tab a
brief declares is drawn, under its own name, the first as the default; each says what it serves,
in the requester's own words and one hover from the source.

**Serving is earned by picks.** A tab serves an intent when its sections pick something that
session recorded. Declare `serves` and pick nothing of it and `--verify` fails: the claim cannot
be kept. An intent that recorded nothing stands outside coverage - it is said once and counted
nowhere, since nothing could serve it and nothing needs to.

**Coverage is mechanical and deliberately dumb.** Every build and every `check` hold the page
against the intents and print what they count, as facts:

- each intent no tab serves, with one hint beside it: the prefixes it wrote, and how many of
  its entries already fall inside each tab's picks;
- each tab against what it serves - how much of what those sessions recorded it picks;
- the prefixes no section picks;
- the counts: what is covered, the spill, how many intents are unserved, how many of the newest
  sessions in a row are - by day, since a day is the finest clock the record keeps, and a day
  on which any intent is served ends the run - and drift, the share of what sessions read on or
  after the newest `born` recorded that nothing picks.

The hint is printed and never acted on. Whether an intent is the same world as a tab that
already picks most of what it wrote is a session's judgment - declare that the tab serves the
intent, add a section, or open a tab - recorded as an arrangement decision with its own falsifier
over these counts. The mechanism decides none of it and carries no threshold.

The opener says the newest unserved intent in its `next:` line - one line, and `check` carries
the rest; an arrangement whose sign appeared comes first in that line, since it is the gap read
by a decision. The Stop gate holds the session's end against the mark its opener took and reminds,
once, about what the session itself left: the record failing worse than it found it, an intent
it left unserved, entries it wrote with no intent recorded. What was already red or unserved
when the session opened never bounces it. The two commands behind the hooks, `mark <state
file>` and `gate <state file>`, are the hooks' own.

## Arrangements: the decisions the page is held to

A tab is an arrangement someone chose, and the choice is a judgment like any other - recorded,
with what it rests on and what would make it wrong. Nothing here invents a second kind of
staleness for it; what is special is only what it is *held against*.

**An arrangement, by shape**, is a judgment that rests on a session source - the occasion it
decides - and whose sign is a name the build computes, read by its `wrong_if` or rested on. `v.`
is the prefix to use and nothing depends on it. A judgment over the page's counts that rests
on no session source decides no occasion; a judgment resting on a session source with no count
in its sign is that session's ordinary decision.

```yaml
v.glazing_tab:
  rests_on: [s.2026_09_03_glazing, page.unserved]
  verdict: "a second tab, for the day the quote is read"
  because: "... the two merge the day a judgment on one tab rests on the other's numbers."
  wrong_if: "page.unserved > 0"
  born: "2026-09-03"          # written by add
  seen: {s.2026_09_03_glazing: "read 2026-09-03", page.unserved: 0}
```

**Its sign is one comparison that can hold.** `check` fails an arrangement whose `wrong_if` is not
`<name> <op> <one value>` - a compound sign reads as evaluable and is never decided, which is
freeze in disguise - or one that can never hold (a count below zero, a share above one), and `add`
refuses both. A `reopened_by:` may stand beside the comparison, never in its place. The sign the
build cannot count goes in `because`, in prose, beside the nearest count that can: *the two merge
the day a judgment on one tab rests on the other's numbers*, beside `page.unserved > 0`. And ask of
every one the question the record asks of every falsifier: under this arrangement, can the sign
still appear?

| write | it fires when | the smallest repair |
|---|---|---|
| `page.unserved > 0` | an intent landed that no tab serves - while this arrangement stands, never its own - a new occasion, or one this tab should claim | a tab declares it serves it, if its sections pick what it wrote; else a section; else a tab, entering last |
| `page.spill > 0` | something flagged fell outside every section | widen a pick; add a section |
| `page.drift > 0.<n>` | that share of what sessions recorded since the page was last decided is picked by nothing; counted from the newest `born` on the page, so a newer decision resets it | widen picks; a section; the threshold is the author's claim |
| `page.recent_unserved > <n>` | the last n sessions in a row were unrepresented | a new occasion, decided |
| `graph.flagged`, `graph.blocked`, `graph.judgments`, `graph.entries` `> <n>` | the record's shape outgrew the layout | the shape the arrangement stood on is reviewed with it |
| `graph.hypotheses`, `graph.contested` `> <n>` | contested claims pile up under the arrangement | consolidate before rearranging |

**Its tabs are the ones that earn its sources.** A tab serves an intent only by picking what that
session recorded, so an arrangement's tabs are the tabs whose picks earn a source it rests on -
one tab, or several when it decides the whole page at once - and a tab that claims no occasion
(no `serves:` line) belongs to every arrangement, as the one bare tab does. It is *linked* while
every source it rests on is earned by some tab. A tab deleted, or gutted with its `serves:` line
kept, cuts the link the same way.

**What the page draws for it**, under the tab's occasion line and quietly: *decided 2026-09-02 ·
stood 4 sessions · on the word of …*. `stood` is derived - the sessions read on a later day than
it was born that one of its tabs served; a later session that landed unserved is the sign, not
evidence, and one that recorded nothing never met it - and it is never stored. The decision drawn
on its tab is not a pick: a tab that wants the session that decided it to count as served picks
the decision in a section of its own.

**A shape move reads by the arrangement's own sign.** When a tab's stored `shape:` no longer
matches the record and an arrangement governs the tab, the move is *fired* if that arrangement's
`wrong_if` holds - the banner, the failure at `--verify`, the `next:` line at `open` - and *muted*
otherwise: said quietly with the counts beside it as facts (spill, unserved, drift, and the share
of what sessions recorded since *this* arrangement was born that nothing picks), and settled by
`kpopper review` whenever a session next reads it. The mechanism draws no line of its own. A tab
no arrangement governs keeps the plain banner.

**The brief is held against the decisions that stand**, not against its own last version - an
out-of-tree record has no git to ask. What the link sees is the radical layer, and only that:

- a tab that earns intents no arrangement rests on is *an arrangement no decision records* -
  a note, at `--verify` and `check`: add one;
- a standing arrangement whose sources no tab earns together - a split, a gutting, a deletion -
  is *the brief does not serve … as it decided*: a failure at `--verify` **and at `check`**, so
  the Stop gate bounces once on a record `--verify` never runs on;
- two arrangements with no source in common, both earning sources on one tab, is a merge no
  decision records: a failure the same way - re-decide the one whose occasion changed, and review
  the other;
- a brief with no arrangement at all is said once by `--verify`, on a record whose sessions could
  be served by one.

Everything else is gradual and needs no decision: a section repopulating, a section added, a pick
widened, text reviewed, a label, a group, a tab declaring it serves one more intent. A section
moved across tabs is not seen by the link; it is seen by its effect on the counts. Deletion has no
green ending of its own: an intent that recorded something is served or unserved for good, so a
deleted tab either leaves its sources unserved - a gap printed at every open - or reads them on
another tab, which is a merge.

**A reversal is the arrangement written again under its own id.** Editing the brief past a
standing decision fails; a decision changes only by `add v.x …` with a new verdict, which is a
contradiction of what the base holds unless one of three things is true, decided by the same door
every same-id write goes through:

1. **its sign holds, with its tabs intact** - the world moved, and the re-decision is admitted in
   place: `born` renewed, `seen` fresh, and one line appended to `replaced:` keeping the born of
   what it replaced, how long it stood, and the sign that ended it. A tab already deleted or
   gutted is refused with *restore the tab, then re-decide*: a deletion makes the very sign it
   would cite;
2. **a person asked** - the new body carries `request: s.<date>_<slug>` and rests on it, a
   session source whose `asked:` is the person's words verbatim and nothing else; admitted the
   same way, and *on the word of* that source on every surface;
3. **neither** - refused, with the exact `--hypothesis` command that writes it beside the record
   instead; the page then says *a hypothesis contests this arrangement*, and `check` says so, until
   a person consolidates. An open question that names the arrangement contests it the same way.

Never twice in a day: a re-decision of an arrangement born today is a contradiction, not a change,
whatever its sign says - two sessions cannot flip it, and a second writer cannot re-decide it
behind the first while the first's repair is still being made (a person's request is the one
exception). A re-decision is exempt from the birth check - it is recorded while the sign that
ended the old decision still holds - and the next build decides it against the repaired brief. A
new tab is the other way round: give the tab first, then decide it, since an arrangement whose own
sign already holds is refused at birth.

**Repair, smallest first.** When a sign appears, clear it with the smallest structural change that
suffices - declare a tab serves one more intent before adding a section; add a section before
moving one; move before removing; remove before rebuilding. A new tab enters last, until it earns
its place. If you chose the bigger change, say in the arrangement's `because` why the smaller one
was not enough. One radical change per session, at its end, when the session knows what it did -
and the record holds you to one re-decision of an arrangement per day. Seniority is evidence,
never a threshold: the page shows how many later sessions a tab served so that a reader can weigh
it; nothing in the mechanism weighs it for them.

## What each check is for

`--verify` is deterministic: every element resolves to an entry, every entry in the payload is
shown, every dependency points at something carried, the session tab is the default when a brief
exists, no authored section picks nothing, and every `serves` is earned. That last one matters more than it looks — a
section about something the record no longer holds is the alert row for a closed problem, and it
costs trust on everything else on the page. A dependency that is missing but *declared* missing
by a `blocked_on` is not a failure; it is reported as a note and drawn as awaited rather than
broken. A reader that cannot tell a declared hole from a mistake makes declaring one pointless.

The browser check is a separate job, not a fallback — it catches the popover that vanishes when
you reach for it, which no assertion about the HTML ever will.

Where two fields genuinely fit one role the reader refuses to guess and asks for three lines.
Add them rather than renaming anything:

```yaml
schema:
  predicate: wrong_if
```

A checker that quietly passes over what it cannot read is worse than no checker, so every
ambiguity is an error and never a skip.

## Fields the reader accepts today

Everything below is read, checked and reported now; where the page does not yet draw a field,
`--verify` says so in a note rather than staying silent. A new field goes into the fixture
record in `tests/fixtures/page` first, so the reader, the page and the tests agree on its shape.

**In the brief**

| field | where | what it is |
|---|---|---|
| `tabs:` | top level | a list of tabs, each with `title`, `occasion` (when the reader opens it and for what), `serves` (the session sources it answers - a claim its sections earn by picking what they recorded), `sections`, and its own `shape`. Every tab is drawn, the first as the default. |
| `groups:` | top level | grouping schemes, any number, each a mapping of group name → selectors under the session's own names; a flat mapping is one scheme. `fronts:` still reads as one scheme. |
| `by:` | on a section | the scheme this section reads by — one the brief declares, or one the record carries by its own shape: `prefix`, or any field the entries carry (`from`, `unit`, `kind`), whose value names the group. |
| `text:` | on a section | connective prose with `{{id}}` references, drawn where the section stands with every reference resolved — the value, a rule's name, a judgment's verdict — and `{{c.id}}` placing that judgment's reasoning at that spot, marked as a judgment and hoverable as one. A section may be text alone; whatever the text places counts as picked up, so it never lands in spill. Every reference must be an entry. |
| `reviewed:`, `seen:` | on a section with `text` | when the text was last read against what it references, and the values it saw. `--verify` compares them with the record the way `check` compares a judgment's snapshot; a value that moved tints the text on the page, marks it in place with what it was, and says what moved beneath. `kpopper review "<section title>"` rewrites both from the record. A reference the `seen` does not carry is noted: the text was never read against it. |

Every tab is checked the same way: its picks must pick something, its shapes must fit what they
pick, its `serves` must name session sources and be earned by its picks, and its own `shape` is
compared with the record's - a tab whose shape moved says so at its top, and `--verify` notes it.

**In the record**

| field | where | what it is |
|---|---|---|
| `asked:` | on a session source (`s.*`) | the request verbatim, frozen; `name:` beside it is the session's own reading, which that session may revise until it stops. Entries the session writes carry `from:` it, and a judgment the request is a premise of rests on it. The hover on the source shows it, a tab that serves it quotes it, and it is what makes the source an intent the page is held against. |
| `{{id}}` | in any text field — `because`, `via`, `note` | a reference, never a retyped value. `check` fails one that names nothing, and one inside a judgment that names something the judgment does not rest on. A card draws it: the value where there is one, the name where there is only a rule, the verdict for a judgment — each hoverable. |
| `born:` | on an arrangement | the day it was decided - stamped by `add` like `seen`, renewed when it is re-decided, refused when typed. Drift is counted from the newest `born` the record carries; how long an arrangement stood is derived from it and never stored (below). |
| `request:` | on an arrangement | the person's request it was taken from: a session source whose `asked:` is the request verbatim, which the arrangement also rests on. Nothing by shape can tell a request from a session's intent, so the claim is explicit, and every surface that shows the decision says *on the word of* that source. |
| `replaced:` | on an arrangement | written by `add` when a decision replaces another under the same id, one line each, oldest first: the born of what it replaced, how many sessions it stood, and the sign that ended it - so the sequence of decisions reads from the record alone. |
| `graph.*`, `page.*` | as a dependency, or inside a falsifier | names the reader computes; see below. |
| `reopened_by:` | on a judgment | the prose sign that re-opens a judgment decided on a session's prior — a `prior.*` claim whose value is the confidence — or on taste. `blocked_on` keeps its meaning: the predicate cannot be evaluated, and why. Not a hole and not waiting: the judgment needs no person, `check` counts it among the declared, and the card shows it in a row of its own. |
| `measure:` | on an entry | the name of the recipe that takes the value again from the tree - a bare name, never a command. `PROVENANCE.measure.yaml` beside the record maps it to an argument list, and only `kpopper remeasure --run` - the pull request's step - runs it; what differs is laid over the record as the hypothesis `tree/<commit>` through the same dry run. Stands on a stored scalar reading alone: `check` fails it on a judgment, a rule, a source, a computed name, or a name that is not one; a hypothesis replacing a measured entry carries the line with it. `pull` says *measured by*; the page does not draw it yet. |

**Computed names**

A judgment may rest on a count, and a falsifier may draw its line against one:
`wrong_if: "page.spill > 0"`. These names are never written and never stored. One becomes an
entry the moment something in the record mentions it, with its value counted each time the
record is read (`graph.*`) or each time the page is built (`page.*` — a predicate over it is
decided by `page --verify`, and `check` says so). Every page name is counted before the
arrangement's own falsifiers are decided, and a falsifier over one that holds fails `page
--verify`. One of them can hold no value: `page.drift` needs a `born` to count from, and a
record whose arrangements carry none leaves it uncounted - the page says *not counted yet*
where its value would stand, `add` refuses to snapshot it, and a falsifier over it stays
undecided.

A count is taken before any judgment that reads a count is decided, so it never includes what
reading it decided: a line drawn against "how many are flagged" cannot be crossed by the
drawing of it. Every surface then decides such a judgment against the same numbers.

| name | counts |
|---|---|
| `graph.entries`, `graph.judgments`, `graph.open` | what the record holds |
| `graph.flagged` | judgments that need a person, for any reason |
| `graph.blocked`, `graph.broken`, `graph.unchecked`, `graph.moved`, `graph.falsified`, `graph.no_predicate` | the reasons, one each |
| `graph.hypotheses`, `graph.contested` | hypotheses waiting beside the record, and the ids two of them hold with different claims |
| `page.spill` | flagged judgments no section of the page picked up |
| `page.unserved` | intents no tab serves - declared and earned by picks; an intent that recorded nothing is not counted |
| `page.recent_unserved` | the newest sessions in a row whose intent no tab serves, counted by day: a day on which any intent is served ends the run |
| `page.drift` | the share of what sessions read on or after the newest `born` recorded that nothing picks; 0 when nothing was added, no value without a `born` |
| `page.covered` | entries and judgments some section picks |

