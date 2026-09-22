use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn both_public_names_report_the_native_version() {
    for binary in [env!("CARGO_BIN_EXE_kpop"), env!("CARGO_BIN_EXE_kpopper")] {
        let result = Command::new(binary).arg("--version").output().unwrap();
        assert!(result.status.success());
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            format!("kpop {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn long_name_forwards_stdin_and_binds_the_canonical_executable() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kpopper"))
        .args(["session-start", "--host", "codex"])
        .current_dir(directory.path())
        .env("XDG_STATE_HOME", directory.path().join("state"))
        .env("KPOPPER_PRIVATE_HOME", directory.path().join("private"))
        .env("TMPDIR", directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = json!({"session_id":"native-alias", "cwd":directory.path()});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let text = String::from_utf8(result.stdout).unwrap();
    let context: Value = serde_json::from_str(
        text.lines()
            .find_map(|line| line.strip_prefix("KPOPPER_AGENT_CONTEXT "))
            .expect("native opener context"),
    )
    .unwrap();
    assert_eq!(
        context["command"],
        json!([
            Path::new(env!("CARGO_BIN_EXE_kpop"))
                .canonicalize()
                .unwrap(),
            "--workspace",
            directory.path().canonicalize().unwrap()
        ])
    );
    assert!(!directory.path().join("GROUNDING.yaml").exists());
}
