"""Plugin hooks must use the Python that setup repaired, independently of PATH."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import venv

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("plugin_runtime", ROOT / "scripts/plugin_runtime.py")
R = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(R)


@unittest.skipIf(os.name == "nt", "Claude/Codex shell hook contracts require POSIX")
class PluginRuntime(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="kpop runtime ")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.plugin = self.root / "plugin with spaces"
        shutil.copytree(ROOT / "scripts", self.plugin / "scripts")
        self.empty = self.root / "empty python"
        venv.EnvBuilder(with_pip=False).create(self.empty)
        self.python = self.empty / "bin/python3"
        self.project = self.root / "project with spaces"
        self.project.mkdir()
        self.env = dict(os.environ, HOME=str(self.root / "home"),
                        KPOPPER_RUNTIME_HOME=str(self.root / "private runtimes"),
                        PATH=str(self.empty / "bin") + os.pathsep + os.environ["PATH"],
                        PYTHONPATH="", TMPDIR=str(self.root))
        self.env.pop("PYTHONHOME", None)
        self.env.pop("VIRTUAL_ENV", None)
        self.payload = {"session_id": "runtime-test", "cwd": str(self.project), "source": "startup"}

    def hook(self, script="session_open.sh", args=("--host", "claude")):
        return subprocess.run(["sh", str(self.plugin / "scripts" / script), *args],
                              input=json.dumps(self.payload), cwd=self.project, env=self.env,
                              capture_output=True, text=True, timeout=30)

    def repaired(self):
        """Supply installed dependencies offline; real PyPI setup is a separate smoke."""
        with mock.patch.dict(os.environ, self.env):
            directory = R.runtime_dir()
        venv.EnvBuilder(with_pip=False).create(directory)
        python = R.venv_python(directory)
        site = Path(subprocess.check_output([str(python), "-c",
                    "import sysconfig; print(sysconfig.get_path('purelib'))"], text=True).strip())
        for name in (*R.MODULES, "six", "webencodings"):
            spec = importlib.util.find_spec(name)
            source = Path(spec.origin)
            if spec.submodule_search_locations:
                shutil.copytree(source.parent, site / source.parent.name)
            else:
                shutil.copy2(source, site / source.name)
        return python, site

    def test_missing_dependencies_name_actual_python_and_safe_remedy(self):
        (self.project / "GROUNDING.yaml").write_text("known:\n  sample: 17\n")
        result = self.hook()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(str(self.python), result.stdout)
        self.assertIn("plugin_runtime.py", result.stdout)
        self.assertIn("setup", result.stdout)
        self.assertIn("yaml", result.stdout)
        self.assertNotIn("KPOPPER_AGENT_CONTEXT", result.stdout)
        self.assertNotIn("Traceback", result.stderr)
        self.assertFalse(Path(self.env["KPOPPER_RUNTIME_HOME"]).exists())

    def test_first_open_cli_write_and_reopen_use_repaired_python(self):
        python, _ = self.repaired()
        result = self.hook(args=("--host", "codex"))
        self.assertEqual(result.returncode, 0, result.stderr)
        context = json.loads(next(line.split(" ", 1)[1] for line in result.stdout.splitlines()
                                  if line.startswith("KPOPPER_AGENT_CONTEXT ")))
        self.assertEqual(context["command"], [str(python), str(self.plugin / "scripts/cli.py")])
        self.assertFalse((self.project / "GROUNDING.yaml").exists())
        for args in (("add", "s.note", '{"name":"Fixture note","read":"2026-09-17"}'),
                     ("add", "m.seats", '{"v":17,"from":"s.note","at":"line 1"}')):
            saved = subprocess.run([*context["command"], *args], env=self.env, cwd=self.project,
                                   capture_output=True, text=True, timeout=30)
            self.assertEqual(saved.returncode, 0, saved.stderr + saved.stdout)
        self.payload["session_id"] = "reopened"
        reopened = self.hook(args=("--host", "codex"))
        self.assertIn("2 entries", reopened.stdout)
        self.assertNotIn("unavailable", reopened.stdout + reopened.stderr)
        self.assertTrue((self.root / "kpopper-base-reopened").exists())
        read = subprocess.run([*context["command"], "pull", "m.seats"], env=self.env, cwd=self.project,
                              capture_output=True, text=True, timeout=30)
        self.assertIn("17", read.stdout)
        self.assertEqual(read.returncode, 0, read.stderr)

    def test_every_claude_and_codex_hook_uses_the_repaired_runtime(self):
        python, site = self.repaired()
        (self.project / "GROUNDING.yaml").write_text("known:\n  m.sample: 17\n")
        trace = self.root / "interpreters.jsonl"
        # A .pth works even on distributions shipping a stdlib sitecustomize.
        (site / "kpop_test_trace.pth").write_text(
            "import json, os, sys; "
            "open(os.environ['KPOP_TEST_TRACE'], 'a').write(json.dumps([sys.executable, sys.argv]) + '\\n')\n")
        env = dict(self.env, KPOP_TEST_TRACE=str(trace), CLAUDE_PLUGIN_ROOT=str(self.plugin),
                   PLUGIN_ROOT=str(self.plugin))
        for config in ("hooks/hooks.json", "adapters/codex/plugin-hooks.json", "adapters/codex/hooks.json"):
            for event, groups in json.loads((ROOT / config).read_text())["hooks"].items():
                for group in groups:
                    for hook in group["hooks"]:
                        command = hook["command"].replace("../../scripts", str(self.plugin / "scripts"))
                        payload = dict(self.payload, hook_event_name=event, prompt="hello")
                        with self.subTest(config=config, command=command):
                            result = subprocess.run(command, shell=True, input=json.dumps(payload), cwd=self.project,
                                                    env=env, capture_output=True, text=True, timeout=30)
                            self.assertEqual(result.returncode, 0, result.stderr)
                            self.assertNotIn("unavailable", result.stdout + result.stderr)
        runs = [json.loads(line) for line in trace.read_text().splitlines()]
        scripts = {Path(args[0]).name for _, args in runs}
        for script in ("session_start.py", "workspace.py", "provenance.py", "ingestion_hooks.py",
                       "ground_hook.py", "watch_hook.py", "followups_hook.py", "edit_hook.py"):
            self.assertIn(script, scripts)
        self.assertTrue(all(executable == str(python) for executable, _ in runs))

    def test_partial_private_runtime_does_not_fall_back_to_path(self):
        python, site = self.repaired()
        shutil.rmtree(site / "html5lib")
        result = self.hook()
        self.assertIn(str(python), result.stdout)
        self.assertIn("html5lib", result.stdout)
        self.assertIn("setup", result.stdout)
        self.assertNotIn("KPOPPER_AGENT_CONTEXT", result.stdout)

    def test_runtime_survives_plugin_cache_replacement(self):
        python, _ = self.repaired()
        updated = self.root / "updated plugin"
        self.plugin.rename(updated)
        self.plugin = updated
        result = self.hook()
        self.assertIn(str(python), result.stdout)
        self.assertIn(str(updated / "scripts/cli.py"), result.stdout)

    def test_healthy_path_python_still_works_without_private_setup(self):
        python, _ = self.repaired()
        self.env["PATH"] = str(python.parent) + os.pathsep + os.environ["PATH"]
        self.env["KPOPPER_RUNTIME_HOME"] = str(self.root / "no setup")
        result = self.hook()
        self.assertIn("KPOPPER_AGENT_CONTEXT", result.stdout)
        self.assertNotIn("unavailable", result.stdout + result.stderr)
        self.assertFalse(Path(self.env["KPOPPER_RUNTIME_HOME"]).exists())

    def test_probe_matches_the_python_environment_of_existing_hooks(self):
        _, site = self.repaired()
        self.env["KPOPPER_RUNTIME_HOME"] = str(self.root / "no setup")
        self.env["PYTHONPATH"] = str(site)
        result = self.hook()
        self.assertIn("KPOPPER_AGENT_CONTEXT", result.stdout)
        self.assertNotIn("unavailable", result.stdout + result.stderr)

    def test_setup_installs_only_inside_verified_venv(self):
        with mock.patch.dict(os.environ, self.env):
            directory = R.runtime_dir()
            python = str(R.venv_python(directory))
            status = {"python": python, "prefix": str(directory), "base_prefix": "/base",
                      "errors": ["yaml missing"]}
            with mock.patch.object(R, "probe", side_effect=[status, dict(status, errors=[])]), \
                    mock.patch.object(R.subprocess, "run") as run:
                self.assertEqual(R.main(["setup"]), 0)
            self.assertEqual(run.call_args_list, [
                mock.call([sys.executable, "-I", "-m", "venv", str(directory)], check=True),
                mock.call([python, "-I", "-m", "pip", "--isolated", "install", *R.REQUIREMENTS], check=True)])

    def test_setup_refuses_pip_when_python_is_not_the_private_venv(self):
        with mock.patch.dict(os.environ, self.env), \
                mock.patch.object(R, "probe", return_value={"python": sys.executable,
                    "prefix": "/system", "base_prefix": "/system", "errors": ["yaml missing"]}), \
                mock.patch.object(R.subprocess, "run") as run:
            self.assertEqual(R.main(["setup"]), 1)
            self.assertEqual(run.call_count, 1)  # venv creation attempted, never pip

    def test_failed_setup_is_not_ready_and_lock_is_released(self):
        with mock.patch.dict(os.environ, self.env), mock.patch.object(R.subprocess, "run",
                side_effect=subprocess.CalledProcessError(1, ["venv"])):
            self.assertEqual(R.main(["setup"]), 1)
            self.assertFalse(R.runtime_dir().with_name(R.runtime_dir().name + ".setup-lock").exists())


class RuntimeContract(unittest.TestCase):
    def test_dependencies_match_package_metadata(self):
        # Python 3.9 has no tomllib; this single-line TOML array is also JSON.
        line = next(line for line in (ROOT / "pyproject.toml").read_text().splitlines()
                    if line.startswith("dependencies = "))
        self.assertEqual(list(R.REQUIREMENTS), json.loads(line.split(" = ", 1)[1]))

    def test_hook_preserves_exit_code_and_arguments(self):
        with mock.patch.object(R, "select", return_value={"python": "/private/python", "errors": []}), \
                mock.patch.object(R.os, "execv") as execute:
            R.main(["hook", "ground_hook.py", "claude", "prompt"])
        execute.assert_called_once_with("/private/python", ["/private/python",
                                        str(R.HERE / "ground_hook.py"), "claude", "prompt"])


if __name__ == "__main__":
    unittest.main()
