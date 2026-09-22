"""Lifecycle diagnostics are context, never new prompts or replacement tool results."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import unittest

from tests import test_session_activity as activity_tests

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(os.name == 'posix', 'POSIX shell hooks; native hooks cover Windows')
class HookDelivery(unittest.TestCase):
    def setUp(self):
        self.fixture = activity_tests.SessionActivity()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)

    def call(self, *args):
        f = self.fixture
        return subprocess.run(['sh', str(ROOT / 'scripts/session_gate.sh'), *args],
            input=json.dumps({'session_id': f.sid, 'cwd': str(f.work),
                             'hook_event_name': 'UserPromptSubmit', 'prompt': 'Which version won?'}),
            cwd=f.work, env=f.env, text=True, capture_output=True, timeout=30)

    def assert_context(self, result, fragment):
        self.assertEqual((result.returncode, result.stderr), (0, ''))
        value = json.loads(result.stdout)
        self.assertEqual(set(value), {'hookSpecificOutput'})
        body = value['hookSpecificOutput']
        self.assertEqual(set(body), {'hookEventName', 'additionalContext'})
        self.assertEqual(body['hookEventName'], 'UserPromptSubmit')
        self.assertIn(fragment, body['additionalContext'])
        self.assertIn("Complete the user's current request", body['additionalContext'])

    def test_missing_intent_is_silent_at_stop_and_context_on_the_next_prompt(self):
        self.fixture.add()
        for _ in range(2):
            result = self.call('--host', 'codex')
            self.assertEqual((result.returncode, result.stdout, result.stderr), (0, '', ''))
        self.assert_context(self.call('--host', 'codex', '--context', 'UserPromptSubmit'), 'p.new')
        self.assertEqual(self.call('--context', 'UserPromptSubmit').stdout, '')

    def test_structural_failures_are_context_but_explicit_checks_still_fail(self):
        f = self.fixture
        f.doc['schema'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        f.doc['judgments'] = {'d.bad': {'verdict': 'bad', 'rests_on': ['p.missing'], 'seen': {}}}
        f.save()
        self.assertEqual(self.call().stdout, '')
        self.assert_context(self.call('--context', 'UserPromptSubmit'), 'd.bad')
        self.assertEqual(f.gate()[0], 2)
        check = subprocess.run([sys.executable, str(ROOT / 'scripts/provenance.py'), 'check'],
            cwd=f.work, env=f.env, capture_output=True, text=True)
        self.assertNotEqual(check.returncode, 0)
        self.assertIn('d.bad', check.stdout)

    def test_failed_assessment_cannot_block_or_replace_the_request(self):
        self.fixture.record.write_text('not: [valid yaml')
        self.assertEqual(self.call().returncode, 0)
        self.assert_context(self.call('--context', 'UserPromptSubmit'), 'unavailable')

    def test_native_default_dispatches_context_and_never_forwards_child_exit_two(self):
        from tests.test_native_distribution import host_target
        target = host_target()
        if target is None:
            self.skipTest('native package target unavailable')
        f = self.fixture
        scripts = f.root / 'native-plugin' / 'scripts'
        scripts.mkdir(parents=True)
        for name in ('hook.sh', 'session_gate.sh', 'native_runtime.sh'):
            shutil.copy2(ROOT / 'scripts' / name, scripts / name)
        binary = scripts / 'runtime' / target / 'kpop'
        binary.parent.mkdir(parents=True)
        trace = f.root / 'native-args.txt'
        binary.write_text('#!/bin/sh\nprintf "%s\\n" "$*" > "$HOOK_TRACE"\nprintf native-context\nexit 2\n')
        binary.chmod(0o755)
        env = dict(f.env, KPOPPER_RUNTIME='rust', HOOK_TRACE=str(trace))
        def invoke(name, *args):
            return subprocess.run(['sh', str(scripts / name), *args], input='{}',
                cwd=f.work, env=env, text=True, capture_output=True, timeout=10)
        stopped = invoke('session_gate.sh', '--host', 'codex')
        self.assertEqual((stopped.returncode, stopped.stdout, stopped.stderr), (0, '', ''))
        self.assertFalse(trace.exists())
        context = invoke('session_gate.sh', '--host', 'codex', '--context', 'UserPromptSubmit')
        self.assertEqual(context.returncode, 0)
        self.assertEqual(trace.read_text().strip(), 'session-context --event UserPromptSubmit --host codex')
        launched = invoke('hook.sh', 'watch_hook.py', 'claude', 'wait')
        self.assertEqual(launched.returncode, 0)
        self.assertEqual(trace.read_text().strip(), '_hook watch claude wait')

    def test_manifests_do_not_register_stop_or_automatic_rewake(self):
        for rel in ('hooks/hooks.json', 'adapters/codex/plugin-hooks.json',
                    'adapters/codex/hooks.json', 'adapters/copilot/vscode/hooks.json',
                    'adapters/cursor/hooks.json', 'adapters/gemini/hooks/hooks.json',
                    'adapters/windsurf/hooks.json'):
            with self.subTest(rel=rel):
                value = json.loads((ROOT / rel).read_text())
                self.assertFalse({'Stop', 'stop', 'SessionEnd', 'agentStop'} & set(value['hooks']))
                self.assertNotIn('asyncRewake', json.dumps(value))

    def test_resolved_findings_are_not_replayed_and_omitted_findings_are_not_consumed(self):
        f = self.fixture
        f.doc['schema'] = {'deps': 'rests_on', 'snapshot': 'seen', 'predicate': 'wrong_if'}
        f.doc['judgments'] = {f'd.bad{i}': {'verdict': 'bad', 'rests_on': [f'p.missing{i}'], 'seen': {}}
                              for i in range(10)}
        f.save()
        first = self.call('--context', 'UserPromptSubmit')
        self.assert_context(first, 'd.bad0')
        # Canonical checks may give more than one diagnostic per judgment. All
        # omitted findings must eventually be delivered rather than marked seen.
        delivered = first.stdout
        for _ in range(5):
            delivered += self.call('--context', 'UserPromptSubmit').stdout
        for i in range(10):
            self.assertIn(f'd.bad{i}', delivered)
        f.doc.pop('judgments')
        f.save()
        self.assertEqual(self.call('--context', 'UserPromptSubmit').stdout, '')


if __name__ == '__main__':
    unittest.main()
