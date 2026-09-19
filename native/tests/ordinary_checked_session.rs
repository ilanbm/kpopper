use kpop_native::{
    ordinary_runtime::Program,
    reasoning_runtime::{OperationalBounds, target_name},
};
use serde_json::{Value as J, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const RECORD: &str = "sources:\n  s.note: {file: note.txt, read: 2026-09-20}\nknown:\n  p.a: {v: 12, from: s.note}\n  p.map:\n    v:\n      true: yes\n      1: one\njudgments:\n  d.keep:\n    wrong_if: 'p.a > 20'\n    seen: {p.a: 12}\n    verdict: Keep\n    rests_on: [p.a]\n";

fn copy_resources(root: &Path) {
    let target = target_name().unwrap();
    fs::create_dir_all(root.join("resources/reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.zip")),
        root.join("resources/reasoning")
            .join(format!("{target}.zip")),
    )
    .unwrap();
    fs::create_dir_all(root.join("resources/ordinary").join(&target)).unwrap();
    let ordinary = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean")
                .join(&target)
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    for name in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(
            ordinary.join(name),
            root.join("resources/ordinary").join(&target).join(name),
        )
        .unwrap();
    }
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), RECORD).unwrap();
    fs::write(root.path().join("note.txt"), "source evidence\n").unwrap();
    copy_resources(root.path());
    root
}

fn command(root: &Path, operation: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
    command
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "--frozen",
            "session",
            operation,
            "--no-settings",
            "--input",
            "GROUNDING.yaml",
            "--project",
            "fixture",
            "--state",
            "state",
            "--assessment-profile",
            "checked-reader/v1",
        ])
        .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"));
    command
}

fn ok(output: std::process::Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn revision(open: &str) -> String {
    open.lines()
        .find_map(|line| line.strip_prefix("project=fixture revision="))
        .unwrap()
        .into()
}

#[test]
fn ordinary_public_open_read_sources_keys_and_stale_inputs() {
    let temp = fixture();
    let root = temp.path();
    let open = ok(command(root, "open")
        .args(["--tokens", "8000"])
        .output()
        .unwrap());
    assert!(open.contains("d.keep"));
    let rev = revision(&open);
    for (operation, arguments, message) in [
        ("search", vec!["--query", "p.a"], "ordinary session search"),
        (
            "context",
            vec!["--id", "p.a", "--direction", "support"],
            "ordinary session context",
        ),
        (
            "propose",
            vec!["--kind", "question", "--text", "Later?"],
            "ordinary session proposals",
        ),
    ] {
        let output = command(root, operation)
            .args(["--revision", &rev])
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains(message));
    }
    let node = ok(command(root, "read")
        .args([
            "--ref",
            "node:d.keep",
            "--revision",
            &rev,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap());
    let node: J = serde_json::from_str(&node).unwrap();
    assert_eq!(node["value"]["checked_bundle_ref"], "checked:d.keep");
    assert!(
        node["value"]["epistemic_card"]
            .as_str()
            .unwrap()
            .contains("RECORDED CLAIM d.keep")
    );
    let scalar = ok(command(root, "read")
        .args([
            "--ref",
            "node:p.map#/body/v",
            "--revision",
            &rev,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap());
    let scalar: J = serde_json::from_str(&scalar).unwrap();
    assert_eq!(scalar["value"], json!({"true":"one"}));
    let source = ok(command(root, "read")
        .args([
            "--ref",
            "source:record",
            "--revision",
            &rev,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap());
    let source: J = serde_json::from_str(&source).unwrap();
    assert_eq!(source["value"]["text"], RECORD);
    fs::write(
        root.join("GROUNDING.yaml"),
        RECORD.replace("v: 12", "v: 13"),
    )
    .unwrap();
    let stale = command(root, "read")
        .args(["--ref", "node:p.a", "--revision", &rev, "--tokens", "8000"])
        .output()
        .unwrap();
    assert_eq!(stale.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("reopen"));
}

#[test]
fn ordinary_verify_claims_returns_acceptance_and_rejected_checks() {
    let temp = fixture();
    let root = temp.path();
    let rev = revision(&ok(command(root, "open")
        .args(["--tokens", "8000"])
        .output()
        .unwrap()));
    let input=[json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"}),json!({"jsonrpc":"2.0","id":"accepted","method":"tools/call","params":{"name":"kpopper_verify_claims","arguments":{"judgment":"d.keep","revision":rev,"assertions":[{"kind":"current","id":"p.a","expected":12}]}}}),json!({"jsonrpc":"2.0","id":"rejected","method":"tools/call","params":{"name":"kpopper_verify_claims","arguments":{"judgment":"d.keep","revision":rev,"assertions":[{"kind":"current","id":"p.a","expected":99}]}}})].into_iter().map(|value|value.to_string()+"\n").collect::<String>();
    let mut child = command(root, "serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = ok(child.wait_with_output().unwrap());
    let messages = output
        .lines()
        .map(|line| serde_json::from_str::<J>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(messages[1]["result"]["isError"], false);
    assert_eq!(
        serde_json::from_str::<J>(
            messages[1]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
        )
        .unwrap()["accepted"],
        true
    );
    assert_eq!(messages[2]["result"]["isError"], true);
    assert!(
        messages[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("accepted")
    );
}

#[test]
fn ordinary_runtime_retains_exit_two_but_compute_still_refuses_it() {
    let temp = fixture();
    let root = temp.path();
    let target = target_name().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let _ = cache;
    let program = Program::open(&root.join("resources/ordinary").join(target)).unwrap();
    let graph = json!({"nodes":{"p.a":{"kind":"known","states":[],"body":{"v":12}},"d.keep":{"kind":"judgment","states":[],"body":{"verdict":"Keep","rests_on":["p.a"],"seen":{"p.a":12},"wrong_if":"p.a > 20"},"assessment_body":{"verdict":"Keep","rests_on":["p.a"],"seen":{"p.a":12},"wrong_if":"p.a > 20"},"assessment_fields":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}}},"edges":[],"topics":{"p.a":["known","p"],"d.keep":["judgments","d"]}});
    let rejected = program
        .assess(
            &graph,
            "d.keep",
            &[json!({"kind":"current","id":"p.a","expected":99})],
            &OperationalBounds::default(),
        )
        .unwrap();
    assert_eq!(rejected["assertions_accepted"], false);
    let legacy = json!({"record":graph,"id":"d.keep","assertions":[{"kind":"current","id":"p.a","expected":99}]});
    assert_eq!(
        program
            .request(&legacy, &OperationalBounds::default())
            .unwrap_err()
            .0,
        "native reasoning process failed"
    );
}

#[test]
#[ignore = "requires immutable Python oracle; set KPOP_PYTHON_SESSION_ORACLE and KPOP_PYTHON_SESSION_ORACLE_SCRIPT"]
fn pinned_python_oracle_matches_native_open_read_and_verify_packets() {
    let python = std::env::var_os("KPOP_PYTHON_SESSION_ORACLE")
        .expect("set KPOP_PYTHON_SESSION_ORACLE to the pinned Python interpreter");
    let script = std::env::var_os("KPOP_PYTHON_SESSION_ORACLE_SCRIPT")
        .expect("set KPOP_PYTHON_SESSION_ORACLE_SCRIPT to the immutable oracle driver");
    let temp = fixture();
    let root = temp.path();
    let oracle = Command::new(python)
        .arg(script)
        .arg(root.join("GROUNDING.yaml"))
        .arg(root.join("python-state"))
        .output()
        .unwrap();
    assert!(
        oracle.status.success(),
        "{}",
        String::from_utf8_lossy(&oracle.stderr)
    );
    let oracle: J = serde_json::from_slice(&oracle.stdout).unwrap();
    let open = ok(command(root, "open")
        .args(["--tokens", "65536"])
        .output()
        .unwrap());
    let revision = revision(&open);
    let oracle_revision = oracle["revision"].as_str().unwrap();
    assert_eq!(
        open.replace(&revision, "REVISION"),
        oracle["open"]
            .as_str()
            .unwrap()
            .replace(oracle_revision, "REVISION")
    );
    assert!(open.contains("d.keep"));
    let mut native: J = serde_json::from_str(&ok(command(root, "read")
        .args([
            "--ref",
            "node:d.keep",
            "--revision",
            &revision,
            "--tokens",
            "65536",
        ])
        .output()
        .unwrap()))
    .unwrap();
    native["revision"] = json!("REVISION");
    let mut expected = oracle["node"].clone();
    expected["revision"] = json!("REVISION");
    assert_eq!(native, expected);
    let mut native: J = serde_json::from_str(&ok(command(root, "read")
        .args([
            "--ref",
            "node:p.map#/body/v",
            "--revision",
            &revision,
            "--tokens",
            "65536",
        ])
        .output()
        .unwrap()))
    .unwrap();
    native["revision"] = json!("REVISION");
    let mut expected = oracle["scalar_key"].clone();
    expected["revision"] = json!("REVISION");
    assert_eq!(native, expected);
    let mut native: J = serde_json::from_str(&ok(command(root, "read")
        .args([
            "--ref",
            "source:record",
            "--revision",
            &revision,
            "--tokens",
            "65536",
        ])
        .output()
        .unwrap()))
    .unwrap();
    native["revision"] = json!("REVISION");
    let mut expected = oracle["source"].clone();
    expected["revision"] = json!("REVISION");
    assert_eq!(native, expected);

    let input=[json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),json!({"jsonrpc":"2.0","method":"notifications/initialized"}),json!({"jsonrpc":"2.0","id":"accepted","method":"tools/call","params":{"name":"kpopper_verify_claims","arguments":{"judgment":"d.keep","revision":revision,"assertions":[{"kind":"current","id":"p.a","expected":12}]}}}),json!({"jsonrpc":"2.0","id":"rejected","method":"tools/call","params":{"name":"kpopper_verify_claims","arguments":{"judgment":"d.keep","revision":revision,"assertions":[{"kind":"current","id":"p.a","expected":99}]}}})].into_iter().map(|value|value.to_string()+"\n").collect::<String>();
    let mut child = command(root, "serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = ok(child.wait_with_output().unwrap());
    let messages = output
        .lines()
        .map(|line| serde_json::from_str::<J>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(messages[1]["result"]["isError"], false);
    assert_eq!(
        serde_json::from_str::<J>(
            messages[1]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
        )
        .unwrap(),
        oracle["accepted"]
    );
    assert_eq!(messages[2]["result"]["isError"], true);
    assert_eq!(
        serde_json::from_str::<J>(
            messages[2]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
        )
        .unwrap(),
        oracle["rejected"]
    );
}
