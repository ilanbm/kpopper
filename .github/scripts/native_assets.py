#!/usr/bin/env python3
"""Validate native build artifacts and stage the public release assets."""

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import tarfile
import tempfile
from typing import BinaryIO, Dict, Iterable, List, Tuple
import zipfile


TARGETS = (
    "linux-x86_64",
    "linux-aarch64",
    "darwin-arm64",
    "darwin-x86_64",
    "windows-x86_64",
)
MAX_MEMBERS = 10_000
MAX_UNCOMPRESSED_BYTES = 512 * 1024 * 1024
MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
MAX_ARTIFACT_FILES = 1_000
MAX_MANIFEST_BYTES = 4 * 1024 * 1024
BLOCK_BYTES = 1024 * 1024


class NativeAssetError(ValueError):
    """A native release input is incomplete or cannot be trusted."""


def _fail(message: str) -> None:
    raise NativeAssetError("native release assets: " + message)


def _validate_identity(version: str, commit: str) -> None:
    if (not version or version in (".", "..") or
            any(not (character.isascii() and
                     (character.isalnum() or character in "._-"))
                for character in version)):
        _fail("invalid version: %r" % version)
    if not commit or commit != commit.strip() or any(character in commit for character in "\r\n\0"):
        _fail("invalid source commit")


def _regular(path: Path, label: str) -> None:
    try:
        mode = path.lstat().st_mode
    except OSError as error:
        _fail("cannot inspect %s %s: %s" % (label, path, error))
    if not stat.S_ISREG(mode):
        _fail("%s must be a regular file: %s" % (label, path))


def _digest_stream(stream: BinaryIO) -> str:
    digest = hashlib.sha256()
    for block in iter(lambda: stream.read(BLOCK_BYTES), b""):
        digest.update(block)
    return digest.hexdigest()


def _digest_file(path: Path) -> str:
    with path.open("rb") as stream:
        return _digest_stream(stream)


def _safe_member(name: str, directory: bool = False) -> str:
    if not isinstance(name, str) or not name or "\\" in name or "\0" in name:
        _fail("unsafe archive path: %r" % name)
    canonical = name[:-1] if directory and name.endswith("/") else name
    path = PurePosixPath(canonical)
    if (not canonical or path.is_absolute() or str(path) != canonical or
            any(part in ("", ".", "..") for part in path.parts)):
        _fail("unsafe archive path: %s" % name)
    for part in path.parts:
        if (any(ord(character) < 32 or character in '<>:"|?*' for character in part) or
                part.endswith((" ", "."))):
            _fail("unsafe archive path: %s" % name)
    return canonical


def _mode_string(value: object, path: str) -> int:
    if not isinstance(value, str) or re.fullmatch(r"[0-7]{4}", value) is None:
        _fail("invalid manifest mode for %s" % path)
    return int(value, 8)


def _manifest(data: bytes, version: str, commit: str, target: str) -> Dict[str, Tuple[str, int]]:
    def unique_object(pairs: List[Tuple[str, object]]) -> Dict[str, object]:
        value: Dict[str, object] = {}
        for key, item in pairs:
            if key in value:
                _fail("duplicate manifest key: %s" % key)
            value[key] = item
        return value

    try:
        document = json.loads(data.decode("utf-8"), object_pairs_hook=unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        _fail("invalid manifest for %s: %s" % (target, error))
    if not isinstance(document, dict):
        _fail("manifest for %s must be an object" % target)
    if document.get("version") != version:
        _fail("manifest version does not match %s" % version)
    if document.get("target") != target:
        _fail("manifest target does not match %s" % target)
    if document.get("source_commit") != commit:
        _fail("manifest source commit does not match %s" % commit)
    entries = document.get("files")
    if not isinstance(entries, list):
        _fail("manifest files for %s must be a list" % target)
    result: Dict[str, Tuple[str, int]] = {}
    folded: Dict[str, str] = {}
    for entry in entries:
        if not isinstance(entry, dict):
            _fail("manifest file entry for %s must be an object" % target)
        path = _safe_member(entry.get("path")) if isinstance(entry.get("path"), str) else ""
        if not path:
            _fail("manifest file path for %s is invalid" % target)
        checksum = entry.get("sha256")
        if not isinstance(checksum, str) or re.fullmatch(r"[0-9a-f]{64}", checksum) is None:
            _fail("invalid manifest SHA256 for %s" % path)
        mode = _mode_string(entry.get("mode"), path)
        if path in result:
            _fail("duplicate manifest path: %s" % path)
        previous = folded.get(path.casefold())
        if previous is not None:
            _fail("case-colliding manifest paths: %s and %s" % (previous, path))
        folded[path.casefold()] = path
        result[path] = (checksum, mode)
    return result


def _check_path_set(kinds: Dict[str, str]) -> None:
    folded: Dict[str, str] = {}
    for path in kinds:
        previous = folded.get(path.casefold())
        if previous is not None:
            _fail("case-colliding archive paths: %s and %s" % (previous, path))
        folded[path.casefold()] = path
    files = {path for path, kind in kinds.items() if kind == "file"}
    for path in kinds:
        parent = PurePosixPath(path).parent
        while str(parent) != ".":
            if str(parent) in files:
                _fail("archive file is also a path parent: %s" % parent)
            parent = parent.parent


def _check_payload(payload: Dict[str, Tuple[str, int]], manifest: Dict[str, Tuple[str, int]],
                   target: str) -> None:
    if set(payload) != set(manifest):
        missing = sorted(set(manifest) - set(payload))
        unlisted = sorted(set(payload) - set(manifest))
        _fail("manifest payload mismatch for %s (missing=%r, unlisted=%r)" %
              (target, missing, unlisted))
    for path, actual in payload.items():
        if actual != manifest[path]:
            if actual[0] != manifest[path][0]:
                _fail("payload SHA256 does not match manifest: %s" % path)
            _fail("payload mode does not match manifest: %s" % path)
    executable = ".exe" if target == "windows-x86_64" else ""
    required = {
        "LICENSE",
        "bin/resources/reasoning/native/gmp-source-and-build.tar.gz",
        "bin/resources/reasoning/notices/lean/THIRD_PARTY_NOTICES.txt",
        "bin/resources/reasoning/notices/rust/INDEX.json",
        "bin/resources/reasoning/notices/rust-toolchain/COPYRIGHT-library.html",
        "bin/resources/reasoning/notices/rust-toolchain/licenses/Apache-2.0.txt",
        "bin/resources/reasoning/notices/rust-toolchain/licenses/MIT.txt",
        "bin/kpop" + executable,
        "bin/kpopper" + executable,
        "bin/resources/reasoning/%s.kpopper-runtime" % target,
        "bin/resources/ordinary/%s/build.json" % target,
        "bin/resources/ordinary/%s/epistemic-core%s" % (target, executable),
    }
    absent = sorted(required - set(payload))
    if absent:
        _fail("required payload files are missing for %s: %s" % (target, ", ".join(absent)))
    if target != "windows-x86_64":
        for path in ("bin/kpop", "bin/kpopper",
                     "bin/resources/ordinary/%s/epistemic-core" % target):
            if not payload[path][1] & 0o111:
                _fail("Unix executable does not have executable mode: %s" % path)


def _validate_tar(path: Path, version: str, commit: str, target: str) -> None:
    top = "kpopper-%s-%s" % (version, target)
    kinds: Dict[str, str] = {}
    payload: Dict[str, Tuple[str, int]] = {}
    manifest_data = None
    total = 0
    try:
        with tarfile.open(path, "r|gz") as archive:
            member_count = 0
            for member in archive:
                member_count += 1
                if member_count > MAX_MEMBERS:
                    _fail("archive has too many members: %s" % path.name)
                if not (member.isdir() or member.isreg()):
                    _fail("archive contains a link or special file: %s" % member.name)
                name = _safe_member(member.name, member.isdir())
                if name != top and not name.startswith(top + "/"):
                    _fail("unexpected archive member: %s" % member.name)
                if name in kinds:
                    _fail("duplicate archive path: %s" % name)
                kinds[name] = "directory" if member.isdir() else "file"
                if member.isdir():
                    continue
                if member.size < 0:
                    _fail("invalid archive member size: %s" % name)
                total += member.size
                if total > MAX_UNCOMPRESSED_BYTES:
                    _fail("archive payload is too large: %s" % path.name)
                stream = archive.extractfile(member)
                if stream is None:
                    _fail("cannot read archive member: %s" % name)
                relative = name[len(top) + 1:]
                if relative == "manifest.json":
                    if member.size > MAX_MANIFEST_BYTES:
                        _fail("archive manifest is too large: %s" % path.name)
                    manifest_data = stream.read(MAX_MANIFEST_BYTES + 1)
                else:
                    payload[relative] = (_digest_stream(stream), member.mode & 0o7777)
    except NativeAssetError:
        raise
    except (OSError, tarfile.TarError) as error:
        _fail("cannot read %s: %s" % (path.name, error))
    _check_path_set(kinds)
    if manifest_data is None:
        _fail("archive manifest is missing: %s" % path.name)
    _check_payload(payload, _manifest(manifest_data, version, commit, target), target)


def _zip_mode(entry: zipfile.ZipInfo, directory: bool) -> int:
    encoded = (entry.external_attr >> 16) & 0xffff
    kind = stat.S_IFMT(encoded)
    allowed = (0, stat.S_IFDIR) if directory else (0, stat.S_IFREG)
    if kind not in allowed:
        _fail("archive contains a link or special file: %s" % entry.filename)
    return encoded & 0o7777


def _validate_zip(path: Path, version: str, commit: str, target: str) -> None:
    top = "kpopper-%s-%s" % (version, target)
    kinds: Dict[str, str] = {}
    payload: Dict[str, Tuple[str, int]] = {}
    manifest_data = None
    total = 0
    try:
        with zipfile.ZipFile(path) as archive:
            entries = archive.infolist()
            if len(entries) > MAX_MEMBERS:
                _fail("archive has too many members: %s" % path.name)
            for entry in entries:
                directory = entry.is_dir()
                name = _safe_member(entry.filename, directory)
                if name != top and not name.startswith(top + "/"):
                    _fail("unexpected archive member: %s" % entry.filename)
                if name in kinds:
                    _fail("duplicate archive path: %s" % name)
                kinds[name] = "directory" if directory else "file"
                mode = _zip_mode(entry, directory)
                if directory:
                    continue
                total += entry.file_size
                if entry.file_size < 0 or total > MAX_UNCOMPRESSED_BYTES:
                    _fail("archive payload is too large: %s" % path.name)
                relative = name[len(top) + 1:]
                with archive.open(entry) as stream:
                    if relative == "manifest.json":
                        if entry.file_size > MAX_MANIFEST_BYTES:
                            _fail("archive manifest is too large: %s" % path.name)
                        manifest_data = stream.read(MAX_MANIFEST_BYTES + 1)
                    else:
                        payload[relative] = (_digest_stream(stream), mode)
    except NativeAssetError:
        raise
    except (OSError, zipfile.BadZipFile, RuntimeError) as error:
        _fail("cannot read %s: %s" % (path.name, error))
    _check_path_set(kinds)
    if manifest_data is None:
        _fail("archive manifest is missing: %s" % path.name)
    _check_payload(payload, _manifest(manifest_data, version, commit, target), target)


def _files_under(root: Path) -> Iterable[Path]:
    def unreadable(error: OSError) -> None:
        raise error

    try:
        for current, directories, files in os.walk(
                root, followlinks=False, onerror=unreadable):
            directories[:] = sorted(directories)
            for name in sorted(files):
                yield Path(current) / name
    except OSError as error:
        _fail("cannot scan artifacts %s: %s" % (root, error))


def _sidecar(path: Path, archive: Path) -> str:
    _regular(path, "checksum sidecar")
    if path.stat().st_size > 1024:
        _fail("checksum sidecar is too large: %s" % path)
    try:
        text = path.read_text(encoding="ascii")
    except (OSError, UnicodeDecodeError) as error:
        _fail("cannot read checksum sidecar %s: %s" % (path, error))
    match = re.fullmatch(r"([0-9A-Fa-f]{64})[ \t]+\*?([^\r\n]+)\r?\n?", text)
    if match is None or match.group(2) != archive.name:
        _fail("invalid checksum sidecar: %s" % path)
    expected = match.group(1).lower()
    actual = _digest_file(archive)
    if actual != expected:
        _fail("archive SHA256 does not match sidecar: %s" % archive.name)
    return actual


def _validate_archives(version: str, commit: str,
                       artifacts: Path) -> List[Tuple[Path, str]]:
    # Kept separate from staging so every input is proven before output is touched.
    if not artifacts.is_dir() or artifacts.is_symlink():
        _fail("artifacts must be a real directory: %s" % artifacts)
    all_files = list(_files_under(artifacts))
    if len(all_files) > MAX_ARTIFACT_FILES:
        _fail("artifact input has too many files")
    selected: List[Tuple[Path, str]] = []
    known = "|".join(re.escape(target) for target in TARGETS)
    native_name = re.compile(r"^kpopper-.+-(?:%s)\.(?:tar\.gz|zip)(?:\.sha256)?$" % known)
    expected_names = {
        "kpopper-%s-%s%s" %
        (version, target, ".zip" if target == "windows-x86_64" else ".tar.gz")
        for target in TARGETS
    }
    for path in all_files:
        if native_name.fullmatch(path.name) and path.name.removesuffix(".sha256") not in expected_names:
            _fail("mis-versioned or mis-targeted native artifact: %s" % path.name)
    for target in TARGETS:
        suffix = ".zip" if target == "windows-x86_64" else ".tar.gz"
        name = "kpopper-%s-%s%s" % (version, target, suffix)
        matches = [path for path in all_files if path.name == name]
        if len(matches) != 1:
            _fail("expected exactly one %s archive; found %d" % (target, len(matches)))
        archive = matches[0]
        _regular(archive, "native archive")
        if archive.stat().st_size > MAX_ARCHIVE_BYTES:
            _fail("native archive is too large: %s" % archive.name)
        sidecar = archive.with_name(archive.name + ".sha256")
        sidecars = [path for path in all_files if path.name == sidecar.name]
        if len(sidecars) != 1 or sidecars[0] != sidecar:
            _fail("expected exactly one adjacent checksum sidecar for %s" % archive.name)
        checksum = _sidecar(sidecar, archive)
        (_validate_zip if target == "windows-x86_64" else _validate_tar)(
            archive, version, commit, target)
        selected.append((archive, checksum))
    return selected


def _empty_output(output: Path) -> bool:
    if not output.exists():
        return False
    if output.is_symlink() or not output.is_dir():
        _fail("output must be absent or an empty real directory: %s" % output)
    try:
        if next(output.iterdir(), None) is not None:
            _fail("output directory is not empty: %s" % output)
    except OSError as error:
        _fail("cannot inspect output directory %s: %s" % (output, error))
    return True


def prepare(version: str, commit: str, artifacts: Path, output: Path,
            repository: Path) -> list[Path]:
    """Validate all five native archives, then stage immutable release inputs."""
    _validate_identity(version, commit)
    artifacts = Path(artifacts)
    output = Path(output)
    repository = Path(repository)
    archives = _validate_archives(version, commit, artifacts)
    installers = [repository / "install.sh", repository / "install.ps1"]
    for installer in installers:
        _regular(installer, "installer")

    existed = _empty_output(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=".%s-stage-" % output.name, dir=output.parent))
    try:
        staged = []
        for source, checksum in archives:
            destination = stage / source.name
            shutil.copyfile(source, destination)
            shutil.copymode(source, destination)
            if _digest_file(destination) != checksum:
                _fail("native archive changed after validation: %s" % source.name)
            staged.append(destination)
        for source in installers:
            destination = stage / source.name
            shutil.copyfile(source, destination)
            shutil.copymode(source, destination)
            staged.append(destination)
        sums = stage / "SHA256SUMS"
        sums.write_text("".join("%s  %s\n" % (_digest_file(path), path.name)
                                for path in staged), encoding="ascii")
        staged.append(sums)

        if not existed:
            try:
                output.mkdir()
            except FileExistsError:
                _fail("output appeared while assets were staged: %s" % output)
        else:
            _empty_output(output)
        delivered: List[Path] = []
        try:
            for source in staged:
                destination = output / source.name
                os.link(source, destination)
                delivered.append(destination)
        except OSError as error:
            for path in reversed(delivered):
                try:
                    path.unlink()
                except OSError:
                    pass
            if not existed:
                try:
                    output.rmdir()
                except OSError:
                    pass
            _fail("cannot stage release assets: %s" % error)
        return delivered
    finally:
        shutil.rmtree(stage, ignore_errors=True)
