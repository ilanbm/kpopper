"""Capture deferred work, evaluate its triggers, and coordinate daily review."""
import argparse
import json
import sys

import yaml

try:
    from . import followups as F, followup_daily as D, followup_install as Install
except ImportError:
    import followups as F
    import followup_daily as D
    import followup_install as Install


def main(argv=None):
    parser = argparse.ArgumentParser(prog="kpopper followups", description=__doc__)
    parser.add_argument("--json", action="store_true", help="structured output (also the default)")
    commands = parser.add_subparsers(dest="command", required=True)
    setup = commands.add_parser("setup", help="choose an existing task destination or create a private fallback")
    setup.add_argument("--store")
    setup.add_argument("--timezone", default="UTC")
    setup.add_argument("--record")
    setup.add_argument("--private", action="store_true", help="deliberately select the private fallback")
    commands.add_parser("status")
    commands.add_parser("list")
    relocate = commands.add_parser("relocate", help="explicitly rebind a moved pinned record, preserving history")
    relocate.add_argument("--record", required=True)
    relocate.add_argument("--evidence", required=True)
    restore = commands.add_parser("restore", help="restore an inspected backup and park uncertain unfinished work")
    restore.add_argument("--backup", required=True)
    restore.add_argument("--evidence", required=True)
    resolve = commands.add_parser("resolve", help="record evidenced completion or cancellation outside this runner")
    resolve.add_argument("id")
    resolve.add_argument("--outcome", required=True, choices=("done", "cancelled"))
    resolve.add_argument("--evidence", required=True)
    scan = commands.add_parser("scan", help="read-only readiness check; never executes a task")
    scan.add_argument("--limit", type=int, default=20)
    show = commands.add_parser("show")
    show.add_argument("id")
    for name in ("add", "observe"):
        command = commands.add_parser(name)
        command.add_argument("--file", required=True, help="YAML or JSON input; - reads stdin")
    refresh = commands.add_parser("refresh")
    refresh.add_argument("id")
    refresh.add_argument("--file", required=True)
    refresh.add_argument("--evidence", required=True)
    claim = commands.add_parser("claim")
    claim.add_argument("id")
    claim.add_argument("--occurrence", required=True)
    claim.add_argument("--owner", required=True)
    claim.add_argument("--daily-token")
    for name in ("renew", "release", "finish"):
        command = commands.add_parser(name)
        command.add_argument("id")
        command.add_argument("--token", required=True)
        if name != "renew":
            command.add_argument("--evidence", required=True)
        if name == "finish":
            command.add_argument("--outcome", choices=("checked", "done", "cancelled", "needs_user"), required=True)
            command.add_argument("--next-at")
    recover = commands.add_parser("recover")
    recover.add_argument("id")
    recover.add_argument("--evidence", required=True)
    resume = commands.add_parser("resume", help="resume reconciled parked work while retaining its schedule")
    resume.add_argument("id")
    resume.add_argument("--evidence", required=True)
    daily = commands.add_parser("daily", help="recommend, bind and operate one daily review per workspace")
    daily_commands = daily.add_subparsers(dest="operation", required=True)
    plan = daily_commands.add_parser("plan")
    plan.add_argument("--time", default="09:00")
    install = daily_commands.add_parser("install", help="check and install daily review through the calling host agent")
    install.add_argument("--owner")
    install.add_argument("--time")
    install.add_argument("--timezone")
    install.add_argument("--store")
    install.add_argument("--private", action="store_true")
    install.add_argument("--check", action="store_true", help="inspect only; no installation reservation")
    install.add_argument("--resume", action="store_true", help="explicitly permit enabling a paused review")
    install.add_argument("--token")
    install.add_argument("--unchanged", action="store_true", help="with --fail: the host explicitly rejected before making any change")
    receipt = install.add_mutually_exclusive_group()
    receipt.add_argument("--inspect", metavar="FILE", help="submit an actual host inventory")
    receipt.add_argument("--result", metavar="FILE", help="submit an independent host readback")
    receipt.add_argument("--reconcile", metavar="FILE", help="retire an inspected host attempt after its pending effects are resolved")
    receipt.add_argument("--fail", metavar="REASON")
    daily_commands.add_parser("status")
    bind = daily_commands.add_parser("bind", help="record the actual host schedule after creation or inspection")
    bind.add_argument("--file", required=True)
    start = daily_commands.add_parser("start")
    start.add_argument("--owner", required=True)
    for name in ("finish", "renew", "recover"):
        operation = daily_commands.add_parser(name)
        if name != "recover":
            operation.add_argument("--token", required=True)
        if name != "renew":
            operation.add_argument("--evidence", required=True)
    args = parser.parse_args(argv)
    try:
        store = F.Store()
        if args.command == "daily" and args.operation == "install":
            if args.unchanged and not args.fail:
                raise F.Refused("--unchanged is valid only with --fail and a definite no-change host result")
            if args.inspect or args.result or args.reconcile or args.fail:
                if not args.token or args.check:
                    raise F.Refused("Submitting an installation receipt needs its token and cannot be check-only")
                path = args.inspect or args.result or args.reconcile
                report = (yaml.load(sys.stdin.read(F.MAX_BYTES + 1), Loader=F.StrictLoader) if path == "-" else F.read_yaml(path)) if path else None
                result = Install.inspect_host(store, args.token, report) if args.inspect else \
                    Install.finish(store, args.token, report) if args.result else \
                    Install.reconcile(store, args.token, report) if args.reconcile else Install.fail(store, args.token, args.fail, args.unchanged)
            else:
                result = Install.begin(store, args.owner, args.time, args.timezone, args.store, args.private, args.check, args.resume)
            print(json.dumps(F.T.normalize(result), ensure_ascii=False, indent=2))
            return 0
        supplied = None
        if hasattr(args, "file"):
            supplied = yaml.load(sys.stdin.read(F.MAX_BYTES + 1), Loader=F.StrictLoader) if args.file == "-" else F.read_yaml(args.file)
        if args.command == "setup":
            result = store.setup(args.store, args.timezone, args.record, args.private)
        elif args.command == "relocate":
            result = store.relocate(args.record, args.evidence)
        elif args.command == "restore":
            result = store.restore(args.backup, args.evidence)
        elif args.command == "resolve":
            result = store.resolve(args.id, args.outcome, args.evidence)
        elif args.command == "status":
            data = store.load(required=False)
            result = {"configured": bool(data), "ledger": str(store.path), "suggested_store": store.suggested_store() if not data else None}
            if data:
                result.update(config=data["config"], daily=D.status(store))
        elif args.command == "scan":
            result = store.scan(args.limit)
        elif args.command == "list":
            result = {"items": [{"id": item["id"], "task": item["task"], "state": item["state"]}
                                for item in store.load()["items"].values()]}
        elif args.command == "show":
            result = store._item(store.load(), args.id)
        elif args.command == "add":
            result = store.add(supplied)
        elif args.command == "observe":
            result = store.observe(supplied)
        elif args.command == "refresh":
            result = store.refresh(args.id, supplied, args.evidence)
        elif args.command == "claim":
            result = store.claim(args.id, args.occurrence, args.owner, args.daily_token)
        elif args.command == "renew":
            result = store.renew(args.id, args.token)
        elif args.command == "release":
            result = store.finish(args.id, args.token, "released", args.evidence)
        elif args.command == "finish":
            result = store.finish(args.id, args.token, args.outcome, args.evidence, args.next_at)
        elif args.command == "recover":
            result = store.recover(args.id, args.evidence)
        elif args.command == "resume":
            result = store.resume(args.id, args.evidence)
        elif args.operation == "plan":
            result = D.plan(store, args.time)
        elif args.operation == "bind":
            result = D.binding(store, supplied)
        elif args.operation == "status":
            result = D.status(store)
        elif args.operation == "start":
            result = D.start(store, args.owner)
        elif args.operation == "finish":
            result = D.finish(store, args.token, args.evidence)
        elif args.operation == "renew":
            result = D.renew(store, args.token)
        else:
            result = D.recover(store, args.evidence)
        print(json.dumps(F.T.normalize(result), ensure_ascii=False, indent=2))
        return 0
    except (OSError, ValueError, KeyError, TypeError, yaml.YAMLError) as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
