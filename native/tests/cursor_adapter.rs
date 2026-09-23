//! The Cursor project adapter opens the record through the checkout's native runtime.
#![cfg(unix)]

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("linux", "x86_64") => "linux-x86_64",
        (os, arch) => panic!("no native target for {os} {arch}"),
    }
}

/// A checkout holding the adapter and the runtime resolver, with this build as its runtime.
fn checkout(root: &Path, runtime: bool) -> PathBuf {
    for file in [
        "scripts/native_runtime.sh",
        "adapters/cursor/scripts/gate-open.sh",
        "VERSION",
    ] {
        let to = root.join(file);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(repo().join(file), &to).unwrap();
    }
    if runtime {
        let binary = root.join("scripts/runtime").join(target()).join("kpop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        symlink(env!("CARGO_BIN_EXE_kpop"), binary).unwrap();
    }
    root.join("adapters/cursor/scripts/gate-open.sh")
}

fn open(script: &Path, cwd: &Path, payload: &Value, tmp: &Path, runtime: Option<&str>) -> Output {
    let mut command = Command::new("sh");
    command
        .arg(script)
        .current_dir(cwd)
        .env("TMPDIR", tmp)
        .env("XDG_STATE_HOME", tmp)
        .env_remove("KPOPPER_ROOT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match runtime {
        Some(value) => command.env("KPOPPER_RUNTIME", value),
        None => command.env_remove("KPOPPER_RUNTIME"),
    };
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn context(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)));
    value["additional_context"].as_str().unwrap().to_owned()
}

#[test]
fn a_symlinked_opener_gives_first_use_guidance_and_creates_no_record() {
    let root = tempfile::tempdir().unwrap();
    checkout(root.path(), true);
    let project = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let scripts = project.path().join(".cursor/scripts");
    fs::create_dir_all(&scripts).unwrap();
    symlink(
        root.path().join("adapters/cursor/scripts/gate-open.sh"),
        scripts.join("gate-open.sh"),
    )
    .unwrap();
    let opened = open(
        Path::new(".cursor/scripts/gate-open.sh"),
        project.path(),
        &json!({"conversation_id": "first"}),
        tmp.path(),
        None,
    );
    assert!(
        context(&opened).starts_with("KPOPPER_START"),
        "{}",
        context(&opened)
    );
    let entries: Vec<_> = fs::read_dir(project.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, [".cursor"]);
    assert!(tmp.path().join("kpopper-base-cursor-first").is_file());
}

#[test]
fn an_existing_record_opens_in_additional_context() {
    let root = tempfile::tempdir().unwrap();
    let script = checkout(root.path(), true);
    let work = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        work.path().join("GROUNDING.yaml"),
        "known:\n  p.sample: {v: 1}\n",
    )
    .unwrap();
    let text = context(&open(
        &script,
        root.path(),
        &json!({"conversation_id": "record", "cwd": work.path()}),
        tmp.path(),
        None,
    ));
    assert!(text.starts_with("1 entries, 0 judgments"), "{text}");
    assert!(text.contains("KPOPPER_AGENT_CONTEXT"), "{text}");
}

#[test]
fn a_missing_runtime_is_reported_to_the_agent_with_its_install_command() {
    let root = tempfile::tempdir().unwrap();
    let script = checkout(root.path(), false);
    let tmp = tempfile::tempdir().unwrap();
    let text = context(&open(
        &script,
        tmp.path(),
        &json!({"conversation_id": "missing", "cwd": tmp.path()}),
        tmp.path(),
        None,
    ));
    assert!(text.contains("native runtime is not installed"), "{text}");
    assert!(
        text.contains(&format!(
            "Install this active copy: sh \"{}/install_native.sh\"",
            fs::canonicalize(root.path())
                .unwrap()
                .join("scripts")
                .display()
        )),
        "{text}"
    );
    assert!(
        text.contains("its hooks never download or install the runtime"),
        "{text}"
    );
    assert!(text.contains("Offer to run the command above"), "{text}");
}

#[test]
fn an_unknown_runtime_choice_stays_silent_on_standard_output() {
    let root = tempfile::tempdir().unwrap();
    let script = checkout(root.path(), true);
    let tmp = tempfile::tempdir().unwrap();
    let output = open(
        &script,
        tmp.path(),
        &json!({"conversation_id": "unknown", "cwd": tmp.path()}),
        tmp.path(),
        Some("java"),
    );
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("KPOPPER_RUNTIME must be rust or python")
    );
}
