"""Choose CI lanes from the files each lane reads; unknown paths or unavailable history run all.

Every lane declares the tracked files its checks read, and the directories whose entries they
list. A pull request runs the lanes whose inputs it changes. The declarations are not trusted
on their own: on Linux, `ci_audit.py` records every file an audited lane opens and fails the
pull request that makes it read something undeclared, which is also the pull request whose
changes the lane already runs for. Every push to main runs every lane on every platform.
"""
import argparse
import fnmatch
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


PYTHON_PACKAGE = ("scripts/*", "pyproject.toml")
PYTHON_RUNNER = (".github/scripts/ci_execution.py", ".github/requirements-test.txt", ".github/test-durations.json",
                 "tests/__init__.py", "tests/ci_pytest.py")
# Imported by the release-asset tests in the suite.
RELEASE_ASSETS = (".github/scripts/native_assets.py",)
# Read by the Python suite beyond the package and its tests, as the audit observed them: hook and
# plugin surfaces, installers, this repository's own record (the contract, priors and remeasure
# tests check it), the ignore files the workspace scan honours, the reasoning workflows the
# distribution test compares, and the workflows and pre-push hook the record's count of automatic
# dry runs reads when the remeasure test runs its recipes.
PYTHON_TESTED = PYTHON_PACKAGE + PYTHON_RUNNER + RELEASE_ASSETS + (
    "tests/*", "adapters/*", "hooks/*", "bin/*", ".claude-plugin/*", ".codex-plugin/*",
    "package.json", "LICENSE", "install.sh", "install.ps1", "native/Cargo.toml", "native/Cargo.lock",
    "GROUNDING.yaml", ".kpopper/hypotheses/*", ".kpopper/measure.yaml",
    ".gitignore", "*/.gitignore", ".gitattributes", "examples/scoped-query/*",
    ".github/workflows/reasoning-runtime.yml", ".github/workflows/reasoning-target.yml",
    ".github/workflows/check.yml", ".github/workflows/native-rust.yml",
    ".github/workflows/publish.yml", ".github/workflows/release.yml",
    ".github/workflows/session.yml", ".githooks/pre-push",
)
# Compiling the reasoning runtime and auditing what it links.
RUNTIME_SOURCES = (
    "scripts/reasoning/lean/*", "scripts/reasoning/native/*", "scripts/reasoning/third_party/*",
    "scripts/reasoning/build_runtime.py", "scripts/reasoning/runtime.py",
    "tests/test_reasoning_runtime.py", "tests/test_reasoning_distribution.py",
    "tests/test_reasoning_composition_kernel.py", "tests/test_reasoning_composition_review.py",
    "tests/test_reasoning_composition_acceptance.py", "tests/test_core_composition.py",
    ".github/workflows/reasoning-runtime.yml", ".github/workflows/reasoning-target.yml",
)
RESEARCH_EXAMPLE_INPUTS = (
    ".github/scripts/check_research_example.py",
    "examples/dark-matter/advanced/exercise.py",
    "examples/dark-matter/advanced/record.yaml",
    "examples/dark-matter/advanced/judgment.yaml",
    "examples/dark-matter/advanced/later-study.yaml",
    "examples/dark-matter/advanced/captured-output.json",
    "examples/dark-matter/advanced/assessment-summary.jq",
)
# The offline DOM suite's own files, which only the documents lane runs.
DOM_SUITE = ("tests/document-support/*", "tests/document_ui_fixture.py", "tests/test_document_ui.cjs")
DOCUMENT_INPUTS = DOM_SUITE + (
    "scripts/document/*", "scripts/documents.py", "scripts/document_cli.py", "scripts/document_html.py",
    "scripts/document-guide.md", "tests/test_document*.py", "tests/fixtures/*", "package.json",
    ".gitignore", "*/.gitignore", ".gitattributes",
)

LANES = {
    # The Python suite on 3.9 and 3.13. Several of its tests scan the checkout, so a file
    # added or removed anywhere can change what they see.
    "python": Lane(PYTHON_TESTED, lists=("*",), ignores=DOM_SUITE),
    # Standalone documents: their Python tests and the offline DOM suite.
    "documents": Lane(PYTHON_PACKAGE + PYTHON_RUNNER + DOCUMENT_INPUTS, lists=("*",), enforced=False),
    # The checked session with the reviewed Lean kernel, on the operating systems.
    "session": Lane(PYTHON_TESTED + (".github/workflows/session.yml",), lists=("*",), ignores=DOM_SUITE,
                    enforced=False),
    # Wheel, sdist and plugin installs on every native target, and the reasoning tests against
    # them. Its reusable workflow is recorded in the corresponding-source archive and cannot
    # record reads, so this declaration is reviewed rather than audited: the package, what the
    # installed tests run, the plugin's own surfaces and the committed runtimes. The plugin copy
    # carries every file, but none outside these decide whether the installs work.
    "installed": Lane(PYTHON_PACKAGE + RUNTIME_SOURCES + (
        "tests/test_reasoning*.py", "tests/reasoning/*", "tests/fixtures/*", "tests/__init__.py",
        "bin/*", "hooks/*", ".claude-plugin/*", ".codex-plugin/*", "package.json", "LICENSE",
    ), audited=False, python_entry_points=("tests/test_reasoning*.py",)),
    # Rebuilding the reasoning runtime from source on every target. Reviewed, like installed.
    "runtime": Lane(RUNTIME_SOURCES, audited=False),
    # The native command: compilation, its tests, the release build and installed acceptance.
    "rust": Lane((
        "native/*", ".github/workflows/native-rust.yml",
        # The originals of what native/shared/ and native/LICENSE copy: the build script
        # refuses a copy that differs from them.
        "scripts/session/rules.txt", "scripts/start-guide.md", "scripts/page/*", "scripts/document/*",
        "scripts/assessment.schema.json", "scripts/reasoning/*.schema.json",
        "scripts/session/lean/*", "scripts/reasoning/lean/*", "scripts/verify_page.js",
        "scripts/expressions.py", "LICENSE",
        # Read at test time and by the packaging steps.
        "scripts/reasoning/native/*", "scripts/reasoning/third_party/*", "scripts/reasoning/build_runtime.py",
        "tests/fixtures/page/*", "install.sh", "install.ps1", "VERSION",
        "scripts/package_native.py", "scripts/collect_rust_licenses.py", "scripts/native-licenses/*",
        # The host adapter scripts and manifests its tests run.
        "scripts/native_runtime.sh", "adapters/gemini/scripts/session-start.sh", "adapters/gemini/hooks/hooks.json",
        "adapters/copilot/cli/hook.sh", "adapters/windsurf/hooks.json",
    ), lists=("native/*", "scripts/reasoning/lean*"), ignores=("native/README.md",), enforced=False,
        # Run from the checkout, with everything they import: the host hooks its tests compare,
        # and the setup that builds the Lean program its tests load.
        python_entry_points=("scripts/followups_hook.py", "scripts/watch_hook.py", "scripts/ground_hook.py",
                             "scripts/edit_hook.py", "scripts/session/core.py"),
        rust_sources=("native",)),
    # The research exercise through the installed package and CLI.
    "examples": Lane(PYTHON_PACKAGE + RESEARCH_EXAMPLE_INPUTS, enforced=False),
}
LANE_NAMES = tuple(LANES)

# Tracked files no lane reads. The record job, and the skill, release and CI contract tests
# it runs on every pull request, still check them. A lane's declaration wins over these.
UNREAD = (
    "*.md", "assets/*", "docs/*", "skills/*", "examples/*", "GROUNDING.yaml", ".kpopper/*",
    ".github/ISSUE_TEMPLATE/*", ".github/pull_request_template.md", ".githooks/*",
    ".github/workflows/release.yml", ".github/workflows/publish.yml",
)

# Run by the record job alone, on every pull request; no lane reads them.
RECORD_JOB = (
    "tests/test_skills.py", "tests/test_release.py", "tests/test_ci_selection.py",
    "tests/test_ci_execution.py", "tests/test_ci_sharding.py", "tests/test_ci_audit.py",
    ".github/scripts/release.py", ".github/scripts/publish_release.py",
    ".github/scripts/publish_crate.py", ".github/scripts/native_shared.py",
)

# CI's own machinery decides what every lane means, so a change to it runs everything.
CI_MACHINERY = (
    ".github/workflows/check.yml", ".github/scripts/ci_selection.py", ".github/scripts/ci_audit.py",
)

# A pull request normally leaves out the Intel macOS target, the slowest leg of both platform
# matrices; main keeps it. A change to what decides platform behaviour takes every target.
PLATFORM_INPUTS = (
    "native/Cargo.toml", "native/Cargo.lock", "native/build.rs", "native/rust-toolchain.toml",
    "native/ci/*", ".github/workflows/native-rust.yml", "install.sh", "install.ps1",
    "scripts/package_native.py", "scripts/collect_rust_licenses.py", "scripts/native-licenses/*",
    "scripts/native_launcher.py", "scripts/install_native.sh", "scripts/native_runtime.sh",
    "scripts/reasoning/native/*", "scripts/reasoning/build_runtime.py", "scripts/reasoning/runtime.py",
    "scripts/reasoning/third_party/*", ".github/workflows/reasoning-runtime.yml",
    ".github/workflows/reasoning-target.yml", "pyproject.toml", ".claude-plugin/*", ".codex-plugin/*",
) + CI_MACHINERY
INTEL_MACOS = "darwin-x86_64"
# The platform matrix of reasoning-runtime.yml, which check.yml runs directly.
RUNTIME_TARGETS = (
    {"runner": "ubuntu-24.04", "target": "linux-x86_64", "pythons": '["3.9", "3.13"]'},
    {"runner": "ubuntu-24.04-arm", "target": "linux-aarch64", "pythons": '["3.9", "3.13"]'},
    {"runner": "macos-15", "target": "darwin-arm64", "pythons": '["3.13"]'},
    {"runner": "macos-15-intel", "target": INTEL_MACOS, "pythons": '["3.9", "3.13"]'},
    {"runner": "windows-2022", "target": "windows-x86_64", "pythons": '["3.9", "3.13"]'},
)

TEST_SUITES = ("core", "documents", "reasoning", "session", "other")
JOB_LANES = {"check": ("python", "documents"), "document-ui": ("documents",), "session": ("session",),
             "reasoning-runtime": ("installed", "runtime"), "native-cli": ("rust",),
             "examples": ("examples",)}


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


def select(changes, full=False, push=False, base_dirs=None, head_dirs=None):
    changes = normalized(changes)
    if full or not changes:
        chosen = set(LANE_NAMES)
    else:
        chosen = set()
        for status, path in changes:
            chosen |= lanes_for(status, path, base_dirs, head_dirs)
    if push:
        # Main checks every consumer; recompiling unchanged runtime sources adds no coverage.
        chosen |= set(LANE_NAMES) - {"runtime"}
    if "runtime" in chosen:
        chosen.add("installed")
    return {lane: lane in chosen for lane in LANE_NAMES}


def platforms(changes, full=False, push=False):
    changes = normalized(changes)
    if full or push or not changes or any(matches(path, PLATFORM_INPUTS) for _, path in changes):
        return "all"
    return "pull-request"


def runtime_targets(scope):
    return [dict(row) for row in RUNTIME_TARGETS if scope == "all" or row["target"] != INTEL_MACOS]


def test_suites(selected):
    # Shared readers can reach every consumer. Keep their full coverage while
    # giving the runner concrete, disjoint suites to execute and report.
    if selected["python"]:
        return list(TEST_SUITES)
    return ["documents"] if selected["documents"] else []


def test_matrix(selected, pull_request=True):
    if not (selected["python"] or selected["documents"]):
        return {"include": []}
    rows = []
    for python in (("3.9", "3.13") if pull_request else ("3.13",)):
        # The slower interpreter gets more machines and CPU headroom for the
        # subprocess-heavy history tests. Document-only changes need one group.
        splits = (8 if python == "3.9" else 4) if selected["python"] else 1
        workers = 2 if python == "3.9" else 4
        rows.extend({"python": python, "group": group, "splits": splits, "workers": workers}
                    for group in range(1, splits + 1))
    return {"include": rows}


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
    try:
        if json.loads(outputs.get("test_suites", "null")) != test_suites(selected):
            failures.append("test plan does not cover the selected CI lanes")
    except (TypeError, ValueError):
        failures.append("missing or invalid test plan")
    try:
        if json.loads(outputs.get("test_matrix", "null")) != test_matrix(selected, pull_request):
            failures.append("test matrix omits a required Python version or shard")
    except (TypeError, ValueError):
        failures.append("missing or invalid test matrix")
    scope = outputs.get("platforms")
    if scope not in ("all", "pull-request") or (scope != "all" and not pull_request):
        failures.append("missing or invalid platform scope")
    else:
        try:
            if json.loads(outputs.get("runtime_targets", "null")) != runtime_targets(scope):
                failures.append("runtime targets do not match the platform scope")
        except (TypeError, ValueError):
            failures.append("missing or invalid runtime targets")
    if selected["runtime"] and not selected["installed"]:
        failures.append("a runtime rebuild requires installed checks")
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
    parser.add_argument("--required", action="store_true")
    args = parser.parse_args()
    if args.required:
        failures = required_failures(json.loads(os.environ["CI_NEEDS"]),
                                     os.environ.get("GITHUB_EVENT_NAME") == "pull_request")
        for failure in failures:
            print(failure)
        return 1 if failures else 0
    changes, base_dirs, head_dirs = ([], None, None) if args.full else \
        changed_files(args.base, args.head, merge_base=not args.push)
    selected = select(changes, full=args.full, push=args.push, base_dirs=base_dirs, head_dirs=head_dirs)
    scope = platforms(changes, full=args.full, push=args.push)
    suites = test_suites(selected)
    matrix = test_matrix(selected, pull_request=not (args.push or args.full))
    targets = runtime_targets(scope)
    print(json.dumps({"changes": changes, "selected": selected, "platforms": scope,
                      "test_suites": suites, "test_matrix": matrix, "runtime_targets": targets}, indent=2))
    if os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as stream:
            for lane, enabled in selected.items():
                stream.write("%s=%s\n" % (lane, str(enabled).lower()))
            stream.write("platforms=" + scope + "\n")
            stream.write("test_suites=" + json.dumps(suites) + "\n")
            stream.write("test_matrix=" + json.dumps(matrix) + "\n")
            stream.write("runtime_targets=" + json.dumps(targets) + "\n")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as stream:
            stream.write("| CI lane | Selected |\n|---|---|\n")
            for lane, enabled in selected.items():
                stream.write("| %s | %s |\n" % (lane, "yes" if enabled else "no"))
            stream.write("\nRecord, skill/release contracts and CI selection tests always run.\n")
            stream.write("\nPlatforms: " + ("every target" if scope == "all" else
                                             "every target except " + INTEL_MACOS) + ".\n")
            stream.write("\nPython test suites: " + (", ".join(suites) or "none") + ".\n")
            stream.write("Python test jobs: " + str(len(matrix["include"])) + ".\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
