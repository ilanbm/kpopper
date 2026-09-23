"""Native launchers bind commands to the active package, without Python fallback."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("native_launcher", ROOT / "scripts/native_launcher.py")
N = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(N)


class PlatformSelection(unittest.TestCase):
    def test_supported_targets_and_unsupported_architecture(self):
        self.assertEqual(N.target_name("Darwin", "arm64"), "darwin-arm64")
        self.assertEqual(N.target_name("Darwin", "x86_64"), "darwin-x86_64")
        self.assertEqual(N.target_name("Linux", "aarch64"), "linux-aarch64")
        self.assertEqual(N.target_name("Windows", "AMD64"), "windows-x86_64")
        with self.assertRaisesRegex(ValueError, "unsupported native platform"):
            N.target_name("Windows", "arm64")

    def test_missing_runtime_is_explicit(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "not installed"):
                N.executable(directory, "linux-x86_64")


@unittest.skipIf(os.name == "nt", "POSIX shell launcher contract")
class NativeShellLaunchers(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="native launcher ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.plugin = self.root / "plugin with spaces"
        (self.plugin / "scripts").mkdir(parents=True)
        (self.plugin / "bin").mkdir()
        for name in ("native_runtime.sh", "hook.sh", "session_open.sh", "session_gate.sh", "ingestion_hook.sh"):
            shutil.copy2(ROOT / "scripts" / name, self.plugin / "scripts" / name)
        for name in ("kpop", "kpopper"):
            shutil.copy2(ROOT / "bin" / name, self.plugin / "bin" / name)
        tools = self.tools = self.root / "path tools"
        tools.mkdir()
        self.write_executable(tools / "uname", '#!/bin/sh\ncase "$1" in -s) echo Darwin;; -m) echo arm64;; esac\n')
        self.python_called = self.root / "python-called"
        self.write_executable(tools / "python3", '#!/bin/sh\n: > "$PYTHON_CALLED"\nexit 99\n')
        self.write_executable(tools / "kpop", '#!/bin/sh\nprintf "wrong PATH executable"\nexit 99\n')
        self.binary = self.plugin / "scripts/runtime/darwin-arm64/kpop"
        self.binary.parent.mkdir(parents=True)
        self.write_executable(self.binary,
                              '#!/bin/sh\nprintf "<%s>\\n" "$@"\ncat\nexit "${NATIVE_EXIT:-0}"\n')
        self.env = dict(os.environ, PATH=str(tools) + os.pathsep + "/usr/bin:/bin",
                        PYTHON_CALLED=str(self.python_called))
        self.env.pop("KPOPPER_RUNTIME", None)

    def write_executable(self, path, text):
        path.write_text(text)
        path.chmod(0o755)

    def invoke(self, path, args=(), code=0):
        return subprocess.run(["sh", str(self.plugin / path), *args],
                              input='{"session_id":"test","cwd":"/example"}',
                              text=True, capture_output=True, timeout=10,
                              env=dict(self.env, NATIVE_EXIT=str(code)), cwd=self.root)

    def test_both_public_names_keep_arguments_and_stdin(self):
        for name in ("kpop", "kpopper"):
            result = self.invoke("bin/" + name, ("add", "p.value", "v=space and $literal"))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, '<add>\n<p.value>\n<v=space and $literal>\n'
                             '{"session_id":"test","cwd":"/example"}')
        self.assertFalse(self.python_called.exists())

    def test_cli_arguments_cannot_select_the_private_resolver_mode(self):
        result = self.invoke("bin/kpop", ("--path",))
        self.assertTrue(result.stdout.startswith("<--path>\n"), result.stdout)

    def test_all_hook_routes_use_native_commands(self):
        for script, args, expected in [
            ("session_open.sh", ("--host", "codex"), "<session-start>\n<--host>\n<codex>"),
            ("session_gate.sh", ("--host", "claude", "--context", "UserPromptSubmit"),
             "<session-context>\n<--event>\n<UserPromptSubmit>\n<--host>\n<claude>"),
            ("ingestion_hook.sh", ("codex", "start"), "<ingestion-hook>\n<codex>\n<start>"),
            ("hook.sh", ("ground_hook.py", "codex", "prompt"), "<_hook>\n<ground>\n<codex>\n<prompt>"),
            ("hook.sh", ("edit_hook.py", "claude"), "<_hook>\n<edit>\n<claude>"),
            ("hook.sh", ("watch_hook.py", "claude", "wait"), "<_hook>\n<watch>\n<claude>\n<wait>"),
            ("hook.sh", ("followups_hook.py",), "<_hook>\n<followups>"),
        ]:
            with self.subTest(script=script, args=args):
                result = self.invoke("scripts/" + script, args)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(result.stdout.startswith(expected + "\n"), result.stdout)
                self.assertTrue(result.stdout.endswith('"cwd":"/example"}'))
        self.assertFalse(self.python_called.exists())

    def test_only_explicit_cli_calls_preserve_exit_two(self):
        for path, args in (("scripts/session_gate.sh", ("--host", "claude", "--context", "UserPromptSubmit")),
                           ("scripts/hook.sh", ("watch_hook.py", "claude", "wait"))):
            self.assertEqual(self.invoke(path, args, code=2).returncode, 0)
        self.assertEqual(self.invoke("bin/kpop", ("check",), code=2).returncode, 2)

    def test_missing_runtime_is_nonblocking_for_hooks_and_fails_cli(self):
        self.binary.unlink()
        for script, args in (("session_gate.sh", ("--context", "UserPromptSubmit")),
                             ("hook.sh", ("ground_hook.py", "claude", "start"))):
            result = self.invoke("scripts/" + script, args)
            self.assertEqual((result.returncode, result.stdout), (0, ""))
            self.assertIn("not installed", result.stderr)
        self.assertEqual(self.invoke("bin/kpop").returncode, 1)
        stopped = self.invoke("scripts/session_gate.sh")
        self.assertEqual((stopped.returncode, stopped.stdout, stopped.stderr), (0, '', ''))
        self.assertFalse(self.python_called.exists())

    def test_only_the_opener_tells_the_session_that_the_runtime_is_missing(self):
        # Claude Code and Codex give the model a SessionStart hook's standard output, never
        # its standard error. The installer is present, so running it would reach curl.
        self.binary.unlink()
        shutil.copy2(ROOT / "scripts/install_native.sh", self.plugin / "scripts/install_native.sh")
        shutil.copy2(ROOT / "install.sh", self.plugin / "install.sh")
        (self.plugin / "VERSION").write_text("0.9.0\n")
        downloaded = self.root / "downloaded"
        self.write_executable(self.tools / "curl", '#!/bin/sh\n: > "$DOWNLOADED"\nexit 99\n')
        env = dict(self.env, CLAUDE_PLUGIN_ROOT=str(self.plugin), PLUGIN_ROOT=str(self.plugin),
                   DOWNLOADED=str(downloaded))
        install = 'Install this active copy: sh "%s"\n' % (self.plugin / "scripts/install_native.sh")
        openers = 0
        for config in ("hooks/hooks.json", "adapters/codex/plugin-hooks.json", "adapters/codex/hooks.json"):
            for event, groups in json.loads((ROOT / config).read_text())["hooks"].items():
                for hook in (hook for group in groups for hook in group["hooks"]):
                    command = hook["command"].replace("../../scripts", str(self.plugin / "scripts"))
                    payload = {"session_id": "missing-runtime", "transcript_path": str(self.root / "t.jsonl"),
                               "cwd": str(self.root), "hook_event_name": event, "source": "startup"}
                    with self.subTest(config=config, command=command):
                        result = subprocess.run(command, shell=True, input=json.dumps(payload), text=True,
                                                capture_output=True, timeout=10, env=env, cwd=self.root)
                        self.assertEqual(result.returncode, 0)
                        if "session_open.sh" not in command:
                            self.assertEqual(result.stdout, "")
                            self.assertIn("not installed", result.stderr)
                            continue
                        openers += 1
                        self.assertEqual(result.stderr, "")
                        self.assertTrue(result.stdout.startswith(
                            "kpopper: native runtime is not installed for darwin-arm64 in this package.\n"
                            + install), result.stdout)
                        self.assertIn("kpopper did not open this session", result.stdout)
                        self.assertIn("Offer to run the command above", result.stdout)
                        self.assertIn("start a new session", result.stdout)
        self.assertEqual(openers, 3)
        self.assertFalse(self.binary.exists())
        self.assertFalse(downloaded.exists())
        self.assertFalse(self.python_called.exists())

    def test_an_unsupported_platform_reaches_the_session_without_an_install_offer(self):
        self.write_executable(self.tools / "uname", '#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo riscv64;; esac\n')
        result = self.invoke("scripts/session_open.sh", ("--host", "claude"))
        self.assertEqual((result.returncode, result.stderr), (0, ""))
        self.assertTrue(result.stdout.startswith("kpopper: unsupported native platform: Linux riscv64\n"
                                                 "kpopper did not open this session"), result.stdout)
        self.assertNotIn("Offer to run", result.stdout)
        grounded = self.invoke("scripts/hook.sh", ("ground_hook.py", "claude", "start"))
        self.assertEqual((grounded.returncode, grounded.stdout), (0, ""))
        self.assertIn("unsupported native platform", grounded.stderr)


if __name__ == "__main__":
    unittest.main()
