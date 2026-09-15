# Packaged arithmetic runtime

`<target>.zip` is generated package data for the experimental `core/v1`
profile. A normal pip or plugin installation uses it offline: no compiler,
network download, or checked-session setup is needed. The Python wrapper is
portable; native computation requires a matching verified archive.

The current data-only transport is KP2/KR2. KP2 retains the scalar request fields;
KR2 adds `preflight_steps` after evaluation `steps`, before node-evaluation counts.
Both phases independently use the requested step bound. Old KP1/KR1 archives are
refused; source/protocol changes require rebuilt archives for every target.

Maintainers build each archive on its target host with pinned Lean 4.33.1
and the exact GMP 6.3.0 source archive:

```sh
python scripts/reasoning/build_runtime.py --work-dir /tmp/kpopper-build \
  --source-bundle scripts/reasoning/native/gmp-source-and-build.tar.gz
```

`--source-root` names the directory containing `Main.lean`, not the repository.
The command downloads build inputs only when invoked explicitly. It runs GMP's
upstream tests, compiles the production Lean modules and proof modules, links
a separate GMP library, inspects dependencies, and produces the archive and
SHA256 sidecar. `--replacement-library FILE` separately rebuilds GMP with
a visible test constructor; CI checks replacement in the installed cache
while keeping the executable unchanged. This test library never enters
the distributed runtime ZIP. `--lean-root`, `--gmp-source`, and `--gmp-prefix` permit reuse
of private build inputs. `build_archive(source_root, lean_root, output, target)`
uses `KPOPPER_GMP_PREFIX` unless passed `gmp_prefix=`. The prefix must contain
the builder's source/target provenance. No build step runs during installation.
On Windows invoke the builder from MSYS2 with
`KPOPPER_MSYS2_ROOT="$(cygpath -m /)"` so subprocesses use its absolute bash/make
paths. This avoids Windows selecting the WSL bash shim. Compiler-supplied GMP
flags are replaced in both internal and public linker lists, temporarily leaving
static-link mode for the replaceable library and restoring it for other libraries.

Each ZIP contains `manifest.json`, `evaluator` (Windows: `evaluator.exe`),
GMP, linkage evidence, and full notices. GMP lives at
`.dylibs/libgmp.10.dylib` on macOS, `.libs/libgmp.so.10` on Linux, and
`libgmp-10.dll` beside the executable on Windows. Every payload file has a
SHA256 in the manifest. Runtime source identity hashes the sorted JSON map
of relative `.lean` names to source hashes, excluding `Proof*` and `Audit*`
files. ZIP paths, timestamps, ordering and executable modes are normalized;
this is deterministic packing, not a claim of reproducible native code across
different compiler/SDK hosts.

One `gmp-source-and-build.tar.gz` beside the target archives supplies GMP's
exact corresponding source and build instructions for all targets. It must
ship in every wheel, sdist and plugin carrying the binary archives. The
shared library may be replaced with an ABI-compatible modification; the
runtime identifies replacement without disabling it. See the included
`THIRD_PARTY_NOTICES.txt` and LGPLv3/GPLv3 texts.

Candidate matrix: `linux-x86_64`, `linux-aarch64`, `darwin-arm64`,
`darwin-x86_64`, `windows-x86_64`. macOS declares minimum 15.0 because
upstream static runtime objects require it. Linux records the largest
required GLIBC version observed in the executable/library. Windows initially
uses Server 2022 as its tested baseline; older Windows support is not inferred.
An archive is not verified merely because its manifest names a platform.
The candidate CI must pass on that platform before integration/publication.

The separate `reasoning-runtime` workflow builds candidates, audits proof and
linkage behavior, and checks wheel/sdist/plugin execution with cold caches
and no compiler on the effective runtime PATH. Python3.13 runs on all five
native targets; Python3.9 runs on both Linux CPUs, Intel macOS and Windows.
`setup-python` does not supply a Darwin arm64 Python3.9 artifact, so that
interpreter/CPU combination is not claimed as executed. The package's base
Python requirement remains >=3.9. The workflow uploads candidates and does
not publish releases. The release completeness gate must require all five
verified archives and matching runtime source identity before publishing.

Runner label reference: <https://docs.github.com/en/actions/reference/runners/github-hosted-runners>.
Pinned toolchain assets: <https://github.com/leanprover/lean4/releases/tag/v4.33.1>.
GMP source: <https://ftp.gnu.org/gnu/gmp/gmp-6.3.0.tar.xz>.
