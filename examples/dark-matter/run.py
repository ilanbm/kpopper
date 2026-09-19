"""Replay a six-paper research record, with real CLI writes and dependency checks.

No model calls or network. Requires Python dependencies, POSIX locks and a ready
local reasoning core for the exact density calculation.
"""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import argparse
import json
from fractions import Fraction
import re
import shutil
import subprocess
import sys
import tempfile
import time

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
    # Concurrent preparation can observe a newer committed record. Only retry
    # this explicit pre-write refusal; semantic failures must remain failures.
    for attempt in range(64):
        try:
            cli(root, "add", entry_id, json.dumps(fields), "--in", collection)
            return
        except RuntimeError as error:
            retryable = str(error).strip() == "refused - record changed while preparing the write; retry"
            if not retryable or attempt == 63:
                raise
            time.sleep(min(0.025 * (attempt + 1), 0.5))


def demonstrate(root):
    data = yaml.safe_load((HERE / "inputs.yaml").read_text())
    record = root / "GROUNDING.yaml"
    # Seed the legacy format explicitly to replay the published example. An
    # absent record would instead start with core/v1 and history in kpopper 1.7.
    seed = yaml.safe_dump({
        "meta": {"scope": "Six-paper worked example; selected source readings and an authored synthesis, not a complete review or live research-agent evaluation."},
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
    for key, fields in data["derived"].items():
        add(root, "known", key, fields)
    for key, fields in data["judgments"].items():
        add(root, "judgments", key, fields)
    for key, fields in data["open"].items():
        add(root, "open", key, fields)

    built = yaml.safe_load(record.read_text())
    expected = data["findings"] | data["frame"] | data["derived"]
    retained = {key: {field: value for field, value in body.items()
                      if field != "seen" or key not in data["derived"]}
                for key, body in built["known"].items()}
    if retained != expected:
        raise RuntimeError("Concurrent writes did not retain the exact prepared findings")
    ratio = (data["findings"]["cmb.omega_c_h2"]["v"] /
             data["findings"]["cmb.omega_b_h2"]["v"])
    for key, authored in data["judgments"].items():
        snapshot = built["judgments"][key]["seen"]
        if set(snapshot) != set(authored["rests_on"]):
            raise RuntimeError(f"{key} did not snapshot its exact declared premises")
        for dependency, observed in snapshot.items():
            if dependency == "cmb.dark_to_baryon_density":
                computation = observed.get("computed", {}) if isinstance(observed, dict) else {}
                value = computation.get("value", {})
                rational = value.get("rational", []) if isinstance(value, dict) else []
                exact = (Fraction(str(data["findings"]["cmb.omega_c_h2"]["v"])) /
                         Fraction(str(data["findings"]["cmb.omega_b_h2"]["v"])))
                if (rational != [str(exact.numerator), str(exact.denominator)] or
                        computation.get("rule") != data["derived"][dependency]["rule"]):
                    raise RuntimeError("The density calculation did not reach the cosmology judgment")
                continue
            wanted = (expected[dependency]["v"] if dependency in expected
                      else data["judgments"][dependency]["verdict"])
            if observed != wanted:
                raise RuntimeError(f"{key} changed or lost the reading of {dependency}")
    checked = cli(root, "check")
    print(f"Three concurrent writers: all {len(data['findings'])} findings from six papers retained.")
    print(f"{len(data['judgments'])} judgments: every exact premise captured in seen.")
    print(f"Density ratio from rounded central values: {ratio:.6f}; not independent evidence.")
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
