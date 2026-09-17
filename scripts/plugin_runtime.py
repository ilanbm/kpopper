"""Explicit dependency setup and a stdlib-only launcher for plugin hooks.

The private venv holds dependencies, never a second copy of the plugin. Hooks only
select and check it; only the user's `setup` command invokes venv and pip.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys

HERE = Path(__file__).resolve().parent
# Keep aligned with project.dependencies; the regression test checks the contract.
REQUIREMENTS = ("PyYAML>=5.1", "html5lib>=1.1,<2", "tinycss2>=1.2,<2", "tzdata")
MODULES = ("yaml", "html5lib", "tinycss2", "tzdata")
PROBE = """
import importlib, json, sys
errors = []
if sys.version_info < (3, 9):
    errors.append('Python 3.9+ is required')
for name in %r:
    try:
        importlib.import_module(name)
    except Exception as error:
        errors.append(name + ': ' + str(error))
print(json.dumps({'python': sys.executable, 'prefix': sys.prefix,
                  'base_prefix': sys.base_prefix, 'errors': errors}))
""" % (MODULES,)


def runtime_dir():
    # Independent of the host cache and bootstrap Python: Claude and Codex select
    # the same repaired runtime even when they inherit different PATHs.
    root = os.environ.get("KPOPPER_RUNTIME_HOME")
    if root:
        root = Path(root).expanduser()
        if not root.is_absolute():
            raise ValueError("KPOPPER_RUNTIME_HOME must be an absolute path")
    else:
        root = Path.home() / ".local" / "share" / "kpopper" / "runtimes"
    key = hashlib.sha256("\n".join(REQUIREMENTS).encode()).hexdigest()[:16]
    return root / key


def venv_python(directory):
    return directory / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def probe(python, isolated=False):
    try:
        # Match a hook script's import environment, including existing user-site
        # installs. For -c, cwd=HERE stands in for the script's sys.path[0].
        command = [str(python), *(["-I"] if isolated else []), "-c", PROBE]
        result = subprocess.run(command, cwd=HERE, capture_output=True,
                                text=True, encoding="utf-8", timeout=10)
        if result.returncode:
            raise ValueError(result.stderr.strip() or "probe exited %s" % result.returncode)
        return json.loads(result.stdout)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        return {"python": str(python), "errors": [str(error)]}


def select():
    directory = runtime_dir()
    # A partial/broken setup must be diagnosed, not hidden by a different PATH.
    python = venv_python(directory) if directory.exists() else Path(sys.executable)
    return probe(python)


def setup_command():
    command = [sys.executable, str(HERE / "plugin_runtime.py"), "setup"]
    prefix = ""
    if os.environ.get("KPOPPER_RUNTIME_HOME"):
        prefix = "KPOPPER_RUNTIME_HOME=" + shlex.quote(os.environ["KPOPPER_RUNTIME_HOME"]) + " "
    return prefix + shlex.join(command)


def diagnostic(status):
    return ("kpopper Python dependencies unavailable; the record has not been opened.\n"
            "Bootstrap Python: %s\nSelected Python: %s\n%s\n"
            "Run in a terminal (creates a private virtualenv; installs dependencies from PyPI):\n%s\n"
            "Then start a new session. Hooks never install packages."
            % (sys.executable, status["python"], "; ".join(status["errors"]), setup_command()))


def setup():
    directory = runtime_dir()
    directory.parent.mkdir(parents=True, exist_ok=True)
    lock = directory.with_name(directory.name + ".setup-lock")
    try:
        lock.mkdir()
    except FileExistsError:
        print("Another setup may be running. Retry after it finishes. If it crashed, remove "
              "this empty lock directory: " + str(lock), file=sys.stderr)
        return 1
    try:
        python = venv_python(directory)
        if directory.exists() and not (directory / "pyvenv.cfg").is_file():
            raise ValueError("Refusing to modify a directory that is not a virtualenv: " + str(directory))
        print("Creating/checking private Python environment: " + str(directory), flush=True)
        subprocess.run([sys.executable, "-I", "-m", "venv", str(directory)], check=True)
        # Setup must install dependencies into the venv itself, even if the
        # terminal supplies them through PYTHONPATH that the host will not inherit.
        status = probe(python, isolated=True)
        # Never hand pip a system interpreter, even if a partial venv is damaged.
        if (Path(status.get("prefix", "")).resolve() != directory.resolve()
                or status.get("prefix") == status.get("base_prefix")):
            raise ValueError("The private Python could not be verified as a virtualenv: " + str(python))
        if status["errors"]:
            print("Installing plugin dependencies into " + str(python), flush=True)
            subprocess.run([str(python), "-I", "-m", "pip", "--isolated", "install", *REQUIREMENTS], check=True)
        status = probe(python, isolated=True)
        if status["errors"]:
            print(diagnostic(status), file=sys.stderr)
            return 1
        print("Ready. Hook Python: " + status["python"])
        print("Plugin code: " + str(HERE))
        print("Start a new host session; KPOPPER_AGENT_CONTEXT.command will name this Python.")
        return 0
    finally:
        lock.rmdir()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("setup", "doctor", "python", "hook"))
    parser.add_argument("args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    try:
        if args.action == "setup":
            return setup()
        status = select()
        if status["errors"]:
            # Startup failures are context the host can show, not a stack trace or
            # a false ready command. Continuing hooks remain non-blocking.
            startup = args.action == "hook" and args.args[:1] == ["session_start.py"]
            print(diagnostic(status), file=sys.stdout if startup else sys.stderr)
            return 0 if args.action == "hook" else 1
        if args.action == "doctor":
            print("Bootstrap Python: " + sys.executable)
            print("Hook Python: " + status["python"])
            print("Dependencies ready: " + ", ".join(MODULES))
            print("Plugin code: " + str(HERE))
            return 0
        if args.action == "python":
            print(status["python"])
            return 0
        if not args.args or Path(args.args[0]).name != args.args[0] or not (HERE / args.args[0]).is_file():
            print("kpopper runtime: hook requires an existing script filename from this plugin",
                  file=sys.stderr)
            return 0  # launcher errors must not become a host's block/rewake signal
        # exec preserves stdin, host exit codes (including Stop/rewake 2), and
        # signal handling. The absolute script is always from the active plugin.
        os.execv(status["python"], [status["python"], str(HERE / args.args[0]), *args.args[1:]])
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print("kpopper runtime: " + str(error), file=sys.stderr)
        return 0 if args.action == "hook" else 1


if __name__ == "__main__":
    raise SystemExit(main())
