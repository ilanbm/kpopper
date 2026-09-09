# kpopper for Cowork

No adapter here, on purpose. Cowork runs the same Claude plugin engine Claude Code
does — `hooks/hooks.json`, `SessionStart`/`Stop`, `$CLAUDE_PLUGIN_ROOT`, all of it,
unmodified. Anything built for this directory would just be a second copy of the
plugin's own root files, drifting from them the moment either side changed.

## Install

In Cowork, open **Customize → Plugins → Add marketplace**, enter `ilanbm/kpopper`,
then install **kpopper** from that marketplace. The full repository URL,
`https://github.com/ilanbm/kpopper`, also works. Open the installed plugin to review
its components.

This installs the same package used by Claude Code; it does not require copying
adapter files into each project. Follow [Cowork's plugin installation guide](https://claude.com/docs/cowork/guide/plugins)
for repository imports and component controls. Installation in one client should
not be assumed to configure another client's local environment.
