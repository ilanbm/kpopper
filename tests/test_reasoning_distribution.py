"""Archive boundary checks; optional real installed-channel checks for candidate CI."""
import hashlib
import importlib.util
import io
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
    def test_both_pinned_linux_gmp_flags_select_replaceable_library(self):
        flags = ['--sysroot', '/lean', '-Wl,-Bstatic', '-lgmp', '-lunwind',
                 '-Wl,-Bdynamic', '-lleanrt', '-lgmp', '-luv']
        linked = builder.dynamic_gmp_flags(flags, '/bundle/libgmp.so.10')
        self.assertEqual(linked.count('/bundle/libgmp.so.10'), 2)
        self.assertNotIn('-lgmp', linked)
        mode = 'dynamic'
        for flag in linked:
            if flag == '-Wl,-Bstatic': mode = 'static'
            if flag == '-Wl,-Bdynamic': mode = 'dynamic'
            if flag == '/bundle/libgmp.so.10': self.assertEqual(mode, 'dynamic')
            if flag == '-lunwind': self.assertEqual(mode, 'static')
        self.assertEqual(builder.dynamic_gmp_flags(['-lgmp'], '/bundle/gmp.dylib'), ['/bundle/gmp.dylib'])
        for unexpected in ([], ['-lgmp'] * 3):
            with self.assertRaisesRegex(ValueError, 'configuration'):
                builder.dynamic_gmp_flags(unexpected, '/bundle/gmp')

    def test_windows_gmp_tools_do_not_resolve_the_system_wsl_shim(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'usr/bin').mkdir(parents=True)
            for name in ('bash.exe', 'make.exe'):
                (root / 'usr/bin' / name).write_bytes(b'fixture')
            env = {'KPOPPER_MSYS2_ROOT': str(root), 'PATH': 'C:/Windows/System32'}
            bash, make = builder.gmp_tools('windows-x86_64', env)
            self.assertEqual(bash, root.resolve() / 'usr/bin/bash.exe')
            self.assertEqual(make, root.resolve() / 'usr/bin/make.exe')
            self.assertEqual(env['CONFIG_SHELL'], bash.as_posix())
        with self.assertRaisesRegex(ValueError, 'KPOPPER_MSYS2_ROOT'):
            builder.gmp_tools('windows-x86_64', {'PATH': 'C:/Windows/System32'})

    def test_named_proof_audit_rejects_admissions_and_missing_targets(self):
        names = ['Kpopper.evaluate', 'Kpopper.arithmetic', 'Kpopper.Proof.binary_sound',
                 'Kpopper.Proof.evaluate_closedRat_sound', 'Kpopper.Proof.evaluate_literal_success',
                 'Kpopper.Query.prepare', 'Kpopper.Query.execute', 'Kpopper.Query.responseFor']
        clean = '\n'.join("'" + name + "' depends on axioms: [propext, Classical.choice, Quot.sound]" for name in names)
        self.assertEqual(builder.validate_axiom_audit(clean), sorted(names))
        for invalid in (clean.replace('propext', 'sorryAx'), clean.replace('propext', 'custom_unproved_axiom'),
                        '\n'.join(clean.splitlines()[:-1]), ''):
            with self.assertRaises(ValueError):
                builder.validate_axiom_audit(invalid)

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
                rebuild = tf.extractfile("SOURCE-BUILD.md").read()
                self.assertIn(b'export KPOPPER_MSYS2_ROOT="$(cygpath -m /)"', rebuild)
                self.assertIn(b'-std=gnu17', rebuild)

    def test_linux_audit_refuses_accidental_cpp_shared_dependency(self):
        text = "Shared library: [libgmp.so.10]\nShared library: [libstdc++.so.6]"
        with patch.object(builder, "run", return_value=text):
            with self.assertRaisesRegex(ValueError, "dependencies"):
                builder.audit_linkage("binary", "gmp", "linux-x86_64", Path("."))

    def test_linux_audit_requires_host_resolution_of_shared_library_symbols(self):
        def outputs(argv):
            if argv[0] == 'readelf' and argv[1] == '-d':
                return 'Shared library: [libgmp.so.10]\nShared library: [libc.so.6]'
            if argv[0] == 'ldd':
                return 'libc.so.6 => /host/libc.so.6\n'
            if argv[0] == 'nm':
                return ' U __gmpz_init\n'
            return 'Name: GLIBC_2.38\n'
        with patch.object(builder, 'run', side_effect=outputs):
            result = builder.audit_linkage('binary', 'gmp', 'linux-x86_64', Path('.'))
            self.assertTrue(result['runtime_relocations_verified'])
            self.assertEqual(result['min_os'], 'glibc 2.38')
        for failure in ('undefined symbol: missing_function', 'libgmp.so.10 => not found'):
            with patch.object(builder, 'run', side_effect=lambda argv: failure if argv[0] == 'ldd' else outputs(argv)):
                with self.assertRaisesRegex(ValueError, 'unresolved Linux'):
                    builder.audit_linkage('binary', 'gmp', 'linux-x86_64', Path('.'))

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


class CommittedBundleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.native = self.root / "native"
        self.native.mkdir()
        self.source = self.root / "lean"
        self.source.mkdir()
        (self.source / "Main.lean").write_text("def main : IO Unit := pure ()\n", encoding="utf-8")
        self.upstream = self.root / "gmp-6.3.0.tar.xz"
        self.upstream.write_bytes(b"synthetic pinned source; no compilation")
        pin = patch.object(builder, "GMP_SHA256", builder.sha256(self.upstream))
        pin.start()
        self.addCleanup(pin.stop)
        for target in builder.TARGETS:
            payload = self.root / target
            payload.mkdir()
            (payload / "evaluator").write_bytes(b"synthetic executable")
            (payload / "libgmp").write_bytes(b"synthetic library")
            shutil.copytree(ROOT / "scripts/reasoning/third_party", payload / "licenses")
            shutil.copyfile(payload / "licenses/THIRD_PARTY_NOTICES.txt", payload / "THIRD_PARTY_NOTICES.txt")
            builder.archive_payload(payload, self.native / (target + ".zip"), {
                "version": 3, "protocols": ["KP2", "KP3", "KP4"], "target": target, "min_os": "test fixture",
                "lean_version": builder.LEAN_VERSION, "source_sha256": builder.source_hash(self.source),
                "files": {}, "executable": "evaluator", "libraries": ["libgmp"],
                "modules": ["arithmetic/v1", "composition/v1", "query/v1"]})
        builder.source_bundle(self.upstream, self.native / "gmp-source-and-build.tar.gz")

    def check(self):
        return builder.check_bundles(self.native, self.source)

    def rewrite_zip(self, mutate):
        path = self.native / "darwin-arm64.zip"
        with zipfile.ZipFile(path) as zf:
            members = [(entry, zf.read(entry)) for entry in zf.infolist()]
        with zipfile.ZipFile(path, "w") as zf:
            for entry, data in mutate(members):
                zf.writestr(entry, data)
        path.with_suffix(".zip.sha256").unlink()

    def rewrite_source(self, mutate):
        path = self.native / "gmp-source-and-build.tar.gz"
        with tarfile.open(path) as tf:
            files = {entry.name: tf.extractfile(entry).read() for entry in tf}
        mutate(files)
        with tarfile.open(path, "w:gz") as tf:
            for name, data in files.items():
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                tf.addfile(entry, io.BytesIO(data))

    def test_complete_bundles_check_without_tools_or_extraction(self):
        before = {p.name: p.read_bytes() for p in self.native.iterdir()}
        with patch.object(builder, "run", side_effect=AssertionError("must not execute")), \
                patch.object(builder.urllib.request, "urlopen", side_effect=AssertionError("must not download")), \
                patch.object(zipfile.ZipFile, "extractall", side_effect=AssertionError("must not extract")), \
                patch.object(tarfile.TarFile, "extractall", side_effect=AssertionError("must not extract")):
            receipt = self.check()
        self.assertEqual(set(receipt["archives"]), set(builder.TARGETS))
        self.assertEqual(before, {p.name: p.read_bytes() for p in self.native.iterdir()})

    def test_missing_target_refuses(self):
        (self.native / "windows-x86_64.zip").unlink()
        with self.assertRaisesRegex(ValueError, "inventory mismatch"):
            self.check()

    def test_stale_runtime_source_refuses(self):
        (self.source / "Main.lean").write_text("changed", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "stale runtime"):
            self.check()

    def test_malformed_manifest_refuses(self):
        self.rewrite_zip(lambda members: [(entry, b"{broken" if entry.filename == "manifest.json" else data)
                                         for entry, data in members])
        with self.assertRaisesRegex(ValueError, "invalid runtime archive"):
            self.check()

    def test_scalar_only_manifest_cannot_claim_the_composition_build(self):
        def downgrade(members):
            changed = []
            for entry, data in members:
                if entry.filename == "manifest.json":
                    manifest = json.loads(data)
                    manifest["protocols"] = ["KP2"]
                    manifest["modules"] = ["arithmetic/v1"]
                    data = json.dumps(manifest).encode()
                changed.append((entry, data))
            return changed
        self.rewrite_zip(downgrade)
        with self.assertRaisesRegex(ValueError, "malformed runtime manifest"):
            self.check()

    def test_mismatched_payload_refuses(self):
        self.rewrite_zip(lambda members: [(entry, b"modified" if entry.filename == "evaluator" else data)
                                         for entry, data in members])
        with self.assertRaisesRegex(ValueError, "payload hash mismatch"):
            self.check()

    def test_unsafe_path_refuses(self):
        self.rewrite_zip(lambda members: members + [(zipfile.ZipInfo("../outside"), b"unsafe")])
        with self.assertRaisesRegex(ValueError, "unsafe archive path"):
            self.check()

    def test_duplicate_manifest_refuses(self):
        import warnings
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            self.rewrite_zip(lambda members: members + [next(x for x in members if x[0].filename == "manifest.json")])
        with self.assertRaisesRegex(ValueError, "archive inventory"):
            self.check()

    def test_missing_source_archive_refuses(self):
        (self.native / "gmp-source-and-build.tar.gz").unlink()
        with self.assertRaisesRegex(ValueError, "corresponding-source archive"):
            self.check()

    def test_corresponding_gmp_source_mismatch_refuses(self):
        self.rewrite_source(lambda files: files.update({"gmp-6.3.0.tar.xz": b"different GMP"}))
        with self.assertRaisesRegex(ValueError, "GMP source hash mismatch"):
            self.check()

    def test_stale_source_build_recipe_refuses(self):
        self.rewrite_source(lambda files: files.update({"build_runtime.py": b"old builder"}))
        with self.assertRaisesRegex(ValueError, "recipe or inventory mismatch"):
            self.check()

    def test_unsafe_source_archive_refuses(self):
        self.rewrite_source(lambda files: files.update({"../outside": b"unsafe"}))
        with self.assertRaisesRegex(ValueError, "unsafe archive path"):
            self.check()


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
def make_runtime():
    return Runtime(os.environ.get("KPOPPER_TEST_ARCHIVE"))
runtime = make_runtime()
assert runtime.implementation["protocol"] == "KP2", runtime.implementation
result = runtime.request({"nodes": {}, "declared": [], "expression": {"op": "div", "args": [{"num": "1"}, {"num": "3"}]}})
assert result["status"] == "ok", result
assert result["value"] == {"type": "number", "numerator": "1", "denominator": "3"}, result
assert runtime.implementation_for({"protocol": "KP3", "nodes": {}, "declared": [], "limits": {}, "expression": {"list": []}})["protocol"] == "KP3"
print(json.dumps({"result": result, "implementation": runtime.implementation}, sort_keys=True))
corpus = json.loads(pathlib.Path(os.environ["KPOPPER_TEST_CORPUS"]).read_text(encoding="utf-8"))
responses = runtime.request_many([case["request"] for case in corpus["cases"]])
for case, actual in zip(corpus["cases"], responses):
    for key, expected in case["expected"].items():
        assert actual[key] == expected, (case["id"], key, actual[key], expected)
print(json.dumps({"channel": "source-candidate" if os.environ.get("KPOPPER_TEST_ARCHIVE") else "extracted-plugin" if len(sys.argv) > 1 else "installed-python", "scalar_cases_passed": len(responses)}, sort_keys=True))
replacement_root = os.environ.get("KPOPPER_TEST_REPLACEMENT_ROOT")
if replacement_root:
    library = pathlib.Path(replacement_root) / (runtime.implementation["target"] + ".library")
    assert library.is_file(), library
    before = hashlib.sha256(runtime.binary.read_bytes()).hexdigest()
    shutil.copyfile(library, runtime.root / runtime.manifest["libraries"][0])
    modified = make_runtime()
    assert modified.implementation["modified_libraries"] == runtime.manifest["libraries"], modified.implementation
    assert hashlib.sha256(modified.binary.read_bytes()).hexdigest() == before
    changed = modified.request({"nodes": {}, "declared": [], "expression": {"op": "div", "args": [{"num": "1"}, {"num": "3"}]}})
    assert changed == result, changed
    receipt = subprocess.run([str(modified.binary)], input=b"KP2\t1000\t128\t256\t0\t0\tn\t1\n", capture_output=True, check=True)
    assert b"KPOPPER_GMP_REPLACEMENT_PROBE" in receipt.stderr, receipt
    print(json.dumps({"replacement": modified.implementation}, sort_keys=True))
'''


def check_distribution(python, plugin_root=None):
    """Exercise actual installed runtime and normal CLI with empty compiler PATH."""
    # Keep the venv invocation path: resolving its symlink can silently select
    # the host interpreter instead of the package installed in this environment.
    python = Path(python).absolute()
    with tempfile.TemporaryDirectory(prefix="kpopper-installed-") as directory:
        root = Path(directory)
        record = root / "GROUNDING.yaml"
        record.write_text("meta:\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  a: {v: 1}\n  b: {v: 3}\n  café: {v: 2, note: 'שלום café'}\n  ratio: {rule: {expr: 'a / b'}}\ndecisions:\n  d: {rests_on: [ratio], wrong_if: {expr: 'ratio > 1'}, seen: {ratio: 0}}\n", encoding="utf-8")
        empty_path = root / "empty-path"
        empty_path.mkdir()
        # Git is an existing record-reader dependency. Keep that one executable
        # available while excluding compilers; otherwise Advanced context could
        # not be captured and the probe would test an unrelated missing tool.
        git = shutil.which('git')
        if not git:
            raise RuntimeError('installed CLI probe needs the existing Git dependency')
        runtime_path = str(empty_path)
        if os.name == 'nt':
            runtime_path += os.pathsep + str(Path(git).parent)
        else:
            (empty_path / 'git').symlink_to(git)
        env = {k: v for k, v in os.environ.items()
               if k not in ("PYTHONPATH", "LEAN_PATH", "LEAN_SYSROOT", "LEAN_CC", "KPOPPER_LEAN_ROOT", "KPOPPER_RUNTIME_ARCHIVE", "KPOPPER_TEST_ARCHIVE", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH")}
        env.update(PATH=runtime_path, XDG_CACHE_HOME=str(root / "cache"),
                   LOCALAPPDATA=str(root / "local"),
                   KPOPPER_RUNTIME_CACHE=str(root / "runtime-cache"),
                   KPOPPER_TEST_CORPUS=str(ROOT / 'tests/reasoning/t0-scalar-slice.json'))
        probe = root / "probe.py"
        probe.write_text(INSTALLED_PROBE, encoding="utf-8")
        argv = [str(python), str(probe)]
        if plugin_root:
            argv += [str(Path(plugin_root).resolve())]
        print(subprocess.check_output(argv, cwd=root, env=env, text=True, encoding="utf-8"), flush=True)
        # Cold cache again for the public consumer entrypoint.
        for cache in (root / "cache", root / "runtime-cache", root / "local", root / "home"):
            if cache.exists():
                shutil.rmtree(cache)
        cli = ([str(python), str(Path(plugin_root).resolve() / "scripts/cli.py")]
               if plugin_root else [str(python), "-m", "kpopper.cli"])
        command = cli + ["assess", "ratio", "d", "café", "--profile", "core/v1", "--record", str(record)]
        env["PYTHONIOENCODING"] = "ascii"
        raw = subprocess.check_output(command, cwd=root, env=env, text=True, encoding="utf-8")
        result = json.loads(raw)
        assert result["assessment_profile"] == "core/v1", result
        assert result["nodes"]["ratio"]["computation"]["value"] == {"type": "number", "numerator": "1", "denominator": "3"}, result
        assert result["nodes"]["café"]["computation"]["value"] == {"type": "number", "numerator": "2", "denominator": "1"}, result
        assert result["nodes"]["café"]["body"]["note"] == "שלום café", result
        assert "operational_error" not in raw, result
        # Preserve the parsed Unicode checks above; CI's own redirected console
        # may use cp1252 even though the installed CLI correctly emitted UTF-8.
        print(json.dumps(result, ensure_ascii=True, sort_keys=True), flush=True)


def check_candidate_archive(archive):
    """Execute a fresh build's scalar corpus, separately from installed channels."""
    with tempfile.TemporaryDirectory(prefix="kpopper-candidate-") as directory:
        root = Path(directory)
        probe = root / "probe.py"
        probe.write_text(INSTALLED_PROBE, encoding="utf-8")
        env = dict(os.environ, PATH=str(root / "no-compilers"), XDG_CACHE_HOME=str(root / "cache"),
                   KPOPPER_TEST_ARCHIVE=str(Path(archive).absolute()),
                   KPOPPER_TEST_CORPUS=str(ROOT / "tests/reasoning/t0-scalar-slice.json"))
        env.pop("PYTHONPATH", None)
        print(subprocess.check_output([sys.executable, str(probe), str(ROOT)], cwd=root,
                                      env=env, text=True, encoding="utf-8"), flush=True)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--installed-python":
        check_distribution(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else None)
    elif len(sys.argv) > 1 and sys.argv[1] == "--archive":
        check_candidate_archive(sys.argv[2])
    else:
        unittest.main()
