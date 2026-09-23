#!/usr/bin/env python3
"""Keep the native crate's copies of the files it shares with the rest of the repository.

crates.io receives native/ alone, so every file the crate embeds or fingerprints from
outside that directory travels inside it as a copy: a file under scripts/ is copied to the
same path under native/shared/, and the licence to native/LICENSE. The originals remain the
files to edit. Inside this repository the build refuses a stale copy, and the release tests
run the same check.

    python3 .github/scripts/native_shared.py           # refresh the copies
    python3 .github/scripts/native_shared.py --check   # list stale copies; exit 1 if any
"""
import argparse
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
SHARED = pathlib.Path("native/shared")
# What the crate reads from scripts/: include_str!/include_bytes! targets and the Lean
# sources the build fingerprints. A trailing slash takes every .lean file below it.
FROM_SCRIPTS = (
    "assessment.schema.json",
    "document/frame.js",
    "document/layer.css",
    "document/layer.js",
    "expressions.py",
    "page/components.css",
    "page/dates.js",
    "page/page.css",
    "page/page.js",
    "reasoning/assessment.schema.json",
    "reasoning/history_assessment.schema.json",
    "reasoning/lean/",
    "session/lean/Main.lean",
    "session/rules.txt",
    "start-guide.md",
    "verify_page.js",
)
FIX = "python3 .github/scripts/native_shared.py"


def lean_sources(directory):
    """The .lean files under a directory, leaving out hidden directories such as build output."""
    return sorted(path for path in directory.rglob("*.lean")
                  if not any(part.startswith(".") for part in path.relative_to(directory).parts))


def copies(root=ROOT):
    """-> {copy: original}, both relative to the repository root."""
    pairs = {pathlib.Path("native/LICENSE"): pathlib.Path("LICENSE")}
    for name in FROM_SCRIPTS:
        if name.endswith("/"):
            base = root / "scripts" / name
            originals = [path.relative_to(root) for path in lean_sources(base)]
            if not originals:
                raise SystemExit(f"scripts/{name} holds no .lean files")
        else:
            originals = [pathlib.Path("scripts") / name]
        for original in originals:
            pairs[SHARED / original.relative_to("scripts")] = original
    return pairs


def stale(root=ROOT):
    """Copies that are missing or differ from their original, and files under native/shared/
    that no longer copy anything, as sorted repository paths."""
    pairs = copies(root)
    found = [copy for copy, original in pairs.items()
             if not (root / copy).is_file() or (root / copy).read_bytes() != (root / original).read_bytes()]
    shared = root / SHARED
    if shared.is_dir():
        found += [path.relative_to(root) for path in shared.rglob("*")
                  if path.is_file() and path.relative_to(root) not in pairs]
    return sorted(path.as_posix() for path in found)


def refresh(root=ROOT):
    """Bring every copy up to date and remove what no longer copies anything. -> the paths changed."""
    changed = stale(root)
    pairs = copies(root)
    for name in changed:
        path = root / name
        original = pairs.get(pathlib.Path(name))
        if original is None:
            path.unlink()
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes((root / original).read_bytes())
    for directory in sorted((root / SHARED).rglob("*"), reverse=True):
        if directory.is_dir() and not any(directory.iterdir()):
            directory.rmdir()
    return changed


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="report stale copies without changing them")
    options = parser.parse_args(argv)
    if options.check:
        found = stale()
        for name in found:
            print(f"stale: {name}")
        if found:
            print(f"the crate's copies differ from their originals; refresh them with: {FIX}")
            return 1
        return 0
    for name in refresh():
        print(f"refreshed: {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
