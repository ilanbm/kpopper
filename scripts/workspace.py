"""Locate a record without reading project content or requiring Git or PyYAML.

The entry file is GROUNDING.yaml; a record born under the earlier name, PROVENANCE.yaml, is
still found wherever it is, and never created again."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ENTRY = "GROUNDING.yaml"
LEGACY = "PROVENANCE.yaml"
NAMES = (ENTRY, LEGACY)


def _git(directory, flag):
    try:
        result = subprocess.run(["git", "rev-parse", flag], cwd=directory, text=True,
                                encoding="utf-8", capture_output=True, timeout=5)
        if result.returncode == 0 and result.stdout.strip():
            return (directory / result.stdout.strip()).resolve()
    except (OSError, subprocess.SubprocessError):
        pass
    return None


def locate(directory=None):
    if directory is not None and (not isinstance(directory, (str, os.PathLike)) or not str(directory).strip()):
        raise ValueError("workspace must be a nonempty path")
    directory = Path(directory or Path.cwd()).expanduser().resolve()
    if not directory.is_dir():
        raise ValueError("workspace is not an accessible directory: " + str(directory))
    git_root = _git(directory, "--show-toplevel")
    common = _git(directory, "--git-common-dir") if git_root else None
    root, record, status, reason = git_root or directory, None, "missing", ""

    # Walk only ancestors, never sibling projects. A repository is a boundary;
    # outside Git, the home directory and filesystem root are boundaries too.
    current = directory
    while True:
        candidate = next((c for c in (current / n for n in NAMES) if os.path.lexists(c)), None)
        if candidate is not None:
            root, record = current, candidate
            status = "found" if candidate.is_file() else "unavailable"
            if status == "unavailable":
                reason = "The record path exists but is not an accessible file."
            break
        parent = current.parent
        if current in (git_root, Path.home().resolve()) or parent == current \
                or parent == Path.home().resolve() or parent == Path(parent.anchor):
            break
        current = parent

    if record is None and common:
        pointer = common / "kpopper-record"
        if os.path.lexists(pointer):
            try:
                text = pointer.read_text(encoding="utf-8").splitlines()
                value = text[0].strip() if text else ""
                if not value:
                    raise ValueError("the registered path is empty")
                record = Path(value).expanduser()
                if not record.is_absolute():
                    record = git_root / record
                record = record.absolute()
                status = "found" if record.is_file() else "unavailable"
                reason = "" if status == "found" else "The registered record is unavailable. Restore its location before creating another."
            except (OSError, UnicodeError, ValueError) as error:
                status, reason = "unavailable", "Cannot read the registered record location: " + str(error)

    try:
        try:
            from .project_modes import Project
        except ImportError:
            from project_modes import Project
        project = Project(directory)
        if project.config_path.exists():
            record = project.record()
            status = 'found' if record.is_file() else ('unavailable' if record.exists() else 'missing')
            reason = '' if status != 'unavailable' else 'The configured record is unavailable.'
        if status == 'missing' and os.environ.get('KPOPPER_READ_MODE') != 'frozen' and project.git:
            try:
                from .knowledge_views import has_pending
            except ImportError:
                from knowledge_views import has_pending
            if has_pending([str(record or project.record())]):
                record, status = project.record(), 'pending'
    except ImportError:
        # File-path discovery remains usable without the optional parser dependency.
        pass

    # Share choices across Git worktrees, while preserving deliberate subprojects.
    if common:
        identity = str(common) + "\0" + str(root.relative_to(git_root))
    else:
        identity = str(root)
    return {"workspace": str(root), "record": str(record or root / ENTRY),
            "status": status, "reason": reason,
            "key": hashlib.sha256(identity.encode("utf-8")).hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace")
    parser.add_argument("--hook", action="store_true", help="read cwd from the host's JSON payload")
    parser.add_argument("--path", action="store_true", help="print a found record path only")
    args = parser.parse_args()
    try:
        directory = args.workspace
        if args.hook:
            payload = json.load(sys.stdin)
            if not isinstance(payload, dict):
                raise ValueError("hook payload must be an object")
            if payload.get("cwd") is not None:
                directory = payload["cwd"]
        result = locate(directory)
        if args.path:
            if result["status"] != "found":
                return 1
            print(result["record"])
        else:
            print(json.dumps(result, ensure_ascii=False))
        return 0
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
