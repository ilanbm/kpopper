#!/usr/bin/env python3
"""One entry point instead of three full paths. The reading and writing commands pass
straight through to provenance.py; page renders the record and can open what it writes.

  kpop open    [file ...]              what a session should read instead of the whole record
  kpop check   [file ...]              does the record still hold together
  kpop affects <entry> [entry ...]     what a change reaches
  kpop pull    <entry|prefix> [...]    values and sources for a subject
  kpop context <id> [id ...]          records and their declared dependencies
  kpop search  "query"               find claims and source passages with their status
  kpop where                           the record this directory answers for
  kpop map [--deep]                  map the work through an available agent
  kpop config [--guidance on|off]     inspect or change local preferences
  kpop session <setup|status|open|read|search|context|propose|serve>  checked, revision-bound session views
  kpop set     <key> <value> [--why "..."] [--as-of DATE]   change one value; the reply is the reach
  kpop add     <id> field=value ...    a new entry or judgment, in id order, its seen filled
  kpop update  --file JSON            apply one source report atomically and return its receipt
  kpop ingest  capture --file JSON    retain a report and process it in the background
                  add --notify-task TASK_ID to return a native Codex delivery job
  kpop ingest  pending                important findings still needing attention
  kpop review  <id | "section title">  it still holds: seen rewritten from what the record holds
  kpop answer  <question> <id> [--why "..."] | <question> --dropped "why"   an open question
                  closed where it stands, with what answered it and the day
  kpop correct <id> field=value ... [--unset FIELD]   an entry no commit holds yet (outside
                  git, one this session wrote) fixed in place; what rests on it is flagged
                  add --hypothesis NAME to set, add or review: the write lands in
                  .kpopper/hypotheses/NAME.yaml beside the record and the base is not touched -
                  where a write contradicts the base, the refusal names this command
  kpop consolidate [--dry-run] [<hypothesis> ...]   the record with its hypotheses laid over
                  it, tested with the reader's own check - and, without --dry-run, folded into the
                  base when the test is clean; --refute NAME "why" leaves one negative finding and
                  deletes the file; --from REF reads another branch's committed record as one more
                  hypothesis, and pull <seed> --from REF shows what it proposes
  kpop remeasure [--run] [file]        the entries that name a recipe, taken again from the
                  tree: the plan alone until --run; what differs is laid over the record as one
                  more hypothesis and tested by the dry run - the pull request runs it
  kpop same    <a> <b> [--keep a|b]    one subject under two ids: b retired into a, every
                  reference rewritten across the record, its hypotheses and the brief
  kpop distinct <a> <b> "<why>"        two subjects that look alike: recorded on a, so the
                  pair never returns as a candidate
  kpop experimental hub [--out PATH] [args...]  the record as one page, .kpopper/build/page.html by default
                  add --open to look at it in your own browser, --tree to land there
                  add --verify to check the page instead of writing one
                  add --checks [PAGE] for the browser checks on a page already written

Run from the directory the record sits in, same as the two scripts underneath - or from
any checkout of a project that registered its record (see `where`).
"""
import argparse, json, os, sys, pathlib, subprocess, webbrowser

try:
    from .applications import APPLICATIONS, ALIASES, show_catalog
except ImportError:
    from applications import APPLICATIONS, ALIASES, show_catalog

HERE = pathlib.Path(__file__).resolve().parent
READ = ("open", "check", "affects", "pull", "where", "set", "add", "review", "answer", "correct",
        "same", "distinct")
COMMANDS = {
    "expressions": ('convert TEXT [--predicate] | migrate [--record FILE] [--apply]', "Convert explicit formulas to structured data; preview checked record migration."),
    "search": ('"QUERY" [--record FILE] [--limit N] [--chars N]', "Find local source evidence; read a hit with --read REF --revision REV."),
    "open": ("[FILE ...] [--chars N] [--budget N] [--profile core/v1]", "Open the current knowledge context."),
    "map": ("[--deep]", "Map the work through an available host agent."),
    "config": ("[--mode simple|advanced] [--record PATH] [--check] [--guidance on|off]", "Inspect the project mode or change local preferences."),
    "check": ("[FILE ...] [--profile core/v1]", "Check the record's consistency and declared conditions."),
    "assess": ("ID [ID ...] [--attention-only]", "Read versioned assessment findings and scoped attention as JSON."),
    "pull": ("SUBJECT [SUBJECT ...] [--from REF] [--history] [--profile core/v1]", "Read a subject and the evidence behind it."),
    "affects": ("SUBJECT [SUBJECT ...] [--profile core/v1]", "Trace what a change reaches."),
    "context": ("ID [ID ...] [OPTIONS]", "Read records and their declared dependencies from the knowledge graph."),
    "add": ("ID FIELD=VALUE ...", "Add a grounded entry or judgment."),
    "set": ("ID VALUE [--why TEXT] [--as-of DATE]", "Update a reading and see what it affects."),
    "update": ("--file JSON|- [--record FILE] [--state-dir PATH]", "Record one or many changes from a source report now; return applied or retained status."),
    "review": ("ID [--as-of DATE]", "Record a judgment's review against current readings."),
    "answer": ("QUESTION ID [--why TEXT] [--as-of DATE] | QUESTION --dropped TEXT",
               "Close an open question with what answered it, or drop it with the reason."),
    "correct": ("ID FIELD=VALUE ... [--unset FIELD] [--why TEXT]",
                "Fix an entry not yet in any commit (outside git: one this session wrote)."),
    "recover": ("[--record FILE] [--rollback]", "Complete an interrupted direct write or restore its exact before images."),
    "history": ("status|reconcile|rebuild|accept|refute|correct|adopt|migrate|capabilities [OPTIONS]", "Inspect, explicitly resolve or adopt history, rebuild a view, or prepare a verified copy."),
    "export": ("ID [ID ...] [--format FORMAT]", "Export a focused readable excerpt with optional Mermaid."),
    "where": ("", "Locate the record for this workspace."),
    "consolidate": ("[--dry-run] [NAME ...]", "Evaluate and reconcile recorded hypotheses."),
    "remeasure": ("[--run] [FILE]", "Inspect or run the record's measurement recipes."),
    "same": ("ID ID [--keep ID]", "Consolidate two identities of the same subject."),
    "distinct": ("ID ID REASON", "Keep similar subjects distinct."),
    "session": ("OPERATION [OPTIONS]", "Manage checked session views and their transport."),
    "ingest": ("OPERATION [OPTIONS]", "Capture source reports and inspect their processing."),
    "followups": ("OPERATION [OPTIONS]", "Capture deferred work, inspect triggers and coordinate daily review."),
    "knowledge": ("status|materialize|import [OPTIONS]", "Inspect contributions or prepare a portable frozen record."),
    "watch": ("OPERATION [OPTIONS]", "Check branch compatibility asynchronously and share scoped external facts."),
    "pending": ("OPERATION [OPTIONS]", "Inspect, configure and reconcile project contribution publication."),
}


def parser():
    help_text = "Commands:\n" + "\n".join(
        "  kpop %-13s %s" % (name, description) for name, (_, description) in COMMANDS.items())
    help_text += "\n\nExperimental applications:\n  kpop experimental  Optional hub and annotated-doc applications (install kpopper[html])."
    help_text += "\n\nUse COMMAND --help for details. --json returns structured output."
    result = argparse.ArgumentParser(prog="kpop", usage="kpop [--workspace PATH] [--no-cache] COMMAND [OPTIONS]",
                                     description="Keep what you know, its grounds, and what needs another look.",
                                     epilog=help_text, formatter_class=argparse.RawDescriptionHelpFormatter)
    result.add_argument('--frozen', action='store_true', help='read committed files without live pending contributions')
    result.add_argument("--workspace", help="working directory for the operation")
    result.add_argument("--json", action="store_true", help="return structured output")
    result.add_argument("--no-cache", action="store_true",
                        help="parse the record instead of reading a kept parse")
    result.add_argument("command", nargs="?", help="operation to perform")
    result.add_argument("args", nargs=argparse.REMAINDER, help=argparse.SUPPRESS)
    return result


def _hub():
    try:
        from .applications import hub
    except ImportError:
        from applications import hub
    return hub


def _automatic_core_profile(command, args):
    """Select existing core consumers for an explicitly declared record."""
    if any(flag in args for flag in ('--help', '-h')):
        return args
    if command not in {'assess', 'export', 'hub', 'search'} or '--profile' in args \
            or command == 'hub' and '--checks' in args:
        return args
    try:
        from . import provenance as P, workspace as W
    except ImportError:
        import provenance as P
        import workspace as W
    records = []
    for index, value in enumerate(args[:-1]):
        if value == '--record':
            records.append(args[index + 1])
    if not records and command == 'hub':
        records = [value for index, value in enumerate(args)
                   if value.lower().endswith(('.yaml', '.yml'))
                   and not (index and args[index - 1] in {'--brief', '--out'})]
    if not records:
        location = W.locate()
        records = [location['record']] if location['status'] == 'found' else []
    return [*args, '--profile', 'core/v1'] if records and P.core_reader_selected(records) else args




def do_page(args):
    """Compatibility helper for callers of the old dispatcher."""
    sys.exit(_hub().main(args))


def page_of(args):
    return _hub().page_of(args)


def default_page(args):
    return _hub().default_page(args)


def do_checks(args):
    return _hub().do_checks(args)


def main():
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        if hasattr(stream, 'reconfigure'):
            stream.reconfigure(encoding='utf-8', newline='\n')
    os.environ['PYTHONIOENCODING'] = 'utf-8'
    root = parser()
    options = root.parse_args()
    if options.command is None:
        root.print_help()
        sys.exit(0)
    cmd, rest = options.command, options.args
    explicit = cmd == "experimental"
    if explicit:
        while rest and rest[0] == "--json":
            options.json, rest = True, rest[1:]
        if not rest or all(arg in ("--help", "-h", "--json") for arg in rest):
            show_catalog(options.json or "--json" in rest)
            sys.exit(0)
        cmd, rest = rest[0], rest[1:]
        if cmd not in APPLICATIONS and cmd not in ALIASES:
            root.error("unknown experimental application: " + cmd)
    elif cmd in APPLICATIONS:
        root.error("use kpop experimental " + cmd + " for this application")
    requested = cmd
    cmd = ALIASES.get(cmd, cmd)
    application = cmd in APPLICATIONS
    if application and requested != cmd:
        invocation = "kpop " + ("experimental " if explicit else "") + requested
        print("%s is a compatibility alias; use kpop experimental %s (experimental application)."
              % (invocation, cmd), file=sys.stderr)
    if options.frozen or '--frozen' in rest:
        os.environ['KPOPPER_READ_MODE'] = 'frozen'
        rest = [a for a in rest if a != '--frozen']
    if options.no_cache:
        # every command below runs as another process: the switch travels in the environment
        os.environ["KPOPPER_NO_CACHE"] = "1"
    if cmd == "start":
        root.error("Use kpop open, kpop map, or kpop config; start is not a public command.")
    if cmd not in COMMANDS and cmd != "_agent" and not application:
        root.error("unknown command: " + cmd)
    if not application and cmd not in {"open", "map", "config", "_agent", "session", "context", "ingest", "update", "document", "followups", "watch", "export", "assess"} and rest in (["--help"], ["-h"]):
        usage, description = COMMANDS[cmd]
        print("usage: kpop " + cmd + (" " + usage if usage else "") + " [--json]\n\n" + description)
        if cmd in {"set", "add", "review", "answer", "correct", "same", "distinct"}:
            print()
            sys.stdout.flush()
            subprocess.run([sys.executable, str(HERE / "provenance.py"), cmd, "--help"])
        elif cmd in {"consolidate", "remeasure"}:
            print()
            sys.stdout.flush()
            subprocess.run([sys.executable, str(HERE / (cmd + ".py")), "--help"])
        sys.exit(0)
    if options.workspace is not None:
        try:
            if not options.workspace.strip():
                raise ValueError("workspace must be a nonempty path")
            os.chdir(pathlib.Path(options.workspace).expanduser())
        except (OSError, ValueError) as error:
            root.error(str(error))
    rest = _automatic_core_profile(cmd, rest)
    if cmd == 'knowledge':
        try:
            from .knowledge_cli import main as knowledge_main
        except ImportError:
            from knowledge_cli import main as knowledge_main
        sys.exit(knowledge_main([a for a in rest if a != '--json']))
    if cmd == 'recover':
        try:
            from .history_recovery_cli import main as recover_main
        except ImportError:
            from history_recovery_cli import main as recover_main
        sys.exit(recover_main((["--json"] if options.json else []) + rest))
    if cmd == 'history':
        try:
            from .history_cli import main as history_main
        except ImportError:
            from history_cli import main as history_main
        sys.exit(history_main((["--json"] if options.json else []) + rest))
    if cmd == "watch":
        try:
            from .watch import main as watch_main
        except ImportError:
            from watch import main as watch_main
        sys.exit(watch_main([arg for arg in rest if arg != "--json"]))
    if cmd == 'pending':
        try:
            from .pending_cli import main as pending_main
        except ImportError:
            from pending_cli import main as pending_main
        sys.exit(pending_main((["--json"] if options.json else []) + rest))
    if cmd == "assess":
        try:
            from .assessment import main as assess_main
        except ImportError:
            from assessment import main as assess_main
        sys.exit(assess_main([arg for arg in rest if arg != "--json"]))
    if cmd == "followups":
        try:
            from .followups_cli import main as followups_main
        except ImportError:
            from followups_cli import main as followups_main
        # The group returns structured data directly, without the legacy text wrapper.
        rest = [arg for arg in rest if arg != "--json"]
        sys.exit(followups_main(rest))
    if cmd in {"open", "map", "config"}:
        try:
            from . import workspace_cli
        except ImportError:
            import workspace_cli
        if options.json:
            rest = ["--json", *rest]
        function = {"open": workspace_cli.open_context, "map": workspace_cli.map_work,
                    "config": workspace_cli.config}[cmd]
        sys.exit(function(rest))
    if cmd == "_agent":
        try:
            from .onboarding import main as start
        except ImportError:
            from onboarding import main as start
        sys.exit(start(rest))
    if cmd == "expressions":
        try:
            from . import expression_cli
        except ImportError:
            import expression_cli
        # Expressions already return structured data; a global JSON option needs
        # no legacy-output wrapper or second subprocess.
        sys.exit(expression_cli.main(rest))
    if cmd == "update":
        try:
            from .ingestion import update_main
        except ImportError:
            from ingestion import update_main
        sys.exit(update_main(rest))
    if cmd == "search":
        try:
            from . import search
        except ImportError:
            import search
        sys.exit(search.main((["--json"] if options.json else []) + rest))
    # Existing operations retain their exit codes and text. JSON wraps that output
    # without reparsing it as evidence or changing what the operation does.
    cutoff = rest.index("--") if "--" in rest else len(rest)
    json_output = options.json or "--json" in rest[:cutoff]
    if cmd == "context" and json_output:
        # Context already emits structured evidence; match the native CLI and
        # avoid wrapping its JSON as an opaque string inside a second envelope.
        rest = [value for value in rest[:cutoff] if value != "--json"] + rest[cutoff:]
        cutoff = rest.index("--") if "--" in rest else len(rest)
        json_output = False
    if json_output:
        forwarded = [value for value in rest[:cutoff] if value != "--json"] + rest[cutoff:]
        result = subprocess.run([sys.executable, str(HERE / "cli.py"), *(["experimental", cmd] if application else [cmd]), *forwarded],
                                capture_output=True, text=True, encoding="utf-8")
        print(json.dumps({"command": requested, "exit_code": result.returncode,
                          "output": result.stdout, "error": result.stderr}, ensure_ascii=False))
        sys.exit(result.returncode)
    if cutoff < len(rest):
        rest = rest[:cutoff] + rest[cutoff + 1:]
    if application:
        if cmd == "hub":
            do_page(rest)
        try:
            from .applications.annotated_doc import main as document_main
        except ImportError:
            from applications.annotated_doc import main as document_main
        sys.exit(document_main(rest))
    if cmd in {"session", "context", "ingest", "export"}:
        script = {"session": "session_cli.py", "context": "context_cli.py", "ingest": "ingestion.py", "export": "export_graph.py"}[cmd]
        tool = [sys.executable, str(HERE / script)] + rest
        if os.name == "nt":
            sys.exit(subprocess.run(tool).returncode)
        os.execv(sys.executable, tool)
    if cmd == "consolidate" or (cmd == "pull" and "--from" in rest):
        # the union and the fold live beside the reader; pull --from is theirs too, since
        # another branch's record is read the way a hypothesis is
        tool = [sys.executable, str(HERE / "consolidate.py")] + ([cmd] if cmd == "pull" else []) + rest
        if os.name == "nt":
            sys.exit(subprocess.run(tool).returncode)
        os.execv(sys.executable, tool)
    if cmd == "remeasure":
        # the tree measured against the record: the one command that runs a recipe, kept in a
        # module of its own so that nothing reading the record imports it
        tool = [sys.executable, str(HERE / "remeasure.py")] + rest
        if os.name == "nt":
            sys.exit(subprocess.run(tool).returncode)
        os.execv(sys.executable, tool)
    if cmd in READ:
        reader = [sys.executable, str(HERE / "provenance.py"), cmd] + rest
        if os.name == "nt":
            # Windows has no exec: it would start the reader and return 0 immediately, so a
            # record that fails check would read as one that passed.
            sys.exit(subprocess.run(reader).returncode)
        # execv, not subprocess: stays one process, so exit code and stdio need no plumbing.
        os.execv(sys.executable, reader)
    print(__doc__.strip("\n"))
    sys.exit(2)


if __name__ == "__main__":
    main()
