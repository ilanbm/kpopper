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
  kpopper page    [--out PATH] [--open] [--tree] [args...]   the record as one page
                  add --verify to check the page instead of writing one

Run from the directory the record sits in, same as the two scripts underneath - or from
any checkout of a project that registered its record (see `where`).
"""
import os, sys, pathlib, subprocess, webbrowser

HERE = pathlib.Path(__file__).resolve().parent
READ = ("open", "check", "affects", "pull", "where", "set", "add", "review")


def do_page(args):
    """--out/--open/--tree belong to this dispatcher, not to render_page.py, so they are
    peeled off here and never forwarded."""
    out, after, tree, rest = "record.html", False, False, []
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
        else:
            rest.append(a); i += 1
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
