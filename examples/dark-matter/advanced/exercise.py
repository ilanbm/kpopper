"""Run a finite research query, change its collection and replay retained history.

Uses the installed package, CLI and public Snapshot/Evaluator API. No model or
network calls. All writes are confined to a new disposable or requested directory.
"""
import argparse
from fractions import Fraction
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

import yaml
from kpopper.reasoning.evaluate import Evaluator
from kpopper.reasoning.snapshot import Snapshot


HERE = Path(__file__).resolve().parent
SUBJECTS = (
    "m.astronomy_count", "m.astronomy_members", "m.particle_identities",
    "cmb.dark_to_baryon_density",
)


def save_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def cli(root, label, *args):
    command = [sys.executable, "-m", "kpopper.cli", "--workspace", str(root), *args]
    process = subprocess.run(command, cwd=root, text=True, capture_output=True, timeout=120)
    captures = root / "captures"
    (captures / f"{label}.stdout").write_text(process.stdout, encoding="utf-8")
    (captures / f"{label}.stderr").write_text(process.stderr, encoding="utf-8")
    save_json(captures / f"{label}.command.json", {
        "argv": command, "cwd": str(root), "exit_code": process.returncode,
    })
    if process.returncode:
        raise RuntimeError(f"{label}: {process.stdout}{process.stderr}")
    return process.stdout


def evaluate(snapshot):
    engine = Evaluator(snapshot)
    return {subject: engine.evaluate({"ref": subject}, declared=[subject]) for subject in SUBJECTS}


def number(value):
    assert value["type"] == "number", value
    return Fraction(int(value["numerator"]), int(value["denominator"]))


def selected(results):
    result = results["m.astronomy_members"]
    assert result["status"] == "ok", result
    value = result["value"]["fields"]["result"]
    assert value["type"] == "list" and all(item["type"] == "text" for item in value["items"])
    return [item["value"] for item in value["items"]]


def summarize(results):
    count = results["m.astronomy_count"]
    missing = results["m.particle_identities"]
    ratio = results["cmb.dark_to_baryon_density"]
    assert count["status"] == ratio["status"] == "ok", results
    exact_count = number(count["value"]["fields"]["result"])
    assert exact_count.denominator == 1
    assert missing["status"] == "unknown" and missing["value"] is None, missing
    return {
        "input_count": count["query_counts"]["input_count"],
        "astronomy_count": int(exact_count),
        "astronomy_members": selected(results),
        "particle_identities": missing["status"],
        "unknown_particle_values": missing["query_counts"]["unknown_value_count"],
        "density_ratio": str(number(ratio["value"])),
        "count_basis": count["basis"]["digest"],
    }


def capture_stage(root, record, label):
    snapshot = Snapshot.capture([str(record)], read_mode="frozen")
    assert "history" in snapshot.to_data()["context"], "Expected active, captured history"
    (root / "captures" / f"{label}.snapshot.json").write_text(snapshot.to_json(), encoding="utf-8")
    results = evaluate(snapshot)
    save_json(root / "captures" / f"{label}.api.json", results)
    report = json.loads(cli(root, f"{label}.assess", "--frozen", "assess", *SUBJECTS,
                            "d.review_scope", "--record", str(record), "--history"))
    assert report["schema_version"] == 3, report
    # These are independent CLI and public API reads of the same committed stage.
    for subject in SUBJECTS:
        computation = report["nodes"][subject]["computation"]
        for field in ("status", "value", "basis", "query_counts"):
            assert computation.get(field) == results[subject].get(field), (subject, field)
    assert report["nodes"]["d.review_scope"]["state"]["falsifier"]["status"] == "not_declared"
    status = json.loads(cli(root, f"{label}.history", "history", "status",
                            "--record", str(record), "--json"))
    assert all(subject["acceptance"] == "accepted" for subject in status["subjects"].values())
    return snapshot, results, status


def demonstrate(root):
    captures = root / "captures"
    captures.mkdir()
    source = root / "fixture"
    source.mkdir()
    shutil.copyfile(HERE / "record.yaml", source / "GROUNDING.yaml")
    shutil.copyfile(HERE / "README.md", source / "README.md")
    # Import the explicit core fixture into a new history copy. No live cutover.
    history = root / "history"
    cli(root, "migrate", "history", "migrate", "--record", str(source / "GROUNDING.yaml"),
        "--to", str(history), "--json")
    record = history / "GROUNDING.yaml"
    judgment = yaml.safe_load((HERE / "judgment.yaml").read_text(encoding="utf-8"))
    cli(root, "add-judgment", "add", "d.review_scope", json.dumps(judgment),
        "--in", "judgments", str(record))

    first, before, first_history = capture_stage(root, record, "before")
    initial = summarize(before)
    assert initial["input_count"] == initial["astronomy_count"] == 5
    assert initial["unknown_particle_values"] == 5
    assert initial["density_ratio"] == "75/14"
    reviewed = yaml.safe_load(record.read_text(encoding="utf-8"))["judgments"]["d.review_scope"]
    assert "wrong_if" not in reviewed
    assert reviewed["seen"]["m.astronomy_count"]["computed"]["basis"] == before["m.astronomy_count"]["basis"]

    later = yaml.safe_load((HERE / "later-study.yaml").read_text(encoding="utf-8"))
    cli(root, "add-lz", "add", "study.lz", json.dumps(later), "--in", "studies", str(record))
    second, after, second_history = capture_stage(root, record, "after")
    revised = summarize(after)
    assert revised["input_count"] == revised["unknown_particle_values"] == 6
    assert revised["astronomy_count"] == initial["astronomy_count"]
    assert revised["astronomy_members"] == initial["astronomy_members"]
    assert second_history["commits"] == first_history["commits"] + 1
    assert second_history["subjects"]["study.lz"]["acceptance"] == "accepted"
    assert first.snapshot_id != second.snapshot_id
    assert (before["m.astronomy_count"]["value"]["fields"]["result"]
            == after["m.astronomy_count"]["value"]["fields"]["result"])
    for subject in ("m.astronomy_count", "m.astronomy_members"):
        assert before[subject]["basis"] != after[subject]["basis"]
        assert before[subject]["query_counts"]["unknown_membership_count"] == 0
        assert after[subject]["query_counts"]["unknown_membership_count"] == 0
        assert [read["kind"] for read in after[subject]["executed_reads"]] == ["node", "scope"]
    assert "study.lz" in after["m.astronomy_count"]["basis"]["scope"]["members"]
    assert "study.lz" not in revised["astronomy_members"]
    assert before["cmb.dark_to_baryon_density"]["basis"] == after["cmb.dark_to_baryon_density"]["basis"]
    assert revised["density_ratio"] == initial["density_ratio"]
    assert yaml.safe_load(record.read_text(encoding="utf-8"))["judgments"]["d.review_scope"] == reviewed

    # Replay the serialized Snapshot with both source directories out of reach.
    # The original history is restored even if a replay assertion fails.
    source.rename(root / "fixture-unavailable")
    history.rename(root / "history-unavailable")
    try:
        retained = Snapshot.from_json((captures / "before.snapshot.json").read_text(encoding="utf-8"))
        replay = evaluate(retained)
        assert retained.snapshot_id == first.snapshot_id
        for subject in SUBJECTS:
            for field in ("status", "value", "basis", "query_counts"):
                assert replay[subject].get(field) == before[subject].get(field), (subject, field)
    finally:
        (root / "history-unavailable").rename(history)
        (root / "fixture-unavailable").rename(source)
    save_json(captures / "replay.api.json", replay)
    summary = {
        "cli_and_api_agree": True,
        "before": initial,
        "after": revised,
        "same_selected_members": initial["astronomy_members"] == revised["astronomy_members"],
        "count_basis_changed": initial["count_basis"] != revised["count_basis"],
        "qualitative_falsifier": "not_declared",
        "review_snapshot_unchanged": True,
        "retained_snapshot": summarize(replay),
        "replay_without_source_files": True,
    }
    save_json(captures / "summary.json", summary)
    print(json.dumps(summary, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="Keep records and captures in a NEW directory")
    args = parser.parse_args()
    if os.name != "posix":
        parser.error("This writing exercise requires POSIX file locks (macOS or Linux).")
    if args.output:
        root = args.output.resolve()
        root.mkdir(parents=True, exist_ok=False)
        demonstrate(root)
    else:
        with tempfile.TemporaryDirectory(prefix="kpopper-dark-matter-query-") as directory:
            demonstrate(Path(directory))


if __name__ == "__main__":
    main()
