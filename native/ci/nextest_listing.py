#!/usr/bin/env python3
"""Require nextest to select for its run exactly the tests that cargo runs.

CI runs the native suite through nextest, which finds the tests by listing each test binary
itself. This compares that listing with cargo's own, for the library, binary and integration
test targets. Both must know the same tests, binary by binary, ignored tests included, and
nextest must select every test that cargo does not ignore, and nothing else. Doc-tests are on
neither side, since nextest does not run them.

The arguments are the cargo arguments of the test run, so that every listing reads the build
that run made. A listing that compiles anything fails the check: its build is not the one that
was tested.

    python ci/nextest_listing.py --locked
"""

import json
import re
import subprocess
import sys

# The nextest profile of the test run, whose filters decide what it selects.
PROFILE = "ci"
# `     Running unittests src/lib.rs (target/debug/deps/kpop_native-0123456789abcdef)`
RUNNING = re.compile(r"^\s*Running .* \((?P<path>[^()]+)\)$")
# `store::locking_tests::lock_is_exclusive: test`, as `--list --format terse` prints it.
LISTED = re.compile(r"^(?P<name>\S+): (?:test|bench)$")
COMPILING = re.compile(r"^\s*Compiling ")


def executable(path):
    """The file name of a test binary, which names one build of one target, without `.exe`."""
    name = re.split(r"[\\/]", path.strip())[-1]
    return name[:-4] if name.lower().endswith(".exe") else name


def compiled(output):
    return [line.strip() for line in output.splitlines() if COMPILING.match(line)]


def nextest_listing(build):
    """Binary ids by file name, the tests nextest knows and those it selects, and its complaints."""
    command = ["cargo", "nextest", "list", "--color", "never", *build, "--profile", PROFILE,
               "--message-format", "json"]
    print("$ " + " ".join(command), flush=True)
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            encoding="utf-8", errors="replace")
    sys.stderr.write(result.stderr)
    if result.returncode:
        raise SystemExit(f"nextest could not list the tests (exit {result.returncode})")
    names, known, selected, unlisted = {}, set(), set(), []
    for binary, suite in json.loads(result.stdout)["rust-suites"].items():
        names[executable(suite["binary-path"])] = binary
        if suite.get("status", "listed") != "listed":
            unlisted.append(f"{binary} ({suite['status']})")
        for test, case in suite["testcases"].items():
            known.add((binary, test))
            if case["filter-match"]["status"] == "matches":
                selected.add((binary, test))
    return names, known, selected, unlisted, compiled(result.stderr)


def cargo_listing(build, names, *options):
    """The test binaries cargo runs, the (binary, test) pairs it lists, and what it compiled."""
    command = ["cargo", "test", "--color", "never", *build, "--lib", "--bins", "--tests", "--",
               "--list", "--format", "terse", *options]
    print("$ " + " ".join(command), flush=True)
    # One stream, in order: cargo names each binary on stderr before the binary lists its tests.
    result = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            encoding="utf-8", errors="replace")
    if result.returncode:
        sys.stderr.write(result.stdout)
        raise SystemExit(f"cargo could not list the tests (exit {result.returncode})")
    binaries, tests, binary = set(), set(), None
    for line in result.stdout.splitlines():
        running = RUNNING.match(line)
        if running:
            name = executable(running.group("path"))
            binary = names.get(name, name)
            binaries.add(binary)
            continue
        listed = LISTED.match(line)
        if listed:
            if binary is None:
                raise SystemExit("cargo listed a test before naming its binary: " + line)
            tests.add((binary, listed.group("name")))
    return binaries, tests, compiled(result.stdout)


def main():
    build = sys.argv[1:]
    names, known, selected, unlisted, rebuilt = nextest_listing(build)
    binaries, listed, rebuilt_listing = cargo_listing(build, names)
    _, ignored, rebuilt_ignored = cargo_listing(build, names, "--ignored")
    runs = listed - ignored
    listed_binaries = set(names.values())
    differences = [
        ("the listings compiled, so their build is not the tested one", rebuilt + rebuilt_listing + rebuilt_ignored),
        ("test binaries nextest did not list", unlisted),
        ("test binaries cargo runs that nextest does not list", binaries - listed_binaries),
        ("test binaries nextest lists that cargo does not run", listed_binaries - binaries),
        ("tests cargo lists that nextest does not know", listed - known),
        ("tests nextest knows that cargo does not list", known - listed),
        ("tests cargo runs that nextest does not select", runs - selected),
        ("tests nextest selects that cargo does not run", selected - runs),
    ]
    found = False
    for heading, items in differences:
        if items:
            found = True
            print(f"{heading}:")
            for item in sorted(items):
                print("  " + (": ".join(item) if isinstance(item, tuple) else item))
    if not runs:
        found = True
        print("cargo lists no test to run")
    if found:
        print("::error::nextest and cargo do not agree on the native tests; the lists above name the differences.")
        raise SystemExit(1)
    print(f"nextest selects the {len(runs)} tests cargo runs in {len(binaries)} test binaries, "
          f"and both list the same {len(ignored)} ignored tests.")


if __name__ == "__main__":
    main()
