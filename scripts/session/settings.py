"""Explicit user settings live outside the versioned record and never come from it."""
from __future__ import annotations
import json
import os
from pathlib import Path
import subprocess
import tempfile


def global_path():
    return Path(os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))) / "kpopper" / "session.json"


def project_path(directory=None):
    override = os.environ.get("KPOPPER_SESSION_CONFIG")
    if override:
        return Path(override).expanduser().absolute()
    directory = Path(directory or Path.cwd()).expanduser().resolve()
    try:
        result = subprocess.run(["git", "rev-parse", "--git-common-dir"], cwd=directory,
                                text=True, encoding="utf-8", capture_output=True)
    except OSError:
        result = None
    if result is not None and result.returncode == 0:
        return (directory / result.stdout.strip()).resolve() / "kpopper-session.json"
    import hashlib
    key = hashlib.sha256(str(directory).encode()).hexdigest()
    return global_path().parent / "projects" / (key + ".json")


def read(path):
    if not path.exists():
        return {}
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or value.get("schema") != 1 or type(value.get("enabled")) is not bool:
        raise ValueError("invalid checked-session settings: " + str(path))
    if value["enabled"]:
        if not isinstance(value.get("python"), str) or not Path(value["python"]).is_absolute():
            raise ValueError("session settings need an absolute Python executable")
        if type(value.get("tokens")) is not int or not 64 <= value["tokens"] <= 65536:
            raise ValueError("session token budget must be 64..65536")
    return value


def current(directory=None):
    if os.environ.get("KPOPPER_SESSION_DISABLE") == "1":
        return {"enabled": False}
    local = read(project_path(directory))
    return local if local else read(global_path())


def write(value, global_scope=False):
    path = global_path() if global_scope else project_path()
    if path.exists():
        read(path)  # Do not replace an unrelated or malformed file silently.
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".session-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, ensure_ascii=False, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    return path
