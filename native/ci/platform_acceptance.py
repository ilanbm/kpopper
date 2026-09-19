#!/usr/bin/env python3
"""Exercise a copied native executable through public cross-platform flows."""

import argparse
import hashlib
import json
import os
import platform
import subprocess
import tempfile
from pathlib import Path


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def snapshot(root):
    return {
        path.relative_to(root).as_posix(): digest(path)
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    workspace = Path(tempfile.mkdtemp(prefix="kpop-native-platform-workspace-"))
    cache = Path(tempfile.mkdtemp(prefix="kpop-native-platform-cache-"))
    env = dict(os.environ, KPOPPER_NATIVE_CACHE=str(cache))
    commands = []

    def run(name, argv, expect=True):
        result = subprocess.run(
            [str(binary), "--workspace", str(workspace), *argv],
            env=env,
            text=True,
            encoding="utf-8",
            capture_output=True,
        )
        (output / f"{name}.stdout").write_text(result.stdout, encoding="utf-8")
        (output / f"{name}.stderr").write_text(result.stderr, encoding="utf-8")
        commands.append({"name": name, "argv": argv, "exit_code": result.returncode})
        if expect and result.returncode != 0:
            raise RuntimeError(f"{name} failed: {result.stderr}")
        return result

    if any(workspace.iterdir()):
        raise RuntimeError("acceptance workspace was not initially empty")
    run("01-add", ["add", "p.hours", "v=10"])
    run("02-set", ["set", "p.hours", "12", "--why", "platform acceptance"])
    before_reads = snapshot(workspace)
    run("03-open", ["open"])
    run("04-check", ["check"])
    run("05-history-status", ["history", "status"])
    if snapshot(workspace) != before_reads:
        raise RuntimeError("read-only commands changed the record workspace")

    contender_commands = [
        ["add", "p.concurrent_a", "v=13"],
        ["add", "p.concurrent_b", "v=14"],
    ]
    contenders = []
    for command in contender_commands:
        contenders.append(
            subprocess.Popen(
                [
                    str(binary),
                    "--workspace",
                    str(workspace),
                    *command,
                ],
                env=env,
                text=True,
                encoding="utf-8",
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        )
    contender_results = []
    for index, process in enumerate(contenders, 1):
        stdout, stderr = process.communicate(timeout=60)
        (output / f"06-concurrent-{index}.stdout").write_text(stdout, encoding="utf-8")
        (output / f"06-concurrent-{index}.stderr").write_text(stderr, encoding="utf-8")
        contender_results.append(process.returncode)
        commands.append(
            {"name": f"06-concurrent-{index}", "exit_code": process.returncode}
        )
    if not any(code == 0 for code in contender_results):
        raise RuntimeError(
            f"both concurrent writers were refused: {contender_results}"
        )
    for index, (code, command) in enumerate(
        zip(contender_results, contender_commands), 1
    ):
        if code != 0:
            run(f"06-concurrent-{index}-retry", command)
    run("07-history-status-after-contention", ["history", "status"])
    recovery = run("08-recover-clean", ["recover", "--json"], expect=False)
    if recovery.returncode == 0:
        raise RuntimeError("recover unexpectedly reported work without a retained journal")

    files = {
        path.relative_to(binary.parent).as_posix(): digest(path)
        for path in sorted(binary.parent.rglob("*"))
        if path.is_file()
    }
    manifest = {
        "commit": os.environ.get("GITHUB_SHA"),
        "runner_os": os.environ.get("RUNNER_OS"),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "python": platform.python_version(),
        "rustc": os.environ.get("KPOPPER_CI_RUSTC"),
        "binary": str(binary),
        "files_sha256": files,
        "workspace_sha256": snapshot(workspace),
        "commands": commands,
    }
    (output / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()
