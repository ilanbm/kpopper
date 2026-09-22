"""Native lifecycle checks for stored booleans becoming derived booleans."""
import os
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import yaml


@unittest.skipUnless(os.environ.get('KPOP_EVOLUTION_NATIVE'), 'requires an explicit native executable')
class RepresentationEvolution(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.workspace = Path(self.temp.name)
        self.binary = os.environ.get('KPOP_EVOLUTION_NATIVE')
        self.env = {k: os.environ[k] for k in ('PATH', 'LANG', 'LC_ALL', 'TMPDIR') if k in os.environ}
        self.env.update(HOME=str(self.workspace), XDG_STATE_HOME=str(self.workspace / '.state'),
                        KPOPPER_NATIVE_CACHE=str(self.workspace / '.cache'))
        if os.environ.get('KPOP_EVOLUTION_RESOURCES'):
            self.env['KPOPPER_NATIVE_RESOURCES'] = os.environ['KPOP_EVOLUTION_RESOURCES']

    def command(self, *args):
        return subprocess.run([self.binary, '--workspace', str(self.workspace), *args],
                              cwd=self.workspace, env=self.env, capture_output=True,
                              text=True, timeout=60)

    def ok(self, *args):
        result = self.command(*args)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def test_new_boolean_rule_recomputes_after_numeric_input_changes(self):
        self.ok('add', 'tank.level', 'v=7', '--as-of', '2025-01-01')
        self.ok('add', 'tank.at_seven', 'rule={expr: "tank.level == 7"}', '--as-of', '2025-01-01')
        self.assertIn('true', self.ok('pull', 'tank.at_seven').lower())
        self.ok('set', 'tank.level', '8', '--as-of', '2025-01-02')
        self.assertIn('false', self.ok('pull', 'tank.at_seven').lower())
        self.ok('check')

    def test_stored_boolean_can_become_rule_without_losing_its_identity(self):
        self.ok('add', 'tank.at_seven', 'v=true', '--as-of', '2025-01-01')
        self.ok('add', 'tank.level', 'v=7', '--as-of', '2025-01-02')
        self.ok('add', 'tank.at_seven', 'rule={expr: "tank.level == 7"}',
                '--reframe', '--as-of', '2025-01-02', '--why', 'Represent the same proposition from the measured level')
        self.assertIn('true', self.ok('pull', 'tank.at_seven').lower())
        self.ok('set', 'tank.level', '8', '--as-of', '2025-01-03')
        self.assertIn('false', self.ok('pull', 'tank.at_seven').lower())
        self.ok('check')

    def test_reframe_preserves_sources_history_and_dependent_snapshots(self):
        source = self.workspace / 'inspection.txt'
        source.write_text('The measured level is seven.\n', encoding='utf-8')
        self.ok('add', 's.inspection', 'file=inspection.txt', 'read=2025-01-01', '--as-of', '2025-01-01')
        self.ok('add', 'tank.at_seven', 'v=true', 'from=s.inspection', 'at=line 1',
                'of=2025-01-01', '--as-of', '2025-01-01')
        self.ok('add', 'd.monitor', 'rests_on=[tank.at_seven]', 'verdict=The level is seven',
                'wrong_if={expr: "tank.at_seven == false"}', '--as-of', '2025-01-01')
        self.ok('add', 'tank.level', 'v=7', 'from=s.inspection', 'at=line 1', '--as-of', '2025-01-02')
        before = yaml.safe_load((self.workspace / 'GROUNDING.yaml').read_text())
        old_versions = {p: p.read_bytes() for p in (self.workspace / '.kpopper/history').rglob('*.yaml')}
        self.assertTrue(old_versions)
        self.ok('add', 'tank.at_seven', 'rule={expr: "tank.level == 7"}', '--reframe',
                '--why', 'Preserve the same proposition as a derived boolean', '--as-of', '2025-01-02')
        after = yaml.safe_load((self.workspace / 'GROUNDING.yaml').read_text())
        def entry(document, name):
            return next(section[name] for section in document.values()
                        if isinstance(section, dict) and name in section)
        self.assertEqual(entry(before, 'd.monitor'), entry(after, 'd.monitor'))
        self.assertEqual(entry(before, 's.inspection'), entry(after, 's.inspection'))
        self.assertNotIn('v', entry(after, 'tank.at_seven'))
        self.assertEqual(entry(after, 'tank.at_seven')['rule'], {'expr': 'tank.level == 7'})
        for path, data in old_versions.items():
            self.assertEqual(path.read_bytes(), data)
        self.ok('set', 'tank.level', '8', '--as-of', '2025-01-03')
        self.assertIn('false', self.ok('pull', 'tank.at_seven').lower())
        self.assertIn('d.monitor', self.ok('affects', 'tank.level'))
        check = self.command('check')
        self.assertNotEqual(check.returncode, 0, 'Dependent judgment must not be silently reviewed')
        self.assertIn('d.monitor', check.stdout + check.stderr)

    def test_invalid_reframes_leave_record_and_existing_history_unchanged(self):
        self.ok('add', 'tank.at_seven', 'v=true', '--as-of', '2025-01-01')
        self.ok('add', 'tank.level', 'v=7', '--as-of', '2025-01-02')
        record = self.workspace / 'GROUNDING.yaml'
        before = record.read_bytes()
        versions = {p: p.read_bytes() for p in (self.workspace / '.kpopper/history').rglob('*.yaml')}
        for rule in ('tank.level == 8', 'tank.level', 'missing.level == 7',
                     'tank.at_seven', 'true'):
            with self.subTest(rule=rule):
                result = self.command('add', 'tank.at_seven', f'rule={{expr: "{rule}"}}',
                                      '--reframe', '--why', 'Attempted representation change')
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(record.read_bytes(), before)
                for path, data in versions.items():
                    self.assertEqual(path.read_bytes(), data)

    def test_reframe_is_explicit_and_needs_a_reason(self):
        self.ok('add', 'tank.at_seven', 'v=true', '--as-of', '2025-01-01')
        self.ok('add', 'tank.level', 'v=7', '--as-of', '2025-01-02')
        before = (self.workspace / 'GROUNDING.yaml').read_bytes()
        for options in ([], ['--reframe'], ['--reframe', '--hypothesis', 'other', '--why', 'test']):
            result = self.command('add', 'tank.at_seven', 'rule={expr: "tank.level == 7"}', *options)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual((self.workspace / 'GROUNDING.yaml').read_bytes(), before)

    def test_legacy_record_is_not_silently_migrated(self):
        record = self.workspace / 'GROUNDING.yaml'
        record.write_text('known:\n  tank.at_seven: {v: true}\n  tank.level: {v: 7}\n', encoding='utf-8')
        before = record.read_bytes()
        result = self.command('add', 'tank.at_seven', 'rule={expr: "tank.level == 7"}',
                              '--reframe', '--why', 'Represent the same proposition')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('active core/v1 history', result.stdout + result.stderr)
        self.assertEqual(record.read_bytes(), before)
        self.assertFalse((self.workspace / '.kpopper/history.yaml').exists())

    def test_reframe_binds_to_the_callers_record_snapshot(self):
        self.ok('add', 'tank.at_seven', 'v=true', '--as-of', '2025-01-01')
        self.ok('add', 'tank.level', 'v=7', '--as-of', '2025-01-01')
        record = self.workspace / 'GROUNDING.yaml'
        stale = hashlib.sha256(record.read_bytes()).hexdigest()
        self.ok('set', 'tank.level', '8', '--as-of', '2025-01-02')
        self.ok('set', 'tank.at_seven', 'false', '--as-of', '2025-01-02')
        before = record.read_bytes()
        result = self.command('add', 'tank.at_seven', 'rule={expr: "tank.level == 7"}',
            '--reframe', '--why', 'The same proposition from the measured level',
            '--expected-record-sha256', stale)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('record changed since', result.stdout + result.stderr)
        self.assertEqual(record.read_bytes(), before)
        self.ok('add', 'tank.at_seven', 'rule={expr: "tank.level == 7"}',
                '--reframe', '--why', 'The same proposition from the measured level',
                '--expected-record-sha256', hashlib.sha256(before).hexdigest())
        self.assertIn('false', self.ok('pull', 'tank.at_seven').lower())
        self.ok('check')

    def test_missing_record_cannot_ignore_expected_snapshot(self):
        for extra in ([], ['--shareability', 'private']):
            result = self.command('add', 'tank.at_seven', 'v=true',
                '--expected-record-sha256', '0' * 64, *extra)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('active core/v1 history', result.stdout + result.stderr)
            self.assertFalse((self.workspace / 'GROUNDING.yaml').exists())


class PythonRepresentationEvolution(RepresentationEvolution):
    __unittest_skip__ = False

    def command(self, *args):
        script = Path(__file__).resolve().parents[1] / 'scripts/provenance.py'
        return subprocess.run([sys.executable, str(script), *args], cwd=self.workspace,
                              env=self.env, capture_output=True, text=True, timeout=60)


if __name__ == '__main__':
    unittest.main()
