# Packaged core runtime

`<target>.kpopper-runtime` is ZIP-formatted generated package data for the explicit, default-off
experimental `core/v1` profile. An ordinary installation uses it offline: no
compiler, network download, or checked-session setup is needed. Native
computation requires a matching verified archive. The domain-specific suffix keeps hosts that reject ZIP files nested in
plugin downloads from mistaking this runtime payload for another plugin package.

The data-only executable has three closed transports. KP2/KR2 retains the scalar
arithmetic request, response and `resources/v2` accounting byte-for-byte. A
request whose potential closure contains a composed node or stored list/record
uses explicit KP3/KR3 and `resources/v3`; no composition tag is accepted on the
KP2 wire. KP3 adds `and`, `or`, `not`, conditionals, lists, records and literal-key
field access, with fixed maxima of 10,000 recursively expanded value nodes,
depth 128 and 16 MiB of canonical value tokens. KR3 keeps potential and executed
reads distinct and returns recursive typed values. Old KP1/KR1 archives are
refused; source/protocol changes require rebuilt archives for every target.
KP4/KR4 adds canonical length-framed JSON for the finite `query/v1` scope
extension and `resources/v4`; KP2 and KP3 lines remain byte-for-byte unchanged.

Manifest schema version 3 advertises both protocol and module sets exactly:

```json
{
  "version": 3,
  "protocols": ["KP2", "KP3", "KP4"],
  "modules": ["arithmetic/v1", "composition/v1", "query/v1"]
}
```

The remaining manifest fields bind the target, minimum OS, Lean version, runtime
source identity, executable, libraries and every payload digest. The adapter
checks the requested protocol and required modules before encoding, requires the
matching KR2, KR3 or KR4 response for each request, and preserves KP2 as the reported
implementation for scalar closures even when the record declares composition.
A missing capability or mismatched/stale archive is a refusal, never a semantic
fallback.

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
The pinned compiler normally initializes all of Lean whenever any `Lean.*`
module is imported. For the data-only JSON parser, the builder selects Lean's
runtime-only bootstrap in generated `Main.c`, preserving recursive module
initialization and the normal IO/task setup. It checks the exact generated
startup and initializer imports; another Lean API requires an explicit build
review. This keeps compiler and elaborator initialization out of the evaluator.
On Windows invoke the builder from MSYS2 with
`KPOPPER_MSYS2_ROOT="$(cygpath -m /)"` so subprocesses use its absolute bash/make
paths. This avoids Windows selecting the WSL bash shim. Compiler-supplied GMP
flags are replaced in both internal and public linker lists, temporarily leaving
static-link mode for the replaceable library and restoring it for other libraries.
GMP is compiled explicitly as GNU C17, including its pre-C23 configure probes.
On Linux its shared-library symbols resolve against the build host's glibc,
which may be newer than Lean's link-only sysroot. An `ldd -r` audit must resolve
every runtime dependency before packaging; the manifest records the highest
GLIBC symbol version required by the executable and GMP together.

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
ship in every distribution carrying the binary archives. The
shared library may be replaced with an ABI-compatible modification; the
runtime identifies replacement without disabling it. See the included
`THIRD_PARTY_NOTICES.txt` and LGPLv3/GPLv3 texts.

Candidate matrix: `linux-x86_64`, `linux-aarch64`, `darwin-arm64`,
`darwin-x86_64`, `windows-x86_64`. macOS declares minimum 15.0 because
upstream static runtime objects require it. Linux records the largest
required GLIBC version observed in the executable/library; the included Linux
archives require glibc 2.38 or newer. Windows initially
uses Server 2022 as its tested baseline; older Windows support is not inferred.
An archive is not verified merely because its manifest names a platform.
The candidate CI must pass on that platform before integration/publication.

The separate `reasoning-runtime` workflow builds candidates, audits proof and
linkage behavior, and checks execution with cold caches and no compiler on
the effective runtime PATH. The workflow uploads candidates and does
not publish releases. The release completeness gate must require all five
verified archives and matching runtime source identity before publishing.

Adding KP3 sources and manifest support does not itself establish cross-platform
or installed validation. Until all five archives are rebuilt from the current
source identity and the candidate/install jobs pass on their named targets, they
remain candidates and publication is not ready. Do not infer platform support
from a manifest, a local native run, or previously verified KP2 archives.

Runner label reference: <https://docs.github.com/en/actions/reference/runners/github-hosted-runners>.
Pinned toolchain assets: <https://github.com/leanprover/lean4/releases/tag/v4.33.1>.
GMP source: <https://ftp.gnu.org/gnu/gmp/gmp-6.3.0.tar.xz>.

CI validates committed bundle integrity before provisioning the platform matrix.
Each target waits only for its matching build; verified GMP dependencies can be
reused without skipping runtime or installed replacement checks. See
[CI selection and execution](../../../docs/ci.md) for the cache contract, test
suites and the explicit `candidate-only` workflow dispatch used to refresh bundles.


The `reasoning-runtime` workflow validates rebuilt candidates with the native
scalar, composition and query conformance suites. Its replacement-library test
requires the separately modified GMP library to execute and report its identity
without changing the evaluator binary. Candidate-only runs upload validated
candidates; other runs also invoke the native distribution acceptance once across
all supported targets. Python is used for the build tooling, not as the installed
reader or a wheel/sdist validation channel.

Maintainers invoke this workflow explicitly. PR and push checks validate the
committed bundles through native platform acceptance; they do not rebuild all
reasoning candidates automatically. This keeps candidate generation separate from
acceptance of the bytes proposed for release.
