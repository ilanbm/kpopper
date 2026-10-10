"""Adversarial contracts for verified release recovery admission."""
import importlib.util
import hashlib
import io
import json
import os
import pathlib
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import warnings
import zipfile
from contextlib import redirect_stdout, redirect_stderr

ROOT = pathlib.Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "release_recovery", ROOT / ".github/scripts/release_recovery.py")
RECOVERY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RECOVERY)


def evidence_zip(commit="a" * 40, scope="full", cases=None, extra=None, junit=None):
    """Build a tiny flat artifact; never use or retain real CI logs in these tests."""
    if cases is None:
        cases = [("suite", "works", "pass")]
    body = "".join(
        f'<testcase classname="{classname}" name="{name}">{"<failure/>" if result == "fail" else "<skipped/>" if result == "skipped" else ""}</testcase>'
        for classname, name, result in cases)
    xml = junit if junit is not None else f"<testsuites><testsuite>{body}</testsuite></testsuites>".encode()
    files = {"commit.txt": (commit + "\n").encode(),
             "validation-scope.txt": (scope + "\n").encode(), "junit.xml": xml}
    files.update(extra or {})
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, content in files.items():
            if content is None:
                continue
            archive.writestr(name, content)
    return out.getvalue()


class EvidenceZip(unittest.TestCase):
    def test_valid_flat_full_evidence_preserves_identity_and_outcome(self):
        identities = [("kpopper::checked_session_store",
                       "proposal_reader_waits_for_transient_publisher_delete_handle", "fail"),
                      ("suite", "passed", "pass"), ("suite", "skipped", "skipped")]
        parsed = RECOVERY.parse_evidence_zip(evidence_zip(cases=identities), "a" * 40)
        self.assertEqual(parsed.commit, "a" * 40)
        self.assertEqual(parsed.scope, "full")
        self.assertEqual(parsed.tests, {(c, n): outcome for c, n, outcome in identities})

    def test_rejects_wrong_head_scope_and_missing_required_member(self):
        for raw, commit in ((evidence_zip(), "b" * 40),
                            (evidence_zip(scope="partial"), "a" * 40),
                            (evidence_zip(extra={"commit.txt": None}), "a" * 40)):
            with self.subTest(commit=commit), self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.parse_evidence_zip(raw, commit)

    def test_rejects_duplicate_cases_and_empty_or_malformed_junit(self):
        duplicate = [("suite", "same", "pass"), ("suite", "same", "pass")]
        empty = evidence_zip(cases=[])
        malformed = evidence_zip(extra={"junit.xml": b"<testsuite>"})
        for raw in (evidence_zip(cases=duplicate), empty, malformed):
            with self.subTest(raw_size=len(raw)), self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.parse_evidence_zip(raw, "a" * 40)

    def test_rejects_junit_whose_declared_suite_size_proves_truncation(self):
        truncated = evidence_zip(junit=b'<testsuite tests="3" failures="0"><testcase classname="suite" name="only"/></testsuite>')
        with self.assertRaises(RECOVERY.RecoveryError):
            RECOVERY.parse_evidence_zip(truncated, "a" * 40)

    def test_rejects_duplicate_and_oversized_identity_files(self):
        for name, value in (("commit.txt", b"a" * 65), ("validation-scope.txt", b"full" + b" " * 13)):
            raw = evidence_zip(extra={name: value})
            with self.subTest(name=name), self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.parse_evidence_zip(raw, "a" * 40)
        duplicate = io.BytesIO()
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            with zipfile.ZipFile(duplicate, "w") as archive:
                for name, value in (("commit.txt", b"a" * 40), ("commit.txt", b"a" * 40),
                                    ("validation-scope.txt", b"full"),
                                    ("junit.xml", b'<testsuite><testcase classname="x" name="y"/></testsuite>')):
                    archive.writestr(name, value)
        with self.assertRaises(RECOVERY.RecoveryError):
            RECOVERY.parse_evidence_zip(duplicate.getvalue(), "a" * 40)

    def test_rejects_entity_declarations_and_ambiguous_archive_members(self):
        dtd = evidence_zip(extra={"junit.xml": b'<!DOCTYPE x [<!ENTITY e "boom">]><testsuite><testcase name="&e;"/></testsuite>'})
        duplicate_member = io.BytesIO()
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            with zipfile.ZipFile(duplicate_member, "w") as archive:
                for name, content in (("commit.txt", b"a" * 40), ("validation-scope.txt", b"full"),
                                      ("junit.xml", b"<testsuite><testcase name='ok'/></testsuite>"),
                                      ("junit.xml", b"<testsuite><testcase name='other'/></testsuite>")):
                    archive.writestr(name, content)
        for raw in (dtd, duplicate_member.getvalue()):
            with self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.parse_evidence_zip(raw, "a" * 40)

    def test_rejects_oversized_junit_member(self):
        oversized = evidence_zip(extra={"junit.xml": b" " * (8 * 1024 * 1024 + 1)})
        with self.assertRaises(RECOVERY.RecoveryError):
            RECOVERY.parse_evidence_zip(oversized, "a" * 40)

    def test_ignores_unneeded_members_but_rejects_unsafe_paths(self):
        accepted = RECOVERY.parse_evidence_zip(
            evidence_zip(extra={"git-trace.json": b"irrelevant trace"}), "a" * 40)
        self.assertEqual(accepted.scope, "full")
        for name in ("../junit.xml", "/commit.txt"):
            with self.subTest(name=name), self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.parse_evidence_zip(evidence_zip(extra={name: b"x"}), "a" * 40)


class AdmissionOutputs(unittest.TestCase):
    def test_gate_outputs_are_bound_to_the_saved_receipt_after_success(self):
        source = "a" * 40
        candidate = SimpleNamespace(fetch_pr=lambda _: {"head": {"sha": source}})
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            env = {"GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_SHA": source,
                   "GITHUB_REPOSITORY": "ilanbm/kpopper", "GITHUB_OUTPUT": str(root / "outputs")}
            args = ["--baseline-run", "1", "--windows-run", "2", "--source", source,
                    "--pr", "3", "--receipt", str(root / "receipt.json")]
            result = {"source": source, "version": "0.16.0"}
            with patch.dict(os.environ, env, clear=True), patch.dict(sys.modules, {"release_candidate": candidate}), \
                    patch.object(RECOVERY, "recover", return_value=result), redirect_stdout(io.StringIO()):
                self.assertEqual(RECOVERY.main(args), 0)
            receipt = (root / "receipt.json").read_bytes()
            self.assertEqual(json.loads(receipt), result)
            self.assertEqual((root / "outputs").read_text(), "validation=distribution\nplatforms=all\n"
                             + "evidence_sha256=" + hashlib.sha256(receipt).hexdigest() + "\n")

    def test_failed_admission_never_emits_a_receipt_or_gate_outputs(self):
        source = "a" * 40
        candidate = SimpleNamespace(fetch_pr=lambda _: {"head": {"sha": source}})
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            env = {"GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_SHA": source,
                   "GITHUB_REPOSITORY": "ilanbm/kpopper", "GITHUB_OUTPUT": str(root / "outputs")}
            args = ["--baseline-run", "1", "--windows-run", "2", "--source", source,
                    "--pr", "3", "--receipt", str(root / "receipt.json")]
            with patch.dict(os.environ, env, clear=True), patch.dict(sys.modules, {"release_candidate": candidate}), \
                    patch.object(RECOVERY, "recover", side_effect=RECOVERY.RecoveryError("stale evidence")), \
                    redirect_stderr(io.StringIO()):
                self.assertEqual(RECOVERY.main(args), 1)
            self.assertFalse((root / "outputs").exists())
            self.assertFalse((root / "receipt.json").exists())


class JobAdmission(unittest.TestCase):
    targets = ["ubuntu-24.04, linux-x86_64, kpop",
               "ubuntu-24.04-arm, linux-aarch64, kpop",
               "macos-14, darwin-arm64, kpop",
               "macos-15-intel, darwin-x86_64, kpop",
               "windows-2022, windows-x86_64, kpop.exe"]

    @staticmethod
    def job(name, conclusion="success", status="completed"):
        return {"name": name, "status": status, "conclusion": conclusion}

    def baseline_jobs(self):
        jobs = [self.job("changes"), self.job("record")]
        jobs.extend(self.job(f"native-cli / tests ({target})",
                             "failure" if target == self.targets[-1] else "success")
                    for target in self.targets)
        jobs.extend(self.job(f"native-cli / release ({target})") for target in self.targets)
        jobs.extend(self.job(name, "failure" if name in {"native-cli / verdict", "candidate-checked",
                                                          "ci-required"} else "success")
                    for name in ("native-cli / verdict", "candidate-checked", "ci-required"))
        return jobs

    def supplemental_jobs(self):
        target = self.targets[-1]
        return [self.job(f"tests ({target})"), self.job(f"release ({target})"), self.job("verdict")]

    def test_admits_only_windows_test_failure_with_expected_aggregate_reds(self):
        RECOVERY.validate_baseline_jobs(self.baseline_jobs())
        RECOVERY.validate_windows_jobs(self.supplemental_jobs())

    def test_baseline_rejects_wrong_failure_incomplete_duplicate_or_missing_required_job(self):
        mutations = []
        jobs = self.baseline_jobs()
        mutations.append([dict(job, conclusion="failure") if job["name"] == "changes" else job for job in jobs])
        mutations.append([dict(job, status="in_progress") if job["name"] == "record" else job for job in jobs])
        mutations.append(jobs + [dict(jobs[0])])
        mutations.append([job for job in jobs if job["name"] != "native-cli / release (macos-14, darwin-arm64, kpop)"])
        mutations.append([dict(job, conclusion="failure") if job["name"] == "native-cli / tests (macos-14, darwin-arm64, kpop)" else job
                          for job in jobs])
        mutations.append(jobs + [self.job("unrecognized critical gate", "failure")])
        for altered in mutations:
            with self.subTest(jobs=altered), self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.validate_baseline_jobs(altered)

    def test_supplemental_rejects_failed_incomplete_or_other_platform_test_jobs(self):
        valid = self.supplemental_jobs()
        bad_sets = [
            [dict(job, conclusion="failure") if job["name"] == "verdict" else job for job in valid],
            [dict(job, status="queued") if job["name"].startswith("tests (") else job for job in valid],
            valid + [self.job("tests (ubuntu-24.04, linux-x86_64, kpop)")],
        ]
        for altered in bad_sets:
            with self.subTest(jobs=altered), self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.validate_windows_jobs(altered)


class EvidenceComparison(unittest.TestCase):
    target_names = set(RECOVERY.TARGETS)
    failing = ("kpopper::checked_session_store",
               "proposal_reader_waits_for_transient_publisher_delete_handle")
    passing = ("kpopper::checked_session_store", "ordinary_reader_remains_available")

    def evidence(self, tests, commit="a" * 40):
        return RECOVERY.Evidence(commit, "full", tests)

    def baseline(self):
        return {target: self.evidence({self.passing: "pass", **(
            {self.failing: "fail"} if target == "windows-x86_64" else {})})
                for target in self.target_names}

    def test_accepts_windows_only_failure_when_fresh_full_report_passes_each_old_case(self):
        failures = RECOVERY.validate_evidence(
            self.baseline(), self.evidence({self.failing: "pass", self.passing: "pass",
                                            ("kpopper::checked_session_store", "new_case"): "pass"}, "b" * 40))
        self.assertEqual(failures, {self.failing})

    def test_rejects_missing_old_failure_skips_and_failures_on_other_platforms(self):
        old = self.baseline()
        missing = self.evidence({self.passing: "pass"}, "b" * 40)
        skipped = self.evidence({self.failing: "pass", self.passing: "skipped"}, "b" * 40)
        old_skip = self.baseline()
        old_skip["linux-x86_64"].tests[self.passing] = "skipped"
        old_fail = self.baseline()
        old_fail["darwin-arm64"].tests[self.passing] = "fail"
        for baseline, windows in ((old, missing), (old, skipped),
                                  (old_skip, self.evidence({self.failing: "pass", self.passing: "pass"}, "b" * 40)),
                                  (old_fail, self.evidence({self.failing: "pass", self.passing: "pass"}, "b" * 40))):
            with self.subTest(baseline=baseline, windows=windows), self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.validate_evidence(baseline, windows)

    def test_rejects_mixed_baseline_source(self):
        mixed = self.baseline()
        mixed["linux-aarch64"] = self.evidence({self.passing: "pass"}, "d" * 40)
        with self.assertRaises(RECOVERY.RecoveryError):
            RECOVERY.validate_evidence(mixed, self.evidence({self.failing: "pass", self.passing: "pass"}, "b" * 40))


class RunIdentity(unittest.TestCase):
    repo = "ilanbm/kpopper"
    commit = "c" * 40
    path = ".github/workflows/check.yml"

    def setUp(self):
        self.workflow = {"id": 17, "path": self.path}
        self.run = {"id": 29, "repository": {"full_name": self.repo},
                    "head_repository": {"full_name": self.repo}, "workflow_id": 17,
                    "path": self.path, "event": "workflow_dispatch", "status": "completed",
                    "conclusion": "failure", "head_sha": self.commit,
                    "head_branch": "release/0.16.0"}

    def test_accepts_failed_baseline_and_successful_windows_run_on_matching_release(self):
        RECOVERY.validate_run(self.run, self.workflow, self.repo, self.path, success=False)
        RECOVERY.validate_run(dict(self.run, event="pull_request"), self.workflow,
                              self.repo, self.path, success=False)
        successful = dict(self.run, id=30, conclusion="success")
        RECOVERY.validate_run(successful, self.workflow, self.repo, self.path, success=True)
        with self.assertRaises(RECOVERY.RecoveryError):
            RECOVERY.validate_run(dict(successful, event="pull_request"), self.workflow,
                                  self.repo, self.path, success=True)

    def test_rejects_run_from_wrong_repo_workflow_head_event_branch_or_state(self):
        changes = [
            {"repository": {"full_name": "attacker/kpopper"}},
            {"head_repository": {"full_name": "attacker/kpopper"}},
            {"workflow_id": 99}, {"path": ".github/workflows/other.yml"},
            {"head_sha": "not-a-commit"}, {"head_branch": "main"},
            {"event": "push"}, {"status": "in_progress"},
            {"conclusion": "success"},
        ]
        for change in changes:
            with self.subTest(change=change), self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.validate_run(dict(self.run, **change), self.workflow, self.repo,
                                      self.path, success=False)
        with self.assertRaises(RECOVERY.RecoveryError):
            RECOVERY.validate_run(self.run, dict(self.workflow, path=".github/workflows/other.yml"),
                                  self.repo, self.path, success=False)


class MemoryRepository:
    def __init__(self, files):
        self.files = dict(files)

    def tree(self, ref):
        return {path: ("100644", hashlib.sha1(data).hexdigest())
                for path, data in self.files[ref].items()}

    def read(self, ref, path):
        return self.files[ref][path]


class ExecutedWorkflow(unittest.TestCase):
    def test_uses_immutable_workflow_reference_instead_of_moved_pr_metadata(self):
        run = {"referenced_workflows": [{"path": "ilanbm/kpopper/.github/workflows/native-rust.yml@refs/pull/1/merge",
                                        "sha": "b" * 40}],
               "pull_requests": [{"head": {"sha": "c" * 40}}]}
        self.assertEqual(RECOVERY.baseline_workflow_source(run, "ilanbm/kpopper"), "b" * 40)
        for references in ([], run["referenced_workflows"] * 2,
                           [{"path": "other/repo/.github/workflows/native-rust.yml", "sha": "b" * 40}]):
            with self.subTest(references=references), self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.baseline_workflow_source(dict(run, referenced_workflows=references), "ilanbm/kpopper")

    def test_merge_commit_must_have_executed_the_same_workflows(self):
        files = {".github/workflows/check.yml": b"caller", ".github/workflows/native-rust.yml": b"native"}
        repo = MemoryRepository({"a" * 40: dict(files), "b" * 40: dict(files)})
        RECOVERY.validate_executed_workflows(repo, "a" * 40, "b" * 40)
        for path in files:
            repo.files["b" * 40] = dict(files, **{path: b"changed workflow"})
            with self.subTest(path=path), self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY.validate_executed_workflows(repo, "a" * 40, "b" * 40)


class SourceBoundary(unittest.TestCase):
    identity = ("kpopper::checked_session_store",
                "proposal_reader_waits_for_transient_publisher_delete_handle")
    test_path = "native/tests/checked_session_store.rs"
    config_path = "native/.config/nextest.toml"
    baseline_ref = "a" * 40
    windows_ref = "b" * 40
    source_ref = "c" * 40

    @staticmethod
    def rust_source(body='let label = "brace } in string"; let raw = r###" } "###; /* outer { /* nested } */ tail */ assert!(true);'):
        return ("#[cfg(windows)]\n#[test]\nfn proposal_reader_waits_for_transient_publisher_delete_handle() {\n"
                f"    {body}\n"
                "}\n\n#[test]\nfn unrelated_test_is_unchanged() { assert!(true); }\n").encode()

    @staticmethod
    def config(extra=""):
        return ("[profile.ci]\nretries = 0\nfail-fast = false\n" + extra).encode()

    def repo(self, current_test=None, current_config=None, baseline_test=None, baseline_config=None,
             windows_test=None, windows_config=None, current_extra=None):
        base = {self.test_path: baseline_test or self.rust_source(),
                self.config_path: baseline_config or self.config(), "VERSION": b"0.16.0\n"}
        supplemental = dict(base)
        supplemental[self.test_path] = windows_test or base[self.test_path]
        supplemental[self.config_path] = windows_config or base[self.config_path]
        current = dict(base)
        if current_test is not None:
            current[self.test_path] = current_test
        if current_config is not None:
            current[self.config_path] = current_config
        current.update(current_extra or {})
        return MemoryRepository({self.baseline_ref: base, self.windows_ref: supplemental,
                                 self.source_ref: current})

    def validate(self, repository):
        return RECOVERY.validate_sources(repository, self.baseline_ref, self.windows_ref,
                                         self.source_ref, {self.identity})

    def override(self):
        return ("\n[[profile.ci.overrides]]\nfilter = 'binary(=checked_session_store) & "
                "test(=proposal_reader_waits_for_transient_publisher_delete_handle)'\n"
                "platform = 'cfg(windows)'\nthreads-required = \"num-test-threads\"\n").encode()

    def test_admits_only_failed_top_level_windows_body_change_and_exact_override(self):
        corrected_test = self.rust_source("let changed = true; assert!(changed);")
        corrected_config = self.config(self.override().decode())
        repo = self.repo(current_test=corrected_test, current_config=corrected_config,
                         windows_test=corrected_test, windows_config=corrected_config)
        fingerprint = self.validate(repo)
        self.assertRegex(fingerprint, r"^[0-9a-f]{64}$")

    def test_admits_only_exact_windows_job_timeout_extension(self):
        path = ".github/workflows/native-rust.yml"
        before = b"jobs:\n  tests:\n    timeout-minutes: ${{ matrix.target == 'darwin-x86_64' && 120 || 75 }}\n    steps: [unchanged]\n"
        after = before.replace(b"matrix.target == 'darwin-x86_64'",
                               b"(matrix.target == 'darwin-x86_64' || matrix.target == 'windows-x86_64')")
        repo = self.repo()
        repo.files[self.baseline_ref][path] = before
        repo.files[self.windows_ref][path] = after
        repo.files[self.source_ref][path] = after
        self.validate(repo)
        for altered in (after.replace(b"120", b"180"), after.replace(b"75", b"90"),
                        after.replace(b"steps: [unchanged]", b"steps: [skipped]")):
            repo.files[self.windows_ref][path] = altered
            repo.files[self.source_ref][path] = altered
            with self.subTest(altered=altered), self.assertRaises(RECOVERY.RecoveryError):
                self.validate(repo)
        repo.files[self.windows_ref][path] = before
        repo.files[self.source_ref][path] = after
        with self.assertRaises(RECOVERY.RecoveryError):
            self.validate(repo)

    def test_rejects_changed_attributes_outside_function_and_non_windows_or_nested_function(self):
        base = self.rust_source()
        changed_body = self.rust_source("let changed = true;")
        bad_sources = [
            base.replace(b"#[cfg(windows)]", b"#[cfg(unix)]"),
            base.replace(b"#[test]", b"#[ignore]"),
            base.replace(b"unrelated_test_is_unchanged", b"unrelated_test_changed"),
            base.replace(b"fn proposal_reader_waits", b"mod nested { fn proposal_reader_waits"),
            base.replace(b"fn proposal_reader_waits", b"macro_rules! nested { () => { fn proposal_reader_waits"),
        ]
        for altered in bad_sources:
            repo = self.repo(current_test=altered, current_config=self.config(self.override().decode()))
            with self.subTest(source=altered), self.assertRaises(RECOVERY.RecoveryError):
                self.validate(repo)
        # The supplemental source is the final source of the test evidence: later edits are refused.
        repo = self.repo(current_test=changed_body,
                         current_config=self.config(self.override().decode()),
                         windows_test=base)
        with self.assertRaises(RECOVERY.RecoveryError):
            self.validate(repo)

    def test_rejects_widened_retry_deadline_and_unreviewed_inputs(self):
        for config in (self.config(self.override().decode()).replace(b"retries = 0", b"retries = 1"),
                       self.config(self.override().decode() + "\nslow-timeout = { period = '60s', terminate-after = 20 }\n")):
            repo = self.repo(current_test=self.rust_source("let changed = true;"), current_config=config)
            with self.subTest(config=config), self.assertRaises(RECOVERY.RecoveryError):
                self.validate(repo)
        for path in ("native/src/production.rs", "native/Cargo.toml",
                     "scripts/package_native.py", "assets/runtime.bin"):
            repo = self.repo(current_test=self.rust_source("let changed = true;"),
                             current_config=self.config(self.override().decode()),
                             current_extra={path: b"changed input"})
            with self.subTest(path=path), self.assertRaises(RECOVERY.RecoveryError):
                self.validate(repo)

    def test_rejects_mode_change_and_changed_source_after_supplemental_run(self):
        repo = self.repo(current_test=self.rust_source("let changed = true;"),
                         current_config=self.config(self.override().decode()))
        # Same bytes with a different Git mode is still a source change.
        original_tree = repo.tree
        def changed_mode(ref):
            tree = original_tree(ref)
            if ref == self.source_ref:
                tree[self.test_path] = ("100755", hashlib.sha1(repo.read(ref, self.test_path)).hexdigest())
            return tree
        repo.tree = changed_mode
        with self.assertRaises(RECOVERY.RecoveryError):
            self.validate(repo)
        repo = self.repo(current_test=self.rust_source("let changed = true;"),
                         current_config=self.config(self.override().decode()),
                         windows_config=self.config())
        with self.assertRaises(RECOVERY.RecoveryError):
            self.validate(repo)


if __name__ == "__main__":
    unittest.main()
