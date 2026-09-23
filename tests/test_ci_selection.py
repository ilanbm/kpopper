"""CI runs the lanes a change reads, from the entire PR, and every tracked file feeds a declared lane."""
import importlib.util
import json
import pathlib
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

    def test_record_job_modules_are_the_ones_the_shards_leave_out(self):
        spec = importlib.util.spec_from_file_location("ci_execution", ROOT / ".github/scripts/ci_execution.py")
        runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runner)
        modules = {path for path in CI.RECORD_JOB if path.startswith("tests/")}
        self.assertEqual(modules, {"tests/" + name for name in runner.RECORD_JOB_MODULES})
        workflow = yaml.safe_load((ROOT / ".github/workflows/check.yml").read_text())
        contracts = "\n".join(step.get("run", "") for step in workflow["jobs"]["record"]["steps"])
        for module in modules:
            self.assertIn(module[:-3].replace("/", "."), contracts)


class Selection(unittest.TestCase):
    def test_rust_change_runs_only_the_rust_lane_without_intel_macos(self):
        # PR #161: four files under native/.
        changes = ["native/src/ordinary_checked_session.rs", "native/src/public_ordinary_readers.rs",
                   "native/src/source_capture.rs", "native/tests/ordinary_checked_session.rs"]
        self.assertEqual(lanes(changes), {"rust"})
        self.assertEqual(CI.platforms(changes), "pull-request")

    def test_documentation_edits_run_no_lane(self):
        # PR #153 edited these; the record job still checks them.
        changes = ["README.md", "assets/README.md", "assets/brand-guide.md", ".kpopper/view.yaml",
                   "docs/ci.md", "CHANGELOG.md", "SECURITY.md", "CODE_OF_CONDUCT.md", "skills/ground/SKILL.md",
                   "examples/dark-matter/README.md", "examples/offer-review/GROUNDING.yaml",
                   ".github/pull_request_template.md", ".github/ISSUE_TEMPLATE/bug_report.yml", "native/README.md"]
        for path in changes:
            with self.subTest(path=path):
                self.assertEqual(lanes([path]), set())

    def test_the_project_record_runs_the_tests_that_read_it(self):
        # The contract, priors and remeasure tests check this repository's own record.
        self.assertEqual(lanes(["GROUNDING.yaml"]), {"python", "session"})
        self.assertEqual(lanes([".kpopper/measure.yaml"]), {"python", "session"})

    def test_release_scripts_and_their_tests_run_only_the_record_job(self):
        # PR #155.
        self.assertEqual(lanes([".github/scripts/publish_release.py", "tests/test_release.py"]), set())

    def test_adding_a_file_runs_the_lanes_that_list_its_directory(self):
        added = ("A", "assets/navigation/new-title.svg")
        expected = {name for name, lane in CI.LANES.items() if CI.lane_lists(lane, "assets/navigation")}
        self.assertEqual(lanes([added], base_dirs={"", "assets", "assets/navigation"}), expected)
        self.assertEqual(lanes([("M", "assets/navigation/README.md")]), set())

    def test_listings_reach_the_first_directory_that_existed(self):
        self.assertEqual(CI.listings("A", "examples/new/deep/x.yaml", base_dirs={"", "examples"}),
                         ["examples/new/deep", "examples/new", "examples"])
        self.assertEqual(CI.listings("D", "docs/sub/only.md", head_dirs={"", "docs"}), ["docs/sub", "docs"])
        self.assertEqual(CI.listings("A", "docs/x.md"), ["docs"])
        self.assertEqual(CI.listings("M", "docs/x.md", base_dirs={""}, head_dirs={""}), [])
        self.assertEqual(CI.listings("A", "top.md", base_dirs={""}), [""])

    def test_shared_python_reaches_every_python_consumer(self):
        selected = lanes(["scripts/provenance.py"])
        self.assertTrue({"python", "documents", "session", "installed"} <= selected, selected)
        self.assertNotIn("runtime", selected)

    def test_compiled_in_assets_reach_the_rust_lane(self):
        for path in ("scripts/page/page.css", "scripts/document/layer.js", "scripts/session/rules.txt",
                     "scripts/start-guide.md", "scripts/session/lean/Main.lean"):
            with self.subTest(path=path):
                self.assertIn("rust", lanes([path]))

    def test_rust_lane_reads_what_its_sources_embed_and_the_scripts_its_tests_run(self):
        # include_str!/include_bytes! targets and the hooks host_hooks.rs executes, with their imports.
        for path in ("scripts/verify_page.js", "examples/offer-review/after/GROUNDING.yaml", "scripts/expressions.py",
                     "scripts/ground_hook.py", "scripts/edit_hook.py", "scripts/followups.py"):
            with self.subTest(path=path):
                self.assertIn("rust", lanes([path]))
        self.assertNotIn("rust", lanes(["scripts/render_page.py"]))
        self.assertTrue(CI.rust_embeds(("native",)) >= {"native/shared/verify_page.js", "native/shared/session/rules.txt"})

    def test_installed_lane_follows_the_imports_of_the_reasoning_tests_it_runs(self):
        # Including an import inside code the acceptance test hands to the installed interpreter.
        for path in ("tests/test_pending_grounding.py", "tests/test_history_authoring.py",
                     "tests/test_history_snapshot_capture.py", "tests/test_reasoning_contract.py"):
            with self.subTest(path=path):
                self.assertIn("installed", lanes([path]))

    def test_runtime_sources_rebuild_and_validate_installation(self):
        for path in ("scripts/reasoning/lean/Kernel.lean", "scripts/reasoning/build_runtime.py",
                     "scripts/reasoning/native/linux-x86_64.kpopper-runtime",
                     "scripts/reasoning/third_party/COPYING.LESSERv3", "tests/test_reasoning_distribution.py"):
            with self.subTest(path=path):
                self.assertTrue({"runtime", "installed"} <= lanes([path]))
        self.assertTrue(CI.select(["README.md"], full=True)["installed"])

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
        for path in ("native/Cargo.lock", "native/build.rs", "install.ps1", ".github/workflows/native-rust.yml",
                     "scripts/reasoning/native/darwin-x86_64.kpopper-runtime", "pyproject.toml"):
            with self.subTest(path=path):
                self.assertEqual(CI.platforms([path]), "all")
        for path in ("native/src/main.rs", "scripts/cli.py", "tests/test_session.py"):
            with self.subTest(path=path):
                self.assertEqual(CI.platforms([path]), "pull-request")
        self.assertNotIn(CI.INTEL_MACOS, {row["target"] for row in CI.runtime_targets("pull-request")})
        self.assertEqual(CI.runtime_targets("all"), [dict(row) for row in CI.RUNTIME_TARGETS])

    def test_main_runs_every_lane_on_every_platform_and_rebuilds_only_changed_runtime(self):
        self.assertEqual(lanes(["README.md"], push=True), ALL - {"runtime"})
        self.assertEqual(CI.platforms(["README.md"], push=True), "all")
        self.assertEqual(lanes(["scripts/reasoning/lean/Kernel.lean"], push=True), ALL)

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
            (root / "scripts/document").mkdir(parents=True)
            (root / "scripts/document/layer.js").write_text("old")
            (root / "docs").mkdir()
            (root / "docs/old.md").write_text("documentation")
            (root / "README.md").write_text("readme")
            commit("base")
            base = git("rev-parse", "HEAD")
            (root / "scripts/document/layer.js").unlink()
            commit("delete runtime asset")
            (root / "docs/old.md").rename(root / "docs/new.md")
            (root / "README.md").write_text("changed")
            (root / "fresh/nested").mkdir(parents=True)
            (root / "fresh/nested/file.txt").write_text("new")
            commit("rename, edit and add")
            changes, base_dirs, head_dirs = CI.changed_files(base, "HEAD", cwd=root)
            self.assertEqual(sorted(changes), [("A", "docs/new.md"), ("A", "fresh/nested/file.txt"),
                                               ("D", "docs/old.md"), ("D", "scripts/document/layer.js"),
                                               ("M", "README.md")])
            self.assertEqual(base_dirs, {"", "docs", "scripts", "scripts/document"})
            self.assertEqual(head_dirs, {"", "docs", "fresh", "fresh/nested"})
            self.assertEqual(CI.listings("D", "scripts/document/layer.js", base_dirs, head_dirs),
                             ["scripts/document", "scripts", ""])
            self.assertEqual(lanes(changes, base_dirs=base_dirs, head_dirs=head_dirs), ALL)


class RequiredResults(unittest.TestCase):
    def results(self, changes, pull_request=True):
        selected = CI.select(changes)
        scope = CI.platforms(changes)
        return {"changes": {"result": "success", "outputs": {
                    **{k: str(v).lower() for k, v in selected.items()},
                    "platforms": scope,
                    "test_suites": json.dumps(CI.test_suites(selected)),
                    "test_matrix": json.dumps(CI.test_matrix(selected, pull_request)),
                    "runtime_targets": json.dumps(CI.runtime_targets(scope))}},
                "record": {"result": "success"},
                **{job: {"result": "success" if any(selected[lane] for lane in lanes) else "skipped"}
                   for job, lanes in CI.JOB_LANES.items()}}

    def test_intentional_skips_pass(self):
        for changes in (["README.md"], ["native/src/main.rs"], ["scripts/cli.py"]):
            with self.subTest(changes=changes):
                self.assertEqual(CI.required_failures(self.results(changes)), [])

    def test_selected_job_cannot_silently_skip_fail_or_cancel(self):
        for job, changes in (("native-cli", ["native/src/main.rs"]), ("session", ["scripts/cli.py"]),
                             ("reasoning-runtime", ["scripts/cli.py"]), ("check", ["scripts/documents.py"])):
            for result in ("skipped", "failure", "cancelled"):
                with self.subTest(job=job, result=result):
                    needs = self.results(changes)
                    needs[job]["result"] = result
                    self.assertTrue(CI.required_failures(needs))

    def test_unselected_job_cannot_run_unexpectedly(self):
        needs = self.results(["native/src/main.rs"])
        needs["check"]["result"] = "success"
        self.assertTrue(CI.required_failures(needs))

    def test_failed_detection_or_missing_output_cannot_pass(self):
        needs = self.results(["README.md"])
        needs["changes"]["result"] = "failure"
        self.assertTrue(CI.required_failures(needs))
        for output in list(CI.LANE_NAMES) + ["platforms", "runtime_targets", "test_suites", "test_matrix"]:
            with self.subTest(output=output):
                needs = self.results(["scripts/cli.py"])
                del needs["changes"]["outputs"][output]
                self.assertTrue(CI.required_failures(needs))

    def test_main_cannot_run_the_pull_request_platform_subset(self):
        needs = self.results(["scripts/cli.py"], pull_request=False)
        self.assertTrue(CI.required_failures(needs, pull_request=False))

    def test_truncated_plans_and_matrices_cannot_pass(self):
        needs = self.results(["scripts/cli.py"])
        matrix = json.loads(needs["changes"]["outputs"]["test_matrix"])
        matrix["include"].pop()
        needs["changes"]["outputs"]["test_matrix"] = json.dumps(matrix)
        self.assertTrue(CI.required_failures(needs))
        needs = self.results(["scripts/cli.py"])
        needs["changes"]["outputs"]["runtime_targets"] = json.dumps(CI.runtime_targets("pull-request")[:-1])
        self.assertTrue(CI.required_failures(needs))
        needs = self.results(["scripts/cli.py"])
        needs["changes"]["outputs"]["test_suites"] = '["documents"]'
        self.assertTrue(CI.required_failures(needs))


class WorkflowCoverage(unittest.TestCase):
    def jobs(self, name="check.yml"):
        return yaml.safe_load((ROOT / ".github/workflows" / name).read_text())["jobs"]

    def test_summary_covers_every_job_and_every_lane(self):
        jobs = self.jobs()
        self.assertEqual(set(jobs["ci-required"]["needs"]), set(jobs) - {"ci-required"})
        self.assertEqual(set(CI.JOB_LANES), set(jobs) - {"ci-required", "changes", "record"})
        self.assertEqual(set(jobs["changes"]["outputs"]),
                         ALL | {"platforms", "test_suites", "test_matrix", "runtime_targets"})
        self.assertEqual({lane for lanes in CI.JOB_LANES.values() for lane in lanes}, ALL)

    def test_runtime_targets_are_the_producer_matrix_called_directly(self):
        producer = yaml.safe_load((ROOT / ".github/workflows/reasoning-runtime.yml").read_text())
        self.assertEqual([dict(row) for row in producer["jobs"]["target"]["strategy"]["matrix"]["include"]],
                         [dict(row) for row in CI.RUNTIME_TARGETS])
        job = self.jobs()["reasoning-runtime"]
        self.assertEqual(job["uses"], "./.github/workflows/reasoning-target.yml")
        self.assertEqual(job["strategy"]["matrix"]["include"], "${{ fromJSON(needs.changes.outputs.runtime_targets) }}")
        self.assertEqual(job["if"], "needs.changes.outputs.installed == 'true'")
        self.assertEqual(job["with"]["rebuild"], "${{ needs.changes.outputs.runtime == 'true' }}")

    def test_native_pull_request_subset_is_the_full_matrix_without_intel_macos(self):
        workflow = (ROOT / ".github/workflows/native-rust.yml").read_text()
        literals = [json.loads(part) for part in workflow.split("'") if part.startswith('[{"runner"')]
        full = max(literals, key=len)
        subset = next(rows for rows in literals if len(rows) == len(full) - 1)
        self.assertEqual(subset, [row for row in full if row["target"] != CI.INTEL_MACOS])
        self.assertEqual({row["target"] for row in full}, {row["target"] for row in CI.RUNTIME_TARGETS})
        job = self.jobs()["native-cli"]
        self.assertEqual(job["if"], "needs.changes.outputs.rust == 'true'")
        self.assertIn("without-darwin-x86_64", job["with"]["target"])
        self.assertIn("needs.changes.outputs.platforms == 'all'", job["with"]["target"])

    def test_shards_cover_supported_interpreters_and_docs_avoid_extra_machines(self):
        rows = CI.test_matrix(CI.select(["scripts/cli.py"]))["include"]
        self.assertEqual({r["group"] for r in rows if r["python"] == "3.9"}, set(range(1, 9)))
        self.assertEqual({r["group"] for r in rows if r["python"] == "3.13"}, set(range(1, 5)))
        docs = CI.test_matrix({lane: lane == "documents" for lane in CI.LANE_NAMES})["include"]
        self.assertEqual(len(docs), 2)
        self.assertTrue(all(r["splits"] == 1 for r in docs))
        main = CI.test_matrix(CI.select(["scripts/cli.py"]), pull_request=False)["include"]
        self.assertEqual({r["python"] for r in main}, {"3.13"})
        self.assertEqual(len(main), 4)

    def test_focused_example_runs_installed_package_on_supported_python_versions(self):
        jobs = self.jobs()
        example = jobs["examples"]
        self.assertEqual(set(example["needs"]), {"changes", "record"})
        self.assertEqual(example["if"], "needs.changes.outputs.examples == 'true'")
        self.assertEqual(example["strategy"]["matrix"]["python"], ["3.9", "3.13"])
        commands = [step.get("run", "") for step in example["steps"]]
        self.assertIn("python -m pip install .", commands)
        self.assertIn("python .github/scripts/check_research_example.py", commands)
        self.assertIn("git diff --exit-code", commands)
        integrity = next(step for step in jobs["changes"]["steps"] if "--check-bundles" in step.get("run", ""))
        self.assertIn("steps.select.outputs.examples == 'true'", integrity["if"])
        self.assertIn("steps.select.outputs.installed == 'true'", integrity["if"])

    def test_workflow_executes_the_declared_matrix_and_verifies_its_manifests(self):
        jobs = self.jobs()
        self.assertEqual(jobs["check"]["strategy"]["matrix"], "${{ fromJSON(needs.changes.outputs.test_matrix) }}")
        commands = "\n".join(s.get("run", "") for s in jobs["ci-required"]["steps"])
        self.assertIn("--verify-results", commands)
        self.assertIn("--matrix", commands)


if __name__ == "__main__":
    unittest.main()
