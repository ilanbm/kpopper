"""Explicit record readers share one core assessment and leave legacy default alone."""
import contextlib
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace

from scripts import provenance as P
from scripts.reasoning import assessment as V2
from scripts.reasoning.snapshot import Snapshot


DOCUMENT = '''meta:
  reasoning:
    version: 2
    profile: core/v1
    requires: [arithmetic/v1]
readings:
  p.input:
    v: 1
decisions:
  d.ready:
    rests_on: [p.input]
    seen:
      p.input: 1
    wrong_if: Supplier changes terms
'''


class CoreProvenanceConsumers(unittest.TestCase):
    def capture_output(self, function, *args):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = function(*args)
        return code, output.getvalue()

    def test_check_pull_and_affects_expose_the_same_context_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'GROUNDING.yaml'
            path.write_text(DOCUMENT, encoding='utf-8')
            paths = [str(path)]
            check_code, checked = self.capture_output(P.core_check, paths)
            pull_code, pulled = self.capture_output(P.core_pull, paths, ['d.ready'])
            affects_code, affected = self.capture_output(P.core_affects, paths, ['p.input'])
        self.assertEqual((check_code, pull_code, affects_code), (0, 0, 0))
        checked_identity = checked.split('core/v1 snapshot ', 1)[1].splitlines()[0]
        self.assertIn('"snapshot_id": "' + checked_identity.split('; findings ')[0], pulled)
        self.assertIn('"findings_revision": "' + checked_identity.split('; findings ')[1], pulled)
        self.assertIn('core/v1 snapshot ' + checked_identity, affected)
        self.assertIn('d.ready', affected)
        self.assertIn('falsifier unknown (declared prose)', checked)

    def test_capture_failure_has_no_findings(self):
        with tempfile.TemporaryDirectory() as directory:
            missing = [str(Path(directory) / 'missing.yaml')]
            code, output = self.capture_output(P.core_check, missing)
        self.assertEqual(code, 1)
        self.assertIn('no findings', output)

    def test_public_dispatch_requires_explicit_core_profile(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'GROUNDING.yaml'
            path.write_text(DOCUMENT, encoding='utf-8')
            command = [sys.executable, str(Path(P.__file__)), 'check',
                       '--profile', 'core/v1', str(path)]
            result = subprocess.run(command, capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('core/v1 snapshot ', result.stdout)

    def test_core_writer_summary_wraps_cached_v2_without_another_evaluation(self):
        snapshot = Snapshot.from_data({
            'meta': {'reasoning': {
                'version': 2, 'profile': 'core/v1', 'requires': ['arithmetic/v1']}},
            'decisions': {'d.ready': {'rests_on': [], 'wrong_if': 'terms change'}},
        })
        base = V2.assess(snapshot)

        class World:
            def __init__(self):
                self.snapshot = snapshot
                self.calls = 0

            def assessment(self):
                self.calls += 1
                return base

        world = World()
        raw = SimpleNamespace(world=world)
        code, output = self.capture_output(
            P._report, [], 'add', 'd.ready', {}, set(), {},
            {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}, raw)
        self.assertIsNone(code)
        self.assertEqual(world.calls, 1)
        self.assertIn('d.ready core/v1:', output)
        self.assertIn('snapshot ' + snapshot.snapshot_id + '; findings ', output)


if __name__ == '__main__':
    unittest.main()
