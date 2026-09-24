# Plugin runtime

## Install the native runtime

The native bundle matching the plugin version is the default runtime. Plugin installation
does not include it: install it explicitly into the active plugin checkout or marketplace
cache. On Unix:

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
an offline archive plus SHA-256. Hooks never download or compile a runtime. The
opener's `KPOPPER_AGENT_CONTEXT.command` points to the canonical native executable.
A globally installed CLI does not satisfy plugin hooks; each active cache needs its
own `scripts/runtime/TARGET` payload.

When the runtime is missing, the session opener prints the diagnostic and the exact
install command for that plugin copy on standard output, which Claude Code and Codex add
to the agent's context at session start. The agent can then offer to run it and ask for a
new session, which opens with kpopper. The opener still exits 0. Every other hook stays
silent and leaves the same diagnostic on standard error, which neither host shows the model.

## Check the installation

In a disposable project, start a new host session. The opening should contain
first-use guidance (or the existing record) and `KPOPPER_AGENT_CONTEXT`, with no
diagnostic. Through that command, add a source and one finding, then start
another session in the same project and read the finding back. Merely seeing the
plugin in the host's installed list is not proof that its hooks ran. Codex also
requires trust of the installed hook definitions.

The automated regression tests exercise a missing runtime, all Claude/Codex hook
command definitions, paths containing spaces, command writes and reopening through
subprocesses. These are hook/protocol checks, not evidence of an
authenticated conversation inside either host. Native Windows shell hooks, remote
hosts and Cowork's separate VM require their own validation. Their interpreter and
home directory must not be inferred from a successful local macOS/Linux setup.

Other adapters find the runtime the same way: the one installed in their own kpopper
copy. `KPOPPER_RUNTIME` accepts `rust` alone; any other value is refused with a message
on standard error and leaves the session untouched.
