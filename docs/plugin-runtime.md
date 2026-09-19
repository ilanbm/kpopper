# Plugin Python runtime

Claude Code and Codex copy plugin files; that does not install Python dependencies.
Their hook commands start a small standard-library-only launcher with `python3`.
The launcher selects a private dependency environment if one exists, otherwise it
checks that bootstrap Python. Opening a record probes only `yaml`
before running the hook. Failures name the actual interpreter and the repair command;
they do not block the host session or claim the record was opened.

Normal setup also installs `tzdata` for followup scheduling on systems without an IANA
timezone database. It is not an HTML dependency and its absence does not block record
opening. Configured followups still require their named timezone to be available.

## Setup and diagnosis

Use an absolute path to a checkout or the active installed plugin, quoted for spaces:

```sh
python3 "/absolute/path/to/kpopper/scripts/plugin_runtime.py" setup
python3 "/absolute/path/to/kpopper/scripts/plugin_runtime.py" doctor
```

`setup` is an explicit network operation. It uses Python's `venv`, then that venv's
`python -m pip` to install the required packages. It never uses `--user` or
`--break-system-packages`, and never installs the plugin package into the venv.
Setup checks dependencies in isolation so a terminal's `PYTHONPATH` cannot make an
empty venv look ready. Ordinary hook checks use the same import environment as the hook.
Python must include working `venv`/`ensurepip` support. A failed venv or pip command
returns nonzero and prints its error; setup is not complete until it prints `Ready`.
After correcting network or Python/venv availability, rerun the same command.
Setup uses pip's `--isolated` mode, so user pip settings and `PIP_INDEX_URL` do not
configure it. This documented path requires access to PyPI; private-index setup is
not provided by this command.

The default location is `~/.local/share/kpopper/runtimes/<dependency-set-hash>`.
Claude and Codex use the same environment under the same OS account even when their
PATHs select different Python installations. A changed dependency set gets a new
directory. Plugin updates keep using code from the active plugin's own directory.
Old runtime directories are not automatically deleted.

For a disposable profile, set `KPOPPER_RUNTIME_HOME` to an **absolute** directory
before both setup and host launch. The diagnostic command preserves that setting.
Changing it only during setup prepares an environment the host will not find.
Virtualenvs are not portable: if the base Python is removed or upgraded incompatibly,
rerun setup with an available Python. A broken private environment takes precedence
over PATH and is reported rather than silently replaced by a different interpreter.

The printed `Hook Python` and the next session's `KPOPPER_AGENT_CONTEXT.command[0]`
must agree. The command's second element names the active plugin's `scripts/cli.py`.
Use both elements together when recording or reading from that session; an unrelated
`kpop` on PATH may be a different installed version. `setup` does not create a global
`kpop` command. For that, see the [standalone CLI guide](reference.md#try-it-from-the-command-line).

## Optional experimental HTML applications

Ordinary setup installs core dependencies only. To add the page and document applications
in the same private runtime, run:

```sh
python3 "/absolute/path/to/kpopper/scripts/plugin_runtime.py" setup --applications html
```

This explicitly adds `html5lib` and `tinycss2`; it does not change which runtime the hooks
select or activate applications on ordinary tasks. Hooks remain usable if an optional
application dependency is missing. Use the next session's `KPOPPER_AGENT_CONTEXT.command`
with `experimental hub` or `experimental annotated-doc`. See [applications](applications.md).

## Check the installation

In a disposable project, start a new host session. The opening should contain
first-use guidance (or the existing record) and `KPOPPER_AGENT_CONTEXT`, with no
dependency warning. Through that command, add a source and one finding, then start
another session in the same project and read the finding back. Merely seeing the
plugin in the host's installed list is not proof that its hooks ran. Codex also
requires trust of the installed hook definitions.

The automated regression tests exercise missing dependencies, runtime selection,
all Claude/Codex hook command definitions, paths containing spaces, CLI writes and
reopening through subprocesses. These are hook/protocol checks, not evidence of an
authenticated conversation inside either host. Native Windows shell hooks, remote
hosts and Cowork's separate VM require their own validation. Their interpreter and
home directory must not be inferred from a successful local macOS/Linux setup.

Other adapters that directly invoke Python retain their documented interpreter
selection. The shared shell opener and ingestion wrapper use this launcher, but that
does not imply that every hook of every adapter uses the private environment.
