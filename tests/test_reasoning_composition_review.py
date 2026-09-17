"""Independent executable regressions for the composition protocol."""
import os
import subprocess
import unittest

from scripts.reasoning.transport import decode_response, encode_request


BINARY = os.environ.get("KPOPPER_REASONING_TEST_BINARY")


@unittest.skipUnless(BINARY, "set KPOPPER_REASONING_TEST_BINARY for native conformance")
class CompositionReviewTests(unittest.TestCase):
    def evaluate(self, expression, nodes, declared=None):
        request = {
            "protocol": "KP3",
            "nodes": nodes,
            "declared": list(nodes) if declared is None else declared,
            "expression": expression,
            "limits": {},
        }
        payload = (encode_request(request) + "\n").encode("ascii")
        run = subprocess.run([BINARY], input=payload, capture_output=True,
                             timeout=30, check=True)
        self.assertEqual(run.stderr, b"")
        return decode_response(run.stdout.decode("ascii").strip())

    def test_long_unselected_potential_cycle_is_reported_as_a_cycle(self):
        count = 128
        nodes = {f"x{index}": {"ref": f"x{(index + 1) % count}"}
                 for index in range(count)}
        result = self.evaluate({
            "if": {"bool": True},
            "then": {"num": "1"},
            "else": {"ref": "x0"},
        }, nodes)
        self.assertEqual((result["status"], result["diagnostics"], result["steps"]),
                         ("error", ["cyclic_reference"], 0))

    def test_long_cycles_depth_limits_and_fan_in_remain_distinct(self):
        count = 200
        cycle = {f"x{index}": {"ref": f"x{(index + 1) % count}"}
                 for index in range(count)}
        cycle["witness"] = {"num": "2"}
        result = self.evaluate({"list": [{"ref": "x0"}, {"ref": "witness"}]}, cycle)
        self.assertEqual((result["status"], result["diagnostics"], result["steps"]),
                         ("error", ["cyclic_reference"], 0))
        self.assertEqual(set(result["potential_reads"]), set(cycle))

        chain = {f"x{index}": ({"ref": f"x{index + 1}"} if index + 1 < count
                                else {"num": "1"})
                 for index in range(count)}
        result = self.evaluate({"ref": "x0"}, chain)
        self.assertEqual((result["status"], result["diagnostics"], result["steps"]),
                         ("limit", ["depth_limit"], 0))
        self.assertEqual(set(result["potential_reads"]), set(chain))

        dag = {
            "left": {"ref": "join"},
            "right": {"ref": "join"},
            "join": {"num": "3"},
        }
        result = self.evaluate({"list": [{"ref": "left"}, {"ref": "right"}]}, dag)
        self.assertEqual((result["status"], result["diagnostics"]), ("ok", []))
        self.assertEqual(result["potential_reads"], ["join", "left", "right"])


if __name__ == "__main__":
    unittest.main()
