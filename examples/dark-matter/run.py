"""Replay three prepared research contributions into one record, with real CLI writes.

No model calls or network. Requires kpopper's Python dependencies and POSIX locks.
"""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile

import yaml

HERE = Path(__file__).resolve().parent
CLI = HERE.parents[1] / "scripts" / "kpopper"


def cli(root, *args):
    result = subprocess.run(
        [sys.executable, str(CLI), "--workspace", str(root), *args],
        cwd=root, capture_output=True, text=True, timeout=60,
    )
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    return result.stdout


def add(root, collection, entry_id, fields):
    cli(root, "add", entry_id, json.dumps(fields), "--in", collection)


def demonstrate(root):
    data = yaml.safe_load((HERE / "inputs.yaml").read_text())
    record = root / "GROUNDING.yaml"
    seed = yaml.safe_dump({
        "meta": {"scope": "Three-paper teaching example; prepared readings, not a complete literature review."},
        "sources": data["sources"], "known": None, "judgments": None, "open": None,
    }, sort_keys=False)
    for collection in ("known", "judgments", "open"):
        seed = seed.replace(f"{collection}: null\n", f"{collection}:\n")
    record.write_text(seed)
    shutil.copyfile(HERE / "README.md", root / "README.md")

    # Overlap three separate CLI writers on the same file. The supported writer
    # lock serializes each mutation; the research tasks can run concurrently.
    with ThreadPoolExecutor(max_workers=3) as pool:
        jobs = [pool.submit(add, root, "known", key, fields)
                for key, fields in data["findings"].items()]
        for job in jobs:
            job.result()
    for key, fields in data["frame"].items():
        add(root, "known", key, fields)
    for key, fields in data["judgments"].items():
        add(root, "judgments", key, fields)
    for key, fields in data["open"].items():
        add(root, "open", key, fields)

    built = yaml.safe_load(record.read_text())
    expected = data["findings"] | data["frame"]
    if built["known"] != expected:
        raise RuntimeError("Concurrent writes did not retain the exact prepared findings")
    snapshot = built["judgments"]["synthesis.dark_matter"]["seen"]
    if snapshot != {key: value["v"] for key, value in expected.items()}:
        raise RuntimeError("The synthesis did not snapshot all four declared premises")
    checked = cli(root, "check")
    print("Three concurrent writers: all three sourced findings retained.")
    print("Synthesis: all three findings and the adopted framework captured in seen.")
    print(checked.strip())
    print("\nRead the connected argument:")
    print(cli(root, "pull", "synthesis.dark_matter", "--budget", "2000").strip())

    # A separate copy asks a different review question. No published result is
    # altered and the assembled record remains available as the original example.
    trial = root / "scope-change"
    trial.mkdir()
    shutil.copyfile(record, trial / "GROUNDING.yaml")
    shutil.copyfile(HERE / "README.md", trial / "README.md")
    original = (trial / "GROUNDING.yaml").read_bytes()
    changed = cli(trial, "set", "research.framework",
        "Assess explanations without adopting base Lambda-CDM for the CMB result.",
        "--why", "Hypothetical change in review scope; no paper finding has changed.",
        "--hypothesis", "scope_change")
    if not re.search(r"MOVED\s+synthesis\.dark_matter", changed):
        raise RuntimeError("The changed review scope did not flag the synthesis")
    if (trial / "GROUNDING.yaml").read_bytes() != original:
        raise RuntimeError("A hypothetical scope change rewrote the base record")
    print("\nHypothetical scope change: stop adopting base Lambda-CDM.")
    print(changed.strip())
    print("MOVED under the hypothesis asks for review; the base and paper findings are untouched.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="Keep the result in a NEW directory")
    args = parser.parse_args()
    try:
        import fcntl  # noqa: F401 -- parallel mutation requires the supported lock
    except ImportError:
        parser.error("This concurrent-writing example requires POSIX file locks (macOS/Linux).")
    if args.output:
        root = args.output.resolve()
        root.mkdir(parents=True, exist_ok=False)
        demonstrate(root)
        print(f"\nSaved example: {root / 'GROUNDING.yaml'}")
    else:
        with tempfile.TemporaryDirectory(prefix="kpopper-dark-matter-") as folder:
            demonstrate(Path(folder))


if __name__ == "__main__":
    main()
