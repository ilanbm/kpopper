"""Dependency reach preserves explanations and scales with the recorded edges."""
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "scripts"))
import provenance as P


class DependencyReach(unittest.TestCase):
    def test_mixed_paths_preserve_order_and_first_explanation(self):
        raw = {
            "input.count": {"v": 1},
            "work.total": {"rule": "input.count * 2", "v": "input.count + 1"},
            "work.after": {"v": "c.final + 1"},
            "other.value": {"v": 5},
            "other.quote": {"quoted": "input.count + 1"},
            "other.alias": {"v": "input.count"},
        }
        # Insertion order differs from the reporting order. The two branches converge.
        judgments = {
            "c.b": {"deps": ["work.total"]},
            "c.a": {"deps": ["work.total", "work.total"]},
            "c.final": {"deps": ["c.a", "c.b"]},
            "c.after": {"deps": ["work.after"]},
            "c.unrelated": {"deps": ["other.value"]},
        }
        hit, touched, derived = P.reach_of(set(raw) | set(judgments), judgments, raw,
                                           ["input.count"])
        self.assertEqual(list(hit.items()), [
            ("c.a", "work.total"), ("c.b", "work.total"),
            ("c.final", "c.b"), ("c.after", "work.after"),
        ])
        self.assertEqual(derived, ["work.total", "work.after"])
        self.assertEqual(touched, {"input.count", "work.total", "work.after", *hit})

    def test_seed_order_preserves_the_reported_cause(self):
        judgments = {"c.result": {"deps": ["input.a", "input.b"]}}
        ids = {"input.a", "input.b", *judgments}
        for seeds in (["input.a", "input.b"], ["input.b", "input.a"]):
            with self.subTest(seeds=seeds):
                self.assertEqual(P.reach_of(ids, judgments, {}, seeds),
                                 ({"c.result": seeds[-1]}, ids, []))

    def test_cycles_terminate_and_keep_their_reach(self):
        raw = {"input.value": {"v": 1}, "work.loop": {"rule": "c.b + 1"}}
        judgments = {"c.a": {"deps": ["input.value", "work.loop"]},
                     "c.b": {"deps": ["c.a"]}, "c.self": {"deps": ["c.self"]}}
        ids = set(raw) | set(judgments)
        self.assertEqual(P.reach_of(ids, judgments, raw, ["input.value"]),
                         ({"c.a": "input.value", "c.b": "c.a"},
                          {"input.value", "work.loop", "c.a", "c.b"}, ["work.loop"]))
        self.assertEqual(P.reach_of(ids, judgments, raw, ["c.self", "c.self"]),
                         ({"c.self": "c.self"}, {"c.self"}, []))

    def test_empty_and_unreferenced_seeds_reach_no_judgments(self):
        judgments = {"c.result": {"deps": ["input.a"]}}
        ids = {"input.a", "input.b", *judgments}
        self.assertEqual(P.reach_of(ids, judgments, {}, []), ({}, set(), []))
        self.assertEqual(P.reach_of(ids, judgments, {}, ["input.b"]),
                         ({}, {"input.b"}, []))

    def test_a_new_call_uses_changed_dependencies(self):
        raw = {"input.a": {"v": 1}, "input.b": {"v": 2},
               "work.total": {"rule": "input.a + 1"}}
        judgments = {"c.result": {"deps": ["work.total"]}}
        ids = set(raw) | set(judgments)
        self.assertIn("c.result", P.reach_of(ids, judgments, raw, ["input.a"])[0])
        raw["work.total"]["rule"] = "input.b + 1"
        self.assertEqual(P.reach_of(ids, judgments, raw, ["input.a"]),
                         ({}, {"input.a"}, []))
        judgments["c.result"]["deps"] = ["input.a"]
        self.assertEqual(P.reach_of(ids, judgments, raw, ["input.a"]),
                         ({"c.result": "input.a"}, {"input.a", "c.result"}, []))

    def test_dependency_reads_scale_with_edges_in_a_long_chain(self):
        # Count dependency inspections instead of asserting a machine-dependent duration.
        reads = [0]

        class Dependencies(list):
            def __iter__(self):
                for value in super().__iter__():
                    reads[0] += 1
                    yield value

            def __contains__(self, item):
                return any(value == item for value in self)

        size = 256
        judgments = {}
        previous = "input.value"
        for index in range(size):
            name = f"c.step{index:04d}"
            judgments[name] = {"deps": Dependencies([previous])}
            previous = name
        ids = {"input.value", *judgments}
        hit, touched, derived = P.reach_of(ids, judgments, {}, ["input.value"])
        self.assertEqual(len(hit), size)
        self.assertEqual(touched, ids)
        self.assertEqual(derived, [])
        self.assertLessEqual(reads[0], 4 * size,
                             "reach must not rescan every judgment at every step")


if __name__ == "__main__":
    unittest.main()
