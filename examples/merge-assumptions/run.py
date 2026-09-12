"""Reproduce the two README merge stories in disposable local Git repositories.

Run from any directory: python3 examples/merge-assumptions/run.py
Requires Git and the Python dependencies used by kpopper. No network is used.
"""
from pathlib import Path
import os
import shutil
import subprocess
import sys
import tempfile

import yaml

HERE = Path(__file__).resolve().parent
CLI = HERE.parents[1] / "scripts" / "kpopper"
ENV = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
ENV.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_SYSTEM=os.devnull,
           GIT_TERMINAL_PROMPT="0", PYTHONDONTWRITEBYTECODE="1")

CACHE_PROBE = """
from cache import lookup
lookup('private', 'alice')
print(any(p['private'] and p['owner'] == 'alice' for p in lookup('private', 'bob')))
"""
DOWNLOAD_PROBE = r"""
from pathlib import Path
import os
import re
import tempfile
from storage import purge_exports
days = int(re.search(r'Download available for (\d+) days',
                     Path('download-email.html').read_text()).group(1))
created = 1735689600
with tempfile.TemporaryDirectory() as folder:
    exported = Path(folder) / 'export.csv'
    exported.write_text('fictional export')
    os.utime(exported, (created, created))
    purge_exports(folder, created + (days - 1) * 86400)
    print(not exported.exists())
"""


def run(root, *args, expected=0):
    result = subprocess.run([str(arg) for arg in args], cwd=root, env=ENV,
                            capture_output=True, text=True, timeout=60)
    output = result.stdout + result.stderr
    if result.returncode != expected:
        raise RuntimeError(f"{args!r}: expected exit {expected}, got {result.returncode}\n{output}")
    return output


def git(root, *args):
    return run(root, "git", "-c", "user.name=Example", "-c", "user.email=example@example.invalid",
               "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false", *args)


def overlay(source, root):
    for path in source.rglob("*"):
        if path.is_file():
            destination = root / path.relative_to(source)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, destination)


def commit(root, message):
    git(root, "add", ".")
    git(root, "commit", "-q", "-m", message)


def verify(root, case, stage, merged=False):
    # These are the branch tests drawn as green ticks, including both suites after merging.
    tests = run(root, sys.executable, "-m", "unittest", "discover", "-s", ".")
    if "Ran 0 tests" in tests:
        raise RuntimeError("The example's branch tests were not discovered")
    record = root / "GROUNDING.yaml"
    before = record.read_bytes()
    # check alone cannot see a code/config change that has not updated the record.
    run(root, sys.executable, CLI, "check", record)
    measured = run(root, sys.executable, CLI, "remeasure", "--run", record,
                   expected=1 if merged else 0)
    decision = "search.shared_cache" if case == "cache" else "downloads.availability"
    if merged and (decision not in measured or "wrong_if holds" not in measured):
        raise RuntimeError(f"Expected the documented failed condition\n{measured}")
    if record.read_bytes() != before:
        raise RuntimeError("Measurement changed the canonical record")
    # An ordinary integration test can also catch this; kpopper does not replace one.
    probe = CACHE_PROBE if case == "cache" else DOWNLOAD_PROBE
    outcome = run(root, sys.executable, "-c", probe).strip()
    if outcome != str(merged):
        raise RuntimeError(f"Unexpected cross-component behavior for {case}/{stage}: {outcome}")
    if git(root, "status", "--porcelain").strip():
        raise RuntimeError("The checks changed the example checkout")
    status = "declared condition fails" if merged else "measurement passes"
    print(f"{case} / {stage}: branch tests pass; {status}")
    if merged:
        for line in measured.splitlines():
            if decision in line and "wrong_if holds" in line:
                print(f"  {line.strip()}")
                break
        print(f"  The separate integration probe also detects the {case} problem.")


def story(case):
    with tempfile.TemporaryDirectory(prefix=f"kpopper-{case}-") as folder:
        root = Path(folder)
        overlay(HERE / case / "base", root)
        # Use the same interpreter for recipes, including on Windows where its name differs.
        path = root / ".kpopper" / "measure.yaml"
        recipes = yaml.safe_load(path.read_text())
        for argv in recipes.values():
            argv[0] = sys.executable
        path.write_text(yaml.safe_dump(recipes, sort_keys=False))
        git(root, "init", "-q", "-b", "base")
        commit(root, "Shared base")
        git(root, "checkout", "-q", "-b", "pr-a", "base")
        overlay(HERE / case / "pr-a", root)
        commit(root, "PR A")
        verify(root, case, "PR A")
        git(root, "checkout", "-q", "-b", "pr-b", "base")
        overlay(HERE / case / "pr-b", root)
        commit(root, "PR B")
        verify(root, case, "PR B")
        git(root, "merge", "--no-edit", "pr-a")
        if len(git(root, "rev-list", "--parents", "-n", "1", "HEAD").split()) != 3:
            raise RuntimeError("Expected a real merge of the two branches")
        print(f"{case}: Git merged PR A and PR B without a text conflict")
        verify(root, case, "merged", merged=True)


if __name__ == "__main__":
    for example in ("cache", "downloads"):
        story(example)
