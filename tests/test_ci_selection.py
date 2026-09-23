"""CI runs the lanes a change reads, from the entire PR, and every tracked file feeds a declared lane."""
import importlib.util
import json
import os
import pathlib
import re
import subprocess
import tempfile
import unittest

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("ci_selection", ROOT / ".github/scripts/ci_selection.py")
CI = importlib.util.module_from_spec(spec)
spec.loader.exec_module(CI)
ALL = set(CI.LANE_NAMES)


def lanes(changes, **options):
    return {lane for lane, selected in CI.select(changes, **options).items() if selected}


def tracked():
    raw = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT)
    return [p.decode() for p in raw.split(b"\0") if p]


class Claims(unittest.TestCase):
    def test_every_tracked_file_is_read_by_a_lane_or_declared_unread(self):
        unclaimed = [path for path in tracked()
                     if not CI.matches(path, CI.CI_MACHINERY + CI.UNREAD + CI.RECORD_JOB)
                     and not any(CI.lane_reads(lane, path) for lane in CI.LANES.values())]
        self.assertEqual(unclaimed, [], "declare these in .github/scripts/ci_selection.py")

    def test_the_record_job_runs_every_contract_module_no_lane_reads(self):
        modules = {path for path in CI.RECORD_JOB
                   if path.startswith("tests/test_") and path.endswith(".py")}
        self.assertTrue(modules)
        workflow = yaml.safe_load((ROOT / ".github/workflows/check.yml").read_text())
        contracts = "\n".join(step.get("run", "") for step in workflow["jobs"]["record"]["steps"])
        for module in modules:
            self.assertTrue((ROOT / module).is_file(), module)
            self.assertIn(module[:-3].replace("/", "."), contracts)


class Selection(unittest.TestCase):
    def test_rust_change_runs_the_rust_lane_without_intel_macos(self):
        changes = ["native/src/ordinary_checked_session.rs", "native/src/public_ordinary_readers.rs",
                   "native/src/source_capture.rs", "native/tests/ordinary_checked_session.rs"]
        self.assertEqual(lanes(changes), {"rust"})
        self.assertEqual(CI.platforms(changes), "pull-request")

    def test_documentation_record_and_plugin_manifest_edits_run_no_lane(self):
        # The record job still checks all of these on every pull request.
        changes = ["README.md", "assets/README.md", "assets/brand-guide.md", ".kpopper/view.yaml",
                   "CHANGELOG.md", "SECURITY.md", "CODE_OF_CONDUCT.md", "skills/ground/SKILL.md",
                   "GROUNDING.yaml", ".kpopper/measure.yaml", "package.json", "hooks/hooks.json",
                   "bin/kpop", ".claude-plugin/plugin.json", "native/README.md",
                   ".github/pull_request_template.md", ".github/ISSUE_TEMPLATE/bug_report.yml"]
        for path in changes:
            with self.subTest(path=path):
                self.assertEqual(lanes([path]), set())

    def test_release_scripts_and_their_tests_run_only_the_record_job(self):
        self.assertEqual(lanes([".github/scripts/publish_release.py", "tests/test_release.py"]), set())

    def test_adding_a_file_runs_the_lanes_that_list_its_directory(self):
        added = ("A", "native/src/new_reader.rs")
        expected = {name for name, lane in CI.LANES.items() if CI.lane_lists(lane, "native/src")}
        self.assertEqual(lanes([added], base_dirs={"", "native", "native/src"}), expected)
        self.assertEqual(lanes([("M", "assets/navigation/README.md")]), set())

    def test_listings_reach_the_first_directory_that_existed(self):
        self.assertEqual(CI.listings("A", "examples/new/deep/x.yaml", base_dirs={"", "examples"}),
                         ["examples/new/deep", "examples/new", "examples"])
        self.assertEqual(CI.listings("D", "docs/sub/only.md", head_dirs={"", "docs"}), ["docs/sub", "docs"])
        self.assertEqual(CI.listings("A", "docs/x.md"), ["docs"])
        self.assertEqual(CI.listings("M", "docs/x.md", base_dirs={""}, head_dirs={""}), [])
        self.assertEqual(CI.listings("A", "top.md", base_dirs={""}), [""])

    def test_the_plugin_plumbing_and_host_wrappers_reach_the_rust_lane(self):
        # Installed acceptance stages a plugin from these and drives its hooks.
        for path in ("scripts/hook.sh", "scripts/session_gate.sh", "scripts/native_runtime.sh",
                     "adapters/codex/plugin-hooks.json", "adapters/cursor/scripts/gate-open.sh",
                     "adapters/copilot/cli/hook.sh", "adapters/windsurf/hooks.json"):
            with self.subTest(path=path):
                self.assertIn("rust", lanes([path]))

    def test_compiled_in_assets_and_packaging_inputs_reach_the_rust_lane(self):
        for path in ("native/shared/page/page.css", "native/shared/session/rules.txt",
                     "native/shared/start-guide.md", "native/shared/expressions.py",
                     "scripts/session/lean/Main.lean", "scripts/reasoning/lean/Kernel.lean",
                     "scripts/reasoning/native/linux-x86_64.kpopper-runtime",
                     "scripts/reasoning/third_party/COPYING.LESSERv3",
                     "scripts/reasoning/build_runtime.py", "scripts/package_native.py",
                     "install.sh", "install.ps1", "VERSION", "LICENSE"):
            with self.subTest(path=path):
                self.assertIn("rust", lanes([path]))

    def test_rust_lane_reads_what_its_sources_embed(self):
        for path in ("native/shared/verify_page.js", "examples/offer-review/after/GROUNDING.yaml"):
            with self.subTest(path=path):
                self.assertIn("rust", lanes([path]))
        self.assertTrue(CI.rust_embeds(("native",))
                        >= {"native/shared/verify_page.js", "native/shared/session/rules.txt"})

    def test_the_bundle_check_reads_the_workflows_recorded_inside_the_archive(self):
        for path in (".github/workflows/reasoning-runtime.yml",
                     ".github/workflows/reasoning-target.yml"):
            with self.subTest(path=path):
                self.assertIn("rust", lanes([path]))

    def test_ci_machinery_runs_every_lane_on_every_platform(self):
        for path in CI.CI_MACHINERY:
            with self.subTest(path=path):
                self.assertEqual(lanes([path]), ALL)
                self.assertEqual(CI.platforms([path]), "all")

    def test_unclaimed_paths_run_everything(self):
        for path in ("new-component/config.toml", "native.toml", ".github/workflows/new.yml"):
            with self.subTest(path=path):
                self.assertEqual(lanes([("A", path)]), ALL)

    def test_platform_inputs_take_every_target_and_code_changes_leave_intel_macos_to_main(self):
        for path in ("native/Cargo.lock", "native/build.rs", "install.ps1",
                     ".github/workflows/native-rust.yml",
                     "scripts/reasoning/native/darwin-x86_64.kpopper-runtime"):
            with self.subTest(path=path):
                self.assertEqual(CI.platforms([path]), "all")
        for path in ("native/src/main.rs", "native/tests/cli.rs", "README.md"):
            with self.subTest(path=path):
                self.assertEqual(CI.platforms([path]), "pull-request")

    def test_main_runs_every_lane_on_every_platform(self):
        self.assertEqual(lanes(["README.md"], push=True), ALL)
        self.assertEqual(CI.platforms(["README.md"], push=True), "all")

    def test_empty_diff_manual_run_and_missing_history_run_everything(self):
        self.assertEqual(lanes([]), ALL)
        self.assertEqual(CI.platforms([]), "all")
        self.assertEqual(lanes(["README.md"], full=True), ALL)
        with tempfile.TemporaryDirectory() as directory:
            changes, base_dirs, head_dirs = CI.changed_files("missing", "HEAD", cwd=directory)
            self.assertEqual((changes, base_dirs, head_dirs), ([], None, None))
            self.assertEqual(lanes(changes), ALL)


class GitRange(unittest.TestCase):
    def test_statuses_deletions_renames_and_directory_sets_come_from_the_whole_range(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)

            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root, stderr=subprocess.PIPE).decode().strip()

            def commit(message):
                git("add", "-A")
                git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-m", message)

            git("init")
            (root / "native/src").mkdir(parents=True)
            (root / "native/src/reader.rs").write_text("old")
            (root / "docs").mkdir()
            (root / "docs/old.md").write_text("documentation")
            (root / "README.md").write_text("readme")
            commit("base")
            base = git("rev-parse", "HEAD")
            (root / "native/src/reader.rs").unlink()
            commit("delete a source file")
            (root / "docs/old.md").rename(root / "docs/new.md")
            (root / "README.md").write_text("changed")
            (root / "fresh/nested").mkdir(parents=True)
            (root / "fresh/nested/file.txt").write_text("new")
            commit("rename, edit and add")
            changes, base_dirs, head_dirs = CI.changed_files(base, "HEAD", cwd=root)
            self.assertEqual(sorted(changes), [("A", "docs/new.md"), ("A", "fresh/nested/file.txt"),
                                               ("D", "docs/old.md"), ("D", "native/src/reader.rs"),
                                               ("M", "README.md")])
            self.assertEqual(base_dirs, {"", "docs", "native", "native/src"})
            self.assertEqual(head_dirs, {"", "docs", "fresh", "fresh/nested"})
            self.assertEqual(CI.listings("D", "native/src/reader.rs", base_dirs, head_dirs),
                             ["native/src", "native", ""])
            self.assertEqual(lanes(changes, base_dirs=base_dirs, head_dirs=head_dirs), ALL)


class RequiredResults(unittest.TestCase):
    def results(self, changes, pull_request=True):
        selected = CI.select(changes)
        scope = CI.platforms(changes)
        return {"changes": {"result": "success", "outputs": {
                    **{k: str(v).lower() for k, v in selected.items()},
                    "platforms": scope}},
                "record": {"result": "success"},
                **{job: {"result": "success" if any(selected[lane] for lane in lanes) else "skipped"}
                   for job, lanes in CI.JOB_LANES.items()}}

    def test_intentional_skips_pass(self):
        for changes in (["README.md"], ["native/src/main.rs"]):
            with self.subTest(changes=changes):
                self.assertEqual(CI.required_failures(self.results(changes)), [])

    def test_selected_job_cannot_silently_skip_fail_or_cancel(self):
        for result in ("skipped", "failure", "cancelled"):
            with self.subTest(result=result):
                needs = self.results(["native/src/main.rs"])
                needs["native-cli"]["result"] = result
                self.assertTrue(CI.required_failures(needs))

    def test_unselected_job_cannot_run_unexpectedly(self):
        needs = self.results(["README.md"])
        needs["native-cli"]["result"] = "success"
        self.assertTrue(CI.required_failures(needs))

    def test_failed_detection_or_missing_output_cannot_pass(self):
        needs = self.results(["README.md"])
        needs["changes"]["result"] = "failure"
        self.assertTrue(CI.required_failures(needs))
        for output in list(CI.LANE_NAMES) + ["platforms"]:
            with self.subTest(output=output):
                needs = self.results(["native/src/main.rs"])
                del needs["changes"]["outputs"][output]
                self.assertTrue(CI.required_failures(needs))

    def test_main_cannot_run_the_pull_request_platform_subset(self):
        needs = self.results(["native/src/main.rs"], pull_request=False)
        self.assertTrue(CI.required_failures(needs, pull_request=False))


class WorkflowCoverage(unittest.TestCase):
    def jobs(self, name="check.yml"):
        return yaml.safe_load((ROOT / ".github/workflows" / name).read_text())["jobs"]

    def test_summary_covers_every_job_and_every_lane(self):
        jobs = self.jobs()
        self.assertEqual(set(jobs["ci-required"]["needs"]), set(jobs) - {"ci-required"})
        self.assertEqual(set(CI.JOB_LANES), set(jobs) - {"ci-required", "changes", "record"})
        self.assertEqual(set(jobs["changes"]["outputs"]), ALL | {"platforms"})
        self.assertEqual({lane for lanes in CI.JOB_LANES.values() for lane in lanes}, ALL)

    def test_native_pull_request_subset_is_the_full_matrix_without_intel_macos(self):
        workflow = (ROOT / ".github/workflows/native-rust.yml").read_text()
        literals = [json.loads(part) for part in workflow.split("'") if part.startswith('[{"runner"')]
        full = max(literals, key=len)
        subset = next(rows for rows in literals if len(rows) == len(full) - 1)
        self.assertEqual(subset, [row for row in full if row["target"] != CI.INTEL_MACOS])
        job = self.jobs()["native-cli"]
        self.assertEqual(job["if"], "needs.changes.outputs.rust == 'true'")
        self.assertIn("without-darwin-x86_64", job["with"]["target"])
        self.assertIn("needs.changes.outputs.platforms == 'all'", job["with"]["target"])

    def test_committed_bundles_are_rejected_before_the_lane_that_reads_them(self):
        steps = self.jobs()["changes"]["steps"]
        gate = next(step for step in steps if "--check-bundles" in step.get("run", ""))
        self.assertEqual(gate["if"], "steps.select.outputs.rust == 'true'")


if __name__ == "__main__":
    unittest.main()
