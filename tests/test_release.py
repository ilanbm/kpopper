"""Unit tests for the native release planner and publisher."""

import hashlib
import importlib.util
import io
import json
import pathlib
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from unittest.mock import patch

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / ".github" / "scripts"


def _load(name):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


R = _load("release")
P = _load("publish_release")


def current_version():
    return R.versions_in(R.read_texts())["VERSION"]


class TheReleaseLine(unittest.TestCase):
    def test_native_version_files_agree_and_legacy_channels_are_excluded(self):
        found = R.versions_in(R.read_texts())
        self.assertEqual(len(set(found.values())), 1, found)
        self.assertEqual(set(found), {
            "VERSION", "native/Cargo.toml", "native/Cargo.lock",
            ".claude-plugin/plugin.json", ".codex-plugin/plugin.json",
            ".claude-plugin/marketplace.json",
        })
        self.assertNotIn("pyproject.toml", R.VERSION_FILES)
        self.assertNotIn("package.json", R.VERSION_FILES)

    def test_with_version_changes_only_native_root_versions(self):
        texts = R.read_texts()
        moved = R.with_version(texts, "9.9.9")
        for name, text in moved.items():
            before, after = texts[name].splitlines(), text.splitlines()
            self.assertEqual(len(before), len(after), name)
            self.assertEqual(sum(a != b for a, b in zip(before, after)), 1, name)
        self.assertIn('name = "serde"\nversion = "1.', moved["native/Cargo.lock"])
        self.assertEqual(
            texts["native/Cargo.lock"].split('name = "serde"\nversion = ', 1)[1].splitlines()[0],
            moved["native/Cargo.lock"].split('name = "serde"\nversion = ', 1)[1].splitlines()[0],
        )

    def test_bump_arithmetic(self):
        self.assertEqual(R.bump_of("## What changed\n\nBump: minor\n"), "minor")
        self.assertIsNone(R.bump_of("Bump: none — this is the release"))
        self.assertEqual(R.bump_of("Bump: MINOR"), "minor")
        self.assertEqual(R.largest(["patch", "minor", "patch"]), "minor")
        self.assertEqual(R.largest([None, None]), "patch")
        self.assertEqual(R.largest(["patch", "minor", None]), "minor")
        self.assertEqual(R.next_version("0.20.3", "major"), "1.0.0")

    def test_missing_version_is_rejected(self):
        with self.assertRaisesRegex(SystemExit, "no version found"):
            R.versions_in({"VERSION": "no version\n"})


class ChangelogBehavior(unittest.TestCase):
    def test_entry_lists_merges_and_record_decisions(self):
        entry = R.changelog_entry("0.21.0", "2026-09-04",
                                  [{"subject": "Accept contract", "pr": 2, "bump": "minor"},
                                   {"subject": "Fix thing", "pr": None, "bump": None}],
                                  ["d.page_is_graph"])
        self.assertIn("## 0.21.0 — 2026-09-04", entry)
        self.assertIn("- Accept contract (#2) — minor", entry)
        self.assertIn("- Fix thing — no bump declared", entry)
        self.assertIn("Decisions recorded: d.page_is_graph", entry)

    def test_new_entry_is_prepended_and_release_body_has_no_bump(self):
        old = R.prepend("", "## 0.20.0 — x\n\n- old\n")
        new = R.prepend(old, "## 0.21.0 — y\n\n- new\n")
        self.assertLess(new.index("0.21.0"), new.index("0.20.0"))
        body = R.pr_body("0.21.0", "0.20.0", "minor",
                         [{"subject": "s", "pr": 2, "bump": "minor"}], [])
        self.assertIsNone(R.bump_of(body))

    def test_section_stops_at_next_version_heading(self):
        log = ("# Changelog\n\n## 0.22.0 — date\n\n- later\n\n"
               "## 0.21.0 — date\n\n- earlier\n")
        self.assertEqual(P.section_for(log, "0.22.0"), "- later")
        self.assertIn("earlier", P.section_for(log, "0.21.0"))
        self.assertIsNone(P.section_for(log, "0.2"))

    def test_current_release_version_has_changelog_section(self):
        changelog = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
        section = P.section_for(changelog, current_version())
        self.assertTrue(section and section.strip())


class PublisherBehavior(unittest.TestCase):
    def _sh(self, calls, head="a" * 40):
        def command(*args, **kwargs):
            calls.append(args)
            if args == ("git", "rev-parse", "HEAD"):
                return head
            return ""
        return command

    def test_tag_carries_the_version(self):
        self.assertEqual(P.tag_for(current_version()), "v" + current_version())

    def test_plan_is_json_and_read_only(self):
        calls = []
        output = io.StringIO()
        version = current_version()
        with patch.object(P, "previous_version", return_value="0.8.0"), \
             patch.object(P.release, "sh", side_effect=self._sh(calls)), \
             redirect_stdout(output):
            self.assertEqual(P.main(["--plan"]), 0)
        self.assertEqual(json.loads(output.getvalue()),
                         {"publish": True, "version": version, "commit": "a" * 40})
        self.assertEqual(calls, [("git", "rev-parse", "HEAD")])

    def test_no_version_change_does_not_publish(self):
        calls = []
        with patch.object(P, "previous_version", return_value=current_version()), \
             patch.object(P.release, "sh", side_effect=self._sh(calls)), \
             patch.object(P, "published") as published:
            self.assertEqual(P.main([]), 0)
        published.assert_not_called()
        self.assertEqual(calls, [("git", "rev-parse", "HEAD")])

    def test_dirty_source_stops_before_build_or_publish(self):
        calls = []
        def dirty(*args, **kwargs):
            calls.append(args)
            if args == ("git", "rev-parse", "HEAD"):
                return "a" * 40
            if args[:3] == ("git", "diff", "--quiet"):
                raise subprocess.CalledProcessError(1, args)
            return ""
        with patch.object(P, "previous_version", return_value="0.8.0"), \
             patch.object(P, "published", return_value=False), \
             patch.object(P, "tag_elsewhere", return_value=None), \
             patch.object(P, "build") as build, \
             patch.object(P.release, "sh", side_effect=dirty):
            with self.assertRaises(subprocess.CalledProcessError):
                P.main([])
        build.assert_not_called()
        self.assertFalse(any(call[:3] == ("gh", "release", "create") for call in calls))

    def test_release_targets_exact_head_and_latest(self):
        calls = []
        commit = "a" * 40
        with tempfile.TemporaryDirectory() as directory:
            candidate = pathlib.Path(directory) / "install.sh"
            candidate.write_bytes(b"asset")
            dist = pathlib.Path(directory) / "dist"
            dist.mkdir()
            with patch.object(P, "previous_version", return_value="0.8.0"), \
                 patch.object(P, "published", return_value=False), \
                 patch.object(P, "tag_elsewhere", return_value=None), \
                 patch.object(P, "build", return_value=[candidate]), \
                 patch.object(P, "DIST", dist), \
                 patch.object(P, "verify_published"), \
                 patch.object(P.release, "sh", side_effect=self._sh(calls, commit)):
                self.assertEqual(P.main([]), 0)
            create = next(call for call in calls if call[:3] == ("gh", "release", "create"))
            self.assertEqual(create[create.index("--target") + 1], commit)
            self.assertIn("--draft", create)
            edit = next(call for call in calls if call[:3] == ("gh", "release", "edit"))
        self.assertEqual(edit[-2:], ("--draft=false", "--latest"))

    def test_moving_head_during_build_stops_before_release_create(self):
        calls = []
        heads = iter(["a" * 40, "b" * 40])
        def command(*args, **kwargs):
            calls.append(args)
            if args == ("git", "rev-parse", "HEAD"):
                return next(heads)
            return ""
        with patch.object(P, "previous_version", return_value="0.8.0"), \
             patch.object(P, "published", return_value=False), \
             patch.object(P, "tag_elsewhere", return_value=None), \
             patch.object(P, "build", return_value=[pathlib.Path("asset.tar.gz")]), \
             patch.object(P.release, "sh", side_effect=command):
            with self.assertRaisesRegex(SystemExit, "checkout changed"):
                P.main([])
        self.assertFalse(any(call[:3] == ("gh", "release", "create") for call in calls))

    def test_build_passes_native_artifact_inputs_to_validator(self):
        import native_assets
        with tempfile.TemporaryDirectory() as directory:
            artifacts = pathlib.Path(directory) / "artifacts"
            dist = pathlib.Path(directory) / "dist"
            commit = "a" * 40
            version = current_version()
            with patch.object(P, "ARTIFACTS", artifacts), patch.object(P, "DIST", dist), \
                 patch.object(P.release, "sh", return_value=commit), \
                 patch.object(native_assets, "prepare", return_value=[dist / "asset"]) as prepare:
                self.assertEqual(P.build(version), [dist / "asset"])
            prepare.assert_called_once_with(version, commit, artifacts, dist, P.ROOT)

    def test_tag_mismatch_stops_before_build(self):
        with patch.object(P, "previous_version", return_value="0.8.0"), \
             patch.object(P, "published", return_value=False), \
             patch.object(P, "tag_elsewhere", return_value="b" * 40), \
             patch.object(P, "build") as build, \
             patch.object(P.release, "sh", side_effect=self._sh([])):
            with self.assertRaisesRegex(SystemExit, "already names"):
                P.main([])
        build.assert_not_called()

    def test_github_release_lookup_errors_are_not_treated_as_absence(self):
        failed = subprocess.CompletedProcess(["gh"], 1, stdout="", stderr="permission denied")
        with patch.object(P.subprocess, "run", return_value=failed):
            with self.assertRaisesRegex(SystemExit, "permission denied"):
                P.published("v" + current_version())

    def test_draft_release_is_looked_up_by_database_id(self):
        payload = {"id": 123, "tag_name": "v0.9.0", "draft": True, "assets": []}

        def github(*args, **kwargs):
            if args == ("gh", "release", "view", "v0.9.0", "--repo", "org/repo",
                        "--json", "databaseId"):
                return json.dumps({"databaseId": 123})
            if args == ("gh", "api", "repos/org/repo/releases/123"):
                return json.dumps(payload)
            raise SystemExit("GitHub cannot find an unpublished release by tag: HTTP 404")

        with patch.dict(P.os.environ, {"GITHUB_REPOSITORY": "org/repo"}), \
             patch.object(P.release, "sh", side_effect=github):
            self.assertEqual(P.release_info("v0.9.0"), payload)

    def test_existing_release_assets_are_verified_by_size_and_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            asset = pathlib.Path(directory) / "install.sh"
            asset.write_bytes(b"asset")
            payload = {"tag_name": "v" + current_version(), "assets": [{"name": asset.name, "size": 5,
                                   "digest": "sha256:" + hashlib.sha256(b"asset").hexdigest()}]}
            with patch.dict(P.os.environ, {"GITHUB_REPOSITORY": "org/repo"}), \
                 patch.object(P.release, "sh", side_effect=[json.dumps({"databaseId": 123}),
                                                           json.dumps(payload)]) as command:
                P.verify_published("v" + current_version(), [asset])
            self.assertEqual(command.call_args_list[-1].args,
                             ("gh", "api", "repos/org/repo/releases/123"))

    def test_release_lookup_refuses_invalid_or_changed_identity(self):
        for identity, payload in (({}, {}), ({"databaseId": True}, {}),
                                  ({"databaseId": 123}, {"tag_name": "v9.9.9"})):
            with self.subTest(identity=identity, payload=payload), \
                 patch.dict(P.os.environ, {"GITHUB_REPOSITORY": "org/repo"}), \
                 patch.object(P.release, "sh", side_effect=[json.dumps(identity), json.dumps(payload)]):
                with self.assertRaises(SystemExit):
                    P.release_info("v0.9.0")

    def test_existing_release_retries_verify_and_never_creates(self):
        calls = []
        with tempfile.TemporaryDirectory() as directory:
            candidate = pathlib.Path(directory) / "asset.tar.gz"
            candidate.write_bytes(b"asset")
            with patch.object(P, "previous_version", return_value="0.8.0"), \
                 patch.object(P, "published", return_value=True), \
                 patch.object(P, "tag_elsewhere", return_value=None), \
                 patch.object(P, "build", return_value=[candidate]), \
                 patch.object(P, "release_info", return_value={"draft": False}), \
                 patch.object(P, "verify_published") as verify, \
                 patch.object(P.release, "sh", side_effect=self._sh(calls)):
                self.assertEqual(P.main([]), 0)
        verify.assert_called_once_with("v" + current_version(), [candidate])
        self.assertFalse(any(call[:3] == ("gh", "release", "create") for call in calls))

    def test_failed_draft_upload_never_promotes(self):
        calls = []
        with tempfile.TemporaryDirectory() as directory:
            candidate = pathlib.Path(directory) / "asset.tar.gz"
            candidate.write_bytes(b"asset")
            def upload_fails(*args, **kwargs):
                calls.append(args)
                if args[:3] == ("gh", "release", "upload"):
                    raise subprocess.CalledProcessError(1, args)
                return "a" * 40
            info = {"draft": True, "target_commitish": "a" * 40, "assets": []}
            with patch.object(P, "previous_version", return_value="0.8.0"), \
                 patch.object(P, "published", return_value=True), \
                 patch.object(P, "tag_elsewhere", return_value=None), \
                 patch.object(P, "build", return_value=[candidate]), \
                 patch.object(P, "release_info", return_value=info), \
                 patch.object(P, "verify_published") as verify, \
                 patch.object(P.release, "sh", side_effect=upload_fails):
                with self.assertRaises(subprocess.CalledProcessError):
                    P.main([])
        verify.assert_not_called()
        self.assertFalse(any(call[:3] == ("gh", "release", "edit") for call in calls))

    def test_exact_source_partial_draft_retry_uploads_and_promotes(self):
        calls = []
        with tempfile.TemporaryDirectory() as directory:
            candidate = pathlib.Path(directory) / "asset.tar.gz"
            candidate.write_bytes(b"asset")
            def command(*args, **kwargs):
                calls.append(args)
                return "a" * 40 if args == ("git", "rev-parse", "HEAD") else ""
            info = {"draft": True, "target_commitish": "a" * 40, "assets": []}
            with patch.object(P, "previous_version", return_value="0.8.0"), \
                 patch.object(P, "published", return_value=True), \
                 patch.object(P, "tag_elsewhere", return_value=None), \
                 patch.object(P, "build", return_value=[candidate]), \
                 patch.object(P, "release_info", return_value=info), \
                 patch.object(P, "verify_published"), \
                 patch.object(P.release, "sh", side_effect=command):
                self.assertEqual(P.main([]), 0)
        upload = next(call for call in calls if call[:3] == ("gh", "release", "upload"))
        edit = next(call for call in calls if call[:3] == ("gh", "release", "edit"))
        self.assertIn("--clobber", upload)
        self.assertLess(calls.index(upload), calls.index(edit))
        self.assertEqual(edit[-2:], ("--draft=false", "--latest"))

    def test_wrong_source_draft_refuses_retry(self):
        calls = []
        with tempfile.TemporaryDirectory() as directory:
            candidate = pathlib.Path(directory) / "asset.tar.gz"
            candidate.write_bytes(b"asset")
            with patch.object(P, "previous_version", return_value="0.8.0"), \
                 patch.object(P, "published", return_value=True), \
                 patch.object(P, "tag_elsewhere", return_value=None), \
                 patch.object(P, "build", return_value=[candidate]), \
                 patch.object(P, "release_info", return_value={"draft": True,
                     "target_commitish": "b" * 40, "assets": []}), \
                 patch.object(P.release, "sh", side_effect=self._sh(calls)):
                with self.assertRaisesRegex(SystemExit, "different source"):
                    P.main([])
        self.assertFalse(any(call[:3] == ("gh", "release", "upload") for call in calls))

    def test_published_asset_name_size_and_digest_mismatches_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            asset = pathlib.Path(directory) / "install.sh"
            asset.write_bytes(b"asset")
            expected = {"name": asset.name, "size": 5,
                        "digest": "sha256:" + hashlib.sha256(b"asset").hexdigest()}
            for change in ({"name": "other.sh"}, {"size": 4}, {"digest": "sha256:" + "0" * 64}):
                with self.subTest(change=change):
                    item = dict(expected)
                    item.update(change)
                    payload = {"assets": [item]}
                    with patch.object(P, "release_info", return_value=payload):
                        with self.assertRaisesRegex(SystemExit, "asset"):
                            P.verify_published("v" + current_version(), [asset])

    def test_verify_validates_assets_without_release_create(self):
        calls = []
        with patch.object(P, "previous_version", return_value="0.8.0"), \
             patch.object(P, "published", return_value=False), \
             patch.object(P, "tag_elsewhere", return_value=None), \
             patch.object(P, "build", return_value=[pathlib.Path("asset.tar.gz")]), \
             patch.object(P, "verify_published") as verify, \
             patch.object(P.release, "sh", side_effect=self._sh(calls)):
            self.assertEqual(P.main(["--verify"]), 0)
        verify.assert_not_called()
        self.assertFalse(any(call[:3] == ("gh", "release", "create") for call in calls))


C = _load("publish_crate")
N = _load("native_shared")
CI = _load("ci_selection")


def package_field(name):
    """A top-level [package] value of native/Cargo.toml, as written."""
    text = (ROOT / "native" / "Cargo.toml").read_text(encoding="utf-8")
    section = text.split("[package]\n", 1)[1].split("\n[", 1)[0]
    found = [line.split("=", 1)[1].strip() for line in section.splitlines()
             if line.split("=", 1)[0].strip() == name]
    return found[0] if found else None


class TheCrate(unittest.TestCase):
    """crates.io receives native/ alone, from the commit each GitHub release is made from."""

    def test_the_crate_carries_current_copies_of_what_it_shares(self):
        self.assertEqual(N.stale(), [], "refresh the copies with: " + N.FIX)

    def test_the_crate_embeds_only_its_own_files(self):
        embedded = CI.rust_embeds(("native/src",))
        self.assertIn("native/shared/page/page.css", embedded)
        self.assertEqual([path for path in embedded if not path.startswith("native/")], [])

    def test_the_manifest_publishes_under_the_release_name(self):
        self.assertEqual(package_field("name"), '"kpopper"')
        self.assertIsNone(package_field("publish"))
        self.assertEqual(package_field("license"), '"MIT"')
        self.assertEqual(package_field("repository"), '"https://github.com/ilanbm/kpopper"')
        self.assertTrue(package_field("description"))
        self.assertTrue((ROOT / "native" / json.loads(package_field("readme"))).is_file())
        manifest = (ROOT / "native" / "Cargo.toml").read_text(encoding="utf-8")
        for pattern in ('"/src/**"', '"/shared/**"', '"/build.rs"', '"/LICENSE"', '"/Cargo.lock"'):
            self.assertIn(pattern, manifest)

    def test_the_declared_minimum_rust_is_the_pinned_toolchain(self):
        pinned = (ROOT / "native" / "rust-toolchain.toml").read_text(encoding="utf-8")
        channel = pinned.split('channel = "', 1)[1].split('"', 1)[0]
        self.assertEqual(json.loads(package_field("rust-version")), ".".join(channel.split(".")[:2]))

    def test_plan_claims_publishes_or_leaves_a_version_alone(self):
        answers = {
            "claim": {"/crates/kpopper": None},
            "published": {"/crates/kpopper": {}, "/crates/kpopper/1.2.3": {"version": {}}},
            "publish": {"/crates/kpopper": {}, "/crates/kpopper/1.2.3": None},
        }
        for action, registry in answers.items():
            with self.subTest(action=action), patch.object(C, "fetch", side_effect=registry.__getitem__):
                found, reason = C.decide("1.2.3")
                self.assertEqual(found, action)
                if action == "claim":
                    self.assertIn("v1.2.3", reason)

    def test_registry_answers_other_than_found_or_missing_stop_the_run(self):
        import urllib.error
        for code in (403, 500, 503):
            error = urllib.error.HTTPError("https://crates.io", code, "no", {}, io.BytesIO())
            with self.subTest(code=code), patch.object(C.urllib.request, "urlopen", side_effect=error):
                with self.assertRaisesRegex(SystemExit, str(code)):
                    C.fetch("/crates/kpopper")
        missing = urllib.error.HTTPError("https://crates.io", 404, "no", {}, io.BytesIO())
        with patch.object(C.urllib.request, "urlopen", side_effect=missing):
            self.assertIsNone(C.fetch("/crates/kpopper"))

    def test_served_requires_the_checksum_of_the_verified_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            crate = pathlib.Path(directory) / "kpopper-1.2.3.crate"
            crate.write_bytes(b"crate")
            verified = hashlib.sha256(b"crate").hexdigest()
            with patch.object(C, "fetch", return_value={"version": {"checksum": verified}}):
                self.assertEqual(C.served("1.2.3", crate, attempts=1, pause=0), verified)
            with patch.object(C, "fetch", return_value={"version": {"checksum": "0" * 64}}):
                with self.assertRaisesRegex(SystemExit, "not the verified"):
                    C.served("1.2.3", crate, attempts=1, pause=0)
            with patch.object(C, "fetch", return_value=None):
                with self.assertRaisesRegex(SystemExit, "does not serve"):
                    C.served("1.2.3", crate, attempts=2, pause=0)

    def test_plan_refuses_a_version_the_release_planner_did_not_publish(self):
        with patch.object(C, "fetch") as fetch:
            with self.assertRaisesRegex(SystemExit, "planner published 0.0.1"):
                C.main(["--plan", "--version", "0.0.1"])
        fetch.assert_not_called()

    def test_one_job_holds_the_registry_identity_and_uploads_only_verified_bytes(self):
        import yaml
        workflows = ROOT / ".github" / "workflows"
        jobs = yaml.safe_load((workflows / "publish.yml").read_text(encoding="utf-8"))["jobs"]
        identity = [name for name, job in jobs.items() if (job.get("permissions") or {}).get("id-token")]
        self.assertEqual(identity, ["crate-publish"])
        crate, publish = jobs["crate"], jobs["crate-publish"]
        self.assertIn("publish", crate["needs"])
        self.assertIn("crate", publish["needs"])
        self.assertEqual(publish["environment"], "crates-io")
        runs = [step.get("run", "") for step in publish["steps"]]
        uses = [step.get("uses", "") for step in publish["steps"]]
        upload = next((i for i, run in enumerate(runs) if "cargo publish" in run), None)
        compare = next((i for i, run in enumerate(runs) if "cmp " in run), None)
        auth = next((i for i, use in enumerate(uses) if use.startswith("rust-lang/crates-io-auth-action@")), None)
        self.assertNotIn(None, (upload, compare, auth), "publish, compare the verified bytes, authenticate")
        asked = next((i for i, run in enumerate(runs) if "publish_crate.py --plan" in run), None)
        self.assertIsNotNone(asked, "the publishing job asks the registry again before uploading")
        self.assertLess(compare, auth)
        self.assertLess(asked, auth)
        self.assertLess(auth, upload)
        for step in (auth, upload):
            self.assertEqual(publish["steps"][step].get("if"), "steps.registry.outputs.action == 'publish'")
        self.assertRegex(uses[auth], r"@[0-9a-f]{40}$")
        self.assertIn("--no-verify", runs[upload])
        self.assertIn("--locked", runs[upload])
        self.assertFalse(any("cargo build" in run or "cargo test" in run for run in runs))
        self.assertTrue(any("cargo package --locked" in step.get("run", "") and "--no-verify" not in step["run"]
                            for step in crate["steps"]))
        for workflow in workflows.glob("*.yml"):
            self.assertNotIn("CARGO_REGISTRY_TOKEN: ${{ secrets", workflow.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
