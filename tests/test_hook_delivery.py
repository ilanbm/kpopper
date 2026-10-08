"""Lifecycle diagnostics are context, never new prompts, replacement tool results or continuations."""
import json
import os
from pathlib import Path
import re
import shlex
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
# Where each manifest's script paths start. A relative path resolves against the directory
# the manifest stands in; the windsurf entry is a placeholder its README asks to replace.
ROOTS = {'$CLAUDE_PLUGIN_ROOT': '.', '$PLUGIN_ROOT': '.', '${extensionPath}': 'adapters/gemini',
         '.cursor': 'adapters/cursor', '/absolute/path/to/kpopper': '.'}
SCRIPT_PATH = re.compile(r'(\$CLAUDE_PLUGIN_ROOT|\$PLUGIN_ROOT|\$\{extensionPath\}|\.cursor'
                         r'|/absolute/path/to/kpopper|\.\.)/([^"\'\s]+)')
# The argv scripts/hook.sh promises the native command for each script name it accepts.
NATIVE_ROUTES = {'session_start.py': ('session-start',), 'ingestion_hooks.py': ('ingestion-hook',),
                 'followups_hook.py': ('_hook', 'followups'), 'watch_hook.py': ('_hook', 'watch'),
                 'ground_hook.py': ('_hook', 'ground'), 'continuation_hook.py': ('_hook', 'continuation'),
                 'edit_hook.py': ('_hook', 'edit')}
DISPATCHERS = ('hook.sh', 'session_open.sh', 'session_gate.sh', 'ingestion_hook.sh')


def manifest_commands(value):
    """Every command string in a manifest, wherever the host nests it."""
    if isinstance(value, dict):
        for key, item in value.items():
            if key == 'command' and isinstance(item, str):
                yield item
            else:
                yield from manifest_commands(item)
    elif isinstance(value, list):
        for item in value:
            yield from manifest_commands(item)


def promised_argv(script, args):
    """The native argv a dispatcher promises for these arguments, or None when it stays silent."""
    if script == 'session_open.sh':
        script, args = 'hook.sh', ['session_start.py', *args]
    elif script == 'ingestion_hook.sh':
        script, args = 'hook.sh', ['ingestion_hooks.py', *args]
    if script == 'hook.sh':
        route = NATIVE_ROUTES.get(args[0] if args else '')
        return None if route is None else ' '.join((*route, *args[1:]))
    host = event = None
    pairs = iter(args)
    for flag in pairs:
        if flag == '--host':
            host = next(pairs, None)
        elif flag == '--context':
            event = next(pairs, None)
    if event not in ('UserPromptSubmit', 'PostToolUse'):
        return None
    return ' '.join(('session-context', '--event', event, *(('--host', host) if host else ())))


def route_problems(manifest, home=None, dispatch=None):
    """Each way a manifest's commands fail to reach a script or the native command.

    `home` is the repository directory the manifest stands for (its own directory by default),
    and `dispatch(script, args)` runs a dispatcher and returns (returncode, argv seen, stderr).
    """
    manifest = Path(manifest)
    home = Path(home) if home is not None else manifest.parent
    problems = []
    for command in manifest_commands(json.loads(manifest.read_text())):
        paths = SCRIPT_PATH.findall(command)
        if not paths:
            problems.append(f'{manifest}: no script path in {command!r}')
        for root, rest in paths:
            script = (home / '..' / rest if root == '..' else ROOT / ROOTS[root] / rest).resolve()
            if not script.is_file():
                problems.append(f'{manifest}: {root}/{rest} does not exist ({command!r})')
        if dispatch is None:
            continue
        tokens = shlex.split(command)
        for index, token in enumerate(tokens):
            script = Path(token).name
            if script not in DISPATCHERS or not SCRIPT_PATH.search(token):
                continue
            args = tokens[index + 1:]
            if script == 'hook.sh' and (not args or args[0] not in NATIVE_ROUTES):
                problems.append(f'{manifest}: hook.sh has no route {args[:1]} ({command!r})')
            expected = promised_argv(script, args)
            returncode, seen, stderr = dispatch(script, args)
            if returncode != 0 or 'unsupported native hook' in stderr or seen != expected:
                problems.append(f'{manifest}: {script} {" ".join(args)} reached {seen!r} with exit '
                                f'{returncode} and stderr {stderr!r}, not {expected!r}')
    return problems


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
        for name in ('hook.sh', 'session_gate.sh', 'native_runtime.sh', 'session_open.sh',
                     'ingestion_hook.sh'):
            shutil.copy2(ROOT / 'scripts' / name, self.scripts / name)
        self.trace = self.root / 'native-args.txt'
        binary = self.scripts / 'runtime' / target / 'kpop'
        binary.parent.mkdir(parents=True)
        binary.write_text('#!/bin/sh\nprintf "%s\\n" "$*" > "$HOOK_TRACE"\nprintf native-context\nexit 2\n')
        binary.chmod(0o755)

    def invoke(self, name, *args, runtime='rust'):
        env = dict(os.environ, KPOPPER_RUNTIME=runtime, HOOK_TRACE=str(self.trace))
        env.pop('CLAUDE_ENV_FILE', None)  # the opener would add the stub package to a live session
        return subprocess.run(['sh', str(self.scripts / name), *args], input='{}', cwd=self.root,
                              env=env, text=True, capture_output=True, timeout=10)

    def dispatch(self, name, args):
        self.trace.unlink(missing_ok=True)
        result = self.invoke(name, *args)
        seen = self.trace.read_text().strip() if self.trace.exists() else None
        return result.returncode, seen, result.stderr

    def test_every_hook_route_reaches_the_command_with_its_promised_argv(self):
        for script in NATIVE_ROUTES:
            with self.subTest(script=script):
                returncode, seen, stderr = self.dispatch('hook.sh', [script, 'claude', 'wait'])
                self.assertEqual(returncode, 0)
                self.assertNotIn('unsupported native hook', stderr)
                self.assertEqual(seen, promised_argv('hook.sh', [script, 'claude', 'wait']))
        self.assertEqual(self.dispatch('hook.sh', ['watch_hook.py', 'claude', 'wait'])[1],
                         '_hook watch claude wait')
        for name, args, argv in (('session_open.sh', ['--host', 'codex'], 'session-start --host codex'),
                                 ('ingestion_hook.sh', ['claude', 'start'], 'ingestion-hook claude start'),
                                 ('session_gate.sh', ['--context', 'PostToolUse'],
                                  'session-context --event PostToolUse')):
            with self.subTest(script=name):
                self.assertEqual(self.dispatch(name, args), (0, argv, ''))

    def test_every_manifest_command_reaches_the_command_it_names(self):
        for rel in MANIFESTS:
            with self.subTest(rel=rel):
                self.assertEqual(route_problems(ROOT / rel, dispatch=self.dispatch), [])

    def test_a_misspelt_route_in_a_manifest_is_named(self):
        manifest = self.root / 'hooks.json'
        text = (ROOT / 'hooks/hooks.json').read_text()
        self.assertIn('\\"watch_hook.py\\" claude wait', text)
        manifest.write_text(text.replace('\\"watch_hook.py\\" claude wait',
                                         '\\"wach_hook.py\\" claude wait', 1))
        problems = route_problems(manifest, home=ROOT / 'hooks', dispatch=self.dispatch)
        self.assertTrue(problems)
        self.assertTrue(all('wach_hook.py' in problem for problem in problems), problems)
        manifest.write_text(text.replace('scripts/hook.sh', 'scripts/hooks.sh', 1))
        problems = route_problems(manifest, home=ROOT / 'hooks')
        self.assertEqual(len(problems), 1)
        self.assertIn('scripts/hooks.sh does not exist', problems[0])

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
    def test_every_script_a_manifest_names_exists(self):
        for rel in MANIFESTS:
            with self.subTest(rel=rel):
                self.assertEqual(route_problems(ROOT / rel), [])

    def test_only_codex_has_passive_continuation_stop_and_no_automatic_rewake(self):
        for rel in MANIFESTS:
            with self.subTest(rel=rel):
                value = json.loads((ROOT / rel).read_text())
                if rel.startswith('adapters/codex/'):
                    handlers = [hook for group in value['hooks'].get('Stop', []) for hook in group['hooks']]
                    self.assertEqual(len(handlers), 1)
                    self.assertIn('continuation_hook.py', handlers[0]['command'])
                    self.assertNotIn('session_gate', handlers[0]['command'])
                    self.assertNotIn('async', handlers[0])
                else:
                    self.assertFalse({'Stop', 'stop', 'SessionEnd', 'agentStop'} & set(value['hooks']))
                self.assertNotIn('asyncRewake', json.dumps(value))


if __name__ == '__main__':
    unittest.main()
