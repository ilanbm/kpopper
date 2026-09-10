# kpopper in ChatGPT Work

**Status: the platform documents a compatible workspace import route; this repository does
not yet have a verified end-to-end Work installation.**

The method fits ongoing project work: linked tasks, recorded commitments, dates, sources
and decisions that later sessions need to understand. Installation and access to those
materials depend on the Work environment and workspace settings.

## Import into a workspace

A workspace administrator can:

1. Open **Admin → Plugins → Add → Import marketplace**.
2. Set **Source** to `https://github.com/ilanbm/kpopper`.
3. Leave **Path** empty: the marketplace is at the repository root.
4. Import the marketplace and make **kpopper** available to the intended workspace members.

Members can then install the available plugin from **Plugins** and start a new Work
conversation. This repository supplies `.claude-plugin/marketplace.json`, one of the
formats supported by [OpenAI's workspace import](https://learn.chatgpt.com/docs/enterprise/plugin-management).
Availability still depends on the workspace's roles and policies; this is not a universal
terminal command for personal Work accounts.

## Make the runtime available

kpopper's implementation uses local files and Python scripts. The Work execution
environment needs the package's scripts, Python 3.9+ with PyYAML, and a writable project
record in a location that later sessions can also access.

OpenAI documents hooks in the runtime used by Work and Codex, but also states that installing
a plugin on the web does not deploy its local hook scripts. The scripts must be available
where execution occurs, and hook trust must be established there.
[Plugin capabilities and runtime requirements](https://learn.chatgpt.com/docs/plugins).

The repository ships Codex and Claude hook configurations. Their successful installation
in a local CLI does not establish that a particular Work environment runs the same hooks,
mounts the same files, or supports the same background delivery tools. There is no hosted
kpopper service or Work-specific adapter that supplies those missing pieces automatically.

Before treating an installation as operational, confirm in that Work environment that the
agent can locate the intended record, run its checks and open it again in a later session.
Validate automatic opening, stop behavior and background delivery separately if used.

## Sources and continuity

An imported plugin does not grant access to calendars, documents or task systems. Those
sources depend on the tools and permissions available to the conversation. Record dated
readings and source locations for the claims actually used in the project; a deadline in
the record does not create a calendar reminder or synchronize a task system.

The [followups capability](../skills/kpopper/FOLLOWUPS.md) can link deferred work to those
readings and produce a daily-review prompt. Actual task-system access and persistent
scheduling still come from the current host's tools after user opt-in. Confirm that the
scheduled environment can reach the same record, private followups ledger and canonical
tasks; a laptop-local setup is not automatically available in a cloud Work environment.

Keep the canonical record in a location shared by the sessions doing the work. A file left
only in one temporary session is not persistent project memory. Use existing project
storage and access arrangements rather than assuming a local Codex path exists in Work.

The [README](../README.md#get-started) lists the other installation routes. The
[record reference](reference.md) covers pointers and stored reasoning, and
[background capture](../skills/kpopper/INGESTION.md) documents the available delivery paths.
