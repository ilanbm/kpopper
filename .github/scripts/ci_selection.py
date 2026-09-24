"""Choose CI lanes from the files each lane reads; unknown paths or unavailable history run all.

Every lane declares the tracked files its checks read, and the directories whose entries they
list. A pull request runs the lanes whose inputs it changes. The declarations are not trusted
on their own: on Linux, `ci_audit.py` records every file an audited lane opens and fails the
pull request that makes it read something undeclared, which is also the pull request whose
changes the lane already runs for. Pull requests default to Linux; the release commit runs
every platform once. Ordinary main pushes retain the record and contract checks alone.
"""
import argparse
import fnmatch
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess


class Lane:
    """What a lane's checks read: file contents, and directories whose entries they list.

    Patterns are fnmatch patterns over repository paths, and `*` also matches `/`. `ignores`
    carves out of `reads` what the lane never opens. `python_entry_points` are scripts the lane
    runs from the checkout (globs allowed): they, and every module they import, count as read.
    Files that `include_str!` or `include_bytes!` in the Rust sources under `rust_sources`
    embed count as read. The audit of a lane that is not `enforced` reports undeclared reads as
    warnings instead of failing.
    """

    def __init__(self, reads, lists=(), ignores=(), audited=True, enforced=True, python_entry_points=(),
                 rust_sources=()):
        self.reads, self.lists, self.ignores = tuple(reads), tuple(lists), tuple(ignores)
        self.audited, self.enforced = audited, enforced
        self.python_entry_points, self.rust_sources = tuple(python_entry_points), tuple(rust_sources)


LANES = {
    # The native command: compilation, its tests, the release build and installed acceptance.
    "rust": Lane((
        "native/*", ".github/workflows/native-rust.yml",
        "tests/test_document_ui.cjs", "tests/document_ui_fixture.py",
        "tests/document_native_bridge.py", "tests/document-support/*",
        # The plugin's shell plumbing, the host wrappers and the packaging tools its tests run:
        # installed acceptance stages a plugin from these and drives its hooks.
        "scripts/*.sh", "adapters/codex/plugin-hooks.json",
        "adapters/cursor/scripts/gate-open.sh", "adapters/gemini/scripts/session-start.sh",
        "adapters/gemini/hooks/hooks.json", "adapters/copilot/cli/hook.sh",
        "adapters/windsurf/hooks.json",
        "scripts/package_native.py", "scripts/collect_rust_licenses.py", "scripts/native-licenses/*",
        "tests/test_native_distribution.py", "install.sh", "install.ps1", "VERSION", "LICENSE",
        # Read at test time, by the packaging steps, and by the bundle check, which compares the
        # committed corresponding-source archive with the recipe and workflows recorded inside it.
        "scripts/session/lean/*", "scripts/reasoning/lean/*", "scripts/reasoning/native/*",
        "scripts/reasoning/third_party/*", "scripts/reasoning/build_runtime.py",
        ".github/workflows/reasoning-runtime.yml", ".github/workflows/reasoning-target.yml",
    ), lists=("native/*", "scripts/reasoning/lean*"), ignores=("native/README.md",), enforced=False,
        # The ordinary Lean program its tests load is compiled from the source beside the crate.
        # The hooks its tests compare with run from the pinned v0.10.0 reference.
        rust_sources=("native",)),
}
LANE_NAMES = tuple(LANES)

# Tracked files no lane reads. The record job, and the skill, release and CI contract tests
# it runs on every pull request, still check them. A lane's declaration wins over these.
UNREAD = (
    "*.md", "assets/*", "docs/*", "skills/*", "examples/*", "GROUNDING.yaml", ".kpopper/*",
    ".github/ISSUE_TEMPLATE/*", ".github/pull_request_template.md", ".githooks/*",
    ".github/workflows/release.yml", ".github/workflows/publish.yml",
    "adapters/*", "hooks/*", "scripts/bin/*",
    ".claude-plugin/*", ".codex-plugin/*", "package.json",
    ".gitignore", "*/.gitignore", ".gitattributes",
)

# Run by the record job alone, on every pull request; no lane reads them.
RECORD_JOB = (
    "tests/__init__.py", "tests/test_skills.py", "tests/test_release.py",
    "tests/test_ci_selection.py", "tests/test_ci_release_flow.py", "tests/test_ci_audit.py", "tests/test_native_release_assets.py",
    "tests/test_native_launcher.py", "tests/test_hook_delivery.py", ".github/requirements-test.txt",
    ".github/scripts/release.py", ".github/scripts/publish_release.py",
    ".github/scripts/publish_crate.py", ".github/scripts/native_assets.py",
)

# Exercise CI selection and auditing on Linux. Their routing logic is platform-independent;
# changes to native execution are classified separately below.
CI_MACHINERY = (
    ".github/workflows/check.yml", ".github/scripts/ci_selection.py", ".github/scripts/ci_audit.py",
)

# Shared toolchains and packaging affect every target. Named platform inputs add their
# affected targets to Linux. A version-only manifest edit is not a toolchain change.
PLATFORM_INPUTS = (
    "native/Cargo.toml", "native/Cargo.lock", "native/build.rs", "native/rust-toolchain.toml",
    "native/ci/*", ".github/workflows/native-rust.yml", "install.sh", "install.ps1",
    "scripts/package_native.py", "scripts/collect_rust_licenses.py", "scripts/native-licenses/*",
    "scripts/install_native.sh", "scripts/native_runtime.sh",
    "scripts/reasoning/native/*", "scripts/reasoning/build_runtime.py",
    "scripts/reasoning/third_party/*", ".github/workflows/reasoning-runtime.yml",
    ".github/workflows/reasoning-target.yml", ".claude-plugin/*", ".codex-plugin/*",
)
PLATFORM_SCOPES = ("linux-x86_64", "linux-windows", "linux-macos", "all")

_release_spec = importlib.util.spec_from_file_location("release", Path(__file__).with_name("release.py"))
_release = importlib.util.module_from_spec(_release_spec)
_release_spec.loader.exec_module(_release)

JOB_LANES = {"native-cli": ("rust",)}


def matches(path, patterns):
    return any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns)


def lane_reads(lane, path, root=None):
    if matches(path, lane.ignores + RECORD_JOB):
        return False
    return (matches(path, lane.reads) or path in python_closure(lane.python_entry_points, root)
            or path in rust_embeds(lane.rust_sources, root))


def repository_root(root=None):
    return Path(root) if root else Path(__file__).resolve().parents[2]


def rust_embeds(directories, root=None):
    """Tracked files the Rust sources under the directories embed with include_str!/include_bytes!."""
    root = repository_root(root)
    key = ("rust", tuple(directories), str(root))
    if key not in CLOSURES:
        found = set()
        for directory in directories:
            for source in sorted((root / directory).rglob("*.rs")):
                try:
                    text = source.read_text(encoding="utf-8")
                except (OSError, UnicodeDecodeError):
                    continue
                for literal in EMBED.findall(text):
                    target = Path(os.path.normpath(str(source.parent / literal)))
                    if target.is_file() and root in target.parents:
                        found.add(target.relative_to(root).as_posix())
        CLOSURES[key] = frozenset(found)
    return CLOSURES[key]


def python_closure(entry_points, root=None):
    """The entry scripts and every module of the checkout they import, as tracked paths."""
    root = repository_root(root)
    key = ("python", tuple(entry_points), str(root))
    if key not in CLOSURES:
        CLOSURES[key] = frozenset(imported_modules(entry_points, root))
    return CLOSURES[key]


CLOSURES = {}
EMBED = re.compile(r'include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)')


def imported_modules(entry_points, root):
    """Follow import statements, including those in code a test passes to another interpreter."""
    seen, pending = set(), [path for pattern in entry_points for path in sorted(root.glob(pattern))]
    while pending:
        path = pending.pop()
        if path in seen or not path.is_file():
            continue
        seen.add(path)
        # Importing a submodule runs every package __init__ above it.
        for parent in path.parents:
            if parent == root or root not in parent.parents:
                break
            if (parent / "__init__.py").is_file() and parent / "__init__.py" not in seen:
                pending.append(parent / "__init__.py")
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        statements = [(dots, module, names) for dots, module, names in FROM_IMPORT.findall(text)]
        statements += [("", module.split(" as ")[0].strip(), "") for line in PLAIN_IMPORT.findall(text)
                       for module in line.split(",")]
        for dots, module, names in statements:
            members = [n.split(" as ")[0].strip() for n in names.strip("()").replace("\n", " ").split(",")]
            if module.split(".")[0] == "kpopper":
                module = "scripts" + module[len("kpopper"):]
            if dots:
                base = path.parent
                for _ in range(len(dots) - 1):
                    base = base.parent
                bases = [base]
            else:
                # A sibling of a script (or of a test on the test path) is importable by its
                # bare name, and a top-level package from the checkout root; the package is
                # `kpopper` once installed and `scripts` in the checkout.
                bases = [path.parent, root]
            parts = [p for p in module.split(".") if p]
            for base in bases:
                target = base.joinpath(*parts) if parts else base
                candidates = [target / "__init__.py"] + ([target.with_suffix(".py")] if parts else [])
                candidates += [(target / m).with_suffix(".py") for m in members if m.isidentifier()]
                candidates += [target / m / "__init__.py" for m in members if m.isidentifier()]
                for candidate in candidates:
                    if candidate.is_file() and root in candidate.parents:
                        pending.append(candidate)
    return {path.relative_to(root).as_posix() for path in seen}


FROM_IMPORT = re.compile(r"^[ \t]*from[ \t]+(\.*)([\w.]*)[ \t]+import[ \t]+(\([^)]*\)|[^\n#;]+)", re.M)
PLAIN_IMPORT = re.compile(r"^[ \t]*import[ \t]+([\w.]+(?:[ \t]+as[ \t]+\w+)?(?:[ \t]*,[ \t]*[\w.]+(?:[ \t]+as[ \t]+\w+)?)*)", re.M)

def lane_lists(lane, directory):
    return matches(directory, lane.lists)


def parents(path):
    """Every directory above a path, deepest first, ending with the root ''."""
    result = []
    while "/" in path:
        path = path.rsplit("/", 1)[0]
        result.append(path)
    return result + [""]


def listings(status, path, base_dirs=None, head_dirs=None):
    """The directories whose entries change when a file is added or removed.

    An added file changes its parent's entries, and every ancestor up to the first that
    already existed; a removal likewise up to the first that still exists. Without the trees,
    assume the parent existed on both sides.
    """
    if status not in ("A", "D"):
        return []
    existing = base_dirs if status == "A" else head_dirs
    chain = parents(path)
    if existing is None:
        return chain[:1]
    changed = []
    for directory in chain:
        changed.append(directory)
        if directory in existing:
            break
    return changed


def lanes_for(status, path, base_dirs=None, head_dirs=None):
    if matches(path, CI_MACHINERY):
        return set(LANE_NAMES)
    readers = {name for name, lane in LANES.items() if lane_reads(lane, path)}
    if not readers and not matches(path, UNREAD + RECORD_JOB):
        # Nothing claims this file: run everything rather than guess.
        return set(LANE_NAMES)
    for directory in listings(status, path, base_dirs, head_dirs):
        readers |= {name for name, lane in LANES.items() if lane_lists(lane, directory)}
    return readers


def normalized(changes):
    return [change if isinstance(change, tuple) else ("M", change) for change in changes]


def select(changes, full=False, push=False, base_dirs=None, head_dirs=None, release=False):
    changes = normalized(changes)
    if full or release or not changes:
        chosen = set(LANE_NAMES)
    elif push:
        chosen = set()
    else:
        chosen = set()
        for status, path in changes:
            chosen |= lanes_for(status, path, base_dirs, head_dirs)
    return {lane: lane in chosen for lane in LANE_NAMES}


def source_at(revision, path, cwd=None):
    return subprocess.check_output(["git", "show", f"{revision}:{path}"], cwd=cwd,
                                   stderr=subprocess.PIPE).decode("utf-8")


def version_only(path, base, head, cwd=None):
    if not base or path not in _release.VERSION_FILES:
        return False
    try:
        before, after = (source_at(ref, path, cwd) for ref in (base, head))
        pattern, _ = _release.VERSION_FILES[path]
        return bool(pattern.search(before) and pattern.search(after)
                    and _release.with_version({path: before}, "0.0.0")
                    == _release.with_version({path: after}, "0.0.0"))
    except (OSError, subprocess.CalledProcessError, UnicodeError):
        return False


def native_execution_contract(text):
    """The native workflow's execution inputs, apart from routing and artifact upload.

    Matrix branches may select existing runners differently without changing execution on
    any runner. Compare their complete row set instead. Unrecognized shapes fail open.
    """
    import yaml

    try:
        workflow = yaml.load(text, Loader=yaml.BaseLoader)
    except yaml.YAMLError as error:
        raise ValueError("unreadable workflow") from error
    if not isinstance(workflow, dict) or not isinstance(workflow.get("jobs"), dict):
        raise ValueError("unrecognized workflow")
    inputs = workflow.get("on", {}).get("workflow_call", {}).get("inputs", {})
    contract = {
        "env": workflow.get("env"), "defaults": workflow.get("defaults"),
        "inputs": {key: value for key, value in inputs.items()
                   if key not in ("target", "validation", "publish", "audit")},
        "jobs": {},
    }
    for name, job in workflow["jobs"].items():
        runner = job.get("runs-on", "")
        if isinstance(runner, str) and runner.startswith("ubuntu-") and "strategy" not in job:
            continue  # Linux-only control jobs are exercised by the Linux run.
        execution = {key: value for key, value in job.items() if key not in (
            "name", "needs", "if", "timeout-minutes", "concurrency", "permissions", "outputs")}
        if "strategy" in execution:
            matrix = execution["strategy"]["matrix"]
            if set(matrix) != {"include"} or not isinstance(matrix["include"], str):
                raise ValueError("unrecognized platform matrix")
            arrays = re.findall(r"'(\[\{\"runner\".*?\])'", matrix["include"])
            if not arrays:
                raise ValueError("platform matrix has no declared runners")
            rows = {json.dumps(row, sort_keys=True) for array in arrays for row in json.loads(array)}
            execution["strategy"] = sorted(rows)
        execution["steps"] = [
            {key: value for key, value in step.items() if key != "name"}
            for step in job.get("steps", [])
            if not step.get("uses", "").startswith("actions/upload-artifact@")
        ]
        contract["jobs"][name] = execution
    if not contract["jobs"]:
        raise ValueError("workflow has no platform jobs")
    return contract


def native_routing_only(path, base, head, cwd=None):
    if not base:
        return False
    try:
        before, after = (native_execution_contract(source_at(ref, path, cwd)) for ref in (base, head))
        return before == after
    except (OSError, subprocess.CalledProcessError, UnicodeError, ImportError, ValueError, KeyError, TypeError, AttributeError):
        return False


def platforms(changes, full=False, push=False, release=False, base="", head="HEAD", cwd=None):
    changes = normalized(changes)
    if full or release or not changes:
        return "all"
    if push:
        return "linux-x86_64"
    windows = macos = False
    for status, path in changes:
        if (not matches(path, UNREAD + RECORD_JOB + CI_MACHINERY)
                and not any(lane_reads(lane, path) for lane in LANES.values())):
            return "all"
        if status == "M" and version_only(path, base, head, cwd):
            continue
        if path == ".github/workflows/native-rust.yml":
            if status == "M" and native_routing_only(path, base, head, cwd):
                continue
            return "all"
        if path.endswith(".ps1") or "windows" in path.lower():
            windows = True
        elif "darwin" in path.lower() or "macos" in path.lower():
            macos = True
        elif matches(path, PLATFORM_INPUTS):
            return "all"
        # Changes inside an existing conditional block matter too, not only edits to cfg.
        # Read both revisions so deleting platform-specific code retains its coverage.
        if base and path.endswith(".rs"):
            for ref in (base, head):
                try:
                    source = source_at(ref, path, cwd)
                except subprocess.CalledProcessError:
                    if status in ("A", "D"):
                        continue
                    return "all"
                except (OSError, UnicodeError):
                    return "all"
                if re.search(r"\b(?:unix|target_arch|target_family)\b|std::os::unix", source):
                    return "all"
                windows |= bool(re.search(r"\bwindows\b|std::os::windows", source))
                macos |= bool(re.search(r'\b(?:macos|darwin)\b', source))
                if "target_os" in source and not (windows or macos):
                    return "all"
    if windows and macos:
        return "all"
    if windows:
        return "linux-windows"
    if macos:
        return "linux-macos"
    return "linux-x86_64"


def tree_directories(revision, cwd=None):
    raw = subprocess.check_output(["git", "ls-tree", "-r", "-z", "--name-only", revision],
                                  cwd=cwd, stderr=subprocess.PIPE)
    directories = {""}
    for item in raw.split(b"\0"):
        if item:
            directories.update(parents(item.decode("utf-8", errors="surrogateescape")))
    return directories


def changed_files(base, head, cwd=None, merge_base=True):
    """(status, path) for the whole range; the base and head directory sets, or Nones."""
    try:
        if merge_base:
            base = subprocess.check_output(["git", "merge-base", base, head], cwd=cwd,
                                           stderr=subprocess.PIPE).decode().strip()
        raw = subprocess.check_output(["git", "diff", "--name-status", "--no-renames", "-z", base, head, "--"],
                                      cwd=cwd, stderr=subprocess.PIPE)
        # --no-renames reports a rename as a deletion and an addition; -z keeps unusual names.
        fields = [f.decode("utf-8", errors="surrogateescape") for f in raw.split(b"\0") if f]
        changes = [(fields[i][0], fields[i + 1]) for i in range(0, len(fields) - 1, 2)]
        return changes, tree_directories(base, cwd), tree_directories(head, cwd)
    except (OSError, subprocess.CalledProcessError, IndexError):
        print("Change history unavailable; selecting every CI lane.")
        return [], None, None


def required_failures(needs, pull_request=True):
    failures = []
    for job in ("changes", "record"):
        if needs.get(job, {}).get("result") != "success":
            failures.append(job + " must succeed")
    outputs = needs.get("changes", {}).get("outputs", {})
    for lane in LANE_NAMES:
        if outputs.get(lane) not in ("true", "false"):
            failures.append("missing or invalid selection: " + lane)
    selected = {lane: outputs.get(lane) == "true" for lane in LANE_NAMES}
    scope = outputs.get("platforms")
    if scope not in PLATFORM_SCOPES:
        failures.append("missing or invalid platform scope")
    release = outputs.get("release")
    if release not in ("true", "false"):
        failures.append("missing or invalid release selection")
    if release == "true" and (pull_request or scope != "all" or not all(selected.values())):
        failures.append("release requires every lane and platform on main")
    for job, lanes in JOB_LANES.items():
        expected = "success" if any(selected[lane] for lane in lanes) else "skipped"
        actual = needs.get(job, {}).get("result")
        if actual != expected:
            failures.append("%s: expected %s, got %s" % (job, expected, actual))
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--push", action="store_true")
    parser.add_argument("--full", action="store_true")
    parser.add_argument("--release", action="store_true")
    parser.add_argument("--required", action="store_true")
    args = parser.parse_args()
    if args.release and not args.push:
        parser.error("--release requires --push")
    if args.required:
        failures = required_failures(json.loads(os.environ["CI_NEEDS"]),
                                     os.environ.get("GITHUB_EVENT_NAME") == "pull_request")
        for failure in failures:
            print(failure)
        return 1 if failures else 0
    changes, base_dirs, head_dirs = ([], None, None) if args.full else \
        changed_files(args.base, args.head, merge_base=not args.push)
    selected = select(changes, full=args.full, push=args.push, release=args.release,
                      base_dirs=base_dirs, head_dirs=head_dirs)
    # Platform comparisons use the same merge base as the lane selection.
    base = args.base
    if base and not args.push:
        try:
            base = subprocess.check_output(["git", "merge-base", base, args.head], stderr=subprocess.PIPE).decode().strip()
        except (OSError, subprocess.CalledProcessError):
            base = ""
    scope = platforms(changes, full=args.full, push=args.push, release=args.release, base=base, head=args.head)
    print(json.dumps({"changes": changes, "selected": selected, "platforms": scope, "release": args.release}, indent=2))
    if os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as stream:
            for lane, enabled in selected.items():
                stream.write("%s=%s\n" % (lane, str(enabled).lower()))
            stream.write("platforms=" + scope + "\n")
            stream.write("release=" + str(args.release).lower() + "\n")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as stream:
            stream.write("| CI lane | Selected |\n|---|---|\n")
            for lane, enabled in selected.items():
                stream.write("| %s | %s |\n" % (lane, "yes" if enabled else "no"))
            stream.write("\nRecord, skill/release contracts and CI selection tests always run.\n")
            stream.write("\nPlatforms: " + scope + ".\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
