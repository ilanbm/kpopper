#!/usr/bin/env python3
"""Exercise installed public commands and the copied plugin with native defaults."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    repository = Path(__file__).resolve().parents[2]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="kpopper-installed-"))
    prefix = root / "command prefix"
    project = root / "project with spaces"
    project.mkdir()
    private_tmp = root / "private-tmp"
    private_tmp.mkdir()
    environment = dict(os.environ, KPOPPER_PRIVATE_HOME=str(root / "private"),
                       XDG_STATE_HOME=str(root / "state"), TMPDIR=str(private_tmp),
                       KPOPPER_NATIVE_CACHE=str(root / "native-cache"))
    for key in ("KPOPPER_RUNTIME", "KPOPPER_NATIVE_RESOURCES", "KPOPPER_READ_MODE", "KPOPPER_AGENT_SESSION"):
        environment.pop(key, None)
    records = []

    def run(name, command, data="", env=None, expected=(0,)):
        started = time.monotonic()
        result = subprocess.run([str(value) for value in command], cwd=project,
                                input=data, text=True, encoding="utf-8", capture_output=True,
                                timeout=180, env=env or environment)
        (output / (name + ".stdout")).write_text(result.stdout, encoding="utf-8")
        (output / (name + ".stderr")).write_text(result.stderr, encoding="utf-8")
        records.append({"name": name, "command": [str(x) for x in command],
                        "exit_code": result.returncode, "seconds": time.monotonic() - started})
        if result.returncode not in expected:
            raise RuntimeError("%s failed: %s %s" % (name, result.stdout, result.stderr))
        return result

    windows = os.name == "nt"
    extension = ".exe" if windows else ""
    if windows:
        shell = shutil.which("pwsh") or shutil.which("powershell")
        if not shell:
            raise RuntimeError("PowerShell is required for Windows installer acceptance")
        installer = [shell, "-NoProfile", "-File", repository / "install.ps1",
                     "-Version", args.version, "-Archive", args.archive.resolve(),
                     "-Sha256", args.sha256]
        run("01-install-cli", [*installer, "-Prefix", prefix])
        bash = Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe"
        if not bash.is_file():
            raise RuntimeError("Git Bash is required for the Windows shell-hook acceptance")
        restricted = [bash.parent, bash.parent.parent / "usr/bin",
                      Path(shutil.which("git")).parent,
                      Path(os.environ["SystemRoot"]) / "System32"]
    else:
        installer = ["sh", repository / "install.sh", "--version", args.version,
                     "--archive", args.archive.resolve(), "--sha256", args.sha256]
        run("01-install-cli", [*installer, "--prefix", prefix])
        tools = root / "host-tools"
        tools.mkdir()
        for name in ("sh", "uname", "dirname", "cat", "git"):
            source = shutil.which(name)
            if not source:
                raise RuntimeError("required host utility is missing: " + name)
            (tools / name).symlink_to(source)
        restricted = [tools]
        bash = tools / "sh"
    native_environment = dict(environment, PATH=os.pathsep.join(str(path) for path in restricted))
    for name in ("python", "python3", "node", "cargo", "rustc"):
        if shutil.which(name, path=native_environment["PATH"]):
            raise RuntimeError("native acceptance PATH unexpectedly exposes " + name)

    binary = prefix / "bin" / ("kpop" + extension)
    alias = prefix / "bin" / ("kpopper" + extension)
    for name, executable in (("02-kpop-version", binary), ("03-kpopper-version", alias)):
        result = run(name, [executable, "--version"], env=native_environment)
        if result.stdout.strip() != "kpop " + args.version:
            raise RuntimeError("wrong installed version: " + result.stdout)
    run("04-source", [binary, "add", "s.smoke", "name=Installed source", "read=2026-09-01"], env=native_environment)
    run("05-reading", [alias, "add", "p.seats", "v=17", "from=s.smoke", "name=Installed seats", "--as-of", "2026-09-01"], env=native_environment)
    entry = project / "GROUNDING.yaml"
    update = {"event_id": "installed-smoke", "date": "2026-09-02",
              "source_quote": "There are 19 installed seats.", "source": "s.smoke",
              "at": "installation acceptance", "record_sha256": hashlib.sha256(entry.read_bytes()).hexdigest(),
              "updates": [{"kind": "set", "id": "p.seats", "value": 19}]}
    result = run("06-update", [binary, "update", "--file", "-"], json.dumps(update), native_environment)
    if json.loads(result.stdout)["state"] != "applied":
        raise RuntimeError("installed update was not applied")
    run("06-rule", [binary, "add", "r.double_seats", "rule=p.seats * 2", "name=Twice the seats"], env=native_environment)
    calculation = run("06-calculation", [binary, "pull", "r.double_seats"], env=native_environment)
    if "38" not in calculation.stdout:
        raise RuntimeError("installed reasoning resources did not calculate the rule")
    before = entry.read_bytes()
    result = run("07-read", [alias, "pull", "p.seats"], env=native_environment)
    if "19" not in result.stdout:
        raise RuntimeError("installed reading is absent")
    run("08-check", [binary, "check"], env=native_environment)
    if entry.read_bytes() != before:
        raise RuntimeError("installed read/check changed source bytes")

    plugin = root / "plugin with spaces"
    plugin.mkdir()
    for name in ("scripts", "bin", "hooks", "skills", "adapters", ".codex-plugin", ".claude-plugin"):
        shutil.copytree(repository / name, plugin / name,
                        ignore=shutil.ignore_patterns("__pycache__", "*.pyc", "runtime"))
    for name in ("VERSION", "install.sh", "install.ps1"):
        shutil.copy2(repository / name, plugin / name)
    run("09-install-plugin", [*installer, "-PluginRoot" if windows else "--plugin-root", plugin])
    plugin_environment = dict(native_environment, PLUGIN_ROOT=plugin.as_posix(),
                              CLAUDE_PLUGIN_ROOT=plugin.as_posix())
    configuration = json.loads((plugin / "adapters/codex/plugin-hooks.json").read_text())
    commands = configuration["hooks"]
    opening = commands["SessionStart"][0]["hooks"][0]["command"]
    payload = {"session_id": "installed-native", "cwd": str(project), "source": "startup",
               "hook_event_name": "SessionStart"}
    result = run("10-plugin-open", [bash, "-c", opening], json.dumps(payload), plugin_environment)
    line = next((line for line in result.stdout.splitlines() if line.startswith("KPOPPER_AGENT_CONTEXT ")), None)
    if line is None:
        raise RuntimeError("installed plugin did not emit its native command")
    context = json.loads(line.split(" ", 1)[1])
    expected = plugin / "scripts/runtime" / args.target / ("kpop" + extension)
    if not os.path.samefile(context["command"][0], expected):
        raise RuntimeError("plugin command escaped its active native package")
    plugin_environment.update(context["environment"])
    result = run("11-context-read", [*context["command"], "pull", "p.seats"], env=plugin_environment)
    if "19" not in result.stdout:
        raise RuntimeError("native context command did not read the installed record")
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
        {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "kpopper_open", "arguments": {"tokens": 8000}}},
    ]
    response = run("12-plugin-mcp", [*context["command"], "session", "serve"],
                   "".join(json.dumps(item) + "\n" for item in requests), plugin_environment)
    messages = [json.loads(line) for line in response.stdout.splitlines()]
    if [message.get("id") for message in messages] != [1, 2, 3] or any("error" in message for message in messages):
        raise RuntimeError("installed MCP handshake/tool call failed")
    if messages[2]["result"].get("isError") or "p.seats" not in json.dumps(messages[2]["result"]):
        raise RuntimeError("installed MCP open did not read the record")
    index = 13
    for event, groups in commands.items():
        for group in groups:
            for hook in group["hooks"]:
                sample = dict(payload, hook_event_name=event, agent_id="smoke-child",
                              prompt="installed seats", tool_name="Read", tool_input={})
                run("%02d-hook-%s" % (index, event), [bash, "-c", hook["command"]],
                    json.dumps(sample), plugin_environment)
                index += 1
    if entry.read_bytes() != before:
        raise RuntimeError("installed hooks changed canonical source bytes")
    (output / "result.json").write_text(json.dumps({
        "version": args.version, "target": args.target, "archive_sha256": args.sha256,
        "prefix": str(prefix), "plugin": str(plugin), "context": context,
        "path": native_environment["PATH"], "commands": records, "status": "passed",
    }, indent=2) + "\n")


if __name__ == "__main__":
    main()
