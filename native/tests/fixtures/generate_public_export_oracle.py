#!/usr/bin/env python3
"""Regenerate public-export parity fixtures from the frozen Python 1.8 oracle."""
import json
import sys
from pathlib import Path


if len(sys.argv) != 3:
    raise SystemExit("usage: generate_public_export_oracle.py BASELINE OUTPUT")

baseline = Path(sys.argv[1]).resolve()
output = Path(sys.argv[2]).resolve()
sys.path.insert(0, str(baseline))

from scripts import export_graph as export  # noqa: E402
from scripts.reasoning.context import CapturedAssessment  # noqa: E402
from scripts.reasoning.snapshot import Snapshot  # noqa: E402

source = json.loads((output.parent / "reasoning-context.json").read_text())
supplied = {case["name"]: case["serialized"] for case in source["contexts"]}


def captured(document):
    snapshot = Snapshot.from_data(
        document,
        context={"read_mode": "supplied", "source_collection": "caller-owned"},
        as_of="2026-09-19",
    )
    return CapturedAssessment.from_snapshot(snapshot)


custom = {
    "unicode-cycle": captured({
        "meta": {"reasoning": {"profile": "core/v1", "version": 2, "requires": []}},
        "readings": {
            "a|[`<>&#!": {
                "from": "b",
                "unit": "ק\u202eג",
                "v": "שלום 日本語\n<script> ``` [x](javascript:bad) #38;",
            },
            "b": {"from": "a|[`<>&#!", "v": "second"},
        },
    }),
}

cap_nodes = {"n00": {"v": 1}}
for index in range(1, 32):
    cap_nodes[f"n{index:02}"] = {
        "rule": {"expr": " + ".join(f"n{prior:02}" for prior in range(index))}
    }
custom["dense-cap"] = captured({
    "meta": {
        "reasoning": {
            "profile": "core/v1",
            "version": 2,
            "requires": ["arithmetic/v1"],
        }
    },
    "readings": cap_nodes,
})


class LiveOrder:
    """A context read back from its data holds every map in name order, while
    `kpop export` captures the record live and reads each judgment's
    dependencies in the order the judgment declares them."""

    def __init__(self, context):
        self._context = context

    def __getattr__(self, name):
        return getattr(self._context, name)

    @property
    def assessment(self):
        report = self._context.assessment
        for node in report["nodes"].values():
            body = node["body"] if isinstance(node["body"], dict) else {}
            declared = body.get(node["fields"]["deps"])
            readings = node["state"]["basis"]["dependencies"]
            if not isinstance(declared, list):
                continue
            order = [dep for dep in dict.fromkeys(declared) if isinstance(dep, str) and dep in readings]
            order += [dep for dep in readings if dep not in order]
            node["state"]["basis"]["dependencies"] = {dep: readings[dep] for dep in order}
        return report


def context(name):
    if name in custom:
        return custom[name]
    return LiveOrder(CapturedAssessment.from_data(supplied[name]))


original_core_modules = export._core_modules


def oracle_case(name, context_name, seeds, direction="support", depth=1, max_nodes=12):
    value = context(context_name)

    class BoundContext:
        @staticmethod
        def capture(_paths):
            return value

    _, render_expression, render_value = original_core_modules()
    export._core_modules = lambda: (BoundContext, render_expression, render_value)
    try:
        packet = export._core_project([], seeds, direction, depth, max_nodes)
    finally:
        export._core_modules = original_core_modules
    markdown = export.render_markdown(packet, False)
    details = export.render_markdown(packet, True)
    mermaid = export.render_mermaid(packet)
    combined = (
        markdown
        + "\n### Optional Mermaid diagram\n\n"
        + "Requires a Mermaid renderer; otherwise this is a code block. "
        + "The text above is the readable representation.\n\n```mermaid\n"
        + mermaid
        + "```\n"
    )
    return {
        "name": name,
        "context": context_name,
        "seeds": seeds,
        "direction": direction,
        "depth": depth,
        "max_nodes": max_nodes,
        "packet": packet,
        "markdown": markdown,
        "markdown_details": details,
        "mermaid": mermaid,
        "markdown_mermaid": combined,
    }


cases = [
    oracle_case("normal-support", "all-None", ["d.a"], depth=2, max_nodes=4),
    oracle_case("impact-frontier", "all-None", ["p.a"], direction="impact", depth=1, max_nodes=2),
    oracle_case("missing-dependency", "rests_on-['missing']-None", ["d.a"], depth=1),
    oracle_case("review-changed", "seen-{'p.b': 4}-None", ["d.a"], depth=1),
    oracle_case("premise-row-limit", "authored-dependency-order-None", ["d.order"], depth=1, max_nodes=32),
    oracle_case("potential-executed", "unknown-query-None", ["q.count"], depth=2),
    oracle_case("duplicate-seed", "all-None", ["d.a", "d.a", "p.b"], depth=1),
    oracle_case("unicode-malicious-cycle", "unicode-cycle", ["a|[`<>&#!"], depth=4),
    oracle_case("internal-edge-cap", "dense-cap", ["n31"], depth=4, max_nodes=32),
]

payload = {
    "oracle": "baseline-f480ea6/scripts/export_graph.py",
    "custom_contexts": {name: value.to_data() for name, value in custom.items()},
    "cases": cases,
}
output.write_text(json.dumps(payload, ensure_ascii=False, separators=(",", ":")) + "\n")
