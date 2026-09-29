use std::process::Command;
use std::io::Write;
use std::process::Stdio;

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
