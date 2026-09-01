# kpopper for Cowork

No adapter here, on purpose. Cowork runs the same Claude plugin engine Claude Code
does — `hooks/hooks.json`, `SessionStart`/`Stop`, `$CLAUDE_PLUGIN_ROOT`, all of it,
unmodified. Anything built for this directory would just be a second copy of the
plugin's own root files, drifting from them the moment either side changed.

## Install

Enable kpopper as an account-level plugin; Cowork picks it up the same way Claude
Code does. There is no separate Cowork-specific install step.
