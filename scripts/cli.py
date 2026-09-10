#!/usr/bin/env python3
"""One entry point instead of three full paths. The reading and writing commands pass
straight through to provenance.py; page renders the record and can open what it writes.

  kpopper open    [file ...]              what a session should read instead of the whole record
  kpopper check   [file ...]              does the record still hold together
  kpopper affects <entry> [entry ...]     what a change reaches
  kpopper pull    <entry|prefix> [...]    values and sources for a subject
  kpopper search  "query"               find claims and source passages with their status
  kpopper where                           the record this directory answers for
  kpopper map [--deep]                  map the work through an available agent
  kpopper config [--guidance on|off]     inspect or change local preferences
  kpopper session <setup|status|open|read|propose|serve>  checked, revision-bound session views
  kpopper set     <key> <value> [--why "..."] [--as-of DATE]   change one value; the reply is the reach
  kpopper add     <id> field=value ...    a new entry or judgment, in id order, its seen filled
  kpopper ingest  capture --file JSON    retain a report and process it in the background
                  add --notify-task TASK_ID to return a native Codex delivery job
  kpopper ingest  pending                important findings still needing attention
  kpopper review  <id | "section title">  it still holds: seen rewritten from what the record holds
                  add --hypothesis NAME to any of the three: the write lands in
                  PROVENANCE.d/NAME.yaml beside the record and the base is not touched - where a
                  write contradicts the base, the refusal names this command
  kpopper consolidate [--dry-run] [<hypothesis> ...]   the record with its hypotheses laid over
                  it, tested with the reader's own check - and, without --dry-run, folded into the
                  base when the test is clean; --refute NAME "why" leaves one negative finding and
                  deletes the file; --from REF reads another branch's committed record as one more
                  hypothesis, and pull <seed> --from REF shows what it proposes
  kpopper remeasure [--run] [file]        the entries that name a recipe, taken again from the
                  tree: the plan alone until --run; what differs is laid over the record as one
                  more hypothesis and tested by the dry run - the pull request runs it
  kpopper same    <a> <b> [--keep a|b]    one subject under two ids: b retired into a, every
                  reference rewritten across the record, its hypotheses and the brief
  kpopper distinct <a> <b> "<why>"        two subjects that look alike: recorded on a, so the
                  pair never returns as a candidate
  kpopper page    [--out PATH] [args...]  the record as one page
                  add --open to look at it in your own browser, --tree to land there
                  add --verify to check the page instead of writing one
                  add --checks [PAGE] for the browser checks on a page already written

Run from the directory the record sits in, same as the two scripts underneath - or from
any checkout of a project that registered its record (see `where`).
"""
import argparse, json, os, shutil, sys, pathlib, subprocess, webbrowser

HERE = pathlib.Path(__file__).resolve().parent
READ = ("open", "check", "affects", "pull", "where", "set", "add", "review", "same", "distinct")
COMMANDS = {
    "expressions": ('convert TEXT [--predicate] | migrate [--record FILE] [--apply]', "Convert explicit formulas to structured data; preview checked record migration."),
    "search": ('"QUERY" [--record FILE] [--limit N] [--chars N]', "Find local source evidence; read a hit with --read REF --revision REV."),
    "open": ("[FILE ...] [--chars N] [--budget N]", "Open the current knowledge context."),
    "map": ("[--deep]", "Map the work through an available host agent."),
    "config": ("[--guidance on|off]", "Read or change local user preferences."),
    "check": ("[FILE ...]", "Check the record's consistency and declared conditions."),
    "pull": ("SUBJECT [SUBJECT ...] [--from REF]", "Read a subject and the evidence behind it."),
    "affects": ("SUBJECT [SUBJECT ...]", "Trace what a change reaches."),
    "add": ("ID FIELD=VALUE ...", "Add a grounded entry or judgment."),
    "set": ("ID VALUE [--why TEXT] [--as-of DATE]", "Update a reading and see what it affects."),
    "review": ("ID [--as-of DATE]", "Record a judgment's review against current readings."),
    "page": ("[--open] [--out PATH] [--verify]", "Render or verify the knowledge page."),
    "where": ("", "Locate the record for this workspace."),
    "consolidate": ("[--dry-run] [NAME ...]", "Evaluate and reconcile recorded hypotheses."),
    "remeasure": ("[--run] [FILE]", "Inspect or run the record's measurement recipes."),
    "same": ("ID ID [--keep ID]", "Consolidate two identities of the same subject."),
    "distinct": ("ID ID REASON", "Keep similar subjects distinct."),
    "session": ("OPERATION [OPTIONS]", "Manage checked session views and their transport."),
    "ingest": ("OPERATION [OPTIONS]", "Capture source reports and inspect their processing."),
}


def parser():
    help_text = "Commands:\n" + "\n".join(
        "  kpopper %-13s %s" % (name, description) for name, (_, description) in COMMANDS.items())
    help_text += "\n\nUse COMMAND --help for details. --json returns structured output."
    result = argparse.ArgumentParser(prog="kpopper", usage="kpopper [--workspace PATH] COMMAND [OPTIONS]",
                                     description="Keep what you know, its grounds, and what needs another look.",
                                     epilog=help_text, formatter_class=argparse.RawDescriptionHelpFormatter)
    result.add_argument("--workspace", help="working directory for the operation")
    result.add_argument("--json", action="store_true", help="return structured output")
    result.add_argument("command", nargs="?", help="operation to perform")
    result.add_argument("args", nargs=argparse.REMAINDER, help=argparse.SUPPRESS)
    return result


def do_checks(args):
    """The browser checks, on a page that was already written. They are Node, so all this
    does is find the copy that shipped beside the reader and hand it over - an installed one
    sits somewhere nobody would think to look."""
    checker = HERE / "verify_page.js"
    if not checker.exists():
        sys.exit(f"the browser checks are not in this install: {checker}")
    node = shutil.which("node")
    if not node:
        sys.exit("the browser checks run on Node 18+, and there is no node on the path")
    return subprocess.run([node, str(checker)] + args).returncode


def do_page(args):
    """--out/--open/--tree/--checks belong to this dispatcher, not to render_page.py, so they
    are peeled off here and never forwarded."""
    out, after, tree, checks, rest = "record.html", False, False, False, []
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--out":
            if i + 1 >= len(args):
                sys.exit("--out needs a path")
            out, i = args[i + 1], i + 2
        elif a == "--open":
            after, i = True, i + 1
        elif a == "--tree":
            tree, i = True, i + 1
        elif a == "--checks":
            checks, i = True, i + 1
        else:
            rest.append(a); i += 1
    if checks:
        # a written page, not a record: nothing here is rendered and nothing is verified
        sys.exit(do_checks(rest))
    script = str(HERE / "render_page.py")
    if "--verify" in rest:
        # nothing is written in this mode; the exit code is the whole answer.
        sys.exit(subprocess.run([sys.executable, script] + rest).returncode)
    proc = subprocess.run([sys.executable, script] + rest, stdout=subprocess.PIPE)
    if proc.returncode:
        sys.exit(proc.returncode)               # a build error leaves no page worth writing
    pathlib.Path(out).write_bytes(proc.stdout)
    if after:
        webbrowser.open("file://" + os.path.abspath(out) + ("#tree" if tree else ""))
    sys.exit(0)


def main():
    root = parser()
    options = root.parse_args()
    if options.command is None:
        root.print_help()
        sys.exit(0)
    cmd, rest = options.command, options.args
    if cmd == "start":
        root.error("Use kpopper open, kpopper map, or kpopper config; start is not a public command.")
    if cmd not in COMMANDS and cmd != "_agent":
        root.error("unknown command: " + cmd)
    if cmd not in {"open", "map", "config", "_agent", "session", "ingest"} and rest in (["--help"], ["-h"]):
        usage, description = COMMANDS[cmd]
        print("usage: kpopper " + cmd + (" " + usage if usage else "") + " [--json]\n\n" + description)
        if cmd in {"set", "add", "review", "same", "distinct"}:
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
    if json_output:
        forwarded = [value for value in rest[:cutoff] if value != "--json"] + rest[cutoff:]
        result = subprocess.run([sys.executable, str(HERE / "cli.py"), cmd, *forwarded],
                                capture_output=True, text=True, encoding="utf-8")
        print(json.dumps({"command": cmd, "exit_code": result.returncode,
                          "output": result.stdout, "error": result.stderr}, ensure_ascii=False))
        sys.exit(result.returncode)
    if cutoff < len(rest):
        rest = rest[:cutoff] + rest[cutoff + 1:]
    if cmd in {"session", "ingest"}:
        script = "session_cli.py" if cmd == "session" else "ingestion.py"
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
    if cmd == "page":
        do_page(rest)
    print(__doc__.strip("\n"))
    sys.exit(2)


if __name__ == "__main__":
    main()
