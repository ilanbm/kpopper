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

## Two tabs, and who writes each

**Record** is the tab nobody writes: everything in the record, grouped by nothing but the
record's own prefixes. It is the fallback when an arrangement is wrong, and it is the only tab
when you supply no brief.

**Now** is the tab *you* write, and you are the only one who can: the arrangement of this record
aimed at what this session is for. Intent is the single input that is not in the record — it
lives in the conversation and dies with it, so it cannot ship and cannot be derived. Put it in a
brief beside the record (`<record>.view.yaml`, picked up automatically):

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
| `fronts` | separate fronts read side by side (a grid on a wide screen) | two or more fronts among the picks |
| `headline` | the one to four numbers everything else turns on | one to four picks, each with a literal value |
| `alerts` | what needs a person, worst first | picks are judgments |
| `cards` | judgments read for their reasoning | — (default for judgments) |
| `table` | the record's own key/value shape | — (default for entries) |
| `lines` | a bare set of names | — |

A run of key/value rows is not an arrangement, it is a dump with a heading. If the only shape
that fits a section is `table`, that is a signal the section is not about anything in
particular.

**Fronts are declared, not inferred.** The record's prefixes say what *kind* a thing is —
`d.` is a date — and that is a different question from which front it belongs to. `d.prg_out`
is a date and it is Prague; nothing in the record says so. So the brief declares the fronts
once, and every renderer can then say what a row is under:

```yaml
fronts:
  The mortgage: [mtg., equity., d.rate_lock, c.equity_10pct]
  Prague:       [prg., d.prg_out, d.prg_deadline]
```

A front then appears as a small tag beside each item wherever a section mixes more than one —
on a timeline row, an alert, a headline caption. Without it a reader looking at a list of
eleven dates has no way to tell which of them are even about the same thing.

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

Two properties keep an opinionated tab honest, and both are mechanical rather than remembered:

- **It may order. It may not drop.** Anything flagged that no section picked up lands in a
  trailing section written by the page, which the brief cannot switch off. An arrangement that
  hides what it did not anticipate is worth less than no arrangement.
- **The arrangement itself can be wrong.** `shape:` is the brief's own `seen` — over the
  record's *shape*, not its values, because a date moving does not make a layout wrong but a
  fourth blocked judgment might. Render with no `shape:` and the command prints the block to
  paste; render after the shape moved and the tab says so at the top.

Rewrite the brief freely. It is the cheapest file in the method — derived from a moment, not
from the record — and an arrangement nobody chose to keep is not one worth maintaining.

## What each check is for

`--verify` is deterministic: every element resolves to an entry, every entry in the payload is
shown, every dependency points at something carried, the session tab is the default when a brief
exists, and no authored section picks nothing. That last one matters more than it looks — a
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
| `tabs:` | top level | a list of tabs, each with `title`, `occasion` (when the reader opens it and for what), `serves` (the session sources it answers), `sections`, and its own `shape`. The page draws the first tab today and counts the rest. |
| `groups:` | top level | named groupings of ids, any number, under the session's own names — what `fronts:` was; `fronts:` still reads as one grouping. |
| `text:` | on a section | connective prose with `{{id}}` references, resolved where the page draws it; `{{c.id}}` asks for that judgment's reasoning at that spot. Checked now — every reference must be an entry — and drawn soon. |
| `reviewed:`, `seen:` | on a section with `text` | when the text was last read against what it references, and the values it saw. `--verify` compares them with the record the way `check` compares a judgment's snapshot and says which moved; the tint follows when the text is drawn. |

A tab the page does not draw yet is checked as if it did: its picks must pick something, its
shapes must fit what they pick, its `serves` must name session sources, and its own `shape`
is compared with the record's.

**In the record**

| field | where | what it is |
|---|---|---|
| `asked:` | on a session source (`s.*`) | the request verbatim, frozen; `name:` beside it is the session's own reading, which that session may revise until it stops. Entries the session writes carry `from:` it. |
| `{{id}}` | in any text field — `because`, `via`, `note` | a reference, never a retyped value. `check` fails one that names nothing, and one inside a judgment that names something the judgment does not rest on. A card draws it: the value where there is one, the name where there is only a rule, the verdict for a judgment — each hoverable. |
| `born:`, `stood:` | on an arrangement judgment (`v.*`) | when the arrangement was decided, and how many builds it has stood. |
| `graph.*`, `page.*` | as a dependency, or inside a falsifier | names the reader computes; see below. |

**Computed names**

A judgment may rest on a count, and a falsifier may draw its line against one:
`wrong_if: "page.spill > 0"`. These names are never written and never stored. One becomes an
entry the moment something in the record mentions it, with its value counted each time the
record is read (`graph.*`) or each time the page is built (`page.*` — a predicate over it is
decided by `page --verify`, and `check` says so). Of the page names, `page.spill` is counted
today — what fell through the arrangement before the arrangement's own falsifiers were
decided — and a falsifier over it that holds fails `page --verify`; the other page names are
reserved and hold no value yet, so a falsifier over one stays undecided until they do.

A count is taken before any judgment that reads a count is decided, so it never includes what
reading it decided: a line drawn against "how many are flagged" cannot be crossed by the
drawing of it. Every surface then decides such a judgment against the same numbers.

| name | counts |
|---|---|
| `graph.entries`, `graph.judgments`, `graph.open` | what the record holds |
| `graph.flagged` | judgments that need a person, for any reason |
| `graph.blocked`, `graph.broken`, `graph.unchecked`, `graph.moved`, `graph.falsified`, `graph.no_predicate` | the reasons, one each |
| `page.spill` | flagged judgments no section of the page picked up |
| `page.unserved`, `page.recent_unserved` | intents no tab serves; recent sessions in a row left unserved |
| `page.drift` | the share of what was added since the arrangement was born that nothing picks |
| `page.covered` | entries and judgments some section picks |

