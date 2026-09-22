#!/usr/bin/env python3
"""Build a deterministic, self-describing native kpopper release archive."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import tarfile
import tempfile
import zipfile


TARGETS = {
    "linux-x86_64": False,
    "linux-aarch64": False,
    "darwin-arm64": False,
    "darwin-x86_64": False,
    "windows-x86_64": True,
}


def fail(message):
    raise SystemExit("package_native: " + message)


def safe_component(value, label):
    forbidden = '/\\<>:"|?*\r\n\0'
    if (not value or value in (".", "..") or value[-1:] in (" ", ".")
            or any(ord(c) < 32 or c in forbidden for c in value)):
        fail("invalid %s: %r" % (label, value))
    return value


def regular_file(path, label):
    try:
        mode = path.lstat().st_mode
    except OSError as error:
        fail("cannot read %s %s: %s" % (label, path, error))
    if not stat.S_ISREG(mode):
        fail("%s must be a regular file: %s" % (label, path))


def resource_entries(root, windows=False):
    if root.is_symlink() or not root.is_dir():
        fail("resources must be a real directory")
    try:
        children = sorted(root.iterdir(), key=lambda p: p.name)
    except OSError as error:
        fail("cannot read resources %s: %s" % (root, error))
    if [p.name for p in children] != ["ordinary", "reasoning"] or not all(p.is_dir() and not p.is_symlink() for p in children):
        fail("resources must contain exactly the ordinary and reasoning directories")
    directories = [PurePosixPath("bin/resources"), PurePosixPath("bin/resources/ordinary"),
                   PurePosixPath("bin/resources/reasoning")]
    files = []
    for base in children:
        for current, dirnames, filenames in os.walk(base, followlinks=False):
            current_path = Path(current)
            dirnames.sort()
            filenames.sort()
            for name in list(dirnames):
                item = current_path / name
                if item.is_symlink() or not item.is_dir():
                    fail("resources may contain only regular files and directories: %s" % item)
                safe_component(name, "resource path component")
                directories.append(PurePosixPath("bin/resources") / item.relative_to(root).as_posix())
            for name in filenames:
                safe_component(name, "resource path component")
                item = current_path / name
                regular_file(item, "resource")
                executable = bool(item.stat().st_mode & 0o111) or (windows and item.suffix.lower() == ".exe")
                files.append((PurePosixPath("bin/resources") / item.relative_to(root).as_posix(),
                              item, 0o755 if executable else 0o644))
    return sorted(set(directories), key=str), sorted(files, key=lambda row: str(row[0]))


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def tar_info(name, mode, directory=False):
    info = tarfile.TarInfo(name + ("/" if directory else ""))
    info.type = tarfile.DIRTYPE if directory else tarfile.REGTYPE
    info.mode = mode
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    info.mtime = 0
    return info


def write_tar(path, top, directories, files, manifest_bytes):
    with path.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            archive.addfile(tar_info(top, 0o755, True))
            for relative in directories:
                archive.addfile(tar_info(top + "/" + str(relative), 0o755, True))
            for relative, source, mode in files:
                info = tar_info(top + "/" + str(relative), mode)
                info.size = source.stat().st_size
                with source.open("rb") as stream:
                    archive.addfile(info, stream)
            info = tar_info(top + "/manifest.json", 0o644)
            info.size = len(manifest_bytes)
            import io
            archive.addfile(info, io.BytesIO(manifest_bytes))


def write_zip(path, top, directories, files, manifest_bytes):
    def info(name, mode, directory=False):
        value = zipfile.ZipInfo(name + ("/" if directory else ""), (1980, 1, 1, 0, 0, 0))
        value.create_system = 3
        value.compress_type = zipfile.ZIP_DEFLATED
        value.external_attr = ((stat.S_IFDIR if directory else stat.S_IFREG) | mode) << 16
        return value
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        archive.writestr(info(top, 0o755, True), b"")
        for relative in directories:
            archive.writestr(info(top + "/" + str(relative), 0o755, True), b"")
        for relative, source, mode in files:
            archive.writestr(info(top + "/" + str(relative), mode), source.read_bytes())
        archive.writestr(info(top + "/manifest.json", 0o644), manifest_bytes)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--alias", required=True, type=Path)
    parser.add_argument("--resources", required=True, type=Path)
    parser.add_argument("--target", required=True, choices=sorted(TARGETS))
    parser.add_argument("--version", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(argv)
    version = args.version
    if not version or any(not (c.isascii() and (c.isalnum() or c in "._-")) for c in version) or version in (".", ".."):
        fail("invalid version: %r" % version)
    commit = args.commit.strip()
    if not commit or any(c in commit for c in "\r\n\0"):
        fail("invalid source commit")
    windows = TARGETS[args.target]
    expected = ("kpop.exe", "kpopper.exe") if windows else ("kpop", "kpopper")
    if args.binary.name != expected[0] or args.alias.name != expected[1]:
        fail("binary names for %s must be %s and %s" % (args.target, *expected))
    regular_file(args.binary, "binary")
    regular_file(args.alias, "alias")
    resource_dirs, resource_files = resource_entries(args.resources, windows=windows)
    files = [(PurePosixPath("bin") / expected[0], args.binary, 0o755),
             (PurePosixPath("bin") / expected[1], args.alias, 0o755), *resource_files]
    files.sort(key=lambda row: str(row[0]))
    manifest = {
        "version": version,
        "target": args.target,
        "source_commit": commit,
        "files": [
            {"path": str(relative), "sha256": digest(source), "mode": "%04o" % mode}
            for relative, source, mode in files
        ],
    }
    manifest_bytes = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")
    top = "kpopper-%s-%s" % (version, args.target)
    suffix = ".zip" if windows else ".tar.gz"
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / (top + suffix)
    with tempfile.TemporaryDirectory(dir=args.output) as temporary:
        candidate = Path(temporary) / archive.name
        directories = [PurePosixPath("bin"), *resource_dirs]
        (write_zip if windows else write_tar)(candidate, top, directories, files, manifest_bytes)
        os.replace(candidate, archive)
    checksum = digest(archive)
    sidecar = archive.with_name(archive.name + ".sha256")
    sidecar.write_text("%s  %s\n" % (checksum, archive.name), encoding="ascii")
    print(json.dumps({"archive": str(archive.resolve()), "sha256": checksum,
                      "checksum_file": str(sidecar.resolve()), "manifest": manifest}, sort_keys=True))


if __name__ == "__main__":
    main()
