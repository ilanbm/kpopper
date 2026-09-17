"""CI selects work from the entire PR, including deletions and renames."""
import importlib.util
import pathlib
import subprocess
import tempfile
import unittest

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("ci_selection", ROOT / ".github/scripts/ci_selection.py")
CI = importlib.util.module_from_spec(spec)
spec.loader.exec_module(CI)


class Selection(unittest.TestCase):
    def test_skill_and_record_pr_avoids_runtime_jobs(self):
        selected = CI.select(["skills/ground/SKILL.md", "GROUNDING.yaml", "tests/test_skills.py"])
        self.assertFalse(any(selected.values()), selected)

    def test_community_pr_keeps_only_mandatory_checks(self):
        paths = [".github/ISSUE_TEMPLATE/bug_report.yml", ".github/ISSUE_TEMPLATE/config.yml",
                 ".github/ISSUE_TEMPLATE/feature_request.yml", ".github/ISSUE_TEMPLATE/question.yml",
                 ".github/pull_request_template.md", ".gitignore", "CODE_OF_CONDUCT.md",
                 "CONTRIBUTING.md", "GROUNDING.yaml", "README.md", "SECURITY.md"]
        for path in paths:
            with self.subTest(path=path):
                self.assertFalse(any(CI.select([path]).values()))
        self.assertFalse(any(CI.select(paths).values()))

    def test_selector_and_its_tests_use_the_mandatory_contract_job(self):
        self.assertFalse(any(CI.select([".github/scripts/ci_selection.py",
                                       "tests/test_ci_selection.py"]).values()))

    def test_community_paths_do_not_hide_mixed_runtime_changes(self):
        selected = CI.select(["SECURITY.md", ".github/ISSUE_TEMPLATE/bug_report.yml",
                              "scripts/document/layer.js"])
        self.assertEqual(selected, dict.fromkeys(CI.LANES, False) | {"documents": True})
        self.assertTrue(all(CI.select(["SECURITY.md", "scripts/reasoning/lean/Kernel.lean"]).values()))

    def test_documents_run_python_and_dom_document_tests(self):
        for path in ("scripts/documents.py", "scripts/document/layer.js", "scripts/document-guide.md",
                     "tests/test_document_cli.py", "tests/test_documents.py", "tests/document_ui_fixture.py",
                     "tests/document-support/package-lock.json"):
            with self.subTest(path=path):
                self.assertEqual(CI.select([path]), dict.fromkeys(CI.LANES, False) | {"documents": True})

    def test_shared_reader_and_cli_run_all_consumers_without_recompiling(self):
        for path in ("scripts/provenance.py", "scripts/cli.py", "scripts/session/core.py",
                     "scripts/reasoning/evaluate.py", "tests/test_session.py", "tests/fixtures/page/PROVENANCE.yaml"):
            with self.subTest(path=path):
                selected = CI.select([path])
                self.assertTrue(all(selected[k] for k in CI.LANES if k != "native"), selected)
                self.assertFalse(selected["native"])

    def test_packaging_changes_test_installation_without_native_compilation(self):
        for path in ("pyproject.toml", "package.json", ".claude-plugin/plugin.json", ".codex-plugin/plugin.json"):
            with self.subTest(path=path):
                selected = CI.select([path])
                self.assertTrue(selected["installed"])
                self.assertTrue(selected["session"])
                self.assertFalse(selected["native"])

    def test_command_rename_does_not_rebuild_unchanged_native_sources(self):
        # PR #106 changed these consumer/configuration paths, but no native input.
        selected = CI.select([".github/pull_request_template.md", ".github/workflows/check.yml",
                              ".github/workflows/session.yml", "skills/watch/agents/openai.yaml",
                              "examples/merge-assumptions/README.md",
                              "pyproject.toml", "scripts/cli.py", "scripts/session/core.py",
                              "tests/test_predicate_literals.py", "scripts/__init__.py"])
        self.assertTrue(all(selected[k] for k in CI.LANES if k != "native"))
        self.assertFalse(selected["native"])

    def test_native_inputs_and_probe_changes_keep_the_full_audit(self):
        for path in ("scripts/reasoning/lean/Kernel.lean", "scripts/reasoning/lean/lean-toolchain",
                     "scripts/reasoning/build_runtime.py", "scripts/reasoning/native/linux-x86_64.zip",
                     "scripts/reasoning/runtime.py", "tests/test_reasoning_runtime.py",
                     "scripts/reasoning/native/gmp-source-and-build.tar.gz",
                     "scripts/reasoning/third_party/COPYING.LESSERv3", "tests/test_reasoning_distribution.py"):
            with self.subTest(path=path):
                self.assertTrue(all(CI.select([path]).values()))

    def test_unknown_files_and_ci_changes_fail_open_to_full_checks(self):
        for path in ("new-component/config.toml", "new-file.md", "skills/ground/helper.py",
                     "docs/check.py", ".kpopper/new-hook.sh", ".github/workflows/reasoning-runtime.yml",
                     ".github/scripts/new-helper.py", ".github/ISSUE_TEMPLATE/helper.py",
                     ".github/ISSUE_TEMPLATE/nested/config.yml", ".github/workflows/new.yml"):
            with self.subTest(path=path):
                self.assertTrue(all(CI.select([path]).values()))

    def test_mixed_pr_takes_union_and_empty_diff_runs_everything(self):
        self.assertTrue(all(CI.select([]).values()))
        selected = CI.select(["README.md", "scripts/document/layer.css", "pyproject.toml"])
        self.assertTrue(selected["documents"])
        self.assertTrue(selected["installed"])
        self.assertFalse(selected["native"])

    def test_main_checks_all_consumers_but_only_rebuilds_changed_native_inputs(self):
        selected = CI.select(["README.md"], push=True)
        self.assertTrue(all(selected[k] for k in CI.LANES if k != "native"))
        self.assertFalse(selected["native"])
        self.assertTrue(all(CI.select(["scripts/reasoning/lean/Kernel.lean"], push=True).values()))

    def test_manual_run_can_force_the_full_native_audit(self):
        self.assertTrue(all(CI.select(["README.md"], full=True).values()))

    def test_missing_history_runs_full(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertTrue(all(CI.select(CI.changed_files("missing", "HEAD", cwd=directory)).values()))


class GitRange(unittest.TestCase):
    def test_all_pr_commits_deletions_and_both_rename_paths_are_selected(self):
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
            (root / "README.md").write_text("documentation")
            commit("base")
            base = git("rev-parse", "HEAD")
            (root / "scripts/document/layer.js").unlink()
            commit("delete runtime asset")
            (root / "README.md").rename(root / "new-file.md")
            commit("rename to unknown path")
            paths = CI.changed_files(base, "HEAD", cwd=root)
            self.assertEqual(set(paths), {"scripts/document/layer.js", "README.md", "new-file.md"})
            self.assertTrue(all(CI.select(paths).values()))


class RequiredResults(unittest.TestCase):
    def results(self, selected):
        return {"changes": {"result": "success", "outputs": {k: str(v).lower() for k, v in selected.items()}},
                "record": {"result": "success"},
                **{job: {"result": "success" if any(selected[lane] for lane in lanes) else "skipped"}
                   for job, lanes in CI.JOB_LANES.items()}}

    def test_intentional_skips_pass(self):
        self.assertEqual(CI.required_failures(self.results(CI.select(["README.md"]))), [])

    def test_document_only_change_requires_both_python_and_dom_checks(self):
        needs = self.results(CI.select(["scripts/documents.py"]))
        self.assertEqual(CI.required_failures(needs), [])
        needs["check"]["result"] = "skipped"
        self.assertTrue(CI.required_failures(needs))

    def test_selected_job_cannot_silently_skip_fail_or_cancel(self):
        for result in ("skipped", "failure", "cancelled"):
            with self.subTest(result=result):
                needs = self.results(CI.select(["scripts/cli.py"]))
                needs["session"]["result"] = result
                self.assertTrue(CI.required_failures(needs))

    def test_failed_detection_or_absent_output_cannot_pass(self):
        needs = self.results(CI.select(["README.md"]))
        needs["changes"]["result"] = "failure"
        self.assertTrue(CI.required_failures(needs))
        needs["changes"]["result"] = "success"
        del needs["changes"]["outputs"]["session"]
        self.assertTrue(CI.required_failures(needs))


class WorkflowCoverage(unittest.TestCase):
    def test_summary_covers_every_job_and_every_optional_family(self):
        jobs = yaml.safe_load((ROOT / ".github/workflows/check.yml").read_text())["jobs"]
        self.assertEqual(set(jobs["ci-required"]["needs"]), set(jobs) - {"ci-required"})
        self.assertEqual(set(CI.JOB_LANES), set(jobs) - {"ci-required", "changes", "record"})
        self.assertEqual(set(jobs["changes"]["outputs"]), set(CI.LANES))

    def test_content_only_pr_keeps_existing_skill_and_release_contracts(self):
        jobs = yaml.safe_load((ROOT / ".github/workflows/check.yml").read_text())["jobs"]
        contracts = "\n".join(step.get("run", "") for step in jobs["record"]["steps"])
        for suite in ("tests.test_skills", "tests.test_release", "tests.test_ci_selection"):
            self.assertIn(suite, contracts)


if __name__ == "__main__":
    unittest.main()
