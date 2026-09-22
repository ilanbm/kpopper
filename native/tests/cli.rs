use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use tempfile::TempDir;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_kpop-native")
}
fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(["--workspace", root.to_str().unwrap()])
        .args(args)
        .env("PATH", "")
        .output()
        .unwrap()
}
fn ok(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn fixture() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().canonicalize().unwrap();
    ok(run(&root, &["init", "--record-id", "test-record"]));
    (temp, root)
}
fn write(root: &Path, kind: &str, id: &str, value: &str, op: &str) -> Output {
    run(
        root,
        &[
            kind,
            id,
            "--value",
            value,
            "--source",
            "fixture",
            "--operation",
            op,
            "--on",
            "2026-09-19T00:00:00Z",
        ],
    )
}
fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, result: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for item in fs::read_dir(root).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                walk(&path, result);
            } else {
                result.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    let mut result = Default::default();
    walk(root, &mut result);
    result
}

#[test]
fn feasibility_store_cannot_silently_ignore_reframe() {
    let (_temp, root) = fixture();
    ok(write(&root, "add", "p.input", "true", "first"));
    let before = snapshot(&root);
    let result = run(&root, &["set", "p.input", "--value", "false", "--operation", "rewrite",
        "--source", "fixture", "--on", "2026-09-19T00:00:00Z", "--reframe"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("reframe is unavailable"));
    assert_eq!(snapshot(&root), before);
}

#[test]
fn fresh_process_write_history_and_read_only_reopen_without_runtime() {
    let (_temp, root) = fixture();
    ok(write(&root, "add", "p.input", "1", "first"));
    let old = snapshot(&root);
    ok(write(&root, "set", "p.input", "2", "second"));
    let before = snapshot(&root);
    let view = ok(run(&root, &["open"]));
    assert_eq!(view["document"]["readings"]["p.input"]["v"], 2);
    let history = ok(run(&root, &["history", "p.input"]));
    assert_eq!(history["objects"].as_array().unwrap().len(), 4);
    assert_eq!(history["commits"], 2);
    assert_eq!(snapshot(&root), before);
    for (path, bytes) in old {
        if path.components().any(|c| c.as_os_str() == "history") {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn null_absence_unicode_and_path_like_subjects_stay_distinct() {
    let (_temp, root) = fixture();
    let subject = "../שבוע / זמן 🦀";
    ok(write(&root, "add", subject, "null", "first"));
    let view = ok(run(&root, &["open"]));
    let readings = view["document"]["readings"].as_object().unwrap();
    assert!(!readings.contains_key("missing"));
    assert!(readings[subject]["v"].is_null());
    assert_eq!(
        fs::read_dir(root.join(".kpopper/history")).unwrap().count(),
        1
    );
    assert!(!root.parent().unwrap().join("שבוע").exists());
}

#[test]
fn exact_retry_is_idempotent_and_collision_or_stale_revision_refuses() {
    let (_temp, root) = fixture();
    let revision = ok(run(&root, &["open"]))["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(write(&root, "add", "p.input", "1", "first"));
    let before = snapshot(&root);
    assert_eq!(
        ok(write(&root, "add", "p.input", "1", "first"))["status"],
        "replayed"
    );
    assert_eq!(snapshot(&root), before);
    assert!(
        !write(&root, "add", "p.input", "2", "first")
            .status
            .success()
    );
    assert!(
        !run(
            &root,
            &[
                "set",
                "p.input",
                "--value",
                "2",
                "--operation",
                "second",
                "--on",
                "now",
                "--expected-revision",
                &revision
            ]
        )
        .status
        .success()
    );
    assert_eq!(snapshot(&root), before);
}

#[test]
fn interruption_on_each_side_of_manifest_has_defined_visibility_and_retry() {
    for stage in ["objects", "commit"] {
        let (_temp, root) = fixture();
        ok(write(&root, "add", "p.input", "1", "first"));
        let output = Command::new(binary())
            .args([
                "--workspace",
                root.to_str().unwrap(),
                "set",
                "p.input",
                "--value",
                "2",
                "--source",
                "fixture",
                "--operation",
                "second",
                "--on",
                "2026-09-19T00:00:00Z",
            ])
            .env("PATH", "")
            .env("KPOP_NATIVE_FAIL_AFTER", stage)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let view = ok(run(&root, &["open"]));
        assert_eq!(
            view["document"]["readings"]["p.input"]["v"],
            if stage == "objects" { 1 } else { 2 }
        );
        assert_eq!(view["view_current"], stage == "objects");
        ok(write(&root, "set", "p.input", "2", "second"));
        assert_eq!(ok(run(&root, &["open"]))["view_current"], true);
        assert_eq!(ok(run(&root, &["history"]))["commits"], 2);
    }
}

#[test]
fn orphan_objects_never_become_committed_and_corrupt_members_fail_closed() {
    let (_temp, root) = fixture();
    ok(write(&root, "add", "p.input", "1", "first"));
    let folder = fs::read_dir(root.join(".kpopper/history"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(
        folder.join(format!("{}.yaml", "a".repeat(64))),
        b"bad yaml [",
    )
    .unwrap();
    assert_eq!(
        ok(run(&root, &["history"]))["objects"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let path = fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap() != format!("{}.yaml", "a".repeat(64)).as_str())
        .unwrap();
    fs::write(&path, b"{}").unwrap();
    assert!(!run(&root, &["open"]).status.success());
}

#[test]
fn unsupported_authority_and_pending_python_journal_refuse_without_changes() {
    let (_temp, root) = fixture();
    let journal = root.join(".kpopper/.history-local");
    fs::create_dir(&journal).unwrap();
    fs::write(journal.join("pending"), b"pending").unwrap();
    let before = snapshot(&root);
    assert!(!run(&root, &["open"]).status.success());
    assert!(!write(&root, "add", "p.a", "1", "first").status.success());
    assert_eq!(snapshot(&root), before);
    fs::remove_file(journal.join("pending")).unwrap();
    let path = root.join(".kpopper/history.yaml");
    let mut marker: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    marker["version"] = json!(2);
    fs::write(path, serde_json::to_vec(&marker).unwrap()).unwrap();
    assert!(!run(&root, &["open"]).status.success());
}

#[test]
fn duplicate_keys_and_oversized_stdin_are_rejected() {
    for raw in [br#"{"a":1,"a":2}"#.to_vec(), vec![b' '; 1_048_577]] {
        let mut child = Command::new(binary())
            .arg("identity")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(&raw);
        assert!(!child.wait_with_output().unwrap().status.success());
    }
}

#[cfg(unix)]
#[test]
fn symlink_and_directory_lock_protect_the_write_boundary() {
    use fs2::FileExt;
    let (_temp, root) = fixture();
    let lock = fs::File::open(&root).unwrap();
    lock.lock_exclusive().unwrap();
    assert!(!write(&root, "add", "p.a", "1", "first").status.success());
    FileExt::unlock(&lock).unwrap();
    let record = root.join("GROUNDING.yaml");
    let target = root.join("untouched");
    fs::rename(&record, &target).unwrap();
    std::os::unix::fs::symlink(&target, &record).unwrap();
    let before = fs::read(&target).unwrap();
    assert!(!write(&root, "add", "p.a", "1", "first").status.success());
    assert_eq!(fs::read(target).unwrap(), before);
}

#[test]
fn hook_uses_absolute_native_command_and_handles_subagents_and_errors() {
    let (temp, root) = fixture();
    ok(write(&root, "add", "p.input", "1", "first"));
    let executable_dir = temp.path().join("binary with spaces");
    fs::create_dir(&executable_dir).unwrap();
    let executable = executable_dir.join("native runtime");
    fs::copy(binary(), &executable).unwrap();
    let invoke = |payload: Value| {
        let mut child = Command::new(&executable)
            .arg("session-start")
            .env("PATH", "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&payload).unwrap())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let output = invoke(json!({"cwd":root,"session_id":"test"}));
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let context: Value = serde_json::from_str(
        stdout
            .lines()
            .find_map(|l| l.strip_prefix("KPOPPER_AGENT_CONTEXT "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        context["command"],
        json!([executable.canonicalize().unwrap(), "--workspace", root])
    );
    assert_eq!(context["environment"]["KPOPPER_AGENT_SESSION"], "test");
    let routed = context["command"].as_array().unwrap();
    let reopened = Command::new(routed[0].as_str().unwrap())
        .args(routed[1..].iter().map(|v| v.as_str().unwrap()))
        .arg("open")
        .env("PATH", "")
        .output()
        .unwrap();
    assert_eq!(ok(reopened)["document"]["readings"]["p.input"]["v"], 1);
    assert!(
        invoke(json!({"cwd":root,"agent_id":"child"}))
            .stdout
            .is_empty()
    );
    let failed = invoke(json!({"cwd":root.join("missing")}));
    assert!(failed.status.success());
    assert!(failed.stdout.is_empty());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("not opened"));
}

#[test]
fn public_session_start_opens_an_ordinary_record_and_returns_its_native_route() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let record = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.load: {v: 61}\njudgments:\n  d.work:\n    verdict: continue\n    rests_on: [p.load]\n    seen: {p.load: 61}\n    reopened_by: a different load reopens this decision\n";
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let mut child = Command::new(binary())
        .arg("session-start")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            &serde_json::to_vec(&json!({"cwd":root,"session_id":"ordinary-session"})).unwrap(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.starts_with("2 entries, 1 judgments"),
        "stdout={stdout:?} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let context: Value = serde_json::from_str(
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("KPOPPER_AGENT_CONTEXT "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        context["command"],
        json!([
            Path::new(binary()).canonicalize().unwrap(),
            "--workspace",
            root
        ])
    );
    assert_eq!(
        context["environment"]["KPOPPER_AGENT_SESSION"],
        "ordinary-session"
    );
    assert_eq!(context["profile"], "native-public/v1");
    assert!(
        stdout.contains("Pass this session environment to record-writing and mapping commands.")
    );
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
}

#[test]
fn public_history_status_reports_the_active_native_authority_without_writes() {
    let data: Value = serde_json::from_str(include_str!("fixtures/history-capture.json")).unwrap();
    let case = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case.get("output").is_some())
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for (name, raw) in case["files"].as_object().unwrap() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw.as_str().unwrap()).unwrap();
    }
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let output = Command::new(binary())
        .args(["--workspace", root.to_str().unwrap(), "history", "status"])
        .output()
        .unwrap();
    let value = ok(output);
    assert_eq!(value["state"], "captured");
    assert_eq!(
        value["record"],
        root.join("GROUNDING.yaml").to_string_lossy().as_ref()
    );
    assert!(value["commits"].as_u64().unwrap() > 0);
    assert!(value["objects"].as_u64().unwrap() > 0);
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
}

#[test]
fn yaml_metadata_is_supported_but_duplicate_keys_tags_and_aliases_refuse() {
    let (_temp, root) = fixture();
    let marker = root.join(".kpopper/native-feasibility.json");
    fs::write(
        &marker,
        "version: 1\nrecord_id: test-record\nprofile: native-feasibility/v1\n",
    )
    .unwrap();
    ok(run(&root, &["open"]));
    for bad in [
        "version: 1\nversion: 1\nrecord_id: test-record\nprofile: native-feasibility/v1\n",
        "version: 1\nrecord_id: test-record\nprofile: !unknown native-feasibility/v1\n",
        "version: 1\nrecord_id: &id test-record\nprofile: *id\n",
    ] {
        fs::write(&marker, bad).unwrap();
        assert!(!run(&root, &["open"]).status.success());
    }
}

#[test]
fn missing_members_and_incomplete_parent_chain_do_not_return_partial_records() {
    let (_temp, root) = fixture();
    ok(write(&root, "add", "p.input", "1", "first"));
    ok(write(&root, "set", "p.input", "2", "second"));
    let path = root.join(".kpopper/history-commits/first.yaml");
    let original = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(!run(&root, &["open"]).status.success());
    fs::write(&path, original).unwrap();
    let history = ok(run(&root, &["history"]));
    let object = &history["objects"][0];
    let path = root.join(".kpopper/history").join(
        kpop_native::identity::subject_path(
            object["subject"].as_str().unwrap(),
            object["id"].as_str().unwrap(),
        )
        .unwrap(),
    );
    fs::remove_file(path).unwrap();
    assert!(!run(&root, &["open"]).status.success());
}

#[test]
fn concurrent_commands_serialize_or_refuse_without_losing_a_commit() {
    let (_temp, root) = fixture();
    let children = (0..2)
        .map(|i| {
            Command::new(binary())
                .args([
                    "--workspace",
                    root.to_str().unwrap(),
                    "add",
                    &format!("p.{i}"),
                    "--value",
                    "1",
                    "--source",
                    "fixture",
                    "--operation",
                    &format!("op-{i}"),
                    "--on",
                    "2026-09-19",
                ])
                .env("PATH", "")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let results = children
        .into_iter()
        .map(|c| c.wait_with_output().unwrap())
        .collect::<Vec<_>>();
    let passed = results.iter().filter(|r| r.status.success()).count();
    assert!(passed >= 1);
    assert_eq!(ok(run(&root, &["open"]))["commits"], passed);
    for (i, result) in results.into_iter().enumerate() {
        if !result.status.success() {
            ok(write(
                &root,
                "add",
                &format!("p.{i}"),
                "1",
                &format!("op-{i}"),
            ));
        }
    }
    assert_eq!(ok(run(&root, &["open"]))["commits"], 2);
}

#[test]
fn yaml_storage_preserves_exponents_and_yaml_sensitive_text() {
    let (_temp, root) = fixture();
    let cases = [
        ("p.large", "1e+20"),
        ("p.small", "1e-20"),
        ("p.control", r#""\u007f\u0085\u2028\ufeff""#),
    ];
    for (i, (subject, value)) in cases.iter().enumerate() {
        ok(write(&root, "add", subject, value, &format!("source-{i}")));
        let result = ok(run(&root, &["open"]));
        let expected = kpop_native::store::json_input(value.as_bytes()).unwrap();
        assert_eq!(
            kpop_native::identity::identity(&result["document"]["readings"][subject]["v"]).unwrap(),
            kpop_native::identity::identity(&expected).unwrap()
        );
    }
}
