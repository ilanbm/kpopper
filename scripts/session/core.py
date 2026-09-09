"""Build explicitly, then run the packaged Lean source from a versioned local cache."""
from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile

from .model import encode, digest

SOURCE = Path(__file__).resolve().parent / "lean" / "Main.lean"
LEAN_VERSION = "4.33.1"
RULES = (SOURCE.parent.parent / "rules.txt").read_text(encoding="utf-8").strip()


def source_hash():
    return hashlib.sha256(SOURCE.read_bytes()).hexdigest()


def cache_directory(source_digest=None):
    override = os.environ.get("KPOPPER_CORE_CACHE")
    base = Path(override).expanduser() if override else Path(
        os.environ.get("XDG_CACHE_HOME", str(Path.home() / ".cache"))) / "kpopper" / "lean"
    target = platform.system().lower() + "-" + platform.machine().lower()
    return (base / target / (source_digest or source_hash())).absolute()


def binary_name():
    return "epistemic-core.exe" if os.name == "nt" else "epistemic-core"


def setup(lean_root=None, rebuild=False):
    """Compile reviewed package code; never download or execute a record's command."""
    captured = SOURCE.read_bytes()
    captured_hash = hashlib.sha256(captured).hexdigest()
    destination = cache_directory(captured_hash)
    if destination.exists() and not rebuild:
        try:
            existing = Core()
        except ValueError as error:
            raise ValueError("managed core cache is invalid; run kpopper session setup --rebuild") from error
        return {**existing.build, "cache": str(existing.root)}
    root = Path(lean_root).expanduser().resolve() if lean_root else None
    if root is None and os.environ.get("KPOPPER_LEAN_ROOT"):
        root = Path(os.environ["KPOPPER_LEAN_ROOT"]).expanduser().resolve()
    suffix = ".exe" if os.name == "nt" else ""
    lean = str(root / "bin" / ("lean" + suffix)) if root else shutil.which("lean")
    if not lean:
        raise ValueError("Lean " + LEAN_VERSION + " is required; pass --lean-root to session setup")
    if root is None:
        # Resolve elan's selected toolchain before changing directory for the build.
        # Keep the proxy's invocation name intact when asking for its prefix.
        prefix = subprocess.check_output([str(Path(lean).absolute()), "--print-prefix"], text=True, encoding="utf-8").strip()
        root = Path(prefix).expanduser().resolve()
        lean = str(root / "bin" / ("lean" + suffix))
    leanc = str(root / "bin" / ("leanc" + suffix))
    lean, leanc = str(Path(lean).absolute()), str(Path(leanc).absolute())
    version = subprocess.check_output([lean, "--version"], text=True, encoding="utf-8").strip()
    if ("version " + LEAN_VERSION + ",") not in version:
        raise ValueError("expected Lean " + LEAN_VERSION + "; found " + version)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".build-", dir=destination.parent) as temporary:
        build_dir = Path(temporary)
        (build_dir / "Main.lean").write_bytes(captured)
        subprocess.run([lean, "-o", "Main.olean", "-c", "Main.c", "Main.lean"], cwd=build_dir, check=True)
        subprocess.run([leanc, "-o", binary_name(), "Main.c"], cwd=build_dir, check=True)
        if source_hash() != captured_hash:
            raise ValueError("Lean source changed during setup; retry against a stable package")
        metadata = {"source_sha256": captured_hash,
                    "binary_sha256": hashlib.sha256((build_dir / binary_name()).read_bytes()).hexdigest(),
                    "lean_version": version, "platform": platform.platform()}
        (build_dir / "build.json").write_text(encode(metadata) + "\n", encoding="utf-8")
        for name in ["Main.lean", "Main.c", "Main.olean"]:
            (build_dir / name).unlink()
        # A directory rename publishes a matching binary/manifest pair at once.
        backup = None
        if rebuild and destination.exists():
            import uuid
            backup = destination.with_name(destination.name + ".backup-" + uuid.uuid4().hex[:12])
            os.rename(destination, backup)
        try:
            os.rename(build_dir, destination)
        except OSError:
            if not destination.exists():
                if backup is not None:
                    os.rename(backup, destination)
                raise
            existing = Core()  # another complete setup won; never interleave files
            return {**existing.build, "cache": str(destination)}
    return {**metadata, "cache": str(destination), "previous_cache": str(backup) if backup else None}


class Core:
    def __init__(self):
        self.root = cache_directory()
        self.binary = self.root / binary_name()
        try:
            self.build = json.loads((self.root / "build.json").read_text(encoding="utf-8"))
            valid = self.build["source_sha256"] == source_hash() and (
                hashlib.sha256(self.binary.read_bytes()).hexdigest() == self.build["binary_sha256"])
        except (OSError, KeyError, ValueError) as error:
            raise ValueError("checked session core is not ready; run kpopper session setup") from error
        if not valid:
            raise ValueError("checked session core changed; run kpopper session setup")
        self.cached = None
        self.program_stamp = self.stamp()

    def stamp(self):
        return tuple((p.stat().st_ino, p.stat().st_size, p.stat().st_mtime_ns)
                     for p in [SOURCE, self.binary, self.root / "build.json"])

    def ensure_program(self):
        if self.stamp() != self.program_stamp:
            raise ValueError("assessment program changed; restart the session service")

    def request(self, payload, rejected_assertions=False):
        self.ensure_program()
        text = json.dumps(payload, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
        # Lean's JSON protocol is UTF-8, independently of the host code page.
        run = subprocess.run([str(self.binary)], input=text, text=True, encoding="utf-8",
                             capture_output=True, timeout=20)
        self.ensure_program()
        if run.returncode not in ([0, 2] if rejected_assertions else [0]):
            raise ValueError(run.stdout.strip() or run.stderr.strip() or "Lean assessment failed")
        result = json.loads(run.stdout)
        result["input_sha256"] = hashlib.sha256(text.encode()).hexdigest()
        result["core_source_sha256"] = self.build["source_sha256"]
        return result

    def scan(self, record):
        self.ensure_program()
        key = digest(record)
        cached = self.cached
        if cached is not None and cached[0] == key:
            return copy.deepcopy(cached[1])
        result = self.request({"operation": "scan", "record": record})
        result["scan_input_sha256"] = result.pop("input_sha256")
        self.cached = (key, result)
        return copy.deepcopy(result)

    def assess(self, record, judgment, assertions=None):
        if assertions is not None:
            return self.request({"record": record, "id": judgment, "assertions": assertions}, True)
        scan = self.scan(record)
        for bundle in scan["assessments"]:
            if bundle["id"] == judgment:
                return {"bundle": bundle, "assertion_checks": [], "assertions_requested": 0,
                        "assertions_accepted": None, "scan_input_sha256": scan["scan_input_sha256"],
                        "core_source_sha256": self.build["source_sha256"]}
        for error in scan["errors"]:
            if error["id"] == judgment:
                raise ValueError(error["error"])
        raise ValueError("not_a_judgment")

    def guard(self, record, view):
        project = record.get("project_context")
        expected = digest({"project": project, "graph": digest(record)})
        if not project or view["project"] != project or view["revision"] != expected:
            raise ValueError("view context does not match this record snapshot")
        if view["orientation"] != record.get("orientation", {}) or view["rules"] != RULES:
            raise ValueError("view orientation or rules changed")
        result = self.request({"operation": "guard_view", "record": record, "view": view})
        if result["accepted"] is not True:
            raise ValueError("invalid projection: " + encode(result))
        return result
