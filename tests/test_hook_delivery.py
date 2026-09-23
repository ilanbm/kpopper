"""Lifecycle diagnostics are context, never new prompts, replacement tool results or continuations."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from tests.test_native_distribution import host_target

ROOT = Path(__file__).resolve().parents[1]
MANIFESTS = ('hooks/hooks.json', 'adapters/codex/plugin-hooks.json',
             'adapters/codex/hooks.json', 'adapters/copilot/vscode/hooks.json',
             'adapters/cursor/hooks.json', 'adapters/gemini/hooks/hooks.json',
             'adapters/windsurf/hooks.json')


@unittest.skipUnless(os.name == 'posix', 'POSIX shell hooks; the native tests cover Windows')
class ShellDispatch(unittest.TestCase):
    """Every hook route reaches the packaged command, and no route turns its exit code into flow control."""

    def setUp(self):
        target = host_target()
        if target is None:
            self.skipTest('no native package target for this platform')
        self.temp = tempfile.TemporaryDirectory(prefix='hook delivery ')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.scripts = self.root / 'plugin' / 'scripts'
        self.scripts.mkdir(parents=True)
        for name in ('hook.sh', 'session_gate.sh', 'native_runtime.sh'):
            shutil.copy2(ROOT / 'scripts' / name, self.scripts / name)
        self.trace = self.root / 'native-args.txt'
        binary = self.scripts / 'runtime' / target / 'kpop'
        binary.parent.mkdir(parents=True)
        binary.write_text('#!/bin/sh\nprintf "%s\\n" "$*" > "$HOOK_TRACE"\nprintf native-context\nexit 2\n')
        binary.chmod(0o755)

    def invoke(self, name, *args, runtime='rust'):
        return subprocess.run(['sh', str(self.scripts / name), *args], input='{}', cwd=self.root,
                              env=dict(os.environ, KPOPPER_RUNTIME=runtime, HOOK_TRACE=str(self.trace)),
                              text=True, capture_output=True, timeout=10)

    def test_each_route_dispatches_and_no_route_forwards_child_exit_two(self):
        stopped = self.invoke('session_gate.sh', '--host', 'codex')
        self.assertEqual((stopped.returncode, stopped.stdout, stopped.stderr), (0, '', ''))
        self.assertFalse(self.trace.exists(), 'a legacy Stop invocation reached the command')
        context = self.invoke('session_gate.sh', '--host', 'codex', '--context', 'UserPromptSubmit')
        self.assertEqual(context.returncode, 0)
        self.assertEqual(self.trace.read_text().strip(), 'session-context --event UserPromptSubmit --host codex')
        launched = self.invoke('hook.sh', 'watch_hook.py', 'claude', 'wait')
        self.assertEqual(launched.returncode, 0)
        self.assertEqual(self.trace.read_text().strip(), '_hook watch claude wait')

    def test_a_runtime_choice_other_than_rust_is_refused_and_dispatches_nothing(self):
        for name, args in (('session_gate.sh', ('--host', 'codex', '--context', 'UserPromptSubmit')),
                           ('hook.sh', ('ground_hook.py', 'claude', 'start'))):
            for choice in ('python', 'java'):
                with self.subTest(hook=name, runtime=choice):
                    result = self.invoke(name, *args, runtime=choice)
                    self.assertEqual((result.returncode, result.stdout), (0, ''))
                    self.assertEqual(result.stderr, 'kpopper: KPOPPER_RUNTIME must be rust\n')
                    self.assertFalse(self.trace.exists())


class Manifests(unittest.TestCase):
    def test_manifests_do_not_register_stop_or_automatic_rewake(self):
        for rel in MANIFESTS:
            with self.subTest(rel=rel):
                value = json.loads((ROOT / rel).read_text())
                self.assertFalse({'Stop', 'stop', 'SessionEnd', 'agentStop'} & set(value['hooks']))
                self.assertNotIn('asyncRewake', json.dumps(value))


if __name__ == '__main__':
    unittest.main()
