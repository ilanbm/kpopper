"""Validation and staging for the five native GitHub release archives."""

import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import stat
import tarfile
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "native_assets", ROOT / ".github" / "scripts" / "native_assets.py")
N = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(N)


class NativeArtifacts(unittest.TestCase):
    version = "0.9.0"
    commit = "a8c6d20d41f1541e3f31923f31e8a4c969c69d42"

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="native-assets-")
        self.root = Path(self.temporary.name)
        self.artifacts = self.root / "artifacts"
        self.repository = self.root / "repository"
        self.output = self.root / "release"
        self.artifacts.mkdir()
        self.repository.mkdir()
        (self.repository / "install.sh").write_bytes(b"#!/bin/sh\n")
        (self.repository / "install.ps1").write_bytes(b"Write-Host kpopper\n")
        for target in N.TARGETS:
            self.write_archive(target)

    def tearDown(self):
        self.temporary.cleanup()

    def members(self, target, version=None, commit=None, manifest_target=None):
        windows = target == "windows-x86_64"
        executable = ".exe" if windows else ""
        names = {
            "bin/kpop" + executable: (b"native-command", 0o755),
            "bin/kpopper" + executable: (b"native-alias", 0o755),
            "bin/resources/reasoning/%s.kpopper-runtime" % target: (b"reasoning", 0o644),
            "bin/resources/ordinary/%s/build.json" % target: (b'{"source":"fixture"}\n', 0o644),
            "bin/resources/ordinary/%s/epistemic-core%s" % (target, executable):
                (b"ordinary-core", 0o755),
        }
        manifest = {
            "version": version or self.version,
            "target": manifest_target or target,
            "source_commit": self.commit if commit is None else commit,
            "files": [
                {"path": path, "sha256": hashlib.sha256(body).hexdigest(), "mode": "%04o" % mode}
                for path, (body, mode) in sorted(names.items())
            ],
        }
        names["manifest.json"] = ((json.dumps(manifest, sort_keys=True) + "\n").encode(), 0o644)
        return names

    def write_archive(self, target, *, members=None, archive_version=None, folder=None):
        version = archive_version or self.version
        top = "kpopper-%s-%s" % (version, target)
        directory = folder or (self.artifacts / target)
        directory.mkdir(parents=True, exist_ok=True)
        suffix = ".zip" if target == "windows-x86_64" else ".tar.gz"
        archive = directory / (top + suffix)
        members = members or self.members(target, version=version)
        if target == "windows-x86_64":
            with zipfile.ZipFile(archive, "w") as output:
                for relative, (body, mode) in members.items():
                    info = zipfile.ZipInfo(top + "/" + relative)
                    info.create_system = 3
                    info.external_attr = (stat.S_IFREG | mode) << 16
                    output.writestr(info, body)
        else:
            with tarfile.open(archive, "w:gz") as output:
                for relative, (body, mode) in members.items():
                    info = tarfile.TarInfo(top + "/" + relative)
                    info.mode = mode
                    info.size = len(body)
                    output.addfile(info, io.BytesIO(body))
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.with_name(archive.name + ".sha256").write_text(
            "%s  %s\n" % (checksum, archive.name), encoding="ascii")
        return archive

    def rewrite(self, target, members):
        folder = self.artifacts / target
        shutil.rmtree(folder)
        return self.write_archive(target, members=members)

    def refuse(self, pattern):
        with self.assertRaisesRegex(N.NativeAssetError, pattern):
            N.prepare(self.version, self.commit, self.artifacts, self.output, self.repository)
        self.assertFalse(self.output.exists())

    def test_good_five_target_set_is_staged_with_installers_and_actual_sums(self):
        delivered = N.prepare(
            self.version, self.commit, self.artifacts, self.output, self.repository)
        self.assertEqual(len(delivered), 8)
        self.assertEqual({path.name for path in delivered},
                         {"kpopper-%s-%s%s" %
                          (self.version, target, ".zip" if target == "windows-x86_64" else ".tar.gz")
                          for target in N.TARGETS} |
                         {"install.sh", "install.ps1", "SHA256SUMS"})
        sums = {}
        for line in (self.output / "SHA256SUMS").read_text(encoding="ascii").splitlines():
            checksum, name = line.split("  ", 1)
            sums[name] = checksum
        self.assertEqual(set(sums), {path.name for path in delivered if path.name != "SHA256SUMS"})
        for name, checksum in sums.items():
            self.assertEqual(checksum, hashlib.sha256((self.output / name).read_bytes()).hexdigest())

    def test_an_existing_empty_output_directory_can_be_populated(self):
        self.output.mkdir()
        delivered = N.prepare(
            self.version, self.commit, self.artifacts, self.output, self.repository)
        self.assertEqual(len(delivered), 8)

    def test_changed_archive_bytes_are_rejected_before_output_is_touched(self):
        archive = self.artifacts / "linux-x86_64" / "kpopper-0.9.0-linux-x86_64.tar.gz"
        with archive.open("ab") as output:
            output.write(b"changed")
        self.refuse("does not match sidecar")

    def test_wrong_commit_version_and_target_are_rejected(self):
        cases = (("linux-x86_64", self.members("linux-x86_64", commit="wrong"), "source commit"),
                 ("linux-x86_64", self.members("linux-x86_64", version="0.8.0"), "manifest version"),
                 ("linux-x86_64", self.members("linux-x86_64", manifest_target="darwin-arm64"),
                  "manifest target"))
        for target, members, pattern in cases:
            with self.subTest(pattern=pattern):
                self.rewrite(target, members)
                self.refuse(pattern)
                self.output = self.root / ("release-" + pattern.replace(" ", "-"))
                self.rewrite(target, self.members(target))
        wrong = self.write_archive("linux-x86_64", archive_version="0.8.0",
                                   folder=self.artifacts / "old")
        self.refuse("mis-versioned")
        self.assertTrue(wrong.exists())

    def test_missing_and_duplicate_platforms_are_rejected(self):
        shutil.rmtree(self.artifacts / "darwin-arm64")
        self.refuse("darwin-arm64 archive; found 0")
        self.write_archive("darwin-arm64")
        source = self.artifacts / "linux-aarch64"
        duplicate = self.artifacts / "duplicate"
        shutil.copytree(source, duplicate)
        self.output = self.root / "duplicate-output"
        self.refuse("linux-aarch64 archive; found 2")

    def test_traversal_and_link_members_are_rejected(self):
        target = "linux-x86_64"
        members = self.members(target)
        members["../escape"] = (b"bad", 0o644)
        self.rewrite(target, members)
        self.refuse("unsafe archive path|unexpected archive member")

        self.rewrite(target, self.members(target))
        archive = self.artifacts / target / "kpopper-0.9.0-linux-x86_64.tar.gz"
        archive.unlink()
        top = "kpopper-0.9.0-linux-x86_64"
        with tarfile.open(archive, "w:gz") as output:
            info = tarfile.TarInfo(top + "/link")
            info.type = tarfile.SYMTYPE
            info.linkname = "/tmp/elsewhere"
            output.addfile(info)
        archive.with_name(archive.name + ".sha256").write_text(
            hashlib.sha256(archive.read_bytes()).hexdigest() + "  " + archive.name + "\n",
            encoding="ascii")
        self.output = self.root / "link-output"
        self.refuse("link or special file")

    def test_unlisted_payload_and_missing_resources_are_rejected(self):
        target = "darwin-x86_64"
        members = self.members(target)
        members["bin/unlisted"] = (b"extra", 0o644)
        self.rewrite(target, members)
        self.refuse("unlisted")
        members = self.members(target)
        del members["bin/resources/reasoning/%s.kpopper-runtime" % target]
        manifest = json.loads(members["manifest.json"][0])
        manifest["files"] = [entry for entry in manifest["files"]
                             if entry["path"] != "bin/resources/reasoning/%s.kpopper-runtime" % target]
        members["manifest.json"] = ((json.dumps(manifest) + "\n").encode(), 0o644)
        self.rewrite(target, members)
        self.output = self.root / "missing-resources-output"
        self.refuse("required payload files are missing")

    def test_payload_digest_and_case_collisions_are_rejected(self):
        target = "darwin-arm64"
        members = self.members(target)
        members["bin/kpop"] = (b"changed-command", 0o755)
        self.rewrite(target, members)
        self.refuse("payload SHA256 does not match")

        members = self.members(target)
        members["bin/KPOP"] = (b"collision", 0o755)
        manifest = json.loads(members["manifest.json"][0])
        manifest["files"].append({
            "path": "bin/KPOP", "sha256": hashlib.sha256(b"collision").hexdigest(),
            "mode": "0755",
        })
        members["manifest.json"] = ((json.dumps(manifest) + "\n").encode(), 0o644)
        self.rewrite(target, members)
        self.output = self.root / "case-output"
        self.refuse("case-colliding archive paths")

    def test_manifest_mode_mismatch_and_nonexecutable_unix_binary_are_rejected(self):
        target = "linux-aarch64"
        members = self.members(target)
        manifest = json.loads(members["manifest.json"][0])
        next(entry for entry in manifest["files"] if entry["path"] == "bin/kpop")["mode"] = "0644"
        members["manifest.json"] = ((json.dumps(manifest) + "\n").encode(), 0o644)
        self.rewrite(target, members)
        self.refuse("mode does not match")

        members = self.members(target)
        members["bin/kpop"] = (members["bin/kpop"][0], 0o644)
        manifest = json.loads(members["manifest.json"][0])
        next(entry for entry in manifest["files"] if entry["path"] == "bin/kpop")["mode"] = "0644"
        members["manifest.json"] = ((json.dumps(manifest) + "\n").encode(), 0o644)
        self.rewrite(target, members)
        self.output = self.root / "mode-output"
        self.refuse("does not have executable mode")

    def test_refusal_preserves_preexisting_output(self):
        self.output.mkdir()
        marker = self.output / "keep.txt"
        marker.write_text("keep", encoding="utf-8")
        with self.assertRaisesRegex(N.NativeAssetError, "not empty"):
            N.prepare(self.version, self.commit, self.artifacts, self.output, self.repository)
        self.assertEqual(marker.read_text(encoding="utf-8"), "keep")
        self.assertEqual(list(self.output.iterdir()), [marker])


if __name__ == "__main__":
    unittest.main()
