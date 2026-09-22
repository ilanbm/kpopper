"""Small pip entry point for the native executable in this exact installation."""
import os
from pathlib import Path
import platform
import sys


def target_name(system=None, machine=None):
    system = system or platform.system()
    machine = (machine or platform.machine()).lower()
    targets = {
        ("Darwin", "arm64"): "darwin-arm64",
        ("Darwin", "aarch64"): "darwin-arm64",
        ("Darwin", "x86_64"): "darwin-x86_64",
        ("Linux", "x86_64"): "linux-x86_64",
        ("Linux", "aarch64"): "linux-aarch64",
        ("Linux", "arm64"): "linux-aarch64",
        ("Windows", "amd64"): "windows-x86_64",
        ("Windows", "x86_64"): "windows-x86_64",
    }
    try:
        return targets[system, machine]
    except KeyError:
        raise ValueError("unsupported native platform: %s %s" % (system, machine)) from None


def executable(root=None, target=None):
    target = target or target_name()
    root = Path(root) if root is not None else Path(__file__).resolve().parent
    name = "kpop.exe" if target == "windows-x86_64" else "kpop"
    binary = root / "runtime" / target / name
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError("native runtime is not installed for %s in this package" % target)
    return binary


def main():
    try:
        binary = executable()
        os.execv(str(binary), [str(binary), *sys.argv[1:]])
    except (OSError, ValueError) as error:
        print("kpopper: %s. See docs/plugin-runtime.md." % error, file=sys.stderr)
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
