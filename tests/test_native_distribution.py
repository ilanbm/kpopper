"""Native archives and installers are deterministic, verified, and path-safe."""

import hashlib
import io
import json
import os
from pathlib import Path
import platform
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[1]
PACKAGER = ROOT / "scripts/package_native.py"
INSTALLER = ROOT / "install.sh"


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def host_target():
    system = platform.system()
    machine = platform.machine().lower()
    if system == "Darwin" and machine in ("arm64", "aarch64"):
        return "darwin-arm64"
    if system == "Darwin" and machine == "x86_64":
        return "darwin-x86_64"
    if system == "Linux" and machine in ("arm64", "aarch64"):
        return "linux-aarch64"
    if system == "Linux" and machine == "x86_64":
        return "linux-x86_64"
    return None


class NativeDistribution(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="native distribution ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.inputs = self.root / "inputs with spaces"
        self.inputs.mkdir()
        self.resources = self.inputs / "resources"
        for kind in ("reasoning", "ordinary"):
            folder = self.resources / kind
            (folder / "nested dir").mkdir(parents=True)
            (folder / "data.txt").write_text(kind + "\n", encoding="utf-8")
            (folder / "nested dir/value.json").write_text('{"kind":"%s"}\n' % kind, encoding="utf-8")
        self.binary = self.inputs / "kpop"
        self.alias = self.inputs / "kpopper"
        self.binary.write_text("#!/bin/sh\nprintf 'kpop <%s>\\n' \"$*\"\ncat\n", encoding="utf-8")
        self.alias.write_text("#!/bin/sh\nprintf 'kpopper <%s>\\n' \"$*\"\ncat\n", encoding="utf-8")
        self.binary.chmod(0o755)
        self.alias.chmod(0o755)
        self.resource_executable = self.resources / "ordinary" / "epistemic-core"
        self.resource_executable.write_text("#!/bin/sh\nprintf 'resource:%s' \"$1\"\n", encoding="utf-8")
        self.resource_executable.chmod(0o755)
        (self.resources / "ordinary" / "epistemic-core.exe").write_bytes(b"windows executable")

    def package(self, target=None, output=None, version="0.9.0"):
        target = target or host_target()
        output = output or (self.root / "output with spaces")
        binary, alias = self.binary, self.alias
        if target == "windows-x86_64":
            binary, alias = self.inputs / "kpop.exe", self.inputs / "kpopper.exe"
            binary.write_bytes(self.binary.read_bytes())
            alias.write_bytes(self.alias.read_bytes())
        result = subprocess.run([
            sys.executable, str(PACKAGER), "--binary", str(binary), "--alias", str(alias),
            "--resources", str(self.resources), "--target", target, "--version", version,
            "--commit", "e6f58c4", "--output", str(output),
        ], text=True, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        return Path(report["archive"]), report

    def install(self, archive, prefix, digest=None, version="0.9.0", env=None):
        return subprocess.run([
            "sh", str(INSTALLER), "--version", version, "--archive", str(archive),
            "--sha256", digest or sha256(archive), "--prefix", str(prefix),
        ], text=True, capture_output=True, timeout=20, env=env)

    def test_packager_emits_deterministic_manifest_and_modes(self):
        target = host_target() or "linux-x86_64"
        first, report = self.package(target, self.root / "first")
        second, _ = self.package(target, self.root / "second")
        self.assertEqual(first.read_bytes(), second.read_bytes())
        self.assertEqual(Path(report["checksum_file"]).read_text().split()[0], sha256(first))
        with tarfile.open(first, "r:gz") as archive:
            members = archive.getmembers()
            self.assertTrue(all(item.isfile() or item.isdir() for item in members))
            top = "kpopper-0.9.0-" + target
            manifest = json.load(archive.extractfile(top + "/manifest.json"))
            files = {item["path"]: item for item in manifest["files"]}
            self.assertEqual(manifest["source_commit"], "e6f58c4")
            self.assertEqual(files["bin/kpop"]["mode"], "0755")
            self.assertEqual(files["bin/resources/ordinary/data.txt"]["mode"], "0644")
            self.assertEqual(files["bin/resources/ordinary/epistemic-core"]["mode"], "0755")
            for item in manifest["files"]:
                payload = archive.extractfile(top + "/" + item["path"]).read()
                self.assertEqual(hashlib.sha256(payload).hexdigest(), item["sha256"])
            self.assertEqual(archive.getmember(top + "/bin/kpop").mode, 0o755)

    def test_windows_zip_has_expected_public_names_and_modes(self):
        archive, _ = self.package("windows-x86_64")
        with zipfile.ZipFile(archive) as source:
            top = "kpopper-0.9.0-windows-x86_64"
            manifest = json.loads(source.read(top + "/manifest.json"))
            files = {item["path"]: item for item in manifest["files"]}
            self.assertIn("bin/kpop.exe", files)
            self.assertIn("bin/kpopper.exe", files)
            self.assertEqual(files["bin/resources/ordinary/epistemic-core.exe"]["mode"], "0755")
            self.assertEqual((source.getinfo(top + "/bin/kpop.exe").external_attr >> 16) & 0o777, 0o755)

    def test_windows_public_install_keeps_resources_adjacent_and_managed(self):
        script = (ROOT / "install.ps1").read_text(encoding="utf-8")
        self.assertIn('$PublicResources = Join-Path $PublicBin "resources"', script)
        self.assertIn('Join-Path $Destination "bin/resources"', script)
        self.assertIn('Move-Item -LiteralPath $TemporaryResources -Destination $PublicResources', script)
        self.assertIn('refusing to replace unmanaged path: $PublicResources', script)
        self.assertIn('Move-Item -LiteralPath $ResourcesBackup -Destination $PublicResources', script)

    def test_packager_rejects_symlink_and_extra_resource_root(self):
        os.symlink(self.resources / "ordinary/data.txt", self.resources / "reasoning/link")
        result = subprocess.run([
            sys.executable, str(PACKAGER), "--binary", str(self.binary), "--alias", str(self.alias),
            "--resources", str(self.resources), "--target", "linux-x86_64", "--version", "0.9.0",
            "--commit", "abc", "--output", str(self.root / "bad")
        ], text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("regular file", result.stderr)

    @unittest.skipIf(os.name == "nt" or host_target() is None, "POSIX installer contract")
    def test_offline_install_handles_spaces_and_preserves_arguments_and_stdin(self):
        archive, _ = self.package()
        prefix = self.root / "prefix with spaces"
        result = self.install(archive, prefix)
        self.assertEqual(result.returncode, 0, result.stderr)
        installed = prefix / "lib/kpopper/0.9.0" / host_target()
        self.assertTrue((installed / "bin/resources/reasoning/data.txt").is_file())
        resource = installed / "bin/resources/ordinary/epistemic-core"
        self.assertTrue(os.access(resource, os.X_OK))
        self.assertEqual(subprocess.check_output([str(resource), "works"], text=True), "resource:works")
        for name in ("kpop", "kpopper"):
            public = prefix / "bin" / name
            self.assertTrue(public.is_symlink())
            invoked = subprocess.run([str(public), "value with spaces", "$literal"], input="stdin-data",
                                     text=True, capture_output=True)
            self.assertEqual(invoked.returncode, 0)
            self.assertIn("<value with spaces $literal>", invoked.stdout)
            self.assertTrue(invoked.stdout.endswith("stdin-data"))

    @unittest.skipIf(os.name == "nt" or host_target() is None, "POSIX installer contract")
    def test_tampered_hash_fails_before_destination_change(self):
        archive, _ = self.package()
        prefix = self.root / "prefix"
        result = self.install(archive, prefix, "0" * 64)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not match", result.stderr)
        self.assertFalse(prefix.exists())

    def malicious_tar(self, name, kind="file"):
        path = self.root / (kind + ".tar.gz")
        with tarfile.open(path, "w:gz") as archive:
            info = tarfile.TarInfo(name)
            info.mtime = 0
            if kind == "link":
                info.type = tarfile.SYMTYPE
                info.linkname = "/tmp/escape"
                archive.addfile(info)
            else:
                data = b"bad"
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
        return path

    @unittest.skipIf(os.name == "nt" or host_target() is None, "POSIX installer contract")
    def test_traversal_and_link_archives_are_rejected(self):
        top = "kpopper-0.9.0-" + host_target()
        for archive in (self.malicious_tar(top + "/../escape"),
                        self.malicious_tar(top + "/bin/kpop", "link")):
            with self.subTest(archive=archive.name):
                result = self.install(archive, self.root / (archive.stem + " prefix"))
                self.assertNotEqual(result.returncode, 0)
                self.assertRegex(result.stderr, "unsafe archive path|link or special")

    @unittest.skipIf(os.name == "nt" or host_target() is None, "POSIX installer contract")
    def test_missing_required_payload_is_rejected(self):
        top = "kpopper-0.9.0-" + host_target()
        archive = self.root / "missing.tar.gz"
        with tarfile.open(archive, "w:gz") as output:
            for name, mode, data in [
                (top + "/bin/kpop", 0o755, b"binary"),
                (top + "/bin/kpopper", 0o755, b"alias"),
                (top + "/manifest.json", 0o644, b"{}\n"),
            ]:
                info = tarfile.TarInfo(name)
                info.mode, info.size = mode, len(data)
                output.addfile(info, io.BytesIO(data))
            directory = tarfile.TarInfo(top + "/bin/resources/reasoning/")
            directory.type, directory.mode = tarfile.DIRTYPE, 0o755
            output.addfile(directory)
        result = self.install(archive, self.root / "missing prefix")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ordinary resources are missing", result.stderr)

    @unittest.skipIf(os.name == "nt" or host_target() is None, "POSIX installer contract")
    def test_plugin_mode_stages_only_verified_bin_contents(self):
        archive, _ = self.package()
        plugin = self.root / "plugin with spaces"
        result = subprocess.run([
            "sh", str(INSTALLER), "--version", "0.9.0", "--archive", str(archive),
            "--sha256", sha256(archive), "--plugin-root", str(plugin),
        ], text=True, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        runtime = plugin / "scripts/runtime" / host_target()
        self.assertTrue((runtime / "kpop").is_file())
        self.assertTrue((runtime / "resources/ordinary/data.txt").is_file())
        self.assertFalse((runtime / "manifest.json").exists())

    def failing_mv_environment(self, destination):
        tools = self.root / ("failing mv " + hashlib.sha256(str(destination).encode()).hexdigest()[:8])
        tools.mkdir()
        real_mv = subprocess.check_output(["sh", "-c", "command -v mv"], text=True).strip()
        script = tools / "mv"
        script.write_text(
            "#!/bin/sh\nlast=\nfor value do last=$value; done\n"
            "if [ \"$last\" = \"$FAIL_MV_DEST\" ] && [ ! -e \"$FAIL_MV_STATE\" ]; then\n"
            "  : > \"$FAIL_MV_STATE\"\n  exit 73\nfi\n"
            "exec \"$REAL_MV\" \"$@\"\n", encoding="utf-8")
        script.chmod(0o755)
        return dict(os.environ, PATH=str(tools) + os.pathsep + os.environ.get("PATH", ""),
                    FAIL_MV_DEST=str(destination), FAIL_MV_STATE=str(tools / "failed-once"), REAL_MV=real_mv)

    @unittest.skipIf(os.name == "nt" or host_target() is None, "POSIX installer contract")
    def test_failed_same_version_activation_restores_payload_and_public_aliases(self):
        prefix = self.root / "rollback prefix"
        original, _ = self.package()
        self.assertEqual(self.install(original, prefix).returncode, 0)
        before = {name: (prefix / "bin" / name).resolve().read_bytes() for name in ("kpop", "kpopper")}
        self.binary.write_text("#!/bin/sh\nprintf changed\n", encoding="utf-8")
        replacement, _ = self.package(output=self.root / "replacement")
        fail_at = prefix / "bin/kpopper"
        result = self.install(replacement, prefix, env=self.failing_mv_environment(fail_at))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cannot activate public command", result.stderr)
        for name in ("kpop", "kpopper"):
            public = prefix / "bin" / name
            self.assertTrue(public.is_symlink())
            self.assertEqual(public.resolve().read_bytes(), before[name])
        destination = prefix / "lib/kpopper/0.9.0" / host_target()
        self.assertFalse(any(destination.parent.glob(".previous-*")))

    @unittest.skipIf(os.name == "nt" or host_target() is None, "POSIX installer contract")
    def test_failed_plugin_activation_restores_previous_runtime(self):
        plugin = self.root / "rollback plugin"
        original, _ = self.package()
        command = ["sh", str(INSTALLER), "--version", "0.9.0", "--archive", str(original),
                   "--sha256", sha256(original), "--plugin-root", str(plugin)]
        self.assertEqual(subprocess.run(command, text=True, capture_output=True).returncode, 0)
        runtime = plugin / "scripts/runtime" / host_target()
        before = (runtime / "kpop").read_bytes()
        self.binary.write_text("#!/bin/sh\nprintf changed\n", encoding="utf-8")
        replacement, _ = self.package(output=self.root / "plugin replacement")
        command[5] = str(replacement)
        command[7] = sha256(replacement)
        result = subprocess.run(command, text=True, capture_output=True,
                                env=self.failing_mv_environment(runtime), timeout=20)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cannot activate plugin runtime", result.stderr)
        self.assertEqual((runtime / "kpop").read_bytes(), before)
        self.assertFalse(any(runtime.parent.glob(".previous-*")))

    @unittest.skipIf(os.name == "nt" or host_target() is None, "POSIX installer contract")
    def test_new_version_retains_previous_version_payload(self):
        prefix = self.root / "versions prefix"
        old, _ = self.package(output=self.root / "old", version="0.8.0")
        self.assertEqual(self.install(old, prefix, version="0.8.0").returncode, 0)
        new, _ = self.package(output=self.root / "new", version="0.9.0")
        self.assertEqual(self.install(new, prefix, version="0.9.0").returncode, 0)
        self.assertTrue((prefix / "lib/kpopper/0.8.0" / host_target() / "bin/kpop").is_file())
        self.assertTrue((prefix / "lib/kpopper/0.9.0" / host_target() / "bin/kpop").is_file())

    @unittest.skipIf(os.name == "nt" or host_target() is None, "POSIX installer contract")
    def test_wrong_platform_missing_payload_and_unmanaged_destination_are_rejected(self):
        wrong = "linux-aarch64" if host_target() != "linux-aarch64" else "linux-x86_64"
        archive, _ = self.package(wrong)
        result = self.install(archive, self.root / "wrong")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unexpected archive member", result.stderr)

        valid, _ = self.package()
        protected = self.root / "protected"
        (protected / "bin").mkdir(parents=True)
        (protected / "bin/kpop").write_text("user file", encoding="utf-8")
        result = self.install(valid, protected)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unmanaged", result.stderr)
        self.assertEqual((protected / "bin/kpop").read_text(), "user file")


if __name__ == "__main__":
    unittest.main()
