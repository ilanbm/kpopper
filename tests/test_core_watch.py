"""Watch retains and consumes the captured core/v1 observation."""
import copy
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import yaml

from scripts import watch as W
from scripts.reasoning.snapshot import Snapshot


CORE = {
    'meta': {'reasoning': {'version': 2, 'profile': 'core/v1',
                           'requires': ['arithmetic/v1', 'composition/v1']}},
    'known': {'v.base': {'v': 10}, 'v.flag': {'v': False}},
    'judgments': {'d.bound': {'rests_on': ['v.base', 'v.flag'],
                              'seen': {'v.base': 10, 'v.flag': False},
                              'verdict': 'base is within the bound',
                              'wrong_if': {'op': 'or', 'args': [
                                  {'op': 'gt', 'args': [{'ref': 'v.base'}, {'num': '20'}]},
                                  {'op': 'eq', 'args': [{'ref': 'v.flag'}, {'bool': True}]},
                              ]}}},
}


class CoreWatchTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        root = Path(self.tmp.name)
        self.env = patch.dict('os.environ', {'XDG_STATE_HOME': str(root / 'state')})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.main, self.work = root / 'main', root / 'work'
        self.main.mkdir()
        self.git(self.main, 'init', '-b', 'main')
        self.git(self.main, 'config', 'user.email', 'fixture@example.test')
        self.git(self.main, 'config', 'user.name', 'Fixture')
        self.save(self.main, CORE)
        self.commit(self.main)
        self.git(self.main, 'worktree', 'add', '-b', 'experiment', str(self.work))
        main = copy.deepcopy(CORE)
        main['known']['v.extra'] = {'v': 1}
        self.save(self.main, main)
        self.commit(self.main)
        self.watch = W.Watch(self.work)
        self.watch.setup(base_ref='main')

    def git(self, cwd, *args):
        result = subprocess.run(['git', *args], cwd=cwd, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def commit(self, cwd):
        self.git(cwd, 'add', '.')
        self.git(cwd, 'commit', '-m', 'Fixture')

    def save(self, cwd, value):
        (cwd / 'GROUNDING.yaml').write_text(yaml.safe_dump(value, sort_keys=False), encoding='utf-8')

    def test_snapshot_contains_portable_core_evidence_and_replays(self):
        snapshot = self.watch.snapshot()
        self.assertIn('core', snapshot['working'])
        captured = Snapshot.from_json(snapshot['working']['core']['snapshot'])
        self.assertEqual(captured.snapshot_id, snapshot['working']['core']['assessment']['snapshot_id'])
        self.assertEqual(snapshot['working']['core']['assessment']['assessment_profile'], 'core/v1')
        self.assertEqual(W.compare(snapshot)['state'], 'clear')

    def test_changed_input_preserves_delta_and_reports_falsifier(self):
        changed = copy.deepcopy(CORE)
        changed['known']['v.flag']['v'] = True
        self.save(self.work, changed)
        result = W.compare(self.watch.snapshot())
        self.assertTrue(any(f['kind'] == 'falsified' and f['id'] == 'd.bound'
                            for f in result['findings']))
        self.assertEqual(result['changed'], ['v.flag'])

    def test_unknown_core_assessment_cannot_be_clear(self):
        changed = copy.deepcopy(CORE)
        changed['judgments']['d.bound']['rests_on'] = ['v.missing']
        self.save(self.work, changed)
        result = W.compare(self.watch.snapshot())
        self.assertEqual(result['state'], 'attention')
        self.assertTrue(any(f['kind'] == 'uncheckable' for f in result['findings']))


if __name__ == '__main__':
    unittest.main()
