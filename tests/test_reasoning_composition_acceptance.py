"""Run composition, history and query cases against an installed distribution.

The semantic cases live in test_core_composition.py and the native query suite.
This harness loads those same test classes under the installed ``kpopper`` namespace,
from a temporary working directory with no checkout or compiler on PATH.  It is
an explicit packaging gate, not another copy of the acceptance expectations.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
TARGET = Path(__file__).parent


CHILD = r'''
import importlib
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import sys
import types
import unittest
import subprocess
import tempfile

target = Path(sys.argv[1]).resolve()
checkout_package = Path(sys.argv[2]).resolve()
archive = sys.argv[3] or None
plugin_root = Path(sys.argv[4]).resolve() if sys.argv[4] else None
if archive:
    os.environ["KPOPPER_COMPOSITION_TEST_ARCHIVE"] = archive
    os.environ["KPOPPER_QUERY_ARCHIVE"] = archive
if plugin_root:
    package_roots = [(plugin_root / "scripts").resolve()]
    channel = "extracted-plugin"
    if not (package_roots[0] / "__init__.py").is_file():
        raise RuntimeError("extracted plugin package is unavailable")
else:
    installed = importlib.util.find_spec("kpopper")
    if installed is None or not installed.submodule_search_locations:
        raise RuntimeError("installed kpopper package is unavailable")
    package_roots = [Path(path).resolve() for path in installed.submodule_search_locations]
    channel = "installed-python"
if checkout_package in package_roots:
    raise RuntimeError("acceptance resolved the checkout instead of the installed package")

# The reusable acceptance class imports ``scripts`` because that is the checkout
# package name.  Give it an isolated package alias whose search path is the
# installed distribution; no checkout module is imported into this process.
alias = types.ModuleType("scripts")
alias.__path__ = [str(path) for path in package_roots]
alias.__package__ = "scripts"
alias.__spec__ = importlib.machinery.ModuleSpec("scripts", loader=None, is_package=True)
alias.__spec__.submodule_search_locations = alias.__path__
sys.modules["scripts"] = alias
# Fixture imports resolve only test helpers; product imports still resolve the
# installed package through the explicit scripts alias above.
fixtures = types.ModuleType("tests")
fixtures.__path__ = [str(target)]
fixtures.__package__ = "tests"
fixtures.__spec__ = importlib.machinery.ModuleSpec("tests", loader=None, is_package=True)
fixtures.__spec__.submodule_search_locations = fixtures.__path__
sys.modules["tests"] = fixtures
for name in ("history_authoring", "history_contract", "history_store",
             "history_transaction", "provenance", "reasoning",
             "reasoning.authoring", "reasoning.runtime", "reasoning.evaluate",
             "reasoning.snapshot"):
    module = importlib.import_module("scripts." + name)
    if "." not in name:
        setattr(alias, name, module)

if archive:
    runtime_module = importlib.import_module("scripts.reasoning.runtime")
    initialize_runtime = runtime_module.Runtime.__init__
    def selected_runtime(instance, supplied=None, **kwargs):
        return initialize_runtime(instance, supplied or archive, **kwargs)
    runtime_module.Runtime.__init__ = selected_runtime


class InstalledOperationalCLI(unittest.TestCase):
    """Small public-CLI checks whose dispatcher is the installed package itself."""

    @staticmethod
    def cli_path():
        path = package_roots[0] / "cli.py"
        if not path.is_file():
            raise RuntimeError("installed public CLI is unavailable: " + str(path))
        return path

    @staticmethod
    def core_record():
        return ("meta:\n"
                "  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}\n"
                "known:\n"
                "  p.input: {v: 10, measure: base_value}\n"
                "judgments:\n"
                "  d.limit:\n"
                "    rests_on: [p.input]\n"
                "    seen: {p.input: 10}\n"
                "    verdict: okay\n"
                "    wrong_if: {op: gt, args: [{ref: p.input}, {num: '20'}]}\n")

    @unittest.skipIf(os.name == 'nt', 'history writes require POSIX locks')
    def test_installed_first_add_creates_history_and_reopens(self):
        import yaml
        with tempfile.TemporaryDirectory(prefix="installed-new-history-") as directory:
            root = Path(directory)
            result = subprocess.run([sys.executable, str(self.cli_path()), "add", "p.input",
                "v=1", "--as-of", "2026-09-01"], cwd=root, text=True,
                capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            record = yaml.safe_load((root / "GROUNDING.yaml").read_text())
            marker = yaml.safe_load((root / ".kpopper/history.yaml").read_text())
            self.assertEqual(record["meta"]["reasoning"]["profile"], "core/v1")
            self.assertEqual(marker["authority"], "history")
            self.assertEqual(record["meta"]["history"]["record_id"], marker["record_id"])
            for command in (["open", "--json"], ["check"], ["pull", "p.input"],
                            ["affects", "p.input"], ["assess", "p.input"],
                            ["export", "p.input"], ["search", "input"],
                            ["page", "--out", str(root / "page.html")],
                            ["session", "open", "--no-settings", "--tokens", "2000"]):
                reopened = subprocess.run([sys.executable, str(self.cli_path()), *command],
                    cwd=root, text=True, capture_output=True, check=False)
                self.assertEqual(reopened.returncode, 0, reopened.stdout + reopened.stderr)

    def history_fixture(self, *, measured=1, judgment=False):
        """Use the checked history fixture while importing all product modules from ``scripts``."""
        from tests.test_history_snapshot_capture import HistorySnapshotCapture
        from scripts import history_authoring as authoring

        fixture = HistorySnapshotCapture()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        self.addCleanup(fixture.temp.cleanup)
        action = {
            "kind": "add", "id": "p.measured", "into": "readings",
            "body": {"v": measured, "measure": "base_value", "of": "2026-09-16"},
        }
        authoring.commit(fixture.entry, authoring.prepare(
            fixture.entry, action, by="writer", operation="measured-reading"),
            verify=lambda data: None)
        if judgment:
            authoring.commit(fixture.entry, authoring.prepare(
                fixture.entry, {"kind": "add", "id": "d.limit", "into": "decisions",
                                 "body": {"verdict": "okay", "rests_on": ["p.measured"],
                                          "wrong_if": {"op": "gt", "args": [
                                              {"ref": "p.measured"}, {"num": "2"}]}}},
                by="writer", operation="bound-judgment"), verify=lambda data: None)
        return fixture

    @staticmethod
    def root_files(root):
        return {path.relative_to(root).as_posix(): path.read_bytes()
                for path in root.rglob("*") if path.is_file() and ".git" not in path.parts}

    @staticmethod
    def commit_fixture(root):
        subprocess.run(["git", "init", "-q", "-b", "main"], cwd=root, check=True)
        subprocess.run(["git", "-c", "user.name=fixture", "-c",
                        "user.email=fixture@example.test", "config", "user.name", "Fixture"],
                       cwd=root, check=True)
        subprocess.run(["git", "-c", "user.name=fixture", "-c",
                        "user.email=fixture@example.test", "config", "user.email",
                        "fixture@example.test"], cwd=root, check=True)
        subprocess.run(["git", "add", "."], cwd=root, check=True)
        subprocess.run(["git", "-c", "user.name=fixture", "-c",
                        "user.email=fixture@example.test", "commit", "-qm", "history fixture"],
                       cwd=root, check=True)

    def run_watch_until_done(self, root, env):
        setup = subprocess.run([sys.executable, str(self.cli_path()), "watch", "setup",
                                "--base-ref", "main"], cwd=root, env=env, text=True,
                               capture_output=True, check=False)
        self.assertEqual(setup.returncode, 0, setup.stdout + setup.stderr)
        scan = subprocess.run([sys.executable, str(self.cli_path()), "watch", "scan"],
                              cwd=root, env=env, text=True, capture_output=True, check=False)
        self.assertEqual(scan.returncode, 0, scan.stdout + scan.stderr)
        status = None
        for _ in range(100):
            current = subprocess.run([sys.executable, str(self.cli_path()), "watch", "status"],
                                     cwd=root, env=env, text=True, capture_output=True,
                                     check=False)
            self.assertEqual(current.returncode, 0, current.stdout + current.stderr)
            status = json.loads(current.stdout)
            if status.get("state") != "pending":
                return status
            import time
            time.sleep(0.05)
        return status

    def test_installed_remeasure_plan_is_read_only(self):
        with tempfile.TemporaryDirectory(prefix="installed-core-remeasure-") as directory:
            root = Path(directory)
            record = root / "GROUNDING.yaml"
            record.write_text(self.core_record(), encoding="utf-8")
            measure = root / ".kpopper"
            measure.mkdir()
            measure.joinpath("measure.yaml").write_text(
                "base_value: " + json.dumps([sys.executable, "-I", "-c", "print(10)"]) + "\n",
                encoding="utf-8")
            before = record.read_bytes()
            result = subprocess.run([sys.executable, str(self.cli_path()), "remeasure", str(record)],
                                    cwd=root, env=os.environ.copy(), text=True,
                                    capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("base_value", result.stdout)
            self.assertIn("nothing ran - add --run", result.stdout)
            self.assertEqual(record.read_bytes(), before)

    @unittest.skipIf(os.name == 'nt', 'watch requires POSIX file locking')
    def test_installed_watch_unchanged_core_snapshot_is_clear(self):
        with tempfile.TemporaryDirectory(prefix="installed-core-watch-") as directory:
            root = Path(directory)
            env = os.environ.copy()
            env["XDG_STATE_HOME"] = str(root / "state")
            record = root / "GROUNDING.yaml"
            record.write_text(self.core_record(), encoding="utf-8")
            subprocess.run(["git", "init", "-q", "-b", "main"], cwd=root,
                           env=env, check=True)
            subprocess.run(["git", "add", "GROUNDING.yaml"], cwd=root, env=env, check=True)
            subprocess.run(["git", "-c", "user.name=fixture", "-c",
                            "user.email=fixture@example.test", "commit", "-qm", "core fixture"],
                           cwd=root, env=env, check=True)
            setup = subprocess.run([sys.executable, str(self.cli_path()), "watch", "setup",
                                    "--base-ref", "main"], cwd=root, env=env, text=True,
                                   capture_output=True, check=False)
            self.assertEqual(setup.returncode, 0, setup.stdout + setup.stderr)
            scan = subprocess.run([sys.executable, str(self.cli_path()), "watch", "scan"],
                                  cwd=root, env=env, text=True, capture_output=True, check=False)
            self.assertEqual(scan.returncode, 0, scan.stdout + scan.stderr)
            status = None
            for _ in range(100):
                current = subprocess.run([sys.executable, str(self.cli_path()), "watch", "status"],
                                         cwd=root, env=env, text=True, capture_output=True,
                                         check=False)
                self.assertEqual(current.returncode, 0, current.stdout + current.stderr)
                status = json.loads(current.stdout)
                if status.get("state") != "pending":
                    break
                import time
                time.sleep(0.05)
            self.assertEqual(status.get("state"), "clear", status)
            self.assertEqual(status.get("findings"), [])

    @unittest.skipIf(os.name == 'nt', 'history fixture requires POSIX locks')
    def test_installed_history_watch_unchanged_snapshot_is_clear_and_read_only(self):
        with tempfile.TemporaryDirectory(prefix="installed-history-watch-") as directory:
            env = os.environ.copy()
            fixture = self.history_fixture()
            root = fixture.root
            env["XDG_STATE_HOME"] = str(Path(directory) / "state")
            self.commit_fixture(root)
            before = self.root_files(root)
            status = self.run_watch_until_done(root, env)
            self.assertEqual(status.get("state"), "clear", status)
            self.assertEqual(status.get("findings"), [])
            self.assertEqual(self.root_files(root), before)

    @unittest.skipIf(os.name == 'nt', 'history fixture requires POSIX locks')
    def test_installed_history_watch_evaluates_never_folding_hypothesis(self):
        from scripts import history_hypotheses
        with tempfile.TemporaryDirectory(prefix="installed-history-scenario-") as directory:
            env = os.environ.copy()
            fixture = self.history_fixture(measured=1, judgment=True)
            root = fixture.root
            env["XDG_STATE_HOME"] = str(Path(directory) / "state")
            self.commit_fixture(root)
            mutation = history_hypotheses.prepare(fixture.entry, "future",
                {"kind": "set", "id": "p.measured", "value": 3},
                head={"folds": "never"}, operation="future-measurement")
            history_hypotheses.commit(fixture.entry, mutation, verify=lambda data: None)
            before = self.root_files(root)
            status = self.run_watch_until_done(root, env)
            self.assertEqual(status.get("state"), "attention", status)
            self.assertTrue(any(item["kind"] == "falsified" and item["id"] == "d.limit"
                                and item.get("perspective") == "scenario"
                                for item in status["findings"]), status)
            self.assertEqual(self.root_files(root), before)

    @unittest.skipIf(os.name == 'nt', 'history fixture requires POSIX locks')
    def test_installed_history_remeasure_equal_reading_plans_then_runs_without_writes(self):
        with tempfile.TemporaryDirectory(prefix="installed-history-remeasure-") as directory:
            fixture = self.history_fixture(measured=1)
            root = fixture.root
            measure = root / ".kpopper"
            measure.mkdir(exist_ok=True)
            measure.joinpath("measure.yaml").write_text(
                "base_value: " + json.dumps([sys.executable, "-I", "-c", "print(1)"]) + "\n",
                encoding="utf-8")
            before = self.root_files(root)
            plan = subprocess.run([sys.executable, str(self.cli_path()), "remeasure",
                                   str(root / "GROUNDING.yaml")], cwd=root, text=True,
                                  capture_output=True, check=False)
            self.assertEqual(plan.returncode, 0, plan.stdout + plan.stderr)
            self.assertIn("base_value", plan.stdout)
            run = subprocess.run([sys.executable, str(self.cli_path()), "remeasure", "--run",
                                 str(root / "GROUNDING.yaml")], cwd=root, text=True,
                                capture_output=True, check=False)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            self.assertIn("p.measured: 1", run.stdout)
            self.assertEqual(self.root_files(root), before)

    @unittest.skipIf(os.name == 'nt', 'history fixture requires POSIX locks')
    def test_installed_history_remeasure_falsifier_is_red_without_writes(self):
        with tempfile.TemporaryDirectory(prefix="installed-history-remeasure-red-") as directory:
            fixture = self.history_fixture(measured=1, judgment=True)
            root = fixture.root
            measure = root / ".kpopper"
            measure.mkdir(exist_ok=True)
            measure.joinpath("measure.yaml").write_text(
                "base_value: " + json.dumps([sys.executable, "-I", "-c", "print(3)"]) + "\n",
                encoding="utf-8")
            before = self.root_files(root)
            run = subprocess.run([sys.executable, str(self.cli_path()), "remeasure", "--run",
                                 str(root / "GROUNDING.yaml")], cwd=root, text=True,
                                capture_output=True, check=False)
            self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
            self.assertIn("FALSIFIED d.limit", run.stdout)
            self.assertEqual(self.root_files(root), before)


suite = unittest.TestSuite()
suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(InstalledOperationalCLI))

for filename, classname in (("test_core_composition.py", "CoreComposition"),
                            ("test_computational_scenario.py", "ComputationalScenario"),
                            ("test_core_operational_acceptance.py", "CoreOperationalAcceptance"),
                            ("test_core_consolidate_cli.py", "CoreConsolidateCLI"),
                            ("test_history_direct_integration.py", "DirectHistory.test_public_cli_shows_captured_history_preview"),
                            ("test_history_operation_context.py", "HistoryOperationContext.test_prepared_fold_removes_only_selected_group_and_retains_context"),
                            ("test_reasoning_query_runtime.py", "NativeQueryAcceptance"),
                            ("test_core_query_transfer.py", "CoreQueryTransfer"),
                            ("test_core_followups.py", "CoreFollowups.test_declared_record_routes_to_core_exact_conditions"),
                            ("test_core_followups.py", "CoreFollowups.test_runtime_unavailable_is_unknown_not_null_or_false"),
                            ("test_core_followups.py", "CoreFollowups.test_same_value_changed_basis_and_unrelated_changes"),
                            ("test_core_followups.py", "CoreFollowups.test_baseline_retains_typed_evidence_not_only_its_hash"),
                            ("test_core_followups.py", "CoreFollowups.test_authored_core_mapping_is_not_an_envelope"),
                            ("test_core_followups.py", "CoreFollowups.test_top_level_reading_is_bound_to_retained_evidence"),
                            ("test_core_followups.py", "CoreFollowups.test_store_reopen_and_claim_uses_core_evidence")):
    spec = importlib.util.spec_from_file_location("installed_acceptance_target", target / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    suite.addTests(unittest.defaultTestLoader.loadTestsFromName(classname, module))
result = unittest.TextTestRunner(verbosity=2).run(suite)
if not result.wasSuccessful():
    raise SystemExit(1)

runtime_module = importlib.import_module("scripts.reasoning.runtime")
native = runtime_module.Runtime(archive) if archive else runtime_module.Runtime()
print(json.dumps({
    "archive_sha256": native.implementation["archive_sha256"],
    "binary_sha256": native.implementation["binary_sha256"],
    "channel": channel,
    "installed_package_roots": [str(path) for path in package_roots],
    "manifest_version": native.manifest["version"],
    "manifest_modules": native.manifest["modules"],
    "manifest_protocols": native.manifest["protocols"],
    "source_sha256": native.implementation["source_sha256"],
    "target": native.implementation["target"],
    "tests_run": result.testsRun,
    "tests_skipped": len(result.skipped),
}, sort_keys=True))
'''


def run_installed(python, archive=None, plugin_root=None):
    """Execute the shared acceptance class using only one installed package."""
    python = Path(python).absolute()
    if not python.is_file():
        raise ValueError("installed Python executable is unavailable: " + str(python))
    if archive is not None:
        archive = Path(archive).resolve()
        if not archive.is_file():
            raise ValueError("candidate runtime archive is unavailable: " + str(archive))
    if plugin_root is not None:
        plugin_root = Path(plugin_root).resolve()
        if not plugin_root.is_dir():
            raise ValueError("extracted plugin root is unavailable: " + str(plugin_root))
    with tempfile.TemporaryDirectory(prefix="composition-installed-") as directory:
        root = Path(directory)
        empty_path = root / "empty-path"
        empty_path.mkdir()
        git = shutil.which("git")
        if not git:
            raise RuntimeError("installed acceptance needs the existing Git dependency")
        runtime_path = str(empty_path)
        if os.name == "nt":
            runtime_path += os.pathsep + str(Path(git).parent)
        else:
            (empty_path / "git").symlink_to(git)
        env = dict(os.environ)
        for name in ("PYTHONPATH", "PYTHONHOME", "LEAN_PATH", "LEAN_SYSROOT", "LEAN_CC",
                     "KPOPPER_LEAN_ROOT", "KPOPPER_RUNTIME_ARCHIVE",
                     "KPOPPER_TEST_ARCHIVE", "KPOPPER_COMPOSITION_TEST_ARCHIVE", "KPOPPER_QUERY_ARCHIVE",
                     "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"):
            env.pop(name, None)
        env.update(PATH=runtime_path, XDG_CACHE_HOME=str(root / "cache"),
                   LOCALAPPDATA=str(root / "local"), PYTHONDONTWRITEBYTECODE="1")
        subprocess.run([str(python), "-B", "-c", CHILD, str(TARGET),
                        str(ROOT / "scripts"), str(archive) if archive else "",
                        str(plugin_root) if plugin_root else ""],
                       cwd=root, env=env, check=True)


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--installed-python", required=True)
    parser.add_argument("--archive")
    parser.add_argument("--plugin-root")
    args = parser.parse_args(argv)
    run_installed(args.installed_python, args.archive, args.plugin_root)


if __name__ == "__main__":
    main()
