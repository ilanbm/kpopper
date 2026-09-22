#!/usr/bin/env python3
"""Collect resolved Rust dependency license texts into a distributable bundle."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


LICENSE_NAME = re.compile(r"^(license|copying|copyright|notice|unlicense|authors?)([._-].*)?$", re.IGNORECASE)
PRIMARY_LICENSE_NAME = re.compile(r"^(license|copying|unlicense)([._-].*)?$", re.IGNORECASE)
LICENSE_TERMS = re.compile(
    r"permission is hereby granted|licensed under the apache license|redistribution and use in source|"
    r"gnu (lesser )?general public license|mozilla public license",
    re.IGNORECASE,
)


def fail(message):
    raise SystemExit("collect_rust_licenses: " + message)


def safe_component(value, label):
    if (not value or value in (".", "..") or
            any(not (character.isascii() and (character.isalnum() or character in "._+-")) for character in value)):
        fail("unsafe %s in Cargo metadata: %r" % (label, value))
    return value


def sha256(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def vcs_revision(package):
    path = Path(package.get("manifest_path") or "").parent / ".cargo_vcs_info.json"
    try:
        return json.loads(path.read_text(encoding="utf-8")).get("git", {}).get("sha1")
    except (OSError, ValueError, AttributeError):
        return None


def load_metadata(args):
    if args.metadata:
        try:
            return json.loads(args.metadata.read_text(encoding="utf-8"))
        except (OSError, ValueError) as error:
            fail("cannot read metadata JSON: %s" % error)
    command = [args.cargo, "metadata", "--format-version", "1", "--locked",
               "--manifest-path", str(args.manifest_path)]
    if args.filter_platform:
        command.extend(["--filter-platform", args.filter_platform])
    try:
        result = subprocess.run(command, check=True, capture_output=True, text=True)
        return json.loads(result.stdout)
    except FileNotFoundError:
        fail("cargo executable was not found")
    except subprocess.CalledProcessError as error:
        fail("cargo metadata failed: %s" % error.stderr.strip())
    except ValueError as error:
        fail("cargo metadata returned invalid JSON: %s" % error)


def package_files(package, all_packages, overrides=None):
    manifest = Path(package.get("manifest_path") or "")
    if not manifest.is_file():
        fail("metadata manifest is unavailable for %s %s" % (package["name"], package["version"]))
    root = manifest.parent
    root_resolved = root.resolve()
    allowed_roots = [root_resolved]
    candidates = []
    declared = package.get("license_file")
    if declared:
        declared_path = Path(declared)
        if not declared_path.is_absolute():
            declared_path = root / declared_path
        try:
            declared_path.resolve().relative_to(root_resolved)
        except ValueError:
            fail("declared license text escapes package root for %s %s" %
                 (package["name"], package["version"]))
        candidates.append(declared_path)
    try:
        candidates.extend(path for path in root.iterdir() if LICENSE_NAME.match(path.name))
    except OSError as error:
        fail("cannot inspect dependency package %s %s: %s" % (package["name"], package["version"], error))
    if not any(PRIMARY_LICENSE_NAME.match(path.name) for path in candidates):
        readme = root / "README.md"
        try:
            readme_text = readme.read_text(encoding="utf-8")
        except (OSError, UnicodeError):
            readme_text = ""
        if (re.search(r"(?im)^#{1,6}\s+license\s*:?\s*$", readme_text) and
                LICENSE_TERMS.search(readme_text)):
            candidates.append(readme)
    if not candidates and package.get("repository") and package.get("license"):
        revision = vcs_revision(package)
        for sibling in all_packages:
            if (sibling.get("id") != package.get("id") and
                    sibling.get("repository") == package.get("repository") and
                    sibling.get("license") == package.get("license") and revision and
                    vcs_revision(sibling) == revision):
                sibling_root = Path(sibling.get("manifest_path") or "").parent
                try:
                    sibling_root = sibling_root.resolve()
                    allowed_roots.append(sibling_root)
                    candidates.extend(path for path in sibling_root.iterdir() if LICENSE_NAME.match(path.name))
                except OSError:
                    continue
    used_override = False
    if not candidates and overrides:
        override = overrides / ("%s-%s" % (package["name"], package["version"]))
        if override.exists():
            if override.is_symlink() or not override.is_dir():
                fail("license override must be a real directory for %s %s" %
                     (package["name"], package["version"]))
            override = override.resolve()
            allowed_roots.append(override)
            candidates.extend(path for path in override.iterdir() if LICENSE_NAME.match(path.name))
            used_override = bool(candidates)
    unique = {}
    for path in candidates:
        try:
            details = path.lstat()
        except OSError:
            fail("declared license text is missing for %s %s" % (package["name"], package["version"]))
        if path.is_symlink() or not path.is_file():
            fail("license text must be a regular file for %s %s" % (package["name"], package["version"]))
        name = path.name
        resolved = path.resolve()
        if not any(resolved.is_relative_to(allowed) for allowed in allowed_roots):
            fail("license text escapes its approved package root for %s %s" %
                 (package["name"], package["version"]))
        safe_component(name, "license filename")
        previous = unique.get(name.lower())
        if previous and previous != resolved:
            if sha256(previous) != sha256(resolved):
                fail("license filename collision for %s %s: %s" % (package["name"], package["version"], name))
            continue
        unique[name.lower()] = resolved
    if not unique:
        fail("no license, copyright, copying, or notice text found for %s %s" %
             (package["name"], package["version"]))
    return sorted(unique.values(), key=lambda path: path.name.lower()), used_override


def collect(metadata, output, overrides=None):
    packages = {package.get("id"): package for package in metadata.get("packages", [])}
    workspace = set(metadata.get("workspace_members", []))
    resolve = metadata.get("resolve") or {}
    nodes = {node.get("id"): node for node in resolve.get("nodes", [])}
    if not nodes:
        fail("Cargo metadata has no resolved dependency graph")
    resolved = set(workspace)
    pending = list(workspace)
    while pending:
        node = nodes.get(pending.pop())
        if not node:
            fail("workspace package is absent from the resolved dependency graph")
        for dependency in node.get("deps", []):
            kinds = dependency.get("dep_kinds") or []
            if kinds and all(kind.get("kind") == "dev" for kind in kinds):
                continue
            package_id = dependency.get("pkg")
            if package_id and package_id not in resolved:
                resolved.add(package_id)
                pending.append(package_id)
    dependencies = []
    for package_id in sorted(resolved - workspace):
        package = packages.get(package_id)
        if not package:
            fail("resolved package is absent from Cargo metadata")
        dependencies.append(package)
    if not dependencies:
        fail("Cargo metadata has no non-workspace dependencies")
    parent = output.parent.resolve()
    parent.mkdir(parents=True, exist_ok=True)
    if output.exists() and (not output.is_dir() or any(output.iterdir())):
        fail("output directory must be absent or empty")
    if output.exists():
        output.rmdir()
    temporary = Path(tempfile.mkdtemp(prefix=".rust-licenses-", dir=parent))
    try:
        index = {"schema_version": 1, "packages": []}
        used_directories = set()
        for package in sorted(dependencies, key=lambda item: (item["name"], item["version"], item["id"])):
            name = safe_component(package.get("name"), "package name")
            version = safe_component(package.get("version"), "package version")
            spdx = package.get("license")
            if not isinstance(spdx, str) or not spdx.strip():
                spdx = "NOASSERTION"
            directory_name = "%s-%s" % (name, version)
            if directory_name.lower() in used_directories:
                fail("dependency output collision for %s" % directory_name)
            used_directories.add(directory_name.lower())
            destination = temporary / directory_name
            destination.mkdir()
            files = []
            sources, used_override = package_files(package, metadata.get("packages", []), overrides=overrides)
            for source in sources:
                target = destination / source.name
                shutil.copyfile(source, target)
                relative = target.relative_to(temporary).as_posix()
                files.append({"path": relative, "sha256": sha256(target)})
            repository = package.get("repository")
            index["packages"].append({"name": name, "version": version, "spdx": spdx,
                                      "repository": repository if isinstance(repository, str) else None,
                                      "text_source": "explicit_override" if used_override else "cargo_package",
                                      "files": files})
        (temporary / "INDEX.json").write_text(json.dumps(index, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        os.replace(temporary, output)
    except BaseException:
        shutil.rmtree(temporary, ignore_errors=True)
        raise
    print(json.dumps({"output": str(output.resolve()), "packages": len(dependencies)}, sort_keys=True))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest-path", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--metadata", type=Path, help="use an existing cargo metadata JSON file")
    parser.add_argument("--overrides", type=Path,
                        help="reviewed fallback texts in CRATE-VERSION directories")
    parser.add_argument("--filter-platform", help="Cargo target triple used to filter resolved dependencies")
    parser.add_argument("--cargo", default=os.environ.get("CARGO", "cargo"))
    args = parser.parse_args(argv)
    collect(load_metadata(args), args.output, overrides=args.overrides)


if __name__ == "__main__":
    main()
