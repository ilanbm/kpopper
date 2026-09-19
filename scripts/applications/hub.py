"""Command lifecycle for the optional experimental kpopper Hub application."""
import os
import pathlib
import shutil
import subprocess
import sys
import webbrowser

from . import require_html

HERE = pathlib.Path(__file__).resolve().parent.parent


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


def page_of(rest):
    """Where the page lands without --out, and what --checks looks at without a path: the
    record's build directory, `.kpopper/build/page.html`, or record.html in the working
    directory for a record under the old name. The record is the yaml files given, minus the
    brief named by --brief, else the one this directory answers for; nothing is created."""
    sys.path.insert(0, str(HERE))
    import provenance as P
    brief = rest[rest.index("--brief") + 1] if "--brief" in rest and rest.index("--brief") + 1 < len(rest) else None
    files = [x for x in rest if x.endswith((".yaml", ".yml")) and x != brief] or P.default_paths()
    return P.layout_of(files)


def default_page(rest):
    """The page's default place, created: the build directory gets a .gitignore that keeps
    everything in it out of the tree - the record is reviewed, what is rebuilt from it is
    not. A record under the old name keeps writing record.html here, and creates nothing."""
    lay = page_of(rest)
    if lay["build"]:
        pathlib.Path(lay["build"]).mkdir(parents=True, exist_ok=True)
        ignore = pathlib.Path(lay["build"]) / ".gitignore"
        if not ignore.exists():
            ignore.write_text("*\n", encoding="utf-8")
    return lay["page"]


def do_page(args):
    """--out/--open/--tree/--checks belong to this dispatcher, not to render_page.py, so they
    are peeled off here and never forwarded. Without --out the page lands in the record's own
    build directory, `.kpopper/build/page.html`, which ignores itself - or, for a record under
    the old name, as `record.html` here, as it always did."""
    out, after, tree, checks, rest = None, False, False, False, []
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
        # a written page, not a record: nothing here is rendered and nothing is verified -
        # without a page named, the one `page` writes by default
        if not any(a.endswith((".html", ".htm")) for a in rest):
            rest = rest + [page_of(rest)["page"]]
        sys.exit(do_checks(rest))
    script = str(HERE / "render_page.py")
    if "--verify" in rest:
        # No HTML is written; derived measurements are retained outside the record.
        sys.exit(subprocess.run([sys.executable, script] + rest).returncode)
    # The renderer reads the record, but this dispatcher owns the destination. Give it the
    # absolute page path so record-relative source files can stay correct wherever --out puts
    # the page. This internal argument is removed by render_page.py before it finds YAML files.
    destination = out if out is not None else page_of(rest)["page"]
    proc = subprocess.run([sys.executable, script, "--page-out", os.path.abspath(destination)] + rest,
                          stdout=subprocess.PIPE)
    if proc.returncode:
        sys.exit(proc.returncode)               # a build error leaves no page worth writing
    if out is None:
        out = default_page(rest)
    pathlib.Path(out).write_bytes(proc.stdout)
    if after:
        # The page's source links are relative to where the page resolves to, so it is opened
        # under that same name: a record reached through a directory link would otherwise start
        # its links one place and finish them in another. The path is turned into a URL rather
        # than pasted into one, because a name is not a URL: a record directory called
        # `notes # 2` ends the address where its name begins, a space cuts it just as short, and
        # a literal % in it is read as the start of an escape. The one fragment this command
        # offers is added after that, where a fragment belongs.
        target = pathlib.Path(os.path.realpath(out)).as_uri()
        webbrowser.open(target + ("#tree" if tree else ""))
    sys.exit(0)



def main(args):
    if args in (["--help"], ["-h"]):
        print("usage: kpop experimental hub [--open] [--out PATH] [--tree] [--verify] [--checks] [FILE ...]\n\n"
              "kpopper Hub: experimental HTML application. Install kpopper[html].\n"
              "--verify checks the page without writing HTML; --checks runs browser checks.")
        return 0
    try:
        require_html()
    except ValueError as error:
        print(str(error), file=sys.stderr)
        return 2
    do_page(args)


def page_side(paths, read_mode=None, reader=None):
    """Derived counts and arrangement facts for an explicit presentation write."""
    require_html()
    if reader is None:
        sys.path.insert(0, str(HERE))
        import provenance as reader
    renderer = reader._peer('render_page')
    mode = read_mode or ('frozen' if reader._RAW_READS.get()
                         else os.environ.get('KPOPPER_READ_MODE', 'live'))
    paths = renderer.record_paths(paths, read_mode=mode)
    brief = renderer.find_brief(paths, read_mode=mode)
    if not brief:
        return {}, None, {}
    info = renderer.build(paths, brief, read_mode=mode)[4]
    return ({key: value for key, value in info['page'].items() if value is not None},
            info['shape'], info.get('arrangements') or {})
