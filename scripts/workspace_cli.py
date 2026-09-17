"""Public workspace operations: open context, map the work, and configure guidance."""
import argparse
import glob
import hashlib
import json
from pathlib import Path
import sys
import subprocess

try:
    from . import onboarding as O, workspace as W, session_start as S
except ImportError:
    import onboarding as O
    import workspace as W
    import session_start as S


def _parser(command, description):
    parser = argparse.ArgumentParser(prog="kpop " + command, description=description)
    parser.add_argument("--json", action="store_true", help="return structured JSON")
    return parser


def _emit(data, text, as_json, code=0):
    if as_json:
        print(json.dumps(data, ensure_ascii=False, indent=2))
    elif text:
        print(text.rstrip(), file=sys.stderr if code else sys.stdout)
    return code


def open_context(argv):
    parser = _parser("open", "Open the current knowledge context, including a workspace with no record yet.")
    parser.add_argument("files", nargs="*", help="explicit record files (uses the legacy reader)")
    parser.add_argument("--chars", type=int, help="character budget for an explicit legacy opening")
    parser.add_argument("--budget", type=int, help="item budget for an explicit legacy opening")
    args = parser.parse_args(argv)
    try:
        location = W.locate()
        explicit = bool(args.files) or args.chars is not None or args.budget is not None
        files = []
        for given in args.files:
            absolute = str(Path(given).expanduser().absolute())
            files.extend(sorted(glob.glob(absolute)) or [absolute])
        files = files or [location["record"]]
        if args.files:
            if any(Path(file).suffix.lower() not in (".yaml", ".yml") for file in files):
                raise ValueError("Record filenames must have a .yaml or .yml extension.")
            path = Path(files[0]).expanduser().absolute()
            location = {**location, "record": str(path),
                        "status": "found" if path.is_file() else "unavailable",
                        "reason": "The requested record is unavailable: " + str(path)}
        data = {k: location[k] for k in ("workspace", "record", "status")}
        try:
            try:
                from . import mapping
            except ImportError:
                import mapping
            job = mapping.read(location)
            if job:
                data["mapping"] = {k: job[k] for k in ("request", "mapping", "mode", "report", "check", "error") if k in job}
        except ValueError as error:
            data["warning"] = str(error)
        if location["status"] == "unavailable":
            return _emit({**data, "error": location["reason"]}, location["reason"], args.json, 1)
        if location["status"] == "missing":
            text = ("No knowledge record yet. Continue your work and keep useful findings as they arise, "
                    "or run `kpop map` for an initial map (`--deep` for a deeper investigation).")
            if data.get("mapping"):
                text += "\nMapping: " + data["mapping"]["mapping"]
            return _emit({**data, "message": text}, text, args.json)
        legacy = None
        if explicit:
            legacy = list(files)
            for flag in ("chars", "budget"):
                value = getattr(args, flag)
                if value is not None:
                    if value < 1:
                        raise ValueError("--" + flag + " must be positive")
                    legacy += ["--" + flag, str(value)]
        record_path = Path(files[0]) if args.json and len(files) == 1 else None
        before_hash = hashlib.sha256(record_path.read_bytes()).hexdigest() if record_path else None
        result, checked = S.read_view(location, legacy)
        if record_path:
            if hashlib.sha256(record_path.read_bytes()).hexdigest() != before_hash:
                raise ValueError("record changed while opening it; retry")
            data["record_sha256"] = before_hash
        data.update(view=result.stdout, checked=checked)
        if result.returncode:
            message = result.stderr.strip() or result.stdout.strip() or "The record could not be opened."
            return _emit({**data, "error": message}, message, args.json, result.returncode)
        text = result.stdout
        try:
            try:
                from .followups import summary
            except ImportError:
                from followups import summary
            followups = summary(location, counts_only=True)
            if followups:
                data["followups"] = followups
                text += "\n" + followups
        except (ImportError, OSError, ValueError, KeyError, TypeError) as error:
            data["followups_error"] = str(error)
            text += "\nFollowups unavailable: " + str(error)
        if data.get("mapping"):
            text += "\nMapping: " + data["mapping"]["mapping"]
        return _emit(data, text, args.json)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        return _emit({"error": str(error)}, str(error), args.json, 2)


def config(argv):
    parser = _parser("config", "Inspect the project mode or change local preferences.")
    parser.add_argument("--guidance", choices=("on", "off"), help="enable or disable conversational explanations")
    parser.add_argument('--mode', choices=('simple', 'advanced'), help='set the project-wide knowledge mode')
    parser.add_argument('--record', help='explicit record location; Simple Git uses an external shared record')
    parser.add_argument('--check', action='store_true', help='preview the mode transition without changing policy')
    parser.add_argument('--expected-generation', type=int, help='refuse a policy change if its generation has moved')
    parser.add_argument('--migration-receipt', help='revalidate a complete core migration before changing the record route')
    parser.add_argument('--rollback', action='store_true', help='restore the exact original closure named by a migration receipt')
    args = parser.parse_args(argv)
    try:
        path = O.state_dir() / "guidance.json"
        value = O._read(path) or {"enabled": True}
        if type(value.get("enabled")) is not bool:
            raise ValueError("Invalid guidance preference: " + str(path))
        if args.guidance is not None and not args.check:
            value = {"enabled": args.guidance == "on"}
            O._write(path, value)
        if __package__:
            from .project_modes import Project
        else:
            from project_modes import Project
        project = Project()
        report = None
        if args.check:
            report = project.preview_transition(args.mode, args.record, migration_receipt=args.migration_receipt,
                                                rollback=args.rollback)
            policy = project.config()
        elif args.mode is not None or args.record is not None:
            policy = project.configure(args.mode, args.record, args.expected_generation,
                                       migration_receipt=args.migration_receipt, rollback=args.rollback)
        else:
            policy = project.config()
        data = {"guidance": value["enabled"], 'project': policy,
                'record': str(project.record(policy))}
        text = ('Mode: ' + policy['mode'] + '\nRecord: ' + data['record'] +
                '\nGuidance: ' + ('on' if value['enabled'] else 'off'))
        if report is not None:
            data['transition'] = report
            text += '\nTransition: ' + ('; '.join(report['blockers']) if report['blockers'] else 'ready')
        return _emit(data, text, args.json, 2 if report and report['blockers'] else 0)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        return _emit({"error": str(error)}, str(error), args.json, 2)


def map_work(argv):
    parser = _parser("map", "Return a mapping task to the calling agent session. The agent accepts, executes, and reports completion.")
    parser.add_argument("--deep", action="store_true", help="investigate the current situation and its history more deeply")
    args = parser.parse_args(argv)
    try:
        try:
            from . import mapping
        except ImportError:
            import mapping
        result = mapping.request(W.locate(), args.deep)
        text = ("Mapping task " + result["request"] + " is " + result["status"] + " for the current agent session.\n"
                "Work is complete only after the agent returns a report.")
        return _emit(result, text, args.json)
    except (OSError, ValueError) as error:
        return _emit({"status": "unavailable", "error": str(error)}, str(error), args.json, 2)
