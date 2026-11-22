# Plugin runtime

## Install the native runtime

The native bundle matching the plugin version is the default runtime. Plugin installation
does not include it: install it explicitly, either into the active plugin checkout or
marketplace cache, or once per version for the whole machine. Into the plugin, on Unix:

```sh
sh "/absolute/path/to/plugin/scripts/install_native.sh"
```

On Windows, pass the plugin's own version, the first line of its `VERSION` file:

```powershell
pwsh -NoProfile -File "C:/path/to/plugin/install.ps1" -Version VERSION -PluginRoot "C:/path/to/plugin"
```

Claude Code lists the plugin directory as `installPath` in `claude plugin list --json`;
`codex plugin add` prints it as the installed plugin root.
That installer places the exact target under `scripts/runtime/<target>` and can use
an offline archive plus SHA-256.

Hosts may make new copies of the plugin, such as a fresh copy per session or a cache per
version, and each copy starts without a runtime. One machine-wide install of the same
version serves every copy of it. Pass the plugin's version again:

```sh
sh "/absolute/path/to/plugin/install.sh" --version VERSION
```

```powershell
pwsh -NoProfile -File "C:/path/to/plugin/install.ps1" -Version VERSION
```

This installs into `~/.local/lib/kpopper/VERSION/<target>` and also points the `kpop` and
`kpopper` commands in `~/.local/bin` at that version.

Hooks never download, install or compile a runtime. They run the first of:

1. the copy's own `scripts/runtime/<target>/kpop`;
2. `~/.local/lib/kpopper/VERSION/<target>/bin/kpop`, where `VERSION` is the copy's own
   version, when the installer's `.kpopper-managed` marker beside `bin` holds that version.

Another version, an unmarked directory, a file that is not executable, an installation made
with `--prefix` and a `kpop` found on `PATH` are never used. On Windows the hooks run in a
POSIX shell and look under its `HOME`, so the machine-wide install is found when that is the
directory PowerShell calls `$HOME`, where `install.ps1` puts it. The opener's
`KPOPPER_AGENT_CONTEXT.command` points to the native executable that opened the session.

When the runtime is missing, the session opener prints the diagnostic and the install
commands on standard output: the one for that plugin copy and, when the copy names its
version, the one for every copy of that version. Claude Code and Codex add that output
to the agent's context at session start. The agent can then offer to run one and ask for a
new session, which opens with kpopper. The opener still exits 0. Every other hook stays
silent and leaves the same diagnostic on standard error, which neither host shows the model.

## Check the installation

In a disposable project, start a new host session. The opening should contain
first-use guidance (or the existing record) and `KPOPPER_AGENT_CONTEXT`, with no
diagnostic. Through that command, add a source and one finding, then start
another session in the same project and read the finding back. Merely seeing the
plugin in the host's installed list is not proof that its hooks ran. Codex also
requires trust of the installed hook definitions.

In Codex, a record normally produces `KPOPPER_CANONICAL_VIEW_ROUTE` and attention
items. A large opening can provide a route without inline bodies. Read it once
and verify that the queued marker is followed by a complete context frame, or
recover with the same command and `--view-transport stdout`. Automatic query
anchors are enabled on that managed route; delta is off unless explicitly opted
in. Existing opt-outs remain valid. See the [upgrade guide](context-upgrade.md).

The automated regression tests exercise a missing runtime, all Claude/Codex hook
command definitions, paths containing spaces, command writes and reopening through
subprocesses. These are hook/protocol checks, not evidence of an
authenticated conversation inside either host. Native Windows shell hooks, remote
hosts and Cowork's separate VM require their own validation. Their interpreter and
home directory must not be inferred from a successful local macOS/Linux setup.

Other adapters find the runtime the same way: the one installed in their own kpopper
copy, or else the machine-wide install of that copy's version. `KPOPPER_RUNTIME` accepts `rust` alone; any other value is refused with a message
on standard error and leaves the session untouched.
