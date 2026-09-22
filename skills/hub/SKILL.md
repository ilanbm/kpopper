---
name: hub
description: "kpopper Hub: optional experimental application for exploring the project record. Use when the user explicitly requests kpopper Hub or its record page, its graph, layout or verification, or has a standing preference for this application. Do not render automatically after mapping or recording. Install the HTML runtime before use. Not for ordinary HTML documents or a text answer from the record. Requests come in any language."
---

# kpopper Hub

This application is experimental and optional. Request it explicitly or honor a standing preference; installation alone does not enable automatic use. Native bundles include the compiled application. In explicit Python compatibility mode, install `kpopper[html]` or run the active plugin's `plugin_runtime.py setup --applications html`.

The page is a visual way to read the record: hover anything for where it came from, click to walk to a dependency. Build it with the native `kpop` command and prefer the canonical executable in `KPOPPER_AGENT_CONTEXT.command`. Do not guess a path or silently use PATH, pip or Python; use the active plugin's Python dispatcher only when `KPOPPER_RUNTIME=python` is explicit. [The method's reference](../kpopper/references/method.md#finding-the-reader) describes the installed copy.

`render_page.py` turns any record into one self-contained HTML file, in two tabs: **Record**,
which nobody writes, and **Now**, the arrangement this session chose. What makes it worth
opening is the provenance layer - hover anything for where it came from, click to walk to a
dependency, and see the graph around it outlined in place.

```bash
kpop experimental hub                        # the page, at .kpopper/build/page.html
kpop experimental hub --open [--tree]        # and look at it in your own browser
kpop experimental hub --verify               # deterministic, no browser
kpop experimental hub --checks .kpopper/build/page.html   # the browser checks, on a page already written
```

The same three, by the scripts themselves - `R` is the reader's path, found as [the method's reference](../kpopper/references/method.md#finding-the-reader) says:

```bash
python3 "$(dirname "$R")/render_page.py" > page.html        # the page
python3 "$(dirname "$R")/render_page.py" --verify           # deterministic, no browser
node    "$(dirname "$R")/verify_page.js" page.html          # what only looking catches
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

When mentioning this skill to the user, include the plugin name: `kpopper:hub` or "hub from the kpopper plugin". Use the user's language and fold it into the explanation of the action; no extra announcement is needed.
