"""Command-line transport and explicit local setup for checked session views."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys

HERE = Path(__file__).resolve().parent


def parser():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("operation", choices=["setup", "status", "open", "read", "search", "context", "propose", "serve", "enable", "disable", "hook-open"])
    p.add_argument("--input", type=Path, help="native record; defaults to the project's registered record")
    p.add_argument("--normalized", action="store_true", help="input is a normalized JSON snapshot")
    p.add_argument("--no-settings", action="store_true", help="use explicit arguments and record-local defaults without user settings")
    p.add_argument("--project", help="stable project name; defaults to the record directory name")
    p.add_argument("--state", type=Path, help="pending-proposal state directory")
    p.add_argument("--profile", type=Path, help="declared navigation profile JSON")
    p.add_argument("--assessment-profile", choices=["checked-reader/v1", "core/v1"],
                   default=None,
                   help="semantic reader profile; defaults to the selected record's declared profile")
    p.add_argument("--encoding", choices=["o200k_base", "cl100k_base"], default="o200k_base")
    p.add_argument("--tokens", type=int)
    p.add_argument("--ref")
    p.add_argument("--revision")
    p.add_argument("--offset", type=int)
    p.add_argument("--query", default="", help="search text; use the original question or a faithful project-aware query")
    p.add_argument("--id", dest="ids", action="append", default=[], help="exact known node ID; repeat for multiple candidates")
    p.add_argument("--limit", type=int, default=8, help="search candidate limit,1..32")
    p.add_argument("--cursor", help="returned search continuation; repeat the same query/IDs/branch/mode")
    p.add_argument("--direction", choices=["support","impact"], help="explicit context traversal direction")
    p.add_argument("--depth", type=int, default=1, help="context depth,0..4; zero reads only seeds")
    p.add_argument("--max-nodes", type=int, default=16, help="context candidate cap including seeds,1..64")
    p.add_argument("--branch", help="declared branch hint; only breaks ranking ties")
    p.add_argument("--search-mode", choices=["lexical","semantic","hybrid"], default="hybrid")
    p.add_argument("--embedding-dir", type=Path, help="optional local pinned E5 assets; search/serve only, never downloaded")
    p.add_argument("--kind", choices=["observed", "inferred", "assumed", "question"])
    p.add_argument("--text")
    p.add_argument("--basis", action="append", default=[])
    p.add_argument("--revisit", default="")
    p.add_argument("--lean-root", type=Path, help="Lean " + "4.33.1" + " toolchain root; setup only")
    p.add_argument("--rebuild", action="store_true", help="quarantine and rebuild the managed core cache")
    p.add_argument("--global", dest="global_scope", action="store_true", help="enable/disable for all projects on this machine")
    return p


def resolve(args):
    from .. import provenance
    from .settings import current
    config = {} if args.no_settings else current(args.input.expanduser().resolve().parent if args.input else None)
    path = (args.input or Path(provenance.default_paths()[0])).expanduser().resolve()
    if not args.input and not path.exists():
        try:
            top = subprocess.run(["git", "rev-parse", "--show-toplevel"], text=True,
                                 encoding="utf-8", capture_output=True)
        except OSError:
            top = None
        if top is not None and top.returncode == 0:
            for name in provenance.ENTRY_NAMES:
                if (Path(top.stdout.strip()) / name).is_file():
                    path = (Path(top.stdout.strip()) / name).resolve()
                    break
    project = args.project or config.get("project") or re.sub(r"[^A-Za-z0-9._-]", "-", path.parent.name).strip("-._")[:70] or "project"
    requested_state = args.state or config.get("state")
    if requested_state:
        state = Path(requested_state).expanduser()
    else:
        base = Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local" / "state"))) / "kpopper"
        identity = hashlib.sha256((project + "\0" + str(path)).encode()).hexdigest()
        state = base / identity
    profile = args.profile or (Path(config["profile"]) if config.get("profile") else None)
    if profile is None:
        candidate = Path(provenance.layout(path)["session"])
        if candidate.is_file():
            profile = candidate
    reader = None if args.normalized else Path(provenance.__file__).resolve()
    return project, path, state, reader, profile


def main(argv=None):
    # Checked-session output is a UTF-8 protocol even when redirected on Windows.
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", newline="\n")
    args = parser().parse_args(argv)
    try:
        if args.operation == "setup":
            from .core import setup
            print(json.dumps(setup(args.lean_root, args.rebuild), indent=2))
            return 0
        if args.operation in {"enable", "disable"}:
            from .settings import write
            if args.assessment_profile not in (None, "checked-reader/v1"):
                raise ValueError("core/v1 session routing is explicit per invocation; default activation is not enabled")
            if args.global_scope and (args.profile or args.project or args.state):
                raise ValueError("profile, project and state settings require project-scoped enablement")
            value = {"schema": 1, "enabled": args.operation == "enable"}
            if value["enabled"]:
                from .core import Core
                from .view import GroundingService
                Core()
                value.update(python=str(Path(sys.executable).absolute()), tokens=args.tokens if args.tokens is not None else 1000)
                if not 64 <= value["tokens"] <= 65536:
                    raise ValueError("tokens must be 64..65536")
                if args.profile:
                    from .view import apply_profile
                    profile = json.loads(args.profile.read_text(encoding="utf-8"))
                    if not isinstance(profile, dict) or not isinstance(profile.get("groups"), dict):
                        raise ValueError("profile must contain declared groups")
                    value["profile"] = str(args.profile.resolve())
                if args.project: value["project"] = args.project
                if args.state: value["state"] = str(args.state.resolve())
            print(json.dumps({"settings": str(write(value, args.global_scope)), **value}, indent=2))
            return 0
        if args.operation == "status":
            from .core import Core, cache_directory, source_hash
            result = {"source_sha256": source_hash(), "cache": str(cache_directory()), "ready": False}
            try:
                core = Core()
                result.update(ready=True, build=core.build)
            except ValueError as error:
                result["reason"] = str(error)
            print(json.dumps(result, indent=2))
            return 0 if result["ready"] else 1
        from .view import CoreGroundingService, GroundingService
        project, path, state, reader, profile = resolve(args)
        assessment_profile = args.assessment_profile
        if assessment_profile is None:
            from .. import provenance
            assessment_profile = ("core/v1" if not args.normalized and
                provenance.core_reader_selected([str(path)]) else "checked-reader/v1")
        service_type = CoreGroundingService if assessment_profile == "core/v1" else GroundingService
        service = service_type(project, path, state, reader, args.encoding, profile=profile,
                               embedding_dir=args.embedding_dir)
        if args.operation == "serve":
            from .mcp_server import make_server
            make_server(service).run(transport="stdio")
            return 0
        if args.operation == "hook-open":
            budget = args.tokens if args.tokens is not None else 1000
            command = [sys.executable, str(HERE.parent / "session_cli.py"), "read", "--no-settings",
                       "--input", str(service.input_path), "--project", service.project,
                       "--state", str(service.state_dir)]
            if args.normalized:
                command.append("--normalized")
            if assessment_profile != "checked-reader/v1":
                command += ["--assessment-profile", assessment_profile]
            if profile:
                command += ["--profile", str(Path(profile).resolve())]
            prefix = shlex.join(command)
            hint = "Read via MCP kpopper_read, or (POSIX shell): " + prefix + " --ref REF --revision REV_FROM_ABOVE\n"
            available = budget - len(service.encoder.encode(hint)) - 1
            for _ in range(3):
                result = service.opening(available) + hint
                overflow = len(service.encoder.encode(result)) - budget
                if overflow <= 0:
                    break
                available -= overflow
            else:
                raise ValueError("hook budget cannot carry the complete view and read route")
        elif args.operation == "open":
            result = service.opening(args.tokens if args.tokens is not None else 700)
        elif args.operation == "read":
            if args.ref is None or args.revision is None:
                raise ValueError("read requires --ref and --revision from open")
            result = service.reading(args.ref, args.revision, args.tokens if args.tokens is not None else 1600, args.offset)
        elif args.operation == "search":
            if args.revision is None:
                raise ValueError("search requires --revision from open")
            result=service.searching(args.query,args.revision,args.tokens if args.tokens is not None else 1000,
                                     args.ids,args.limit,args.branch,args.search_mode,args.cursor)
        elif args.operation == "context":
            if args.revision is None or args.direction is None:
                raise ValueError("context requires --revision and --direction support|impact")
            result=service.contextualizing(args.ids,args.revision,args.direction,
                                           args.tokens if args.tokens is not None else 2000,args.depth,args.max_nodes)
        else:
            if args.revision is None or args.kind is None or args.text is None:
                raise ValueError("propose requires --revision, --kind and --text")
            result = service.proposing(args.revision, args.kind, args.text, args.basis, args.revisit)
        print(result, end="" if result.endswith("\n") else "\n")
        return 0
    except ModuleNotFoundError as error:
        print(json.dumps({"error": "checked sessions need the optional dependencies; install kpopper[session] with Python 3.10+", "missing": error.name}), file=sys.stderr)
        return 2
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        print(json.dumps({"error": str(error)}), file=sys.stderr)
        return 2
