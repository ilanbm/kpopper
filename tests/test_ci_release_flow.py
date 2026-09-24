"""Release coverage is run once, and only its successful artifacts may be published."""
import importlib.util
import json
import pathlib
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("ci_selection", ROOT / ".github/scripts/ci_selection.py")
CI = importlib.util.module_from_spec(spec)
spec.loader.exec_module(CI)


class ReleaseSelection(unittest.TestCase):
    def test_ordinary_code_uses_linux(self):
        self.assertEqual(CI.platforms(["native/src/main.rs"]), "linux-x86_64")

    def test_ordinary_main_leaves_native_tests_to_pull_requests(self):
        self.assertEqual(CI.select(["native/src/main.rs"], push=True), {"rust": False})
        self.assertEqual(CI.platforms(["native/src/main.rs"], push=True), "linux-x86_64")

    def test_release_builds_native_and_filters_tests_by_accumulated_changes(self):
        self.assertEqual(CI.select(["README.md"], release=True), {"rust": True})
        self.assertEqual(CI.platforms(["native/src/main.rs"], release=True), "linux-x86_64")
        self.assertEqual(CI.platforms(["install.ps1"], release=True), "linux-windows")
        self.assertEqual(CI.platforms([], release=True), "all")

    def test_windows_installer_keeps_linux_and_windows(self):
        self.assertEqual(CI.platforms(["install.ps1"]), "linux-windows")

    def test_unknown_files_and_missing_history_keep_full_coverage(self):
        self.assertEqual(CI.platforms(["unclassified.toml"]), "all")
        self.assertEqual(CI.platforms([]), "all")

    def test_platform_code_is_checked_even_when_its_cfg_line_does_not_change(self):
        for source, scope in (("#[cfg(windows)] fn path() {}", "linux-windows"),
                              ('#[cfg(target_os = "macos")] fn path() {}', "linux-macos"),
                              ("#[cfg(unix)] fn path() {}", "all")):
            with self.subTest(scope=scope), patch.object(CI, "source_at", return_value=source):
                self.assertEqual(CI.platforms(["native/src/path.rs"], base="base"), scope)

    def test_deleting_platform_code_keeps_its_coverage(self):
        with patch.object(CI, "source_at", side_effect=["#[cfg(windows)] fn path() {}", "fn path() {}"]):
            self.assertEqual(CI.platforms(["native/src/path.rs"], base="base"), "linux-windows")

    def test_version_only_manifest_change_is_not_a_dependency_change(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root, stderr=subprocess.PIPE).decode().strip()
            def commit():
                git("add", ".")
                git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "fixture")
            git("init")
            (root / "native").mkdir()
            manifest = root / "native/Cargo.toml"
            manifest.write_text('[package]\nname = "kpopper"\nversion = "0.1.0"\n[dependencies]\nyaml = "1"\n')
            commit()
            base = git("rev-parse", "HEAD")
            manifest.write_text(manifest.read_text().replace('"0.1.0"', '"0.1.1"'))
            commit()
            args = dict(base=base, head="HEAD", cwd=root)
            self.assertEqual(CI.platforms(["native/Cargo.toml"], **args), "linux-x86_64")
            manifest.write_text(manifest.read_text().replace('yaml = "1"', 'yaml = "2"'))
            commit()
            self.assertEqual(CI.platforms(["native/Cargo.toml"], **args), "all")


class WorkflowSelection(unittest.TestCase):
    path = ".github/workflows/native-rust.yml"

    def workflow(self):
        return yaml.load((ROOT / self.path).read_text(), Loader=yaml.BaseLoader)

    def scope(self, before, after):
        with patch.object(CI, "source_at", side_effect=[yaml.safe_dump(before), yaml.safe_dump(after)]):
            return CI.platforms([self.path], base="base")

    def test_routing_and_upload_changes_need_only_linux(self):
        before, after = self.workflow(), self.workflow()
        after["concurrency"]["group"] = "another-routing-group"
        after["on"]["workflow_call"]["inputs"]["target"]["default"] = "linux-x86_64"
        for name in ("tests", "release"):
            job = after["jobs"][name]
            job["timeout-minutes"] = "90"
            job["needs"] = ["another-job"]
            job["strategy"]["fail-fast"] = "true"
            # More routes to the same runners do not change how those runners execute.
            job["strategy"]["matrix"]["include"] = job["strategy"]["matrix"]["include"].replace(
                "inputs.target == 'linux-x86_64'", "inputs.target == 'linux-only'")
            for step in job["steps"]:
                if step.get("uses", "").startswith("actions/upload-artifact@"):
                    step["if"] = "inputs.publish"
        self.assertEqual(self.scope(before, after), "linux-x86_64")

    def test_platform_execution_changes_keep_full_coverage(self):
        for field, value in (("run", "cargo build --new-flag"), ("shell", "pwsh"),
                             ("env", {"RUSTFLAGS": "--new-flag"}), ("if", "runner.os == 'Windows'")):
            before, after = self.workflow(), self.workflow()
            step = next(step for step in after["jobs"]["release"]["steps"] if "cargo build" in step.get("run", ""))
            step[field] = value
            with self.subTest(field=field):
                self.assertEqual(self.scope(before, after), "all")

    def test_runner_or_global_environment_changes_keep_full_coverage(self):
        before, after = self.workflow(), self.workflow()
        job = after["jobs"]["tests"]
        job["strategy"]["matrix"]["include"] = job["strategy"]["matrix"]["include"].replace("windows-2022", "windows-2025")
        self.assertEqual(self.scope(before, after), "all")
        after = self.workflow()
        after["env"] = {"RUSTFLAGS": "--new-flag"}
        self.assertEqual(self.scope(before, after), "all")

    def test_unreadable_or_unrecognized_workflow_keeps_full_coverage(self):
        for text in ("not a workflow", "jobs: [invalid"):
            with self.subTest(text=text), patch.object(CI, "source_at", return_value=text):
                self.assertEqual(CI.platforms([self.path], base="base"), "all")
        before, after = self.workflow(), self.workflow()
        after["jobs"]["tests"]["strategy"]["matrix"]["include"] = "${{ fromJSON(needs.plan.outputs.matrix) }}"
        self.assertEqual(self.scope(before, after), "all")


class PublishBoundary(unittest.TestCase):
    def workflow(self, name):
        # BaseLoader preserves the YAML 1.2 Actions key `on`.
        return yaml.load((ROOT / ".github/workflows" / name).read_text(), Loader=yaml.BaseLoader)

    def test_publish_is_explicit_and_runs_trusted_code_on_main(self):
        workflow = self.workflow("publish.yml")
        self.assertEqual(set(workflow["on"]), {"workflow_dispatch"})
        self.assertEqual(set(workflow["on"]["workflow_dispatch"]["inputs"]), {"release_pr", "check_run"})
        jobs = workflow["jobs"]
        self.assertNotIn("build", jobs)
        self.assertEqual(jobs["plan"]["if"], "github.ref == 'refs/heads/main'")
        self.assertEqual(jobs["publish"]["needs"], "plan")
        plan = next(step for step in jobs["plan"]["steps"] if step.get("id") == "plan")
        self.assertIn("release_candidate.py --plan", plan["run"])

    def test_candidate_checks_can_read_the_pr_and_bot_dispatches_them(self):
        self.assertEqual(self.workflow("check.yml")["permissions"]["pull-requests"], "read")
        self.assertEqual(self.workflow("release.yml")["permissions"]["actions"], "write")
        script = (ROOT / ".github/scripts/release.py").read_text()
        self.assertIn('"workflow", "run", "check.yml", "--ref", branch', script)

    def test_downloads_are_bound_to_checked_run_and_publisher_is_trusted(self):
        workflow = self.workflow("publish.yml")
        for job in workflow["jobs"].values():
            for step in job.get("steps", []):
                use = step.get("uses", "")
                if use.startswith("actions/download-artifact@"):
                    self.assertEqual(step["with"]["run-id"], "${{ needs.plan.outputs.run_id }}")
                    self.assertEqual(step["with"]["github-token"], "${{ github.token }}")
        checkouts = [step["with"] for step in workflow["jobs"]["publish"]["steps"]
                     if step.get("uses", "").startswith("actions/checkout@")]
        self.assertEqual(checkouts[0]["ref"], "${{ github.sha }}")
        self.assertEqual(checkouts[1]["path"], "candidate")
        self.assertEqual(checkouts[1]["ref"], "${{ needs.plan.outputs.source }}")
        script = next(step["run"] for step in workflow["jobs"]["publish"]["steps"]
                      if "publish_release.py" in step.get("run", ""))
        self.assertIn("--source-directory candidate", script)
        rerun = workflow["jobs"]["merge-gate"]["steps"][0]["run"]
        self.assertIn("/actions/jobs/$GATE_JOB/rerun", rerun)

    def test_publisher_and_refresher_serialize_candidate_changes(self):
        publisher, refresher = self.workflow("publish.yml"), self.workflow("release.yml")
        self.assertEqual(publisher["concurrency"], refresher["concurrency"])
        self.assertEqual(publisher["concurrency"]["cancel-in-progress"], "false")

    def test_release_build_matrix_remains_complete_when_tests_are_filtered(self):
        workflow = self.workflow("native-rust.yml")
        expression = workflow["jobs"]["release"]["strategy"]["matrix"]["include"]
        rows = json.loads(re.search(r"inputs.publish && '([^']+)'", expression).group(1))
        self.assertEqual({row["target"] for row in rows}, {
            "linux-x86_64", "linux-aarch64", "darwin-arm64", "darwin-x86_64", "windows-x86_64"})
        self.assertNotIn("inputs.publish &&", workflow["jobs"]["tests"]["strategy"]["matrix"]["include"])

    def test_new_main_push_does_not_cancel_a_release_being_checked(self):
        workflow = self.workflow("check.yml")
        self.assertIn("github.sha", workflow["concurrency"]["group"])
        self.assertIn("github.event_name == 'push'", workflow["concurrency"]["group"])
        native = self.workflow("native-rust.yml")
        self.assertIn("inputs.publish", native["concurrency"]["group"])

    def test_selected_targets_have_matching_tests_and_distribution_acceptance(self):
        workflow = self.workflow("native-rust.yml")
        expected = {
            "linux-x86_64": {"linux-x86_64"},
            "linux-windows": {"linux-x86_64", "windows-x86_64"},
            "linux-macos": {"linux-x86_64", "darwin-arm64", "darwin-x86_64"},
        }
        for name in ("tests", "release"):
            expression = workflow["jobs"][name]["strategy"]["matrix"]["include"]
            choices = {target: {row["target"] for row in json.loads(rows)} for target, rows in
                       re.findall(r"inputs.target == '([^']+)'\s*&& '([^']+)'", expression)}
            for scope, targets in expected.items():
                self.assertEqual(choices[scope], targets)
            fallback = json.loads(re.findall(r"\|\| '(\[.*?\])'", expression)[-1])
            self.assertEqual({row["target"] for row in fallback}, {
                "linux-x86_64", "linux-aarch64", "darwin-arm64", "darwin-x86_64", "windows-x86_64"})
        upload = next(step for step in workflow["jobs"]["release"]["steps"]
                      if step.get("with", {}).get("name") == "verified-crate")
        self.assertIn("inputs.publish", upload["if"])


if __name__ == "__main__":
    unittest.main()
