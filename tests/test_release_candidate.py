"""Release candidates cannot expose a plugin before its matching native downloads."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("release_candidate", ROOT / ".github/scripts/release_candidate.py")
C = importlib.util.module_from_spec(spec)
spec.loader.exec_module(C)


class CandidateTree(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.root_patch = patch.object(C, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.git("init", "-q")
        for name in C.release.VERSION_FILES:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            text = (ROOT / name).read_text()
            path.write_text(C.release.with_version({name: text}, "1.0.0")[name])
        (self.root / "CHANGELOG.md").write_text("# Changelog\n\n## 1.0.0 — today\n\nOld\n")
        self.commit()
        self.base = self.git("rev-parse", "HEAD")
        self.git("branch", "old-release")
        for name in C.release.VERSION_FILES:
            path = self.root / name
            path.write_text(C.release.with_version({name: path.read_text()}, "1.1.0")[name])
        (self.root / "CHANGELOG.md").write_text("# Changelog\n\n## 1.1.0 — today\n\nNew\n")
        self.commit()
        self.head = self.git("rev-parse", "HEAD")
        self.pr = {"number": 5, "state": "open", "draft": False,
                   "base": {"ref": "main", "sha": self.base, "repo": {"full_name": "owner/repo"}},
                   "head": {"ref": "release/1.1.0", "sha": self.head, "repo": {"full_name": "owner/repo"}}}

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, stderr=subprocess.PIPE).decode().strip()

    def commit(self, amend=False):
        self.git("add", ".")
        self.git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "fixture",
                 *(["--amend"] if amend else []))

    def validate(self):
        self.head = self.git("rev-parse", "HEAD")
        self.pr["head"]["sha"] = self.head
        return C.validate_candidate(self.head, self.base, self.pr, "owner/repo")

    def test_version_only_candidate_is_accepted(self):
        self.assertEqual(self.validate(), "1.1.0")

    def test_product_code_or_manifest_configuration_cannot_enter_publisher(self):
        for name in ("install.sh", "native/Cargo.toml"):
            with self.subTest(name=name):
                self.git("reset", "--hard", self.head)
                path = self.root / name
                path.write_text((path.read_text() if path.exists() else "") + "\nmalicious = true\n")
                self.commit(amend=True)
                with self.assertRaises(SystemExit):
                    C.validate_candidate(self.git("rev-parse", "HEAD"), self.base,
                                         {**self.pr, "head": {**self.pr["head"], "sha": self.git("rev-parse", "HEAD")}}, "owner/repo")

    def test_mode_changes_are_rejected(self):
        (self.root / "CHANGELOG.md").chmod(0o755)
        self.commit(amend=True)
        with self.assertRaisesRegex(SystemExit, "modes"):
            self.validate()

    def test_moved_base_or_foreign_repository_cannot_publish(self):
        self.pr["head"]["repo"]["full_name"] = "attacker/repo"
        with self.assertRaises(SystemExit):
            self.validate()
        self.pr["head"]["repo"]["full_name"] = "owner/repo"
        self.pr["base"]["sha"] = "0" * 40
        with self.assertRaises(SystemExit):
            self.validate()

    def test_squash_merge_reuses_checks_only_when_the_entire_tree_matches(self):
        source = self.head
        self.git("checkout", "-q", "old-release")
        self.git("merge", "--squash", source)
        self.commit()
        merged = self.git("rev-parse", "HEAD")
        real_git = C.git
        def local_git(*args):
            if args[0] == "fetch":
                return ""
            if args == ("rev-parse", "FETCH_HEAD^{commit}"):
                return source
            return real_git(*args)
        with patch.object(C, "git", side_effect=local_git), patch.object(C, "verify_available") as available:
            C.verify_main(merged)
            available.assert_called_once_with(source, "1.1.0")
            (self.root / "unexpected").write_text("new source")
            self.commit()
            with self.assertRaisesRegex(SystemExit, "differs"):
                C.verify_main(self.git("rev-parse", "HEAD"))

    def test_release_selector_includes_platform_changes_before_the_version_pr(self):
        candidate = self.head
        self.git("checkout", "-q", "old-release")
        (self.root / "install.ps1").write_text("# changed Windows behavior\n")
        self.commit()
        feature = self.git("rev-parse", "HEAD")
        self.git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "cherry-pick", candidate)
        head = self.git("rev-parse", "HEAD")
        script = str(ROOT / ".github/scripts/ci_selection.py")
        import sys
        def selection(base):
            result = subprocess.check_output([sys.executable, script, "--release", "--base", base, "--head", head],
                                             cwd=self.root, text=True)
            return json.loads(result)["platforms"]
        self.assertEqual(selection(feature), "linux-x86_64")
        self.assertEqual(selection(self.base), "linux-windows")


class RunBoundary(unittest.TestCase):
    def setUp(self):
        self.env = patch.dict(os.environ, GITHUB_REPOSITORY="owner/repo")
        self.env.start()
        self.addCleanup(self.env.stop)
        self.pr = {"head": {"sha": "a" * 40, "ref": "release/1.1.0"}}
        self.workflow = {"id": 7, "path": ".github/workflows/check.yml"}
        self.run = {"id": 123, "workflow_id": 7, "path": ".github/workflows/check.yml",
                    "repository": {"full_name": "owner/repo"}, "head_repository": {"full_name": "owner/repo"},
                    "status": "completed", "event": "pull_request", "head_sha": "a" * 40,
                    "head_branch": "release/1.1.0"}
        self.jobs = [{"id": i, "name": name, "conclusion": "success"} for i, name in enumerate(
            ["changes", "record", "native-cli / verdict", "candidate-checked", "ci-required"], 1)]
        self.jobs[-1]["conclusion"] = "failure"

    def test_only_pending_publication_gate_may_be_red(self):
        self.assertEqual(C.checked_run(self.run, self.jobs, self.pr, self.workflow, 123), 5)
        self.jobs[1]["conclusion"] = "failure"
        with self.assertRaises(SystemExit):
            C.checked_run(self.run, self.jobs, self.pr, self.workflow, 123)

    def test_stale_foreign_running_and_wrong_workflow_runs_are_refused(self):
        for field, value in (("head_sha", "b" * 40), ("workflow_id", 9), ("status", "in_progress"),
                             ("id", 124), ("path", ".github/workflows/other.yml"),
                             ("repository", {"full_name": "attacker/repo"}),
                             ("event", "push"), ("head_repository", {"full_name": "attacker/repo"})):
            with self.subTest(field=field), self.assertRaises(SystemExit):
                C.checked_run({**self.run, field: value}, self.jobs, self.pr, self.workflow, 123)

    def test_missing_gate_or_failed_native_job_cannot_be_masked(self):
        with self.assertRaises(SystemExit):
            C.checked_run(self.run, self.jobs[:-1], self.pr, self.workflow, 123)
        with self.assertRaises(SystemExit):
            C.checked_run(self.run, self.jobs + [{"name": "native-cli / tests", "conclusion": "cancelled"}], self.pr, self.workflow, 123)

    def test_release_baseline_ignores_drafts_and_the_candidate_itself(self):
        pages = [[{"tag_name": tag, "draft": draft, "prerelease": False} for tag, draft in
                  [("v1.0.0", False), ("v1.0.1", True), ("v1.1.0", False), ("legacy", False)]]]
        with patch.object(C.release, "sh", return_value=json.dumps(pages)), patch.object(C, "git", return_value="source") as git:
            self.assertEqual(C.previous_published("1.1.0"), "source")
            self.assertEqual(git.call_args_list[0].args, ("fetch", "--no-tags", "origin", "refs/tags/v1.0.0"))

    def test_no_previous_release_selects_missing_history_fallback(self):
        with patch.object(C.release, "sh", return_value="[[]]"):
            self.assertEqual(C.previous_published("1.1.0"), "")

    def test_publication_requires_up_to_date_branches(self):
        rule = {"type": "required_status_checks", "parameters": {
            "strict_required_status_checks_policy": False, "required_status_checks": [{"context": "ci-required"}]}}
        with patch.object(C, "api", return_value=[rule]):
            with self.assertRaises(SystemExit):
                C.require_strict_gate()
            rule["parameters"]["strict_required_status_checks_policy"] = True
            C.require_strict_gate()

    def test_published_and_draft_versions_freeze_the_refresher(self):
        pages = [[{"tag_name": "v1.1.0", "draft": draft}] for draft in (True, False)]
        with patch.object(C.release, "sh", return_value=json.dumps(pages)):
            self.assertEqual(C.release.frozen_releases("1.0.0"), ["v1.1.0", "v1.1.0"])
            self.assertEqual(C.release.frozen_releases("1.1.0"), [])

    def test_queued_dispatch_cannot_build_a_newer_candidate_under_an_old_run(self):
        with patch.dict(os.environ, GITHUB_SHA="b" * 40), patch.object(C, "fetch_pr", return_value=self.pr):
            with self.assertRaisesRegex(SystemExit, "moved"):
                C.select({"inputs": {"release_pr": "5"}}, "workflow_dispatch")

    def test_manual_diagnostic_cannot_greenlight_a_version_pr_without_publication(self):
        for branch in ("release/1.1.0", "feature-with-version-change"):
            with self.subTest(branch=branch), \
                 patch.dict(os.environ, GITHUB_SHA="a" * 40, GITHUB_REF="refs/heads/" + branch), \
                 patch.object(C, "version_at", side_effect=["1.1.0", "1.0.0"]):
                with self.assertRaisesRegex(SystemExit, "release_pr"):
                    C.select({"inputs": {}}, "workflow_dispatch")

    def test_manual_diagnostics_without_a_version_change_still_work(self):
        with patch.dict(os.environ, GITHUB_SHA="a" * 40, GITHUB_REF="refs/heads/main"), \
             patch.object(C, "version_at", return_value="1.0.0"):
            self.assertEqual(C.select({"inputs": {}}, "workflow_dispatch")["release"], "false")

    def test_missing_downloads_or_wrong_tag_cannot_open_merge_gate(self):
        info = {"draft": False, "prerelease": False, "assets": []}
        with patch.object(C, "api", return_value=info), patch.object(C, "git", return_value="a" * 40):
            with self.assertRaisesRegex(SystemExit, "missing platform"):
                C.verify_available("a" * 40, "1.1.0")
            with self.assertRaisesRegex(SystemExit, "tag"):
                C.verify_available("b" * 40, "1.1.0")

    def test_a_download_http_failure_keeps_the_merge_gate_closed(self):
        import native_assets
        names = {f"kpopper-1.1.0-{target}." + ("zip" if target.startswith("windows") else "tar.gz")
                 for target in native_assets.TARGETS} | {"install.sh", "install.ps1", "SHA256SUMS"}
        info = {"draft": False, "prerelease": False, "assets": [
            {"name": name, "size": 100, "digest": "sha256:" + "a" * 64,
             "browser_download_url": "https://example.invalid/" + name} for name in names]}
        with patch.object(C, "api", return_value=info), patch.object(C, "git", return_value="a" * 40), \
             patch.object(C.subprocess, "run", side_effect=subprocess.CalledProcessError(22, "curl")):
            with self.assertRaises(subprocess.CalledProcessError):
                C.verify_available("a" * 40, "1.1.0")


if __name__ == "__main__":
    unittest.main()
