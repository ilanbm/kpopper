#!/usr/bin/env python3
"""One entry point instead of three full paths. The reading and writing commands pass
straight through to provenance.py; page renders the record and can open what it writes.

  kpopper open    [file ...]              what a session should read instead of the whole record
  kpopper check   [file ...]              does the record still hold together
  kpopper affects <entry> [entry ...]     what a change reaches
  kpopper pull    <entry|prefix> [...]    values and sources for a subject
  kpopper where                           the record this directory answers for
  kpopper set     <key> <value> [--why "..."] [--as-of DATE]   change one value; the reply is the reach
  kpopper add     <id> field=value ...    a new entry or judgment, in id order, its seen filled
  kpopper review  <id | "section title">  it still holds: seen rewritten from what the record holds
                  add --hypothesis NAME to any of the three: the write lands in
                  PROVENANCE.d/NAME.yaml beside the record and the base is not touched - where a
                  write contradicts the base, the refusal names this command
  kpopper consolidate [--dry-run] [<hypothesis> ...]   the record with its hypotheses laid over
                  it, tested with the reader's own check - and, without --dry-run, folded into the
                  base when the test is clean; --refute NAME "why" leaves one negative finding and
                  deletes the file; --from REF reads another branch's committed record as one more
                  hypothesis, and pull <seed> --from REF shows what it proposes
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
import os, shutil, sys, pathlib, subprocess, webbrowser

HERE = pathlib.Path(__file__).resolve().parent
READ = ("open", "check", "affects", "pull", "where", "set", "add", "review", "same", "distinct")


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
    argv = sys.argv[1:]
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__.strip("\n"))
        sys.exit(0)
    cmd, rest = argv[0], argv[1:]
    if cmd == "consolidate" or (cmd == "pull" and "--from" in rest):
        # the union and the fold live beside the reader; pull --from is theirs too, since
        # another branch's record is read the way a hypothesis is
        tool = [sys.executable, str(HERE / "consolidate.py")] + ([cmd] if cmd == "pull" else []) + rest
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
