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
    args.output.write_text(
        json.dumps({"oracle": args.oracle_root.name, "cases": cases}, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"group cases {len(cases)}")


if __name__ == "__main__":
    main()
