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
    def test_rust_change_runs_the_rust_lane_on_linux(self):
        changes = ["native/src/ordinary_checked_session.rs", "native/src/public_ordinary_readers.rs",
                   "native/src/source_capture.rs", "native/tests/ordinary_checked_session.rs"]
        self.assertEqual(lanes(changes), {"rust"})
        self.assertEqual(CI.platforms(changes), "linux-x86_64")

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

    def test_ci_machinery_exercises_every_lane_on_linux(self):
        for path in CI.CI_MACHINERY:
            with self.subTest(path=path):
                self.assertEqual(lanes([path]), ALL)
                self.assertEqual(CI.platforms([path]), "linux-x86_64")

    def test_unclaimed_paths_run_everything(self):
        for path in ("new-component/config.toml", "native.toml", ".github/workflows/new.yml"):
            with self.subTest(path=path):
                self.assertEqual(lanes([("A", path)]), ALL)

    def test_shared_platform_inputs_take_every_target_and_ordinary_code_uses_linux(self):
        for path in ("native/Cargo.lock", "native/build.rs", ".github/workflows/native-rust.yml"):
            with self.subTest(path=path):
                self.assertEqual(CI.platforms([path]), "all")
        for path in ("native/src/main.rs", "native/tests/cli.rs", "README.md"):
            with self.subTest(path=path):
                self.assertEqual(CI.platforms([path]), "linux-x86_64")

    def test_release_main_runs_every_lane_on_every_platform(self):
        self.assertEqual(lanes(["README.md"], push=True, release=True), ALL)
        self.assertEqual(CI.platforms(["README.md"], push=True, release=True), "all")

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
                    "platforms": scope, "release": "false"}},
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
        for output in list(CI.LANE_NAMES) + ["platforms", "release"]:
            with self.subTest(output=output):
                needs = self.results(["native/src/main.rs"])
                del needs["changes"]["outputs"][output]
                self.assertTrue(CI.required_failures(needs))

    def test_release_cannot_run_the_pull_request_platform_subset(self):
        needs = self.results(["native/src/main.rs"])
        needs["changes"]["outputs"]["release"] = "true"
        self.assertTrue(CI.required_failures(needs, pull_request=False))
        needs["changes"]["outputs"]["platforms"] = "all"
        self.assertEqual(CI.required_failures(needs, pull_request=False), [])
        self.assertTrue(CI.required_failures(needs, pull_request=True))


class NativeDocumentUI(unittest.TestCase):
    def test_document_inputs_select_the_native_lane(self):
        for path in ('tests/test_document_ui.cjs', 'tests/document_ui_fixture.py',
                     'tests/document_native_bridge.py', 'tests/document-support/package-lock.json',
                     'native/src/annotated_document.rs', 'native/shared/document/layer.js',
                     'native/ci/document_ui.py'):
            self.assertIn('rust', lanes([path]), path)
            self.assertTrue(CI.lane_reads(CI.LANES['rust'], path), path)

    def test_required_linux_native_job_runs_the_dom_suite(self):
        import yaml
        jobs = yaml.safe_load((ROOT / '.github/workflows/native-rust.yml').read_text())['jobs']
        steps = jobs['tests']['steps']
        step = next((s for s in steps if 'python native/ci/document_ui.py' in s.get('run', '')), None)
        self.assertIsNotNone(step, 'the required native test job must execute DOM interaction tests')
        self.assertIn("linux-x86_64", step['if'])
        self.assertNotIn('continue-on-error', step)
        self.assertTrue(any('npm ci' in s.get('run', '') for s in steps))
        self.assertIn('tests', jobs['verdict']['needs'])


class WorkflowCoverage(unittest.TestCase):
    def jobs(self, name="check.yml"):
        return yaml.safe_load((ROOT / ".github/workflows" / name).read_text())["jobs"]

    def test_summary_covers_every_job_and_every_lane(self):
        jobs = self.jobs()
        self.assertEqual(set(jobs["ci-required"]["needs"]), set(jobs) - {"ci-required"})
        self.assertEqual(set(CI.JOB_LANES), set(jobs) - {"ci-required", "changes", "record"})
        self.assertEqual(set(jobs["changes"]["outputs"]), ALL | {"platforms", "release"})
        self.assertEqual({lane for lanes in CI.JOB_LANES.values() for lane in lanes}, ALL)

    def test_native_targets_follow_the_selector_and_start_without_waiting_for_record(self):
        job = self.jobs()["native-cli"]
        self.assertEqual(job["needs"], "changes")
        self.assertEqual(job["if"], "needs.changes.outputs.rust == 'true'")
        self.assertEqual(job["with"]["target"], "${{ needs.changes.outputs.platforms }}")
        self.assertEqual(job["with"]["publish"], "${{ needs.changes.outputs.release == 'true' }}")

    def test_committed_bundles_are_rejected_before_the_lane_that_reads_them(self):
        steps = self.jobs()["changes"]["steps"]
        gate = next(step for step in steps if "--check-bundles" in step.get("run", ""))
        self.assertEqual(gate["if"], "steps.select.outputs.rust == 'true'")


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
