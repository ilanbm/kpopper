"""CI runs the lanes a change reads, from the entire PR, and every tracked file feeds a declared lane."""
import importlib.util
import json
import os
import pathlib
import re
import shlex
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
    def test_native_crate_change_runs_both_lanes_without_intel_macos(self):
        changes = ["native/src/ordinary_checked_session.rs", "native/src/public_ordinary_readers.rs",
                   "native/src/source_capture.rs", "native/tests/ordinary_checked_session.rs"]
        self.assertEqual(lanes(changes), {"rust", "runtime"})
        self.assertEqual(CI.platforms(changes), "pull-request")

    def test_reasoning_runtime_inputs_select_the_runtime_lane(self):
        for path in ("scripts/reasoning/lean/Main.lean", "scripts/reasoning/build_runtime.py",
                     "native/build.rs", "native/src/reasoning_query.rs",
                     "native/tests/reasoning_runtime.rs"):
            with self.subTest(path=path):
                self.assertIn("runtime", lanes([path]))

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
        expected = {name for name, lane in CI.LANES.items()
                    if CI.lane_lists(lane, "native/src") or CI.lane_reads(lane, added[1])}
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

    def test_main_runs_every_lane_except_runtime_when_its_inputs_are_unchanged(self):
        self.assertEqual(lanes(["README.md"], push=True), ALL - {"runtime"})
        self.assertEqual(CI.platforms(["README.md"], push=True), "all")

    def test_main_rebuilds_runtime_when_its_inputs_change(self):
        self.assertEqual(lanes(["scripts/reasoning/lean/Main.lean"], push=True), ALL)
        self.assertEqual(CI.platforms(["scripts/reasoning/lean/Main.lean"], push=True), "all")

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

    def test_reusable_workflow_calls_match_declared_inputs(self):
        workflow_dir = ROOT / ".github/workflows"
        for caller_path in sorted(workflow_dir.glob("*.yml")):
            caller = yaml.safe_load(caller_path.read_text())
            for job_name, job in caller.get("jobs", {}).items():
                target = job.get("uses", "")
                if not target.startswith("./.github/workflows/"):
                    continue
                callee_path = ROOT / target.removeprefix("./")
                callee = yaml.safe_load(callee_path.read_text())
                triggers = callee.get("on", callee.get(True, {}))
                declared = triggers.get("workflow_call", {}).get("inputs", {})
                supplied = job.get("with", {})
                with self.subTest(caller=caller_path.name, job=job_name):
                    self.assertLessEqual(set(supplied), set(declared))
                    required = {name for name, spec in declared.items()
                                if spec.get("required") is True and "default" not in spec}
                    self.assertLessEqual(required, set(supplied))

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

    def test_runtime_matrix_respects_pr_platform_scope_and_dispatch_defaults_to_all(self):
        workflow = yaml.safe_load((ROOT / ".github/workflows/reasoning-runtime.yml").read_text())
        matrix = workflow["jobs"]["target"]["strategy"]["matrix"]["include"]
        self.assertIn("inputs.target-scope == 'pull-request'", matrix)
        for target in ("linux-x86_64", "linux-aarch64", "darwin-arm64", "windows-x86_64"):
            self.assertIn(target, matrix)
        self.assertIn("darwin-x86_64", matrix)
        dispatch = workflow.get("on", workflow.get(True))["workflow_dispatch"]["inputs"]
        self.assertEqual(dispatch["target-scope"]["default"], "all")

    def test_reasoning_dependency_cache_is_saved_only_by_main_pushes(self):
        workflow = self.jobs("reasoning-target.yml")
        build = workflow["build"]
        save = next(step for step in build["steps"]
                    if step.get("name") == "Save successfully built dependencies before running integration checks")
        self.assertIn("github.ref == 'refs/heads/main'", save["if"])
        self.assertIn("github.event_name == 'workflow_dispatch'", save["if"])
        self.assertIn("github.event_name == 'push'", save["if"])

    def test_reasoning_target_always_builds_and_full_mode_checks_the_artifact_round_trip(self):
        jobs = self.jobs("reasoning-target.yml")
        self.assertNotIn("if", jobs["build"])
        check = next(step for step in jobs["candidate-validation"]["steps"]
                     if step.get("name") == "Verify the downloaded candidate archive and sidecar")
        self.assertIn("hashlib.sha256", check["run"])
        self.assertEqual(jobs["candidate-validation"]["runs-on"], "ubuntu-latest")
        uploaded = [step.get("with", {}).get("name") for step in jobs["build"]["steps"]
                    if "upload-artifact" in step.get("uses", "")]
        self.assertNotIn("gmp-replacement-${{ inputs.target }}", uploaded)

    def test_main_checks_are_not_cancelled_by_a_later_main_commit(self):
        workflow = yaml.safe_load((ROOT / ".github/workflows/check.yml").read_text())
        self.assertIn("github.sha", workflow["concurrency"]["group"])
        self.assertEqual(workflow["concurrency"]["cancel-in-progress"],
                         "${{ github.event_name == 'pull_request' }}")
        native = yaml.safe_load((ROOT / ".github/workflows/native-rust.yml").read_text())
        self.assertIn("github.sha", native["concurrency"]["group"])
        self.assertIn("inputs.validation", native["concurrency"]["group"])
        self.assertEqual(native["concurrency"]["cancel-in-progress"],
                         "${{ github.event_name != 'push' }}")
        check_call = self.jobs()["native-cli"]["with"]
        publish = yaml.safe_load((ROOT / ".github/workflows/publish.yml").read_text())
        publish_call = publish["jobs"]["build"]["with"]
        self.assertNotIn("validation", check_call)
        self.assertEqual(publish_call["validation"], "distribution")

    def test_called_runtime_runs_have_unique_non_cancelling_groups(self):
        workflow = yaml.safe_load((ROOT / ".github/workflows/reasoning-runtime.yml").read_text())
        self.assertIn("github.run_id", workflow["concurrency"]["group"])
        self.assertFalse(workflow["concurrency"]["cancel-in-progress"])

    def test_committed_bundles_are_rejected_before_the_lane_that_reads_them(self):
        steps = self.jobs()["changes"]["steps"]
        gate = next(step for step in steps if "--check-bundles" in step.get("run", ""))
        self.assertEqual(gate["if"], "steps.select.outputs.rust == 'true' || steps.select.outputs.runtime == 'true'")


# Each runner of the native matrix, as its RUNNER_OS/RUNNER_ARCH name it.
RUNNER_PLATFORMS = {"ubuntu-24.04": "Linux/X64", "ubuntu-24.04-arm": "Linux/ARM64", "macos-14": "macOS/ARM64",
                    "macos-15-intel": "macOS/X64", "windows-2022": "Windows/X64"}
# The attributes that keep a fenced block of a doc comment Rust code for rustdoc.
RUSTDOC_ATTRIBUTES = {"rust", "ignore", "no_run", "should_panic", "compile_fail", "test_harness", "standalone_crate"}


class NativeTestPool(unittest.TestCase):
    """The native tests run in one nextest pool: pinned, checked, never retried, and all that cargo lists."""

    def jobs(self):
        return yaml.safe_load((ROOT / ".github/workflows/native-rust.yml").read_text())["jobs"]

    def only(self, steps, predicate):
        found = [index for index, step in enumerate(steps) if predicate(step)]
        self.assertEqual(len(found), 1, found)
        return found[0]

    def commands(self, run):
        """The cargo commands of each validation in the test step's case statement."""
        commands, mode = {}, None
        for line in (line.strip() for line in run.splitlines()):
            label = re.fullmatch(r"([\w-]+|\*)\)", line)
            if label:
                mode = label.group(1)
                commands[mode] = []
            elif line == ";;":
                mode = None
            elif mode and line.startswith("cargo "):
                commands[mode].append(shlex.split(line))
        return commands

    def test_tests_job_installs_nextest_at_a_pinned_version_checked_by_sha256(self):
        jobs = self.jobs()
        steps = jobs["tests"]["steps"]
        install = self.only(steps, lambda step: "cargo-nextest-" in step.get("run", ""))
        tests = self.only(steps, lambda step: step.get("id") == "native-tests")
        self.assertLess(install, tests)
        # Skipped for a distribution, as the tests are.
        self.assertEqual(steps[install]["if"], "inputs.validation != 'distribution'")
        self.assertEqual(steps[install]["if"], steps[tests]["if"])
        script = steps[install]["run"]
        [version] = re.findall(r"^\s*version=(\d+\.\d+\.\d+)$", script, re.M)
        pinned = {}
        for platforms, asset, digest in re.findall(r"^\s*([\w/ |]+)\) asset=(\S+) sha256=(\S+) ;;$", script, re.M):
            for platform in platforms.split("|"):
                pinned[platform.strip()] = (asset, digest)
        include = jobs["tests"]["strategy"]["matrix"]["include"]
        rows = max((json.loads(text) for text in include.split("'")[1::2] if text.startswith("[")), key=len)
        self.assertEqual({RUNNER_PLATFORMS[row["runner"]] for row in rows}, set(pinned))
        for platform, (asset, digest) in pinned.items():
            with self.subTest(platform=platform):
                self.assertRegex(asset, r"^cargo-nextest-" + re.escape(version) + r"-[\w-]+\.tar\.gz$")
                self.assertRegex(digest, r"^[0-9a-f]{64}$")
        self.assertIn("/releases/download/cargo-nextest-$version/$asset", script)
        # The archive is refused unless it has the pinned sha256, before anything is extracted.
        extract = script.index("tar -x")
        for check in ("sha256sum", "shasum -a 256", '!= "$sha256"'):
            with self.subTest(check=check):
                self.assertLess(script.index(check), extract)
        self.assertIn('>> "$GITHUB_PATH"', script)

    def test_every_validation_tests_through_nextest_and_full_runs_the_whole_suite(self):
        steps = self.jobs()["tests"]["steps"]
        commands = self.commands(steps[self.only(steps, lambda step: step.get("id") == "native-tests")]["run"])
        self.assertEqual(set(commands), {"full", "hooks", "final-fixes", "*"})
        for mode in ("full", "hooks", "final-fixes"):
            self.assertTrue(commands[mode], mode)
            for command in commands[mode]:
                with self.subTest(mode=mode, command=command):
                    self.assertEqual(command[:3], ["cargo", "nextest", "run"])
                    self.assertIn("--locked", command)
                    self.assertIn("--no-fail-fast", command)
                    self.assertEqual(command[command.index("--profile") + 1], "ci")
        # One pool of every test: no target selection and no filter.
        [full] = commands["full"]
        self.assertEqual(sorted(full[3:]), sorted(["--locked", "--no-fail-fast", "--profile", "ci"]))
        # A retry could turn a failure green, and the command line or the environment would override the profile.
        workflow = (ROOT / ".github/workflows/native-rust.yml").read_text()
        self.assertNotIn("--retries", workflow)
        self.assertNotIn("NEXTEST_RETRIES", workflow)

    @unittest.skipIf(importlib.util.find_spec("tomllib") is None, "tomllib reads TOML from Python 3.11")
    def test_ci_profile_never_retries_and_stops_a_hang_well_inside_the_job(self):
        import tomllib  # The record job runs Python 3.13.
        config = tomllib.loads((ROOT / "native/.config/nextest.toml").read_text(encoding="utf-8"))
        profile = config["profile"]["ci"]
        self.assertEqual(profile["retries"], 0)
        self.assertIs(profile["fail-fast"], False)

        def retries(value):
            if isinstance(value, dict):
                for key, item in value.items():
                    if key == "retries":
                        yield item
                    yield from retries(item)
            elif isinstance(value, list):
                for item in value:
                    yield from retries(item)

        # Any profile's retries or a per-test override's would reach this profile as well.
        self.assertEqual([value for value in retries(config) if value != 0], [])
        timeout = profile["slow-timeout"]
        period = re.fullmatch(r"(\d+)s", timeout["period"])
        self.assertTrue(period, timeout["period"])
        self.assertEqual(timeout.get("on-timeout", "fail"), "fail")
        # A hung test fails under its own name, and the job has time left to finish the rest.
        job = self.jobs()["tests"]["timeout-minutes"] * 60
        self.assertLess(int(period.group(1)) * timeout["terminate-after"], job / 2)
        self.assertEqual(profile["junit"]["path"], "junit.xml")

    def test_run_keeps_its_junit_report_and_a_listing_check_follows_it(self):
        steps = self.jobs()["tests"]["steps"]
        tests = self.only(steps, lambda step: step.get("id") == "native-tests")
        report = self.only(steps, lambda step: "native/target/nextest/ci/junit.xml" in step.get("run", ""))
        evidence = self.only(steps, lambda step: (step.get("with") or {}).get("name")
                             == "native-test-evidence-${{ matrix.target }}")
        self.assertLess(tests, report)
        self.assertLess(report, evidence)
        self.assertEqual(steps[report]["if"], "${{ !cancelled() }}")
        self.assertIn('"$RUNNER_TEMP/native-evidence/', steps[report]["run"])
        listing = self.only(steps, lambda step: "ci/nextest_listing.py" in step.get("run", ""))
        self.assertLess(tests, listing)
        self.assertEqual(steps[listing]["working-directory"], steps[tests]["working-directory"])
        self.assertEqual(steps[listing]["if"], "env.KPOPPER_CI_VALIDATION == 'full'")
        # The listings read the build the full run made: its cargo arguments, without nextest's own.
        [full] = self.commands(steps[tests]["run"])["full"]
        build = full[3:]
        at = build.index("--profile")
        profile = build[at + 1]
        del build[at:at + 2]
        build.remove("--no-fail-fast")
        self.assertEqual(shlex.split(steps[listing]["run"]), ["python", "ci/nextest_listing.py", *build])
        script = (ROOT / "native/ci/nextest_listing.py").read_text(encoding="utf-8")
        self.assertEqual(re.findall(r'^PROFILE = "([\w-]+)"$', script, re.M), [profile])

    def test_only_the_tests_job_uses_nextest_and_the_oracle_comparison_stays_on_cargo_test(self):
        jobs = self.jobs()
        for name, job in jobs.items():
            with self.subTest(job=name):
                self.assertEqual("nextest" in json.dumps(job), name == "tests")
        steps = jobs["tests"]["steps"]
        oracle = steps[self.only(steps, lambda step: "-- --ignored" in step.get("run", ""))]["run"]
        self.assertIn("cargo test ", oracle)
        self.assertNotIn("nextest", oracle)

    def test_native_sources_hold_no_doc_test_that_nextest_would_skip(self):
        def runs(info):
            """Whether rustdoc runs a fenced block with this info string as a doc-test."""
            tokens = {token for token in re.split(r"[\s,{}]+", info) if token}
            other = {token for token in tokens if token not in RUSTDOC_ATTRIBUTES
                     and not re.fullmatch(r"edition\d+|ignore-[\w-]+|E\d{4}", token)}
            if other and "rust" not in tokens:
                return False  # text, or another language
            return not tokens & {"ignore", "no_run"}

        found = []
        for source in sorted((ROOT / "native/src").rglob("*.rs")):
            fence = None
            for number, line in enumerate(source.read_text(encoding="utf-8").splitlines(), 1):
                doc = re.match(r"\s*//[/!](?!/)(.*)", line)
                if not doc:
                    fence = None
                    continue
                marker = re.match(r"\s{0,4}(`{3,}|~{3,})(.*)", doc.group(1))
                if fence:
                    if (marker and marker.group(1)[0] == fence[0] and len(marker.group(1)) >= len(fence)
                            and not marker.group(2).strip()):
                        fence = None
                elif marker:
                    fence = marker.group(1)
                    if runs(marker.group(2)):
                        found.append("%s:%d" % (source.relative_to(ROOT).as_posix(), number))
        steps = self.jobs()["tests"]["steps"]
        if found and not any(re.search(r"cargo test\b.*--doc", step.get("run", "")) for step in steps):
            self.fail("nextest skips doc-tests, so the workflow must also run `cargo test --doc`: "
                      + ", ".join(found))


if __name__ == "__main__":
    unittest.main()
