#!/usr/bin/env python3
"""Generate the independent history-group oracle for the native conformance test."""

import argparse
import copy
import json
import sys
from pathlib import Path


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--oracle-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-transaction-fixture", type=Path, required=True)
    parser.add_argument(
        "--fixture-root",
        default="/tmp/kpop-native-group-fixture",
        help="Absolute virtual root used in generated member paths.",
    )
    return parser.parse_args()


def main() -> None:
    args = arguments()
    sys.path.insert(0, str(args.oracle_root))
    from scripts import history_contract as C
    from scripts import history_group_activation as G
    from scripts import history_transaction as T
    from scripts.pending_grounding import _encode, identity, json_bytes

    # Transaction member names are portable POSIX paths. Python 1.8's layout
    # helper emits host separators on Windows even for the synthetic '/' root
    # used by its detached validator. Adapt only that pure layout observation;
    # group entries still use the real host's Path.resolve and all validation,
    # receipts, blobs and hashes come from the pinned Python implementation.
    layout_adapter = None
    if sys.platform == "win32":
        from scripts import provenance
        original_layout = provenance.layout

        def portable_layout(path):
            return {
                key: value.replace(chr(92), "/") if isinstance(value, str) else value
                for key, value in original_layout(path).items()
            }

        provenance.layout = portable_layout
        layout_adapter = "synthetic-posix-layout-separators/v1"

    source = json.loads(args.source_transaction_fixture.read_text(encoding="utf-8"))
    cases = []

    def rawdata(data):
        return json_bytes(_encode(data))

    def keep(name, raw):
        case = {"name": name, "raw": raw.decode()}
        try:
            group = G.GroupPrepared.from_bytes(raw)
            case["output"] = _encode(group.to_data())
            case["canonical"] = group.to_bytes().decode()
        except C.HistoryError as error:
            case["error"] = error.code
        cases.append(case)

    def make(mode="activate", count=2):
        raw = next(
            case["raw"]
            for case in source["cases"]
            if case["name"] == f"GROUNDING.yaml-{mode}"
        )
        original = T.PreparedMutation.from_bytes(raw.encode())
        data = original.to_data()
        mutations = {}
        entries = [
            str(
                (Path(args.fixture_root) / f"member-{index}" / "GROUNDING.yaml")
                .resolve()
            )
            for index in range(count)
        ]
        group = {"version": 1, "operation": "group", "entries": entries}
        for entry in entries:
            baseline = {
                **data["baseline"],
                "transaction_root": str(Path(entry).parent),
                "group": group,
                "deployment": {
                    "inventory": {"native": "fixture"},
                    "expected_digests": {"source": "a" * 64},
                },
            }
            mutations[entry] = T.PreparedMutation(
                operation=data["operation"],
                authority=data["authority"],
                baseline=baseline,
                files=original.files,
                receipt=data["receipt"],
                entry="GROUNDING.yaml",
                transition=data["transition"],
            )
        return G.GroupPrepared(
            operation="group",
            entries=entries,
            mutations=mutations,
            inventory={"native": "fixture"},
            expected_digests={"source": "a" * 64},
        )

    for mode in ("activate", "deactivate"):
        for count in (2, 3):
            keep(f"{mode}-{count}", make(mode, count).to_bytes())
    group = make()
    original = group.to_data()
    for key in original:
        data = copy.deepcopy(original)
        del data[key]
        keep(f"missing-{key}", rawdata(data))
        for bad in (None, True, 0, "bad", [], {}):
            data = copy.deepcopy(original)
            data[key] = bad
            data["digest"] = identity({k: v for k, v in data.items() if k != "digest"})
            keep(f"{key}-{bad!r}", rawdata(data))
    keep("trailing-newline", group.to_bytes() + b"\n")
    for field, bad in (("inventory", {}), ("expected_digests", {}), ("direction", "deactivate")):
        data = copy.deepcopy(original)
        data[field] = bad
        data["digest"] = identity({k: v for k, v in data.items() if k != "digest"})
        keep(f"different-{field}", rawdata(data))

    args.output.parent.mkdir(parents=True, exist_ok=True)
    reference = args.source_transaction_fixture.with_name("history-group.json")
    if reference.is_file():
        old = json.loads(reference.read_text(encoding="utf-8"))["cases"]
        categories = lambda values: {case["name"]: ("output" in case, case.get("error")) for case in values}
        if categories(old) != categories(cases):
            raise RuntimeError("platform group oracle changed the source cases or refusal categories")
    args.output.write_text(
        json.dumps({"oracle": args.oracle_root.name, "layout_adapter": layout_adapter, "cases": cases}, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"group cases {len(cases)}")


if __name__ == "__main__":
    main()
