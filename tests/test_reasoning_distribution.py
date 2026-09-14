"""Archive boundary checks; optional real installed-channel checks for candidate CI."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tarfile
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("reasoning_builder", ROOT / "scripts/reasoning/build_runtime.py")
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class ArchiveContractTests(unittest.TestCase):
    def test_source_identity_excludes_proofs_and_binds_runtime_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Main.lean").write_text("def main := pure ()")
            (root / "Proof.lean").write_text("theorem example : True := trivial")
            digest = builder.source_hash(root)
            (root / "Proof.lean").write_text("proof changed")
            (root / "AuditDependencies.lean").write_text("audit changed")
            self.assertEqual(digest, builder.source_hash(root))
            inventory = {"Main.lean": hashlib.sha256((root / "Main.lean").read_bytes()).hexdigest()}
            self.assertEqual(digest, hashlib.sha256(json.dumps(inventory, sort_keys=True, separators=(",", ":")).encode()).hexdigest())
            (root / "Main.lean").rename(root / "Renamed.lean")
            self.assertNotEqual(digest, builder.source_hash(root))

    def test_archive_is_deterministic_complete_and_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle = root / "bundle"
            bundle.mkdir()
            (bundle / "evaluator").write_bytes(b"executable")
            (bundle / "license.txt").write_text("notice")
            one, two = root / "one.zip", root / "two.zip"
            manifest = {"version": 1, "executable": "evaluator"}
            generated = builder.archive_payload(bundle, one, manifest)
            builder.archive_payload(bundle, two, manifest)
            self.assertEqual(one.read_bytes(), two.read_bytes())
            with zipfile.ZipFile(one) as zf:
                actual = json.loads(zf.read("manifest.json"))
                self.assertEqual(actual, generated)
                self.assertEqual(set(zf.namelist()), set(actual["files"]) | {"manifest.json"})
                for path, digest in actual["files"].items():
                    self.assertEqual(hashlib.sha256(zf.read(path)).hexdigest(), digest)
                self.assertEqual(zf.getinfo("evaluator").external_attr >> 16 & 0o777, 0o755)
                self.assertEqual(zf.getinfo("license.txt").external_attr >> 16 & 0o777, 0o644)
            self.assertEqual(one.with_suffix(".zip.sha256").read_text().split()[0], builder.sha256(one))

    @unittest.skipIf(os.name == "nt", "symlink creation needs an optional Windows privilege")
    def test_archive_rejects_symlink_payload(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "evaluator").symlink_to(__file__)
            with self.assertRaisesRegex(ValueError, "symlinks"):
                builder.archive_payload(root, root.parent / "unused.zip", {"executable": "evaluator"})

    def test_archive_rejects_supplied_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "manifest.json").write_text("{}")
            with self.assertRaisesRegex(ValueError, "generated"):
                builder.archive_payload(root, root.parent / "unused.zip", {"executable": "evaluator"})

    def test_compiler_build_refuses_wrong_host_before_running_tools(self):
        with patch.object(builder, "host_target", return_value="unsupported-cpu"), patch.object(builder, "run") as run:
            with self.assertRaisesRegex(ValueError, "host"):
                builder.build_archive(".", ".", "candidate.zip", "darwin-arm64")
            run.assert_not_called()

    def test_download_rejects_wrong_hash_without_refetch(self):
        with tempfile.TemporaryDirectory() as directory:
            file = Path(directory) / "source.tar.xz"
            file.write_bytes(b"wrong archive")
            with patch.object(builder.urllib.request, "urlopen") as network:
                with self.assertRaisesRegex(ValueError, "hash mismatch"):
                    builder.download("https://invalid.example/", file, "0" * 64)
                network.assert_not_called()

    def test_corresponding_source_contains_exact_tarball_and_rebuild_recipe(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            upstream = root / "gmp-6.3.0.tar.xz"
            upstream.write_bytes(b"stand-in upstream source archive")
            one, two = root / "one.tar.gz", root / "two.tar.gz"
            with patch.object(builder, "GMP_SHA256", builder.sha256(upstream)):
                builder.source_bundle(upstream, one)
                builder.source_bundle(upstream, two)
            self.assertEqual(one.read_bytes(), two.read_bytes())
            with tarfile.open(one) as tf:
                self.assertEqual(tf.extractfile("gmp-6.3.0.tar.xz").read(), upstream.read_bytes())
                self.assertEqual(tf.extractfile("build_runtime.py").read(), Path(builder.__file__).read_bytes())
                self.assertIn("reasoning-runtime.yml", tf.getnames())
                self.assertIn("COPYING.LESSERv3", tf.getnames())
                self.assertIn("COPYINGv3", tf.getnames())
                self.assertIn(b"build_gmp", tf.extractfile("SOURCE-BUILD.md").read())

    def test_linux_audit_refuses_accidental_cpp_shared_dependency(self):
        text = "Shared library: [libgmp.so.10]\nShared library: [libstdc++.so.6]"
        with patch.object(builder, "run", return_value=text):
            with self.assertRaisesRegex(ValueError, "dependencies"):
                builder.audit_linkage("binary", "gmp", "linux-x86_64", Path("."))

    def test_linkage_receipt_removes_private_build_paths(self):
        dependency = "DLL Name: libgmp-10.dll\nDLL Name: KERNEL32.dll\n"
        with patch.object(builder, "run", return_value="/private/build/evaluator.exe\n" + dependency):
            receipt = builder.audit_linkage("/private/build/evaluator.exe", "/private/build/libgmp-10.dll", "windows-x86_64", Path("."))
        self.assertNotIn("/private/build", receipt["inspection"])
        self.assertIn("evaluator.exe", receipt["inspection"])

    def test_windows_audit_refuses_missing_gmp_dll(self):
        with patch.object(builder, "run", return_value="DLL Name: KERNEL32.dll"):
            with self.assertRaisesRegex(ValueError, "dependencies"):
                builder.audit_linkage("binary", "gmp", "windows-x86_64", Path("."))


# Run in a child so source imports cannot mask an incomplete installed package.
INSTALLED_PROBE = r'''
import hashlib, json, os, pathlib, shutil, subprocess, sys
for tool in ("lean", "lake", "leanc", "clang", "gcc"):
    assert shutil.which(tool) is None, (tool, "compiler leaked into runtime PATH")
if len(sys.argv) > 1:
    sys.path.insert(0, sys.argv[1])
    from scripts.reasoning.runtime import Runtime
else:
    from kpopper.reasoning.runtime import Runtime
runtime = Runtime()
result = runtime.request({"nodes": {}, "declared": [], "expression": {"op": "div", "args": [{"num": "1"}, {"num": "3"}]}})
assert result["status"] == "ok", result
assert result["value"] == {"type": "number", "numerator": "1", "denominator": "3"}, result
print(json.dumps({"result": result, "implementation": runtime.implementation}, sort_keys=True))
replacement_root = os.environ.get("KPOPPER_TEST_REPLACEMENT_ROOT")
if replacement_root:
    library = pathlib.Path(replacement_root) / (runtime.implementation["target"] + ".library")
    assert library.is_file(), library
    before = hashlib.sha256(runtime.binary.read_bytes()).hexdigest()
    shutil.copyfile(library, runtime.root / runtime.manifest["libraries"][0])
    modified = Runtime()
    assert modified.implementation["modified_libraries"] == runtime.manifest["libraries"], modified.implementation
    assert hashlib.sha256(modified.binary.read_bytes()).hexdigest() == before
    changed = modified.request({"nodes": {}, "declared": [], "expression": {"op": "div", "args": [{"num": "1"}, {"num": "3"}]}})
    assert changed == result, changed
    receipt = subprocess.run([str(modified.binary)], input="KP1\t1000\t128\t256\t0\t0\tn\t1\n", text=True, capture_output=True, check=True)
    assert "KPOPPER_GMP_REPLACEMENT_PROBE" in receipt.stderr, receipt
    print(json.dumps({"replacement": modified.implementation}, sort_keys=True))
'''


def check_distribution(python, plugin_root=None):
    """Exercise actual installed runtime and normal CLI with empty compiler PATH."""
    python = Path(python).resolve()
    with tempfile.TemporaryDirectory(prefix="kpopper-installed-") as directory:
        root = Path(directory)
        record = root / "GROUNDING.yaml"
        record.write_text("meta:\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  a: {v: 1}\n  b: {v: 3}\n  ratio: {rule: {expr: 'a / b'}}\ndecisions:\n  d: {rests_on: [ratio], wrong_if: {expr: 'ratio > 1'}, seen: {ratio: 0}}\n")
        empty_path = root / "empty-path"
        empty_path.mkdir()
        env = {k: v for k, v in os.environ.items()
               if k not in ("PYTHONPATH", "LEAN_PATH", "LEAN_SYSROOT", "LEAN_CC", "KPOPPER_LEAN_ROOT", "KPOPPER_RUNTIME_ARCHIVE", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH")}
        env.update(PATH=str(empty_path), XDG_CACHE_HOME=str(root / "cache"),
                   HOME=str(root / "home"), LOCALAPPDATA=str(root / "local"),
                   KPOPPER_RUNTIME_CACHE=str(root / "runtime-cache"))
        probe = root / "probe.py"
        probe.write_text(INSTALLED_PROBE)
        argv = [str(python), str(probe)]
        if plugin_root:
            argv += [str(Path(plugin_root).resolve())]
        print(subprocess.check_output(argv, cwd=root, env=env, text=True), flush=True)
        # Cold cache again for the public consumer entrypoint.
        for cache in (root / "cache", root / "runtime-cache", root / "local", root / "home"):
            if cache.exists():
                shutil.rmtree(cache)
        cli = ([str(python), str(Path(plugin_root).resolve() / "scripts/cli.py")]
               if plugin_root else [str(python), "-m", "kpopper.cli"])
        command = cli + ["assess", "ratio", "d", "--profile", "core/v1", "--record", str(record)]
        raw = subprocess.check_output(command, cwd=root, env=env, text=True)
        result = json.loads(raw)
        assert result["assessment_profile"] == "core/v1", result
        assert result["nodes"]["ratio"]["computation"]["value"] == {"type": "number", "numerator": "1", "denominator": "3"}, result
        assert "operational_error" not in raw, result
        print(raw, flush=True)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--installed-python":
        check_distribution(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else None)
    else:
        unittest.main()
