use base64::Engine;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn git(root: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn fixture(source: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.name", "Fixture"]);
    git(&root, &["config", "user.email", "fixture@example.test"]);
    fs::write(root.join("GROUNDING.yaml"), "known: {}\n").unwrap();
    git(&root, &["add", "GROUNDING.yaml"]);
    git(
        &root,
        &["-c", "commit.gpgsign=false", "commit", "-m", "fixture"],
    );
    let record = root.join("legacy.yaml");
    fs::write(&record, source).unwrap();
    (temp, root, record)
}

fn invoke(root: &Path, record: &Path, shareability: &str, extra: &[&str]) -> Output {
    invoke_event(root, record, shareability, "event-fixed", extra)
}

fn invoke_event(
    root: &Path,
    record: &Path,
    shareability: &str,
    event_id: &str,
    extra: &[&str],
) -> Output {
    import_command(root, record, shareability, event_id, extra)
        .output()
        .unwrap()
}

fn import_command(
    root: &Path,
    record: &Path,
    shareability: &str,
    event_id: &str,
    extra: &[&str],
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
    command
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "knowledge",
            "import",
            record.to_str().unwrap(),
            "--shareability",
            shareability,
            "--scope",
            "external",
            "--environment",
            "vendor",
            "--event-id",
            event_id,
        ])
        .args(extra)
        .env(
            "KPOPPER_PRIVATE_HOME",
            root.parent().unwrap().join("private"),
        );
    command
}

#[cfg(unix)]
#[test]
fn incomplete_prepared_git_tree_is_refused_before_moving_pending_ref() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, root, record) = fixture(PUBLIC);
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let real_git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(real_git.status.success());
    let wrapper = bin.join("git");
    fs::write(
        &wrapper,
        br#"#!/bin/sh
for argument in "$@"; do
  case "$argument" in
    100644,*,events/event-fixed.json)
      "$KPOP_TEST_REAL_GIT" -C "$KPOP_TEST_GIT_ROOT" read-tree --empty || exit
      ;;
  esac
done
exec "$KPOP_TEST_REAL_GIT" "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let before = fs::read(&record).unwrap();
    let output = import_command(&root, &record, "project", "event-fixed", &[])
        .env("PATH", path)
        .env(
            "KPOP_TEST_REAL_GIT",
            String::from_utf8(real_git.stdout).unwrap().trim(),
        )
        .env("KPOP_TEST_GIT_ROOT", &root)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        parsed(&output)["error"],
        "pending tree verification failed before publication"
    );
    assert_eq!(fs::read(&record).unwrap(), before);
    assert!(
        !Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--verify", "refs/kpopper/pending_grounding"])
            .output()
            .unwrap()
            .status
            .success()
    );
}

fn parsed(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

const PUBLIC: &str = "sources:\n  s.vendor: {name: Vendor}\nknown:\n  fact.import:\n    v: 10\n    from: s.vendor\n    scope: {kind: external, environment: vendor}\n";

#[test]
fn public_import_matches_the_python_revision_tree_and_replay_contract() {
    let (_temp, root, record) = fixture(
        "sources:\n  s.vendor:\n    name: Vendor\n    file: proof.txt\nknown:\n  fact.import:\n    v: 10\n    from: s.vendor\n    scope: {kind: external, environment: vendor}\n",
    );
    let evidence = root.join("allowed");
    fs::create_dir(&evidence).unwrap();
    fs::write(evidence.join("proof.txt"), "proof bytes\n").unwrap();
    let canonical = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let source = fs::read(&record).unwrap();
    let first = invoke(
        &root,
        &record,
        "project",
        &["--evidence", evidence.to_str().unwrap()],
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first = parsed(&first);
    assert_eq!(
        first["revision"],
        "855b24965bdc8b6450e194b190aa0007ec9c4e605addb3c235f7f4df48a2dad3"
    );
    assert_eq!(first["state"], "captured");
    assert_eq!(first["replay"], false);
    assert_eq!(
        first["publication_attempt"],
        json!({"started":false,"reason":"no standing publication permission"})
    );
    let head = first["ledger_commit"].as_str().unwrap();
    let tree =
        String::from_utf8(git(&root, &["ls-tree", "-r", "--name-only", head]).stdout).unwrap();
    assert_eq!(
        tree.lines().collect::<Vec<_>>(),
        vec![
            "contributions/855b24965bdc8b6450e194b190aa0007ec9c4e605addb3c235f7f4df48a2dad3/evidence/proof.txt",
            "contributions/855b24965bdc8b6450e194b190aa0007ec9c4e605addb3c235f7f4df48a2dad3/manifest.json",
            "events/event-fixed.json",
        ]
    );
    let replay = parsed(&invoke(
        &root,
        &record,
        "project",
        &["--evidence", evidence.to_str().unwrap()],
    ));
    assert_eq!(replay["replay"], true);
    assert_eq!(replay["ledger_commit"], first["ledger_commit"]);
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), canonical);
    assert_eq!(fs::read(&record).unwrap(), source);
}

#[test]
fn private_unclear_and_private_locator_only_create_private_drafts() {
    for shareability in ["private", "unclear"] {
        let (_temp, root, record) = fixture(PUBLIC);
        let output = invoke(&root, &record, shareability, &[]);
        assert!(output.status.success());
        let value = parsed(&output);
        assert_eq!(value["state"], "private draft");
        assert!(Path::new(value["path"].as_str().unwrap()).is_file());
        assert!(!root.join(".git/kpopper/project/project.json").exists());
        assert_eq!(
            fs::read(root.join("GROUNDING.yaml")).unwrap(),
            b"known: {}\n"
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["rev-parse", "--verify", "refs/kpopper/pending_grounding"])
                .output()
                .unwrap()
                .status
                .code()
                != Some(0)
        );
    }
    let (_temp, root, record) = fixture(
        "sources:\n  s.private: {location: 'file:///private/source.txt'}\nknown:\n  fact.import:\n    v: 10\n    from: s.private\n    scope: {kind: external, environment: vendor}\n",
    );
    assert_eq!(
        parsed(&invoke(&root, &record, "project", &[]))["state"],
        "private draft"
    );
}

#[test]
fn malformed_nontext_incomplete_and_missing_evidence_fail_without_writes() {
    let cases = [
        ("known: [\n", "invalid_history_yaml"),
        (
            "known:\n  fact.import:\n    1: value\n    scope: {kind: external, environment: vendor}\n",
            "invalid_yaml_key",
        ),
        (
            "schema: {deps: []}\nknown:\n  fact.import:\n    v: 10\n    scope: {kind: external, environment: vendor}\n",
            "invalid_snapshot",
        ),
        (
            "known:\n  fact.import:\n    v: 10\n    rests_on: [fact.missing]\n    scope: {kind: external, environment: vendor}\n",
            "missing_contribution_dependency",
        ),
        (
            "known:\n  fact.import:\n    v: 10\n    file: proof.txt\n    scope: {kind: external, environment: vendor}\n",
            "import needs an explicit --evidence root",
        ),
    ];
    for (source, error) in cases {
        let (_temp, root, record) = fixture(source);
        let canonical = fs::read(root.join("GROUNDING.yaml")).unwrap();
        let output = invoke(&root, &record, "project", &[]);
        assert_eq!(output.status.code(), Some(2));
        assert!(
            parsed(&output)["error"].as_str().unwrap().contains(error),
            "expected {error} for {source:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), canonical);
        assert!(!root.join(".git/kpopper/project/project.json").exists());
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["rev-parse", "--verify", "refs/kpopper/pending_grounding"])
                .output()
                .unwrap()
                .status
                .code()
                != Some(0)
        );
    }
}

#[test]
fn reused_event_with_changed_input_refuses_without_moving_the_ref() {
    let (_temp, root, record) = fixture(PUBLIC);
    let first = invoke(&root, &record, "project", &[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );
    let first = parsed(&first);
    let head = first["ledger_commit"].clone();
    fs::write(&record, PUBLIC.replace("v: 10", "v: 11")).unwrap();
    let output = invoke(&root, &record, "project", &[]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        parsed(&output)["error"],
        "event ID was already used with different content or target"
    );
    let current =
        String::from_utf8(git(&root, &["rev-parse", "refs/kpopper/pending_grounding"]).stdout)
            .unwrap();
    assert_eq!(current.trim(), head.as_str().unwrap());
    assert_eq!(
        fs::read(root.join("GROUNDING.yaml")).unwrap(),
        b"known: {}\n"
    );
}

#[test]
fn oversized_evidence_is_refused_before_pending_state() {
    let (_temp, root, record) = fixture(
        "sources:\n  s.vendor: {file: proof.txt}\nknown:\n  fact.import: {v: 10, from: s.vendor, scope: {kind: external, environment: vendor}}\n",
    );
    let evidence = root.join("evidence");
    fs::create_dir(&evidence).unwrap();
    fs::File::create(evidence.join("proof.txt"))
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let output = invoke(
        &root,
        &record,
        "project",
        &["--evidence", evidence.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(parsed(&output)["error"], "evidence_limit");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--verify", "refs/kpopper/pending_grounding"])
            .output()
            .unwrap()
            .status
            .code()
            != Some(0)
    );
}

#[test]
fn concurrent_import_events_are_both_retained() {
    let (_temp, root, record) = fixture(PUBLIC);
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| invoke_event(&root, &record, "project", "event-one", &[]));
        let second = scope.spawn(|| invoke_event(&root, &record, "project", "event-two", &[]));
        (first.join().unwrap(), second.join().unwrap())
    });
    for output in [&first, &second] {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    let head =
        String::from_utf8(git(&root, &["rev-parse", "refs/kpopper/pending_grounding"]).stdout)
            .unwrap();
    let names =
        String::from_utf8(git(&root, &["ls-tree", "-r", "--name-only", head.trim()]).stdout)
            .unwrap();
    assert!(names.contains("events/event-one.json"));
    assert!(names.contains("events/event-two.json"));
}

#[test]
#[ignore = "requires immutable Python oracle; set KPOP_PYTHON_KNOWLEDGE_ORACLE and KPOP_PYTHON_KNOWLEDGE_ORACLE_SCRIPT"]
fn public_import_matches_full_python_receipt_and_state_images() {
    let python = std::env::var_os("KPOP_PYTHON_KNOWLEDGE_ORACLE")
        .expect("set KPOP_PYTHON_KNOWLEDGE_ORACLE to the pinned Python interpreter");
    let script = std::env::var_os("KPOP_PYTHON_KNOWLEDGE_ORACLE_SCRIPT")
        .expect("set KPOP_PYTHON_KNOWLEDGE_ORACLE_SCRIPT to the immutable oracle driver");
    let source = "sources:\n  s.vendor:\n    name: Vendor\n    file: proof.txt\nknown:\n  fact.import:\n    v: 10\n    from: s.vendor\n    scope: {kind: external, environment: vendor}\n";
    let (_native_temp, native_root, native_record) = fixture(source);
    let native_evidence = native_root.join("allowed");
    fs::create_dir(&native_evidence).unwrap();
    fs::write(native_evidence.join("proof.txt"), "proof bytes\n").unwrap();
    let native_output = invoke(
        &native_root,
        &native_record,
        "project",
        &["--evidence", native_evidence.to_str().unwrap()],
    );
    assert!(native_output.status.success());
    let mut native_receipt = parsed(&native_output);

    let (_python_temp, python_root, python_record) = fixture(source);
    let python_evidence = python_root.join("allowed");
    fs::create_dir(&python_evidence).unwrap();
    fs::write(python_evidence.join("proof.txt"), "proof bytes\n").unwrap();
    let oracle = Command::new(python)
        .arg(script)
        .arg(&python_root)
        .arg(&python_record)
        .arg(&python_evidence)
        .env(
            "KPOPPER_PRIVATE_HOME",
            python_root.parent().unwrap().join("private"),
        )
        .output()
        .unwrap();
    assert!(
        oracle.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&oracle.stdout),
        String::from_utf8_lossy(&oracle.stderr)
    );
    let oracle: Value = serde_json::from_slice(&oracle.stdout).unwrap();
    let mut python_receipt = oracle["receipt"].clone();
    for receipt in [&mut native_receipt, &mut python_receipt] {
        receipt["captured_at_ns"] = json!("TIME");
        receipt["commit"] = json!("COMMIT");
        receipt["ledger_commit"] = json!("COMMIT");
    }
    assert_eq!(native_receipt, python_receipt);

    let head = String::from_utf8(
        git(
            &native_root,
            &["rev-parse", "refs/kpopper/pending_grounding"],
        )
        .stdout,
    )
    .unwrap();
    let names =
        String::from_utf8(git(&native_root, &["ls-tree", "-r", "--name-only", head.trim()]).stdout)
            .unwrap();
    let mut native_files = serde_json::Map::new();
    for name in names.lines() {
        let bytes = git(&native_root, &["show", &format!("{}:{name}", head.trim())]).stdout;
        native_files.insert(
            name.into(),
            json!(base64::engine::general_purpose::STANDARD.encode(bytes)),
        );
    }
    let mut python_files = oracle["files"].as_object().unwrap().clone();
    let event = "events/event-fixed.json";
    for files in [&mut native_files, &mut python_files] {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(files[event].as_str().unwrap())
            .unwrap();
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        value["captured_at_ns"] = json!("TIME");
        files.insert(
            event.into(),
            json!(
                base64::engine::general_purpose::STANDARD
                    .encode(serde_json::to_vec(&value).unwrap())
            ),
        );
    }
    assert_eq!(native_files, python_files);
    let native_config = fs::read(native_root.join(".git/kpopper/project/project.json")).unwrap();
    assert_eq!(
        json!(base64::engine::general_purpose::STANDARD.encode(native_config)),
        oracle["config"]
    );
}
