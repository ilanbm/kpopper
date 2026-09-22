use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn run(root: &Path, resources: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .args(["--workspace", root.to_str().unwrap(), "session"])
        .args(args)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
        .env_remove("KPOPPER_SESSION_CONFIG")
        .env_remove("KPOPPER_SESSION_DISABLE");
    if let Some(resources) = resources {
        command.env("KPOPPER_NATIVE_RESOURCES", resources);
    } else {
        command.env("KPOPPER_NATIVE_RESOURCES", root.join("missing-resources"));
    }
    command.output().unwrap()
}
fn packet(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn resources(root: &Path) -> PathBuf {
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let resources = root.join("resources");
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        resources
            .join("reasoning")
            .join(format!("{target}.kpopper-runtime")),
    )
    .unwrap();
    let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean")
                .join(&target)
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    let destination = resources.join("ordinary").join(target);
    fs::create_dir_all(&destination).unwrap();
    for name in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(program.join(name), destination.join(name)).unwrap();
    }
    resources
}

#[test]
fn missing_runtime_status_is_read_only_and_disable_preserves_legacy_preferences() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let legacy = root.join("config/kpopper/session.json");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let legacy_bytes = b"{\"schema\":1,\"enabled\":false}\n";
    fs::write(&legacy, legacy_bytes).unwrap();
    let status = run(&root, None, &["status"]);
    assert_eq!(status.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&status.stdout).unwrap()["ready"],
        false
    );
    assert!(!root.join("cache").exists());
    let disabled = packet(run(&root, None, &["disable", "--global"]));
    assert_eq!(disabled["enabled"], false);
    assert_eq!(fs::read(&legacy).unwrap(), legacy_bytes);
    let path = PathBuf::from(disabled["settings"].as_str().unwrap());
    assert_eq!(path, root.join("config/kpopper/native-session.json"));
    fs::write(&path, b"foreign config").unwrap();
    assert_eq!(
        run(&root, None, &["disable", "--global"]).status.code(),
        Some(2)
    );
    assert_eq!(fs::read(path).unwrap(), b"foreign config");
}

#[test]
fn verified_programs_enable_native_preferences_and_existing_legacy_settings_are_read() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let resources = resources(&root);
    let status = packet(run(&root, Some(&resources), &["status"]));
    assert_eq!(status["ready"], true);
    assert!(!root.join("cache").exists());
    let enabled = packet(run(
        &root,
        Some(&resources),
        &["enable", "--tokens", "1200"],
    ));
    assert_eq!(enabled["tokens"], 1200);
    assert!(
        enabled["native"]
            .as_str()
            .is_some_and(|s| Path::new(s).is_absolute())
    );
    assert!(enabled.get("python").is_none());
    let path = PathBuf::from(enabled["settings"].as_str().unwrap());
    let before = fs::read(&path).unwrap();
    assert_eq!(
        run(&root, Some(&resources), &["enable", "--tokens", "63"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    packet(run(&root, Some(&resources), &["setup"]));
    assert!(root.join("cache").is_dir());
    let cached = PathBuf::from(status["runtime_cache"].as_str().unwrap());
    let binary = cached.join(status["core"]["manifest"]["executable"].as_str().unwrap());
    fs::write(&binary, b"corrupt cache").unwrap();
    assert_eq!(
        run(&root, Some(&resources), &["status"]).status.code(),
        Some(1)
    );
    let repaired = packet(run(&root, Some(&resources), &["setup", "--rebuild"]));
    assert!(Path::new(repaired["previous_cache"].as_str().unwrap()).is_dir());
    assert_ne!(fs::read(binary).unwrap(), b"corrupt cache");
    fs::remove_file(path).unwrap();
    let legacy = root.join("config/kpopper/session.json");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    let selected = root.join("legacy-selected-state");
    fs::write(&legacy, serde_json::to_vec(&json!({"schema":1,"enabled":true,"python":root.join("absent-python"),"tokens":1000,"state":selected,"project":"fixture"})).unwrap()).unwrap();
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.a: {v: 1}\n").unwrap();
    let opened = run(&root, Some(&resources), &["open"]);
    assert!(
        opened.status.success(),
        "{}",
        String::from_utf8_lossy(&opened.stderr)
    );
    assert!(selected.is_dir());
}

#[test]
fn enabled_host_open_uses_checked_native_route_and_keeps_failures_explicit() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let resources = resources(&root);
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.a: {v: 1}\n").unwrap();
    packet(run(
        &root,
        Some(&resources),
        &["enable", "--tokens", "1200"],
    ));
    let hook = |available: bool| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .arg("session-start")
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
            .env(
                "KPOPPER_NATIVE_RESOURCES",
                if available {
                    resources.clone()
                } else {
                    root.join("missing-resources")
                },
            )
            .env_remove("KPOPPER_SESSION_CONFIG")
            .env_remove("KPOPPER_SESSION_DISABLE")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_vec(&json!({"cwd":root})).unwrap().as_slice())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let opened = hook(true);
    assert!(opened.status.success());
    let output = String::from_utf8(opened.stdout).unwrap();
    assert!(output.contains("Read via MCP kpopper_read"), "{output}");
    assert!(output.contains("KPOPPER_AGENT_CONTEXT"));
    let failed = hook(false);
    assert!(failed.status.success(), "host startup must not be blocked");
    assert!(
        String::from_utf8(failed.stdout)
            .unwrap()
            .contains("Checked session view unavailable")
    );
    assert!(!failed.stderr.is_empty());

    let external = tempfile::tempdir().unwrap();
    let external = external.path().canonicalize().unwrap();
    let external_record = external.join("GROUNDING.yaml");
    fs::write(&external_record, "known:\n  p.a: {v: 1}\n").unwrap();
    fs::create_dir_all(root.join(".kpopper")).unwrap();
    fs::write(
        root.join(".kpopper/project.json"),
        serde_json::to_vec(
            &json!({"version":1,"mode":"simple","generation":1,"record":external_record}),
        )
        .unwrap(),
    )
    .unwrap();
    let foreign_state = root.join("foreign-selected-state");
    let foreign_config = root.join("config/kpopper/native-projects").join(format!(
        "{}.json",
        kpop_native::identity::sha256(external.to_string_lossy().as_bytes())
    ));
    fs::create_dir_all(foreign_config.parent().unwrap()).unwrap();
    fs::write(&foreign_config, serde_json::to_vec(&json!({"schema":1,"enabled":true,"native":root.join("wrong-program"),"tokens":1200,"state":foreign_state})).unwrap()).unwrap();
    let external_open = hook(true);
    assert!(external_open.status.success());
    assert!(
        String::from_utf8(external_open.stdout)
            .unwrap()
            .contains("Read via MCP kpopper_read")
    );
    assert!(
        !foreign_state.exists(),
        "an external record must not redirect the host configuration"
    );
}

#[test]
fn cursor_start_returns_host_guidance_without_creating_a_record() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let private_tmp = root.join("tmp");
    fs::create_dir(&private_tmp).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(["session-start", "--cursor", "--host", "codex"])
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("TMPDIR", &private_tmp)
        .env_remove("KPOPPER_SESSION_CONFIG")
        .env_remove("KPOPPER_SESSION_DISABLE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::to_vec(&json!({"cwd":root,"conversation_id":"fixture"}))
                .unwrap()
                .as_slice(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let packet: Value = serde_json::from_slice(&output.stdout).unwrap();
    let text = packet["additional_context"].as_str().unwrap();
    assert!(text.contains("$record keeps findings"), "{text}");
    assert!(text.contains("cursor-fixture"));
    assert!(!root.join("GROUNDING.yaml").exists());
    let baseline: Value =
        serde_json::from_slice(&fs::read(private_tmp.join("kpopper-base-cursor-fixture")).unwrap())
            .unwrap();
    assert_eq!(baseline["fails"], 0);
}
