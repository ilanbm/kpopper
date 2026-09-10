---
name: page
description: "Render, arrange or verify the knowledge record's page: the two-tab HTML view with its provenance layer. Use when the user wants to see or show the record, build or check the page, or write or change a brief, tab or arrangement (the Now tab). Not for ordinary HTML documents - that is the document skill."
---

# Page

The page is how a person meets the record: hover anything for where it came from, click to walk to a dependency. Build it with the shipped renderer and never by hand. The command line is `kpopper` where it is on PATH; otherwise the `command` in the `KPOPPER_AGENT_CONTEXT` line the session opener printed (`python3 <plugin>/scripts/cli.py`) runs the same code. Do not guess a path and do not write a second reader - [the method's reference](../kpopper/references/method.md#finding-the-reader) says how to find the installed copy when neither is at hand.

`render_page.py` turns any record into one self-contained HTML file, in two tabs: **Record**,
which nobody writes, and **Now**, the arrangement this session chose. What makes it worth
opening is the provenance layer - hover anything for where it came from, click to walk to a
dependency, and see the graph around it outlined in place.

```bash
kpopper page --out record.html       # the page
kpopper page --open [--tree]        # and look at it in your own browser
kpopper page --verify               # deterministic, no browser
kpopper page --checks record.html   # the browser checks, on a page already written
```

The same three, by the scripts themselves - `R` is the reader's path, found as [the method's reference](../kpopper/references/method.md#finding-the-reader) says:

```bash
python3 "$(dirname "$R")/render_page.py" > record.html      # the page
python3 "$(dirname "$R")/render_page.py" --verify           # deterministic, no browser
node    "$(dirname "$R")/verify_page.js" record.html        # what only looking catches
```

**Read [`PAGE.md`](../kpopper/PAGE.md) before writing a brief or choosing a layout** - the brief
format, the closed set of renderers and what each one requires, and the rules that keep an
opinionated arrangement honest all live there. It is a reference, not part of the opening
cost: skip it entirely on a session that never builds a page.

```bash
sed -n '1,400p' "$(dirname "$R")/../skills/kpopper/PAGE.md"
```

**Do not write this layer yourself.** It ships here for the same reason the reader does: it
took a browser and six bugs to get right, and a session rebuilding it will produce something
worse and not know. Write layout if you need layout; call this for the mechanism.
