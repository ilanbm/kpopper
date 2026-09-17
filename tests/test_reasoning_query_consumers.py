"""Generic-consumer conformance for the additive ``query/v1`` module.

Leaf-consumer independence is enforced immediately.  The end-to-end query
matrix becomes active only when the explicit implementation module exists.
"""
import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import yaml

from scripts import documents as D
from scripts import export_graph as E
from scripts import render_page as R
from scripts import search as S
from scripts.reasoning import assessment as V2
from scripts.reasoning import history_assessment as V3
from scripts.reasoning.contract import digest
from scripts.reasoning.context import CapturedAssessment
from scripts.reasoning.snapshot import Snapshot
from scripts.session import view as SESSION


ROOT = Path(__file__).resolve().parents[1]
QUERY_READY = importlib.util.find_spec("scripts.reasoning.query") is not None
LEAF_CONSUMERS = (
    ROOT / "scripts" / "export_graph.py",
    ROOT / "scripts" / "search.py",
    ROOT / "scripts" / "session" / "view.py",
    ROOT / "scripts" / "render_page.py",
    ROOT / "scripts" / "documents.py",
    ROOT / "scripts" / "document_html.py",
)


def document():
    return {
        "meta": {"name": "Query consumer fixture", "reasoning": {
            "version": 2,
            "profile": "core/v1",
            "requires": ["arithmetic/v1", "composition/v1", "query/v1"],
        }},
        "sources": {"s.fixture": {
            "name": "Fixture evidence", "file": "evidence.md", "read": "2026-09-18",
        }},
        "items": {
            "item.a": {"enabled": True, "amount": 4, "v": 4, "from": "s.fixture",
                       "at": "row a", "of": "2026-09-18"},
            "item.b": {"enabled": False, "amount": 90, "from": "s.fixture",
                       "at": "row b", "of": "2026-09-18"},
        },
        "scopes": {"scope.items": {"collection_scope": {
            "collection": "items", "fields": ["amount", "enabled"],
        }}},
        "calculations": {"m.enabled": {"rule": {"query": {
            "version": 1,
            "scope": "scope.items",
            "op": "count",
            "where": {"op": "eq", "args": [
                {"column": "enabled"}, {"bool": True},
            ]},
        }}}},
    }


class LeafConsumerIndependence(unittest.TestCase):
    def test_leaf_consumers_neither_import_nor_branch_on_query_v1(self):
        for path in LEAF_CONSUMERS:
            source = path.read_text(encoding="utf-8")
            with self.subTest(path=path):
                self.assertNotIn("reasoning.query", source)
                self.assertNotIn("query/v1", source)

    def test_generic_projection_already_accepts_scope_witnesses(self):
        computation = {
            "potential_dependencies": [{
                "kind": "scope", "scope_id": "scope.items",
                "definition_digest": "a" * 64,
                "membership_digest": "b" * 64,
                "projected_inputs_digest": "c" * 64,
            }],
            "potential_ids": ["scope.items"],
            "executed_reads": [{
                "kind": "scope", "scope_id": "scope.items",
                "definition_digest": "a" * 64,
                "membership_digest": "b" * 64,
                "projected_inputs_digest": "c" * 64,
            }],
        }
        from scripts.reasoning.projection import project_witnesses
        projected = project_witnesses(computation)
        self.assertEqual(projected, {
            "potential": ["scope.items"],
            "executed": ["scope.items"],
            "unexecuted": [],
            "dependencies": [{"id": "scope.items", "classification": "executed"}],
        })

    def test_scope_membership_witness_cannot_be_omitted_from_potential_reads(self):
        base = document()
        base["meta"]["reasoning"]["requires"] = ["arithmetic/v1", "composition/v1"]
        del base["calculations"]
        # The shared scope capture gives this mutation test a real definition,
        # membership and projected-input identity without requiring query/v1.
        before = Snapshot.from_data(base)
        result = V2.dependency_result(before, "scope.items")
        after_document = copy.deepcopy(base)
        after_document["items"]["item.c"] = {
            "enabled": False, "amount": 12, "from": "s.fixture",
        }
        changed = Snapshot.from_data(after_document).capture_scope("scope.items").witness
        self.assertNotEqual(result["potential_dependencies"], [changed])

        forged = copy.deepcopy(result)
        forged["executed_reads"] = [changed]
        forged["potential_dependencies"] = []
        with self.assertRaisesRegex(ValueError, "executed reads exceed potential"):
            V3._validate_result(forged, before.snapshot_id)


@unittest.skipUnless(QUERY_READY, "query/v1 implementation module is absent")
class QueryConsumerConformance(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.record = self.root / "GROUNDING.yaml"
        self.record.write_text(yaml.safe_dump(document(), sort_keys=False), encoding="utf-8")
        (self.root / "evidence.md").write_text("query consumer evidence\n", encoding="utf-8")
        # Capture the authored revision as the real consumers do; search binds
        # its secondary corpus to those exact file observations.
        self.snapshot = Snapshot.capture([str(self.record)], read_mode="frozen")
        self.v2 = V2.assess(self.snapshot)
        self.v3 = V3.from_v2(self.snapshot, self.v2)
        self.context = CapturedAssessment(self.snapshot, self.v3)

    def test_v2_v3_and_context_share_query_result_and_scope_impact(self):
        self.assertEqual(self.v2["snapshot_id"], self.snapshot.snapshot_id)
        self.assertEqual(self.v3["snapshot_id"], self.snapshot.snapshot_id)
        self.assertEqual(self.context.snapshot_id, self.snapshot.snapshot_id)
        self.assertEqual(self.context.findings_revision, self.v3["findings_revision"])
        for report in (self.v2, self.v3):
            computation = report["nodes"]["m.enabled"]["computation"]
            self.assertEqual(computation["status"], "ok")
            self.assertEqual(computation["value"]["fields"]["result"], {
                "type": "number", "numerator": "1", "denominator": "1",
            })
            self.assertIn("query/v1", computation["modules"])
        impacts = self.context.view["impacts"]
        self.assertIn(("scope.items", "m.enabled", "executed"), {
            (edge["from"], edge["to"], edge["classification"]) for edge in impacts
        })

    def test_export_search_session_page_and_document_keep_one_identity(self):
        with mock.patch.object(CapturedAssessment, "capture", return_value=self.context):
            exported = E.project([str(self.record)], ["m.enabled"], depth=1,
                                 profile="core/v1")
            searched = S.search("enabled", record=str(self.record), profile="core/v1")

        revision = self.context.session_revision({"project": "fixture"})
        session = SESSION._core_graph_data(
            self.context, revision, {"project": "fixture"})
        _, _, _, _, page = R.core_build_from_context(
            self.context,
            b"title: Query\nsections:\n- title: Query\n  pick: all\n",
        )

        key = D.selection_key({"source": "record", "pointer": "/item.a/v"})
        inputs = {key: {"source": "record", "pointer": "/item.a/v"}}
        with mock.patch.object(Snapshot, "capture", return_value=self.snapshot):
            _, document_receipt = D._core_record_source(
                self.record, inputs, "2026-09-18T00:00:00Z")

        identities = [
            (exported["snapshot_id"], exported["findings_revision"]),
            (searched["snapshot_id"], searched["findings_revision"]),
            (session["core_session"]["snapshot_id"],
             session["core_session"]["findings_revision"]),
            (page["snapshot_id"], page["findings_revision"]),
            (document_receipt["snapshot_id"], document_receipt["findings_revision"]),
        ]
        self.assertEqual(identities, [
            (self.context.snapshot_id, self.context.findings_revision),
        ] * len(identities))

        self.assertIn(("m.enabled", "impact", "scope.items"), exported["edges"])
        search_hit = next(row for row in searched["results"] if row["id"] == "m.enabled")
        self.assertIn("scope.items", search_hit["rule_dependencies"])
        self.assertIn({"from": "m.enabled", "rel": "rests_on", "to": "scope.items"},
                      session["edges"])

    def test_omitting_scope_witness_invalidates_the_query_computation(self):
        forged = copy.deepcopy(self.v2)
        computation = forged["nodes"]["m.enabled"]["computation"]
        computation["potential_dependencies"] = [
            witness for witness in computation["potential_dependencies"]
            if witness.get("kind") != "scope"
        ]
        forged['assessment_revision'] = digest({
            key: value for key, value in forged.items() if key != 'assessment_revision'})
        with self.assertRaisesRegex(ValueError, "witness|potential|scope|dependency"):
            V3.from_v2(self.snapshot, forged)


if __name__ == "__main__":
    unittest.main()
