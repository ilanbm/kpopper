"""Run the real composition/history cases against an installed distribution.

The semantic cases live in test_core_composition.py.  This harness deliberately
loads that same test class under the installed ``kpopper`` package namespace,
from a temporary working directory with no checkout or compiler on PATH.  It is
an explicit packaging gate, not another copy of the acceptance expectations.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
TARGET = Path(__file__).with_name("test_core_composition.py")


CHILD = r'''
import importlib
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import sys
import types
import unittest

target = Path(sys.argv[1]).resolve()
checkout_package = Path(sys.argv[2]).resolve()
archive = sys.argv[3] or None
plugin_root = Path(sys.argv[4]).resolve() if sys.argv[4] else None
if archive:
    os.environ["KPOPPER_COMPOSITION_TEST_ARCHIVE"] = archive
if plugin_root:
    package_roots = [(plugin_root / "scripts").resolve()]
    channel = "extracted-plugin"
    if not (package_roots[0] / "__init__.py").is_file():
        raise RuntimeError("extracted plugin package is unavailable")
else:
    installed = importlib.util.find_spec("kpopper")
    if installed is None or not installed.submodule_search_locations:
        raise RuntimeError("installed kpopper package is unavailable")
    package_roots = [Path(path).resolve() for path in installed.submodule_search_locations]
    channel = "installed-python"
if checkout_package in package_roots:
    raise RuntimeError("acceptance resolved the checkout instead of the installed package")

# The reusable acceptance class imports ``scripts`` because that is the checkout
# package name.  Give it an isolated package alias whose search path is the
# installed distribution; no checkout module is imported into this process.
alias = types.ModuleType("scripts")
alias.__path__ = [str(path) for path in package_roots]
alias.__package__ = "scripts"
alias.__spec__ = importlib.machinery.ModuleSpec("scripts", loader=None, is_package=True)
alias.__spec__.submodule_search_locations = alias.__path__
sys.modules["scripts"] = alias
for name in ("history_authoring", "history_contract", "history_store",
             "history_transaction", "provenance", "reasoning",
             "reasoning.authoring", "reasoning.runtime", "reasoning.evaluate",
             "reasoning.snapshot"):
    module = importlib.import_module("scripts." + name)
    if "." not in name:
        setattr(alias, name, module)

spec = importlib.util.spec_from_file_location("composition_acceptance_target", target)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
suite = unittest.defaultTestLoader.loadTestsFromTestCase(module.CoreComposition)
result = unittest.TextTestRunner(verbosity=2).run(suite)
if not result.wasSuccessful():
    raise SystemExit(1)

runtime_module = importlib.import_module("scripts.reasoning.runtime")
native = runtime_module.Runtime(archive) if archive else runtime_module.Runtime()
print(json.dumps({
    "archive_sha256": native.implementation["archive_sha256"],
    "binary_sha256": native.implementation["binary_sha256"],
    "channel": channel,
    "installed_package_roots": [str(path) for path in package_roots],
    "manifest_version": native.manifest["version"],
    "manifest_modules": native.manifest["modules"],
    "manifest_protocols": native.manifest["protocols"],
    "source_sha256": native.implementation["source_sha256"],
    "target": native.implementation["target"],
    "tests_run": result.testsRun,
}, sort_keys=True))
'''


def run_installed(python, archive=None, plugin_root=None):
    """Execute the shared acceptance class using only one installed package."""
    python = Path(python).absolute()
    if not python.is_file():
        raise ValueError("installed Python executable is unavailable: " + str(python))
    if archive is not None:
        archive = Path(archive).resolve()
        if not archive.is_file():
            raise ValueError("candidate runtime archive is unavailable: " + str(archive))
    if plugin_root is not None:
        plugin_root = Path(plugin_root).resolve()
        if not plugin_root.is_dir():
            raise ValueError("extracted plugin root is unavailable: " + str(plugin_root))
    with tempfile.TemporaryDirectory(prefix="composition-installed-") as directory:
        root = Path(directory)
        empty_path = root / "empty-path"
        empty_path.mkdir()
        git = shutil.which("git")
        if not git:
            raise RuntimeError("installed acceptance needs the existing Git dependency")
        runtime_path = str(empty_path)
        if os.name == "nt":
            runtime_path += os.pathsep + str(Path(git).parent)
        else:
            (empty_path / "git").symlink_to(git)
        env = dict(os.environ)
        for name in ("PYTHONPATH", "PYTHONHOME", "LEAN_PATH", "LEAN_SYSROOT", "LEAN_CC",
                     "KPOPPER_LEAN_ROOT", "KPOPPER_RUNTIME_ARCHIVE",
                     "KPOPPER_TEST_ARCHIVE", "KPOPPER_COMPOSITION_TEST_ARCHIVE",
                     "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"):
            env.pop(name, None)
        env.update(PATH=runtime_path, XDG_CACHE_HOME=str(root / "cache"),
                   LOCALAPPDATA=str(root / "local"), PYTHONDONTWRITEBYTECODE="1")
        subprocess.run([str(python), "-B", "-c", CHILD, str(TARGET),
                        str(ROOT / "scripts"), str(archive) if archive else "",
                        str(plugin_root) if plugin_root else ""],
                       cwd=root, env=env, check=True)


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--installed-python", required=True)
    parser.add_argument("--archive")
    parser.add_argument("--plugin-root")
    args = parser.parse_args(argv)
    run_installed(args.installed_python, args.archive, args.plugin_root)


if __name__ == "__main__":
    main()
