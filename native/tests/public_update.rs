use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn record(path: &Path) {
    fs::write(
        path.join("GROUNDING.yaml"),
        r#"meta:
  updated: 2026-09-01
sources:
  s.old:
    url: https://example.test/old
    read: 2026-09-01
known:
  p.price:
    v: 10
    from: s.old
    of: 2026-09-01
judgments:
  d.price:
    rests_on: [p.price]
    verdict: price is acceptable
    wrong_if: p.price > 11
    seen: {p.price: 10}
"#,
    )
    .unwrap();
}

fn run(root: &Path, state: &Path, report: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "update",
            "--file",
            "-",
            "--state-dir",
            state.to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(report).unwrap().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn hash(path: &Path) -> String {
    kpop_native::identity::sha256(&fs::read(path).unwrap())
}

#[test]
fn source_and_two_dependent_writes_publish_once_and_retry_exactly() {
    let temp = tempfile::tempdir().unwrap();
    record(temp.path());
    let entry = temp.path().join("GROUNDING.yaml");
    let state = temp.path().join("state");
    let report = json!({
        "event_id":"batch-one", "record_sha256":hash(&entry), "date":"2026-09-20", "source_quote":"price 12, fee 3",
        "updates":[
            {"kind":"set","id":"p.price","value":12},
            {"kind":"add","id":"p.fee","body":{"v":3}},
            {"kind":"add","id":"p.total","body":{"rule":"p.price + p.fee"}}
        ]
    });
    let first = run(temp.path(), &state, &report);
    assert!(
        first.status.success(),
        "{}{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let receipt: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(receipt["state"], "applied");
    assert_eq!(receipt["newly_fired_judgments"], json!(["d.price"]));
    assert_eq!(receipt["reach"]["judgments"], json!(["d.price"]));
    assert!(
        receipt["diagnostics"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
    );
    assert_eq!(receipt["target_after_sha256"].as_str().unwrap().len(), 64);
    assert!(
        state
            .join("signals")
            .join(format!(
                "{}.json",
                receipt["signal_ids"][0].as_str().unwrap()
            ))
            .is_file()
    );
    assert!(receipt["mutation"]["receipt"]["before"]["batch"].is_object());
    let after = fs::read(&entry).unwrap();
    let text = String::from_utf8(after.clone()).unwrap();
    assert!(text.contains("p.fee:"));
    assert!(text.contains("p.total:"));
    assert!(text.contains("v: 12"));
    let again = run(temp.path(), &state, &report);
    assert!(again.status.success());
    assert_eq!(again.stdout, first.stdout);
    assert_eq!(fs::read(entry).unwrap(), after);
}

#[test]
fn late_failure_and_private_report_retain_source_without_shared_changes() {
    for (name, extra, last) in [
        (
            "late",
            json!({}),
            json!({"kind":"add","id":"c.bad","body":{"rests_on":["p.missing"],"verdict":"bad","wrong_if":"p.missing > 0"}}),
        ),
        (
            "private",
            json!({"privacy":"private"}),
            json!({"kind":"add","id":"p.ok","body":{"v":2}}),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        record(temp.path());
        let entry = temp.path().join("GROUNDING.yaml");
        let before = fs::read(&entry).unwrap();
        let state = temp.path().join("state");
        let mut report = json!({"event_id":name,"record_sha256":hash(&entry),"date":"2026-09-20","source_quote":"retained","updates":[{"kind":"set","id":"p.price","value":12},last]});
        report
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let output = run(temp.path(), &state, &report);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt["state"], "needs_primary");
        assert_eq!(fs::read(&entry).unwrap(), before);
        assert_eq!(
            fs::read(receipt["source_file"].as_str().unwrap()).unwrap(),
            b"retained"
        );
    }
}

#[test]
fn stale_primary_hash_retains_report_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    record(temp.path());
    let entry = temp.path().join("GROUNDING.yaml");
    let before = fs::read(&entry).unwrap();
    let output = run(
        temp.path(),
        &temp.path().join("state"),
        &json!({
            "event_id":"stale", "record_sha256":"0000000000000000000000000000000000000000000000000000000000000000",
            "date":"2026-09-20", "source_quote":"new fact", "updates":[{"kind":"add","id":"p.new","body":{"v":2}}]
        }),
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read(entry).unwrap(), before);
}

#[test]
fn private_dependency_closure_is_retained_without_a_shared_write() {
    let temp = tempfile::tempdir().unwrap();
    record(temp.path());
    let entry = temp.path().join("GROUNDING.yaml");
    let private = fs::read_to_string(&entry).unwrap().replace(
        "    read: 2026-09-01\nknown:",
        "    read: 2026-09-01\n    private: true\nknown:",
    );
    fs::write(&entry, private).unwrap();
    let before = fs::read(&entry).unwrap();
    let output = run(
        temp.path(),
        &temp.path().join("state"),
        &json!({
            "event_id":"private-closure", "date":"2026-09-20", "source_quote":"price 12",
            "updates":[{"kind":"set","id":"p.price","value":12}]
        }),
    );
    assert_eq!(output.status.code(), Some(1));
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(receipt["reason"].as_str().unwrap().contains("private"));
    assert_eq!(fs::read(entry).unwrap(), before);
}

#[test]
fn private_shareability_and_nested_permissions_never_enter_shared_record() {
    for marker in [
        json!({"shareability":"private"}),
        json!({"shareability":"unclear"}),
        json!({"shareability":null}),
        json!({"scope":{"kind":"project","environment":"fixture","permissions":{"privacy":"private"}}}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        record(temp.path());
        let entry = temp.path().join("GROUNDING.yaml");
        let before = fs::read(&entry).unwrap();
        let state = temp.path().join("state");
        let mut report = json!({"event_id":"private-envelope", "date":"2026-09-20", "source_quote":"private fixture evidence", "target":"p.price", "value":12});
        report
            .as_object_mut()
            .unwrap()
            .extend(marker.as_object().unwrap().clone());
        let output = run(temp.path(), &state, &report);
        assert_eq!(output.status.code(), Some(1));
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt["state"], "needs_primary");
        assert_eq!(fs::read(&entry).unwrap(), before);
        assert!(
            fs::read_dir(state.join("journals"))
                .unwrap()
                .next()
                .is_none()
        );
        assert_eq!(
            fs::read(receipt["source_file"].as_str().unwrap()).unwrap(),
            b"private fixture evidence"
        );
    }
}
