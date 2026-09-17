# kpopper for Cowork

Cowork installs the repository's Claude plugin package; there are no duplicate
adapter files here. Anthropic documents plugins and hooks in Cowork, but accepting
the same package does not establish identical execution behavior to Claude Code.
**kpopper's Cowork runtime has not been verified end to end.**
[Cowork plugin documentation](https://claude.com/docs/cowork/guide/plugins)

## Install

In Cowork, open **Customize → Plugins → Add marketplace**, enter `ilanbm/kpopper`,
then install **kpopper** from that marketplace. The full repository URL,
`https://github.com/ilanbm/kpopper`, also works. Open the installed plugin and review
its skills and hooks; individual components can be enabled or disabled.
[Installation and component controls](https://claude.com/docs/cowork/guide/plugins)

Start a Cowork task with access to the project folder that holds the record.
Cowork runs work inside an isolated virtual machine, so host paths and dependencies
available to a local Claude Code session must not be assumed to exist there.
[Cowork execution environment](https://claude.com/docs/plugins/overview)

## First-use check

1. Confirm the installed skills appear and ask Cowork to open the project's
   existing record. If no hook opening appeared, invoke `/kpopper:ground` explicitly.
2. Confirm `python3` can run the installed plugin's `scripts/cli.py` and its required
   dependencies inside the task environment. Use the installed plugin path, not a
   path copied from a different client's cache.
3. In a disposable project, save one sourced fact and a judgment, reopen the task,
   then change the fact and run `check` to confirm the judgment is flagged.
4. Confirm the saved `GROUNDING.yaml` and its `.kpopper/` companions persist in the
   selected project folder and can be opened by another host.

The root package declares `SessionStart`, `Stop`, tool and prompt hooks, including
optional asynchronous workers. Still unverified in Cowork: their actual delivery,
`CLAUDE_PLUGIN_ROOT` resolution, Python dependencies, stop decisions, asynchronous
rewakes, resume/compaction behavior and the record page's presentation. Installation
alone does not verify any of these. If automatic hooks are unavailable, explicit
skill and CLI use can be checked separately.

The documentation was reviewed on 2026-09-16. No live Cowork task or user
configuration was changed during that review.
