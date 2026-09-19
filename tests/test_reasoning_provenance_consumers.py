"""Explicit record readers share one core assessment and leave legacy default alone."""
import contextlib
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock

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

    @unittest.skipUnless(os.name == 'posix', 'record writers require POSIX locks')
    def test_writer_cli_keeps_profile_for_action_dispatch(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'GROUNDING.yaml'
            path.write_text('known:\n  p.input: {v: 1}\n', encoding='utf-8')
            result = subprocess.run([
                sys.executable, str(Path(P.__file__)), 'add', 'p.new', 'v=2',
                '--profile', 'core/v1', str(path)], capture_output=True, text=True, check=False)
            written = path.read_text(encoding='utf-8')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('profile: core/v1', written)
        self.assertIn('p.new:', written)

    def test_transitive_affects_never_upgrades_a_potential_path(self):
        view = {'impacts': [
            {'from': 'x', 'to': 'mid', 'classification': 'potential', 'witnesses': []},
            {'from': 'mid', 'to': 'downstream', 'classification': 'executed', 'witnesses': []},
        ], 'findings_revision': 'f' * 64}
        context = SimpleNamespace(view=view, snapshot_id='s' * 64,
                                  findings_revision='f' * 64)
        output = io.StringIO()
        with mock.patch.object(P, '_core_context', return_value=context), \
                contextlib.redirect_stdout(output):
            self.assertEqual(P.core_affects(['unused'], ['x']), 0)
        self.assertIn('POTENTIAL mid', output.getvalue())
        self.assertIn('POTENTIAL downstream', output.getvalue())

    def test_core_pull_refuses_legacy_history_and_ignored_budget_options(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'GROUNDING.yaml'
            path.write_text(DOCUMENT, encoding='utf-8')
            base = [sys.executable, str(Path(P.__file__)), 'pull', 'd.ready',
                    '--profile', 'core/v1', str(path)]
            for option, expected in ((['--history'], '--history'),
                                     (['--budget', '1'], '--budget')):
                result = subprocess.run(base + option, capture_output=True, text=True, check=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('core_profile_option_unsupported: ' + expected,
                              result.stderr + result.stdout)

    def test_hub_checks_an_unresolved_selector_without_failing_the_core(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'GROUNDING.yaml'
            path.write_text(DOCUMENT, encoding='utf-8')
            view_dir = root / '.kpopper'
            view_dir.mkdir()
            (view_dir / 'view.yaml').write_text(
                'title: Missing\nsections:\n- title: Missing\n  pick: missing.prefix\n',
                encoding='utf-8')
            code, output = self.capture_output(P.core_check, [str(path)])
            hub = subprocess.run([sys.executable, str(Path(P.__file__).with_name('cli.py')),
                'experimental', 'hub', '--verify', str(path)], capture_output=True, text=True)
        self.assertEqual(code, 0, output)
        self.assertNotIn('page selectors unresolved', output)
        self.assertIn('page layout not checked; use kpop experimental hub --verify', output)
        self.assertEqual(hub.returncode, 1, hub.stdout + hub.stderr)
        self.assertIn('core page selectors unresolved: missing.prefix', hub.stdout)

    def test_hub_checks_shape_and_renderer_without_failing_the_core(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'GROUNDING.yaml'
            path.write_text(DOCUMENT, encoding='utf-8')
            view_dir = root / '.kpopper'
            view_dir.mkdir()
            (view_dir / 'view.yaml').write_text(
                'title: Stale\n'
                'shape: {entries: 999, judgments: 999, flagged: 999, blocked: 999}\n'
                'sections:\n- title: Bad fit\n  as: comparison\n  pick: all\n',
                encoding='utf-8')
            code, output = self.capture_output(P.core_check, [str(path)])
            hub = subprocess.run([sys.executable, str(Path(P.__file__).with_name('cli.py')),
                'experimental', 'hub', '--verify', str(path)], capture_output=True, text=True)
        self.assertEqual(code, 0, output)
        self.assertNotIn('page shape moved', output)
        self.assertEqual(hub.returncode, 1, hub.stdout + hub.stderr)
        self.assertIn('core page renderer does not fit', hub.stdout)
        self.assertIn('core page shape moved', hub.stdout)

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
