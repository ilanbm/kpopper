"""Conformance at the reviewed first-party reasoning-module boundary.

The compatibility tests are intentionally live before ``query/v1`` exists.  The
query-specific class is enabled by the presence of its explicit implementation
module; no other failure is converted into a skip.
"""
import copy
import hashlib
import importlib.util
from pathlib import Path
import unittest

from scripts.reasoning import modules
from scripts.reasoning import authoring
from scripts.reasoning.contract import CapabilityError
from scripts.reasoning.evaluate import Evaluator
from scripts.reasoning.snapshot import Snapshot
from scripts.reasoning.transport import encode_request


ROOT = Path(__file__).resolve().parents[1]
QUERY_READY = importlib.util.find_spec("scripts.reasoning.query") is not None


def _contains_executable(value):
    """Prepared envelopes are detached data, never deferred host behavior."""
    pending = [value]
    while pending:
        item = pending.pop()
        if callable(item) or isinstance(item, (Path, type)):
            return True
        if isinstance(item, dict):
            pending.extend(item.keys())
            pending.extend(item.values())
        elif isinstance(item, (list, tuple, set, frozenset)):
            pending.extend(item)
        elif item is not None and type(item) not in (str, bool, int, float):
            return True
    return False


def _base_document():
    return {
        "meta": {"reasoning": {
            "version": 2,
            "profile": "core/v1",
            "requires": ["arithmetic/v1", "composition/v1"],
        }},
        "parameters": {"p.a": {"v": 2}},
        "calculations": {"m.pair": {"rule": {
            "list": [{"ref": "p.a"}, {"num": "3"}],
        }}},
    }


def _query_document(*, add_nonmatch=False):
    requires = ["arithmetic/v1", "composition/v1", "query/v1"]
    items = {
        "item.a": {"enabled": True, "amount": 4},
        "item.b": {"enabled": False, "amount": 90},
    }
    if add_nonmatch:
        items["item.c"] = {"enabled": False, "amount": 12}
    return {
        "meta": {"reasoning": {
            "version": 2, "profile": "core/v1", "requires": requires,
        }},
        "items": items,
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


class ExistingExtensionCompatibility(unittest.TestCase):
    def test_scalar_and_composition_wire_bytes_remain_exact(self):
        scalar = {
            "nodes": {"p.a": {"num": "1.25"}},
            "expression": {"op": "add", "args": [
                {"ref": "p.a"}, {"num": "2"},
            ]},
            "declared": ["p.a"],
            "limits": {"steps": 7, "depth": 8, "digits": 9},
        }
        composition = {
            "protocol": "KP3",
            "nodes": {},
            "expression": {"if": {"bool": True},
                           "then": {"list": [{"num": "1"}, {"text": "é"}]},
                           "else": {"record": {}}},
            "declared": [],
            "limits": {"steps": 7, "depth": 8, "digits": 9,
                       "value_nodes": 10, "value_depth": 11, "value_bytes": 12},
        }
        self.assertEqual(
            encode_request(scalar),
            "KP2\t7\t8\t9\t1\t702e61\t1\t702e61\tn\t1.25\to\tadd\t2\tr\t702e61\tn\t2",
        )
        self.assertEqual(
            encode_request(composition),
            "KP3\t7\t8\t9\t10\t11\t12\t0\t0\ti\tb\t1\tl\t2\tn\t1\ts\tc3a9\tm\t0",
        )

    def test_scalar_reference_corpora_remain_byte_identical(self):
        expected = {
            "scalar-v1.json": "2fe601355605a505df8d5b1237dc82dd8f1ad8abc6f2b8f621b13d76fed4219f",
            "t0-scalar-slice.json": "a161751421380c022578ced6c5857957021dddf4f801da0a0b3ec70e547df45f",
        }
        for name, digest in expected.items():
            with self.subTest(name=name):
                content = (ROOT / "tests" / "reasoning" / name).read_bytes()
                self.assertEqual(hashlib.sha256(content).hexdigest(), digest)

    def test_registry_is_static_first_party_and_fails_closed(self):
        self.assertTrue({"arithmetic/v1", "composition/v1"} <= set(modules.REGISTRY))
        self.assertEqual(set(modules.REGISTRY) - {"query/v1"},
                         {"arithmetic/v1", "composition/v1"})
        if "query/v1" in modules.REGISTRY:
            self.assertTrue(QUERY_READY)
        with self.assertRaisesRegex(CapabilityError, "module is not installed"):
            modules.module("record.supplied/module")
        source = Path(modules.__file__).read_text(encoding="utf-8")
        for dynamic_loader in ("import_module(", "spec_from_file_location(", "run_path(",
                               "entry_points(", "__import__("):
            with self.subTest(loader=dynamic_loader):
                self.assertNotIn(dynamic_loader, source)

    def test_existing_prepared_requests_are_detached_data(self):
        snapshot = Snapshot.from_data(_base_document())
        request, result = Evaluator(snapshot).prepare({"ref": "m.pair"}, ["m.pair"])
        self.assertFalse(_contains_executable(request))
        self.assertFalse(_contains_executable(result))
        before = snapshot.to_data()
        request["nodes"]["m.pair"] = {"null": True}
        result["potential_dependencies"].clear()
        self.assertEqual(snapshot.to_data(), before)


@unittest.skipUnless(QUERY_READY, "query/v1 implementation module is absent")
class QueryExtensionContract(unittest.TestCase):
    def prepared(self, *, add_nonmatch=False):
        snapshot = Snapshot.from_data(_query_document(add_nonmatch=add_nonmatch))
        operation = copy.deepcopy(
            snapshot.to_data()["nodes"]["m.enabled"]["body"]["rule"])
        request, result = Evaluator(snapshot).prepare(operation, ["scope.items"])
        return snapshot, operation, request, result

    def test_query_is_a_static_registered_module_with_a_data_only_envelope(self):
        self.assertIs(modules.module("query/v1"), modules.REGISTRY["query/v1"])
        snapshot, operation, request, result = self.prepared()
        self.assertFalse(_contains_executable(request))
        self.assertFalse(_contains_executable(result))
        self.assertIn("query/v1", result["modules"])
        self.assertEqual(result["snapshot_id"], snapshot.snapshot_id)
        self.assertIsNone(result["basis"])
        self.assertIsNone(result["computation_id"])
        self.assertEqual(request["basis_template"]["recipe"], "query-inputs/v1")
        self.assertEqual(request["basis_template"]["operation"], operation)
        self.assertEqual(result["resource_profile"]["version"], "resources/v4")
        self.assertEqual(result["potential_ids"], ["scope.items"])
        self.assertEqual([item["kind"] for item in result["potential_dependencies"]],
                         ["scope"])

    def test_query_preparation_returns_the_closed_reviewed_envelope(self):
        from scripts.reasoning import query

        snapshot = Snapshot.from_data(_query_document())
        capture = snapshot.capture_scope("scope.items")
        authored = copy.deepcopy(
            snapshot.to_data()["nodes"]["m.enabled"]["body"]["rule"])
        prepared = query.prepare(
            capture,
            authored,
            request_id="0" * 64,
            declared_capabilities=["arithmetic/v1", "composition/v1", "query/v1"],
        )
        self.assertEqual(set(prepared), {
            "version", "module", "protocol", "resources", "normalized_operation",
            "required_modules", "potential_dependencies", "potential_ids",
            "basis_template", "request",
        })
        self.assertEqual((prepared["version"], prepared["module"], prepared["protocol"]),
                         (1, "query/v1", "KP4"))
        self.assertEqual(prepared["resources"], "resources/v4")
        self.assertEqual(prepared["request"]["resources"]["version"], "resources/v4")
        self.assertEqual(prepared["normalized_operation"], authored)
        self.assertEqual(prepared["potential_dependencies"], [capture.witness])
        self.assertEqual(prepared["potential_ids"], ["scope.items"])
        self.assertEqual(prepared["basis_template"]["recipe"], "query-inputs/v1")
        self.assertEqual(prepared["basis_template"]["operation"],
                         prepared["normalized_operation"])
        self.assertEqual(prepared["basis_template"]["scope"], capture.basis)
        self.assertFalse(_contains_executable(prepared))

    def test_authoring_infers_query_capability_from_the_stored_scope_rule(self):
        document = _query_document()
        document['meta']['reasoning']['requires'] = ['arithmetic/v1']
        declared = authoring.declaration(document)
        self.assertEqual(declared['requires'], ['arithmetic/v1', 'query/v1'])
        with self.assertRaisesRegex(CapabilityError, 'query/v1'):
            authoring.validate_declared(document)

    def test_query_root_is_only_launchable_as_the_exact_stored_root(self):
        snapshot = Snapshot.from_data(_query_document())
        request, result = Evaluator(snapshot).prepare(
            {'op': 'add', 'args': [{'ref': 'm.enabled'}, {'num': '1'}]},
            ['m.enabled'])
        self.assertIsNone(request)
        self.assertEqual(result['status'], 'error')
        self.assertEqual([item['code'] for item in result['diagnostics']],
                         ['invalid_expression'])

    def test_membership_invalidation_is_carried_only_by_the_scope_witness(self):
        _, operation, request, before = self.prepared()
        _, changed_operation, changed_request, after = self.prepared(add_nonmatch=True)
        self.assertEqual(operation, changed_operation)
        self.assertEqual(before["value"], after["value"])
        self.assertNotEqual(request["basis_template"]["digest"],
                            changed_request["basis_template"]["digest"])
        self.assertIsNone(before["basis"])
        self.assertIsNone(after["basis"])
        self.assertIsNone(before["computation_id"])
        self.assertIsNone(after["computation_id"])
        self.assertNotEqual(before["potential_dependencies"], after["potential_dependencies"])
        self.assertEqual(before["potential_ids"], after["potential_ids"])

        # The native request is allowed only the captured scope rows.  It may not
        # regain ambient access to the Snapshot or turn member IDs into grants.
        self.assertFalse(_contains_executable(request))
        self.assertFalse(_contains_executable(changed_request))
        self.assertFalse({"item.a", "item.b", "item.c"} & set(after["potential_ids"]))


if __name__ == "__main__":
    unittest.main()
