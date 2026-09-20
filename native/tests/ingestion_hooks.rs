use kpop_native::ingestion_hooks::{Options, Output, run};
use serde_json::json;
use std::path::Path;

#[test]
fn real_pending_findings_are_offered_once_per_epoch_in_bounded_batches() {
    use kpop_native::ingestion_orchestration as ingestion;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let record = root.join("GROUNDING.yaml");
    let state = root.join("state");
    std::fs::write(&record, "known:\n  p.value: {v: 1}\n").unwrap();
    for index in 0..9 {
        let envelope = serde_json::to_vec(&json!({
            "event_id": format!("event-{index}"), "kind":"report", "date":"2026-09-20",
            "source_quote":"Captured fixture report", "target":format!("p.missing{index}"), "value":4
        })).unwrap();
        ingestion::capture(&envelope, Some(&record), Some(&state), root, false).unwrap();
    }
    let receipts = ingestion::process(Some(&record), Some(&state), root, None, 32).unwrap();
    assert_eq!(receipts.len(), 9);
    assert!(receipts.iter().all(|r| r["state"] == "needs_primary"));
    let options = Options { host:"codex".into(), mode:"start".into(),
        record:Some(record), state_dir:Some("state".into()), wait_seconds:0.0 };
    let first = run(&options, json!({"session_id":"fixture","agent_id":null}), root).unwrap();
    assert_eq!(first.code, 0);
    let packet: serde_json::Value = serde_json::from_str(&first.stdout).unwrap();
    let context = packet["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
    assert!(context.contains("More findings remain"));
    let notices: serde_json::Value = serde_json::from_str(context.lines().next().unwrap()
        .strip_prefix("KPOPPER_ATTENTION ").unwrap()).unwrap();
    assert_eq!(notices.as_array().unwrap().len(), 8);
    let second = run(&options, json!({"session_id":"fixture","source":"compact"}), root).unwrap();
    assert!(second.stdout.contains("KPOPPER_ATTENTION"));
    assert!(!second.stdout.contains("More findings remain"));
    let third = run(&options, json!({"session_id":"fixture","source":"compact"}), root).unwrap();
    assert!(third.stdout.is_empty() && third.stderr.is_empty());
    let resumed = run(&options, json!({"session_id":"fixture","source":"resume"}), root).unwrap();
    assert!(resumed.stdout.contains("More findings remain"));
}

#[test]
fn native_cli_hook_consumes_payload_cwd_without_python() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let temp = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args(["ingestion-hook", "codex", "start"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    child.stdin.take().unwrap().write_all(serde_json::to_string(&json!({
        "session_id":"fixture", "cwd":temp.path()
    })).unwrap().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
}

#[test]
fn malformed_or_child_payloads_are_silent_success() {
    let options = Options {
        host: "claude".into(),
        mode: "start".into(),
        record: None,
        state_dir: None,
        wait_seconds: 0.0,
    };
    let silent = Output { stdout: String::new(), stderr: String::new(), code: 0 };
    assert_eq!(run(&options, json!(null), Path::new(".")).unwrap(), silent);
    assert_eq!(run(&options, json!({"session_id":"s","agent_id":"child"}), Path::new(".")).unwrap(), silent);
}
