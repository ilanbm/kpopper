#!/usr/bin/env python3
"""Maintainer-only native archive builder; never imported by install or evaluation."""
import argparse
import hashlib
import gzip
import io
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

LEAN_VERSION = "4.33.1"
GMP_SHA256 = "a3c2b80201b89e68616f4ad30bc66aee4927c3ce50e33929ca819d5c43538898"
TARGETS = {
    "darwin-arm64": ("darwin_aarch64", "557e9976f853138716ac82b645aab9d75e2bf9ea893113b5a438e4eeb78ed47d"),
    "darwin-x86_64": ("darwin", "fe0b865ec22fdbbcad583417be2a67349683b82395d3d415fa8776f228740fc7"),
    "linux-x86_64": ("linux", "0376ac87487246b40dd077268c097e701f552e94c6d020d2373b50c7444fa22f"),
    "linux-aarch64": ("linux_aarch64", "02968637d34adfe887c8e2a2b1a6c895c16bc162727d060079fd15f189b734b0"),
    "windows-x86_64": ("windows", "c39360867edfff6b090f20c16e18581c969ce839b71e813d76022ec04ec73e4d"),
}


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def source_hash(lean_dir):
    lean_dir = Path(lean_dir)
    files = {p.relative_to(lean_dir).as_posix(): sha256(p)
             for p in lean_dir.glob("*.lean")
             if not p.name.startswith(("Proof", "Audit"))}
    # lakefile.lean is build configuration, if present; current package uses TOML.
    return hashlib.sha256(json.dumps(files, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def run(argv, cwd=None, env=None):
    print("+ " + json.dumps([str(x) for x in argv]), flush=True)
    return subprocess.check_output([str(x) for x in argv], cwd=cwd, env=env,
                                   stderr=subprocess.STDOUT, text=True, encoding="utf-8")


def host_target():
    system = platform.system().lower()
    arch = {"amd64": "x86_64", "arm64": "aarch64"}.get(platform.machine().lower(), platform.machine().lower())
    if system == "darwin" and arch == "aarch64":
        arch = "arm64"
    return system + "-" + arch


def download(url, output, expected):
    output = Path(output)
    if not output.exists():
        with urllib.request.urlopen(url) as response, output.open("wb") as stream:
            shutil.copyfileobj(response, stream)
    if sha256(output) != expected:
        raise ValueError("source archive hash mismatch: " + output.name)


def prepare_toolchain(target, directory):
    """Fetch an upstream release only for maintainer builds; verify pinned SHA256."""
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    suffix, digest = TARGETS[target]
    name = "lean-" + LEAN_VERSION + "-" + suffix
    archive = directory / (name + ".zip")
    download("https://github.com/leanprover/lean4/releases/download/v" + LEAN_VERSION + "/" + archive.name,
             archive, digest)
    if not (directory / name).exists():
        with zipfile.ZipFile(archive) as zf:
            for member in zf.infolist():
                path = Path(member.filename)
                if path.is_absolute() or ".." in path.parts:
                    raise ValueError("unsafe toolchain archive")
                mode = member.external_attr >> 16
                dest = directory / member.filename
                if stat.S_ISLNK(mode):
                    link = zf.read(member).decode("utf-8")
                    resolved = (dest.parent / link).resolve()
                    if Path(link).is_absolute() or directory not in resolved.parents:
                        raise ValueError("unsafe toolchain symlink")
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    dest.symlink_to(link)
                else:
                    zf.extract(member, directory)
                    if mode and not member.is_dir():
                        dest.chmod(mode & 0o777)
    return directory / name


GMP_REPLACEMENT_PATCH = r"""
/* Optional distribution replacement test; never part of production GMP. */
#include <stdio.h>
__attribute__((constructor)) static void kpopper_gmp_replacement_probe(void) {
    fputs("KPOPPER_GMP_REPLACEMENT_PROBE\n", stderr);
}
"""


def validate_axiom_audit(output):
    """Compiling a proof is insufficient: Lean also accepts admitted axioms."""
    required = {'Kpopper.evaluate', 'Kpopper.arithmetic', 'Kpopper.Proof.binary_sound',
                'Kpopper.Proof.evaluate_closedRat_sound', 'Kpopper.Proof.evaluate_literal_success'}
    allowed = {'propext', 'Classical.choice', 'Quot.sound'}
    found = set()
    for name, names in re.findall(r"'([^']+)'\s+depends on axioms:\s*\[([^\]]*)\]", output):
        axioms = {item.strip() for item in names.split(',') if item.strip()}
        if axioms - allowed:
            raise ValueError('unapproved proof axiom: ' + ', '.join(sorted(axioms - allowed)))
        found.add(name)
    if required - found:
        raise ValueError('proof audit omitted required declarations: ' + ', '.join(sorted(required - found)))
    return sorted(found)


def dynamic_gmp_flags(flags, library):
    """Replace both internal and public GMP flags from the pinned toolchain.

    Linux/Windows internal flags put GMP under -Bstatic. Select our exact shared
    library there while restoring that mode for Lean's other bundled libraries.
    Link-map and symbol audits below still reject accidentally embedded GMP.
    """
    if flags.count('-lgmp') not in (1, 2):
        raise ValueError('unexpected compiler GMP link configuration: ' + repr(flags))
    result, static = [], False
    for flag in flags:
        if flag == '-lgmp':
            result.extend(['-Wl,-Bdynamic', str(library), '-Wl,-Bstatic'] if static else [str(library)])
        else:
            result.append(flag)
            if flag in ('-Wl,-Bstatic', '-Wl,-static'):
                static = True
            elif flag in ('-Wl,-Bdynamic', '-Wl,-call_shared'):
                static = False
    return result


def gmp_tools(target, env):
    if not target.startswith('windows'):
        return 'bash', 'make'
    # Windows CreateProcess searches system directories before PATH and can pick
    # the WSL bash shim. The invoking MSYS2 shell supplies its actual root.
    root = env.get('KPOPPER_MSYS2_ROOT')
    if not root:
        raise ValueError('Windows GMP builds require KPOPPER_MSYS2_ROOT from cygpath -m /')
    root = Path(root).resolve()
    bash, make = root / 'usr/bin/bash.exe', root / 'usr/bin/make.exe'
    if not bash.is_file() or not make.is_file():
        raise ValueError('MSYS2 bash and make are unavailable at the configured root')
    env['PATH'] = os.pathsep.join([str(root / 'mingw64/bin'), str(root / 'usr/bin'), env.get('PATH', '')])
    env['CONFIG_SHELL'] = bash.as_posix()
    env['SHELL'] = bash.as_posix()
    return bash, make


def build_gmp(archive, directory, target, *, replacement_probe=False):
    """Build exact upstream GMP privately; run its upstream test suite."""
    archive, directory = Path(archive).resolve(), Path(directory).resolve()
    if sha256(archive) != GMP_SHA256:
        raise ValueError("GMP source hash mismatch")
    directory.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as tf:
        # The complete archive is pinned. Reject unsafe members even after verification.
        for member in tf.getmembers():
            if Path(member.name).is_absolute() or ".." in Path(member.name).parts:
                raise ValueError("unsafe GMP source archive")
        tf.extractall(directory)
    source = directory / "gmp-6.3.0"
    if replacement_probe:
        marker = source / "version.c"
        marker.write_bytes(marker.read_bytes() + GMP_REPLACEMENT_PATCH.encode())
    build = directory / "build"
    build.mkdir(exist_ok=True)
    prefix = directory / "install"
    env = dict(os.environ)
    bash, make = gmp_tools(target, env)
    # GMP 6.3.0's compiler probes use pre-C23 empty parameter lists. GCC 15+
    # defaults to C23, which changes those declarations to zero-argument types.
    flags = "-O2 -std=gnu17"
    if target.startswith("darwin"):
        flags += " -mmacosx-version-min=15.0"
        env["MACOSX_DEPLOYMENT_TARGET"] = "15.0"
    # Portable generic C avoids build-host-specific GMP assembler/CPU tuning.
    args = [bash, (source / "configure").as_posix(), "--prefix=" + prefix.as_posix(),
            "--enable-shared", "--disable-static", "--disable-cxx", "--disable-assembly", "CFLAGS=" + flags]
    if target.startswith("windows"):
        args += ["--host=x86_64-w64-mingw32", "CC=gcc", "LDFLAGS=-static-libgcc"]
    for argv in (args, [make, "-j2"], [make, "-j2", "check"], [make, "install"]):
        print(run(argv, cwd=build, env=env), flush=True)
    (prefix / "kpopper-gmp-provenance.json").write_text(json.dumps({
        "source_sha256": GMP_SHA256, "target": target, "configure": [str(x) for x in args],
        "patches": ["replacement-constructor.patch"] if replacement_probe else [], "tests": "make check passed"},
        sort_keys=True) + "\n", encoding="utf-8")
    return prefix


def archive_payload(bundle, output, manifest):
    bundle, output = Path(bundle), Path(output)
    files = sorted(p for p in bundle.rglob("*") if p.is_file())
    if any(p.is_symlink() for p in bundle.rglob("*")):
        raise ValueError("payload cannot contain symlinks")
    if (bundle / "manifest.json").exists():
        raise ValueError("manifest is generated, not supplied")
    manifest = dict(manifest, files={p.relative_to(bundle).as_posix(): sha256(p) for p in files})
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
        for name, data, mode in [(p.relative_to(bundle).as_posix(), p.read_bytes(),
                                  0o755 if p.relative_to(bundle).as_posix() == manifest["executable"] else 0o644)
                                 for p in files] + [("manifest.json", (json.dumps(manifest, sort_keys=True, indent=2) + "\n").encode(), 0o644)]:
            entry = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            entry.create_system = 3
            entry.external_attr = (stat.S_IFREG | mode) << 16
            entry.compress_type = zipfile.ZIP_DEFLATED
            zf.writestr(entry, data, compresslevel=9)
    output.with_suffix(".zip.sha256").write_bytes((sha256(output) + "  " + output.name + "\n").encode())
    return manifest


def build_archive(source_root, lean_root, output, target, *, gmp_prefix=None):
    """Compile production sources, audit dynamic linkage, write a deterministic ZIP."""
    source_root, lean_root = Path(source_root).resolve(), Path(lean_root).resolve()
    if target not in TARGETS or target != host_target():
        raise ValueError("build target must match supported build host")
    if gmp_prefix is None:
        gmp_prefix = os.environ.get("KPOPPER_GMP_PREFIX")
    if not gmp_prefix:
        raise ValueError("a privately built GMP prefix is required")
    gmp_prefix = Path(gmp_prefix).resolve()
    provenance = json.loads((gmp_prefix / "kpopper-gmp-provenance.json").read_text(encoding="utf-8"))
    if (provenance.get("source_sha256") != GMP_SHA256 or provenance.get("target") != target
            or provenance.get("patches", []) or provenance.get("tests") != "make check passed"):
        raise ValueError("GMP build provenance mismatch")
    ext = ".exe" if target.startswith("windows") else ""
    lean, leanc = lean_root / ("bin/lean" + ext), lean_root / ("bin/leanc" + ext)
    if not re.search(r"\b4\.33\.1\b", run([lean, "--version"])):
        raise ValueError("wrong Lean compiler version")
    env = {k: v for k, v in os.environ.items() if k not in ("LEAN_CC", "LEAN_SYSROOT", "LEAN_PATH")}
    if target.startswith("darwin"):
        env["MACOSX_DEPLOYMENT_TARGET"] = "15.0"
    with tempfile.TemporaryDirectory(prefix="kpopper-native-") as temp:
        work = Path(temp)
        bundle = work / "bundle"
        bundle.mkdir()
        snapshot = work / "source"
        snapshot.mkdir()
        for path in source_root.glob("*.lean"):
            shutil.copyfile(path, snapshot / path.name)
        source_digest = source_hash(snapshot)
        sources = {p.stem: p for p in snapshot.glob("*.lean")}
        env["LEAN_PATH"] = str(work)
        objects = []
        compiled = set()
        def compile_module(name, runtime=True):
            if name in compiled:
                return
            path = sources[name]
            for imported in re.findall(r"^import\s+(\w+)", path.read_text(encoding="utf-8"), re.M):
                if imported in sources:
                    if runtime and imported.startswith(("Proof", "Audit")):
                        raise ValueError("runtime imports proof-only module")
                    compile_module(imported, runtime)
            shutil.copyfile(path, work / path.name)
            argv = [lean, "-o", work / (name + ".olean")]
            if runtime:
                argv += ["-c", work / (name + ".c")]
            compiled_output = run(argv + [work / path.name], cwd=work, env=env)
            print(compiled_output, flush=True)
            if name == 'Audit':
                validate_axiom_audit(compiled_output)
            if runtime:
                obj = work / (name + ".o")
                print(run([leanc, "-O3", "-c", "-o", obj, work / (name + ".c")], env=env), flush=True)
                objects.append(obj)
            compiled.add(name)
        compile_module("Main")
        for name in sorted(sources):
            if name.startswith(("Proof", "Audit")):
                compile_module(name, runtime=False)
        if 'Audit' not in compiled:
            raise ValueError('required proof audit module is missing')
        helper = work / "Flags.lean"
        helper.write_text('import Lean.Compiler.FFI\nopen Lean.Compiler.FFI\ndef main : IO Unit := do\n'
                          '  let root := System.FilePath.mk "' + lean_root.as_posix() + '"\n'
                          '  for arg in getCFlags root ++ getInternalCFlags root ++ getInternalLinkerFlags root ++ getLinkerFlags root true do\n'
                          '    IO.println arg\n', encoding="utf-8")
        flags = run([lean, "--run", helper], env=env).splitlines()
        if target.startswith("darwin"):
            library = ".dylibs/libgmp.10.dylib"
            supplied = gmp_prefix / "lib/libgmp.10.dylib"
        elif target.startswith("linux"):
            library = ".libs/libgmp.so.10"
            supplied = gmp_prefix / "lib/libgmp.so.10"
        else:
            library = "libgmp-10.dll"
            supplied = gmp_prefix / "bin/libgmp-10.dll"
        dest = bundle / library
        dest.parent.mkdir(exist_ok=True)
        shutil.copyfile(supplied, dest)
        if target.startswith("darwin"):
            run(["install_name_tool", "-id", "@loader_path/" + library, dest])
            run(["codesign", "--force", "--sign", "-", dest])
        linked = gmp_prefix / "lib/libgmp.dll.a" if target.startswith("windows") else dest
        flags = dynamic_gmp_flags(flags, linked)
        if target.startswith("linux"):
            # Lean's link-only glibc is older than the build host used for GMP.
            # Resolve the shared library against the host loader below, and bind
            # its actual GLIBC requirements in min_os. Never skip that audit.
            flags += ["-Wl,-rpath,$ORIGIN/.libs", "-Wl,--allow-shlib-undefined"]
        if target.startswith("windows"):
            flags += ["-Wl,--whole-archive", "-lleanmanifest", "-Wl,--no-whole-archive"]
        executable = bundle / ("evaluator" + ext)
        mapfile = work / "link.map"
        mapflag = "-Wl,-map," if target.startswith("darwin") else "-Wl,-Map,"
        print(run([lean_root / ("bin/clang" + ext), "-O3", "-o", executable] + objects + flags + [mapflag + str(mapfile)], env=env), flush=True)
        if "libgmp.a(" in mapfile.read_text(encoding="utf-8", errors="replace"):
            raise ValueError("static GMP entered linker map")
        audit = audit_linkage(executable, dest, target, lean_root)
        run(["strip", executable])
        if target.startswith("darwin"):
            run(["codesign", "--force", "--sign", "-", executable])
        notices = Path(__file__).parent / "third_party"
        shutil.copytree(notices, bundle / "licenses")
        shutil.copyfile(notices / "THIRD_PARTY_NOTICES.txt", bundle / "THIRD_PARTY_NOTICES.txt")
        (bundle / "linkage.json").write_bytes((json.dumps(audit, indent=2, sort_keys=True) + "\n").encode())
        if source_hash(source_root) != source_digest:
            raise ValueError("runtime sources changed during build")
        return archive_payload(bundle, output, {"version": 1, "protocol": "KP2", "target": target,
            "min_os": audit["min_os"], "lean_version": LEAN_VERSION,
            "source_sha256": source_digest, "executable": executable.name,
            "libraries": [library], "modules": ["arithmetic/v1"]})


def audit_linkage(executable, library, target, lean_root):
    """Fail closed on external dependencies and confirm GMP is dynamically imported."""
    result = {"target": target}
    if target.startswith("darwin"):
        text = run(["otool", "-L", executable]) + run(["otool", "-L", library])
        deps = re.findall(r"^\s+(\S+) \(", text, re.M)
        if any(not x.startswith(("/usr/lib/", "/System/Library/", "@loader_path/.dylibs/libgmp.10.dylib")) for x in deps):
            raise ValueError("unbundled Darwin dependency: " + text)
        load_commands = run(["otool", "-l", executable]) + run(["otool", "-l", library])
        minimums = re.findall(r"minos (\d+\.\d+)", load_commands)
        if len(minimums) != 2 or any(tuple(map(int, x.split("."))) > (15, 0) for x in minimums):
            raise ValueError("unexpected macOS minimum version: " + repr(minimums))
        result["min_os"] = "15.0"
        symbols = run(["nm", "-u", executable])
        if "___gmp" not in symbols:
            raise ValueError("GMP dynamic import missing")
        defined = run(["nm", "-U", executable])
        if re.search(r"\b___gmp", defined):
            raise ValueError("defined GMP symbols remain")
    elif target.startswith("linux"):
        text = run(["readelf", "-d", executable]) + run(["readelf", "-d", library])
        deps = re.findall(r"Shared library: \[([^]]+)\]", text)
        allowed = {"libgmp.so.10", "libc.so.6", "libm.so.6", "libpthread.so.0", "libdl.so.2", "librt.so.1", "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1"}
        if "libgmp.so.10" not in deps or set(deps) - allowed:
            raise ValueError("unexpected Linux dependencies: " + repr(deps))
        relocations = run(["ldd", "-r", executable]) + run(["ldd", "-r", library])
        if re.search(r"undefined symbol|not found", relocations, re.I):
            raise ValueError("unresolved Linux runtime dependency: " + relocations)
        result["runtime_relocations_verified"] = True
        symbols = run(["nm", "-D", executable])
        if not re.search(r"\bU __gmp", symbols) or re.search(r"\b[TDB] __gmp", symbols):
            raise ValueError("GMP must remain dynamically imported")
        versions = run(["readelf", "--version-info", executable]) + run(["readelf", "--version-info", library])
        glibc = re.findall(r"GLIBC_(\d+)\.(\d+)", versions)
        result["min_os"] = "glibc " + ".".join(str(x) for x in max(tuple(map(int, x)) for x in glibc))
    else:
        objdump = lean_root / "bin/llvm-objdump.exe"
        tool = str(objdump) if objdump.exists() else "objdump"
        text = run([tool, "-p", executable]) + run([tool, "-p", library])
        deps = re.findall(r"DLL Name: (\S+)", text, re.I)
        # Windows supplies ICU as a system component. The compiler links it
        # through an import library and ships no copy of its own, so it is an
        # operating system dependency like the entries beside it.
        allowed = {"libgmp-10.dll", "kernel32.dll", "msvcrt.dll", "ucrtbase.dll", "advapi32.dll", "user32.dll", "userenv.dll", "ws2_32.dll", "shell32.dll", "ole32.dll", "iphlpapi.dll", "psapi.dll", "ntdll.dll", "bcrypt.dll", "dbghelp.dll", "secur32.dll", "icu.dll"}
        if "libgmp-10.dll" not in [x.lower() for x in deps] or any(x.lower() not in allowed and not x.lower().startswith("api-ms-win-") for x in deps):
            raise ValueError("unexpected Windows dependencies: " + repr(deps))
        result["min_os"] = "Windows Server 2022 (CI-tested baseline)"
    result["dependencies"] = deps
    result["inspection"] = text.replace(str(executable), Path(executable).name).replace(str(library), Path(library).name)
    return result


def _source_bundle_files(gmp_bytes):
    """The complete corresponding-source contents for this maintained recipe."""
    if hashlib.sha256(gmp_bytes).hexdigest() != GMP_SHA256:
        raise ValueError("GMP source hash mismatch")
    here = Path(__file__).resolve().parent
    readme = """# GMP 6.3.0 corresponding source and build instructions

The unmodified upstream tarball gmp-6.3.0.tar.xz is included verbatim.
SHA256: """ + GMP_SHA256 + """
Source: https://ftp.gnu.org/gnu/gmp/gmp-6.3.0.tar.xz
GMP is distributed under LGPL-3.0-or-later. Full LGPLv3 and GPLv3 texts
and component notices accompany this archive and each runtime archive.

Prerequisites (maintainers or recipients modifying GMP): Python >=3.9,
a C compiler, bash, GNU make, m4, and the platform SDK. On Windows use
MSYS2 MINGW64 with mingw-w64-x86_64-gcc, make, m4 and diffutils. No
system installation is performed; make install writes to the private prefix.

From this extracted directory run:

    python -c 'from build_runtime import build_gmp, host_target; build_gmp("gmp-6.3.0.tar.xz", "gmp-build", host_target())'

On Windows, first export the MSYS2 root from that MINGW64 shell:

    export KPOPPER_MSYS2_ROOT="$(cygpath -m /)"

Then run the same Python command above. This selects MSYS2 bash/make explicitly.

The unchanged source is configured with --enable-shared --disable-static
--disable-cxx --disable-assembly and CFLAGS='-O2 -std=gnu17'; macOS additionally uses
-mmacosx-version-min=15.0, Windows --host=x86_64-w64-mingw32 CC=gcc LDFLAGS=-static-libgcc.
The script contains the exact commands, runs make check, and saves build
provenance in gmp-build/install/kpopper-gmp-provenance.json. No GMP source
patches are used. The included workflow records the producer platform setup.

Use the resulting ABI-compatible library to replace the library in your
extracted runtime cache: .dylibs/libgmp.10.dylib on macOS,
.libs/libgmp.so.10 on Linux, libgmp-10.dll on Windows. On macOS run
install_name_tool -id @loader_path/.dylibs/libgmp.10.dylib FILE
and codesign --force --sign - FILE. These load-name/signature adjustments
are the only post-build modifications. No distributor signing key or
application relink is needed. Modification/reverse engineering to debug
GMP modifications is permitted. The runtime reports replacement identity.

Optional replacement test: build_gmp(..., replacement_probe=True) appends
replacement-constructor.txt to version.c before building. This separately
identified test change prints KPOPPER_GMP_REPLACEMENT_PROBE to stderr.
It is never applied to production GMP archives.
"""
    files = {"gmp-6.3.0.tar.xz": gmp_bytes,
             "build_runtime.py": Path(__file__).read_bytes(),
             "SOURCE-BUILD.md": readme.encode(),
             "replacement-constructor.txt": GMP_REPLACEMENT_PATCH.encode(),
             "SOURCE.json": (json.dumps({"version": "6.3.0", "sha256": GMP_SHA256,
                 "url": "https://ftp.gnu.org/gnu/gmp/gmp-6.3.0.tar.xz", "patches": [],
                 "targets": sorted(TARGETS)}, sort_keys=True, indent=2) + "\n").encode()}
    workflow = here.parents[1] / ".github/workflows/reasoning-runtime.yml"
    files["reasoning-runtime.yml"] = workflow.read_bytes()
    for name in ("COPYING.LESSERv3", "COPYINGv3", "THIRD_PARTY_NOTICES.txt",
                 "GCC-COPYING.RUNTIME", "GCC-runtime-NOTICES.txt", "MinGW-w64-runtime-LICENSE.txt"):
        files[name] = (here / "third_party" / name).read_bytes()
    return files


def source_bundle(gmp_archive, output):
    """Ship exact corresponding GMP source with all maintained platform recipes."""
    output = Path(output)
    files = _source_bundle_files(Path(gmp_archive).read_bytes())
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w") as tf:
            for name, data in sorted(files.items()):
                entry = tarfile.TarInfo(name)
                entry.size, entry.mtime, entry.mode = len(data), 0, 0o644
                tf.addfile(entry, io.BytesIO(data))
    return sha256(output)


def _safe_member(name):
    if not isinstance(name, str) or not name or "\\" in name or ":" in name:
        raise ValueError("unsafe archive path")
    path = PurePosixPath(name)
    if path.is_absolute() or str(path) != name or any(x in (".", "..") for x in path.parts):
        raise ValueError("unsafe archive path: " + name)
    return name


def _json_object(data):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate JSON key: " + key)
            result[key] = value
        return result
    return json.loads(data, object_pairs_hook=unique)


def check_bundles(native_dir=None, source_root=None):
    """Read-only completeness/integrity gate for the files that will be shipped.

    Does not build, download, extract or execute any runtime or source archive.
    Execution evidence belongs to the separately named candidate/install jobs.
    """
    here = Path(__file__).resolve().parent
    native = Path(native_dir) if native_dir is not None else here / "native"
    source = Path(source_root) if source_root is not None else here / "lean"
    if not list(source.glob("*.lean")):
        raise ValueError("runtime source identity is unavailable")
    expected_source = source_hash(source)
    expected_names = {target + ".zip" for target in TARGETS}
    actual_names = {path.name for path in native.glob("*.zip")}
    if actual_names != expected_names:
        raise ValueError("runtime archive inventory mismatch: missing=" + repr(sorted(expected_names - actual_names))
                         + "; unexpected=" + repr(sorted(actual_names - expected_names)))
    receipts = {}
    notice_files = {"licenses/" + p.name: p.read_bytes()
                    for p in (here / "third_party").iterdir() if p.is_file()}
    notice_files["THIRD_PARTY_NOTICES.txt"] = (here / "third_party/THIRD_PARTY_NOTICES.txt").read_bytes()
    for target in sorted(TARGETS):
        archive = native / (target + ".zip")
        if archive.is_symlink() or not archive.is_file():
            raise ValueError("runtime archive must be a regular file: " + archive.name)
        try:
            with zipfile.ZipFile(archive) as zf:
                entries = zf.infolist()
                names = [entry.filename for entry in entries]
                if len(entries) > 256 or len(names) != len(set(names)) or "manifest.json" not in names:
                    raise ValueError("malformed runtime archive inventory: " + target)
                if sum(entry.file_size for entry in entries) > 512 * 1024 * 1024:
                    raise ValueError("runtime archive payload limit")
                for entry in entries:
                    _safe_member(entry.filename)
                    mode = entry.external_attr >> 16
                    if entry.is_dir() or stat.S_ISLNK(mode) or (stat.S_IFMT(mode) not in (0, stat.S_IFREG)) \
                            or entry.file_size > 128 * 1024 * 1024:
                        raise ValueError("unsupported runtime archive member")
                if zf.getinfo("manifest.json").file_size > 1024 * 1024:
                    raise ValueError("runtime manifest size limit")
                manifest = _json_object(zf.read("manifest.json"))
                required = {"version", "protocol", "target", "min_os", "lean_version", "source_sha256",
                            "files", "executable", "libraries", "modules"}
                if not isinstance(manifest, dict) or set(manifest) != required \
                        or type(manifest["version"]) is not int or manifest["version"] != 1 \
                        or manifest["protocol"] != "KP2" or manifest["target"] != target \
                        or manifest["lean_version"] != LEAN_VERSION or manifest["modules"] != ["arithmetic/v1"] \
                        or not isinstance(manifest["min_os"], str) or not manifest["min_os"].strip() \
                        or not isinstance(manifest["files"], dict):
                    raise ValueError("malformed runtime manifest: " + target)
                if manifest["source_sha256"] != expected_source:
                    raise ValueError("stale runtime source identity: " + target)
                files = manifest["files"]
                exe, libs = manifest["executable"], manifest["libraries"]
                if not isinstance(exe, str) or exe not in files or not isinstance(libs, list) \
                        or not libs or any(not isinstance(x, str) or x not in files or x == exe for x in libs) \
                        or len(libs) != len(set(libs)) or set(names) != set(files) | {"manifest.json"}:
                    raise ValueError("malformed runtime executable/library inventory")
                if zf.getinfo(exe).external_attr >> 16 & 0o111 != 0o111:
                    raise ValueError("runtime executable mode is missing")
                for name, digest in files.items():
                    _safe_member(name)
                    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
                        raise ValueError("malformed runtime payload digest")
                    data = zf.read(name)
                    if hashlib.sha256(data).hexdigest() != digest:
                        raise ValueError("runtime payload hash mismatch: " + target + "/" + name)
                    if name in notice_files and data != notice_files[name]:
                        raise ValueError("runtime notices mismatch: " + name)
                if not set(notice_files) <= set(files):
                    raise ValueError("runtime notices are incomplete")
            digest = sha256(archive)
            sidecar = archive.with_suffix(".zip.sha256")
            if sidecar.exists() and sidecar.read_text(encoding="ascii").strip() != digest + "  " + archive.name:
                raise ValueError("runtime archive sidecar hash mismatch")
            receipts[target] = digest
        except (OSError, zipfile.BadZipFile, KeyError, TypeError, UnicodeError, json.JSONDecodeError) as error:
            raise ValueError("invalid runtime archive " + target + ": " + str(error)) from error
    source_archive = native / "gmp-source-and-build.tar.gz"
    if source_archive.is_symlink() or not source_archive.is_file():
        raise ValueError("missing regular GMP corresponding-source archive")
    try:
        contents = {}
        total = 0
        with tarfile.open(source_archive, "r:gz") as tf:
            for entry in tf:
                _safe_member(entry.name)
                total += entry.size
                if not entry.isfile() or entry.name in contents or len(contents) >= 32 \
                        or entry.size > 16 * 1024 * 1024 or total > 32 * 1024 * 1024:
                    raise ValueError("malformed GMP corresponding-source inventory")
                contents[entry.name] = tf.extractfile(entry).read()
        expected = _source_bundle_files(contents.get("gmp-6.3.0.tar.xz", b""))
        if contents != expected:
            raise ValueError("GMP corresponding-source recipe or inventory mismatch")
    except (OSError, tarfile.TarError, UnicodeError) as error:
        raise ValueError("invalid GMP corresponding-source archive: " + str(error)) from error
    return {"runtime_source_sha256": expected_source, "archives": receipts,
            "corresponding_source_sha256": sha256(source_archive)}


def main():
    # Build diagnostics quote Lean proof statements, which are UTF-8. A
    # redirected stream on Windows would otherwise encode them as cp1252.
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, default=Path(__file__).parent / "lean")
    parser.add_argument("--lean-root", type=Path)
    parser.add_argument("--target", choices=sorted(TARGETS), default=host_target())
    parser.add_argument("--output", type=Path)
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--check-bundles", action="store_true")
    parser.add_argument("--bundle-dir", type=Path)
    parser.add_argument("--gmp-source", type=Path)
    parser.add_argument("--gmp-prefix", type=Path)
    parser.add_argument("--source-bundle", type=Path)
    parser.add_argument("--replacement-library", type=Path)
    args = parser.parse_args()
    if args.check_bundles:
        print(json.dumps(check_bundles(args.bundle_dir, args.source_root), sort_keys=True, indent=2))
        return
    if args.work_dir is None:
        parser.error("--work-dir is required for a maintainer build")
    args.work_dir.mkdir(parents=True, exist_ok=True)
    lean = args.lean_root or prepare_toolchain(args.target, args.work_dir / "toolchain")
    if args.gmp_prefix:
        gmp = args.gmp_prefix
    else:
        archive = args.gmp_source or args.work_dir / "gmp-6.3.0.tar.xz"
        download("https://ftp.gnu.org/gnu/gmp/gmp-6.3.0.tar.xz", archive, GMP_SHA256)
        gmp = build_gmp(archive, args.work_dir / "gmp", args.target)
    output = args.output or Path(__file__).parent / "native" / (args.target + ".zip")
    print(json.dumps(build_archive(args.source_root, lean, output, args.target, gmp_prefix=gmp), indent=2))
    if args.replacement_library:
        archive = args.gmp_source or args.work_dir / "gmp-6.3.0.tar.xz"
        download("https://ftp.gnu.org/gnu/gmp/gmp-6.3.0.tar.xz", archive, GMP_SHA256)
        replacement = build_gmp(archive, args.work_dir / "gmp-replacement", args.target, replacement_probe=True)
        library = ("lib/libgmp.10.dylib" if args.target.startswith("darwin") else
                   "lib/libgmp.so.10" if args.target.startswith("linux") else "bin/libgmp-10.dll")
        args.replacement_library.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(replacement / library, args.replacement_library)
        if args.target.startswith("darwin"):
            run(["install_name_tool", "-id", "@loader_path/.dylibs/libgmp.10.dylib", args.replacement_library])
            run(["codesign", "--force", "--sign", "-", args.replacement_library])
    if args.source_bundle:
        archive = args.gmp_source or args.work_dir / "gmp-6.3.0.tar.xz"
        download("https://ftp.gnu.org/gnu/gmp/gmp-6.3.0.tar.xz", archive, GMP_SHA256)
        print("corresponding-source sha256 " + source_bundle(archive, args.source_bundle))


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        print(error.output, flush=True)
        raise
