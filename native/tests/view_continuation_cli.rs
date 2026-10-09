use std::process::Command;
use std::io::Write;
use std::process::Stdio;
use std::fs;

#[test]
fn session_view_advertises_managed_continuation_controls() {
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(["session", "view", "--help"])
        .output()
        .expect("run kpop help");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--context-session"), "{help}");
    assert!(help.contains("--view-transport"), "{help}");
    assert!(help.contains("--no-auto-anchors"), "{help}");
}

#[test]
fn managed_continuation_controls_are_rejected_for_open() {
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(["session", "open", "--context-session", "probe"])
        .output()
        .expect("run kpop");
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("only supported by session view"), "{error}");
}

#[test]
fn inactive_continuation_hooks_are_silent_and_errors_never_panic() {
    let root = tempfile::tempdir().unwrap();
    for (session, diagnostic) in [("unused-continuation-session", false), ("", true)] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .args(["_hook", "continuation", "codex"])
            .current_dir(root.path())
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().unwrap();
        let payload = serde_json::json!({"session_id":session,"hook_event_name":"Stop","cwd":root.path()});
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!error.contains("panicked"));
        assert_eq!(error.contains("context continuation unavailable"), diagnostic, "{error}");
    }
}

#[test]
fn unmanaged_or_disabled_hook_ignores_broken_project_config_without_locating_workspace() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join(".kpopper")).unwrap();
    fs::write(root.path().join(".kpopper/project.json"), "{not json").unwrap();
    for (session, disabled) in [
        (format!("unmanaged-no-state-{}", std::process::id()), false),
        (format!("disabled-no-locate-{}", std::process::id()), true),
    ] {
        if disabled {
            kpop_native::view_continuation::initialize(root.path(), &session, false).unwrap();
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .args(["_hook", "continuation", "codex"])
            .current_dir(root.path())
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().unwrap();
        let payload = serde_json::json!({
            "session_id":session,
            "hook_event_name":"PostToolUse",
            "cwd":root.path(),
            "tool_response":{"output":"no continuation marker"}
        });
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty(), "hook inspected project config: {}", String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
fn malformed_private_continuation_state_is_reported_by_the_dedicated_hook() {
    let root = tempfile::tempdir().unwrap();
    let session = format!("malformed-state-{}", std::process::id());
    kpop_native::view_continuation::initialize(root.path(), &session, true).unwrap();
    let state = kpop_native::session_activity::temporary_directory()
        .join(format!("kpopper-view-{session}"))
        .join("state.json");
    fs::write(state, "{broken").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(["_hook", "continuation", "codex"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    let payload = serde_json::json!({
        "session_id":session,
        "hook_event_name":"PostToolUse",
        "cwd":root.path(),
        "tool_response":{"output":"KPOPPER_CONTEXT_QUEUED {}"}
    });
    child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("kpopper context continuation unavailable"));
}
