use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const RECORD: &str = "meta:\n  name: Session\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  p.a: {v: 12, note: 'שלום 🌱'}\njudgments:\n  d.keep: {verdict: Keep, rests_on: [p.a], seen: {p.a: 12}, wrong_if: 'p.a > 20'}\n";

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), RECORD).unwrap();
    #[cfg(unix)]
    {
        let git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|p| p.join("git"))
            .find(|p| p.is_file())
            .expect("Git is required for workspace discovery");
        fs::create_dir(root.path().join("tools")).unwrap();
        std::os::unix::fs::symlink(git, root.path().join("tools/git")).unwrap();
    }
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(root.path().join("resources/reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.zip")),
        root.path()
            .join("resources/reasoning")
            .join(format!("{target}.zip")),
    )
    .unwrap();
    root
}
fn command(root: &Path, operation: &str, runtime: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
    #[cfg(unix)]
    command.env("PATH", root.join("tools"));
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
        ])
        .env(
            "KPOPPER_NATIVE_RESOURCES",
            root.join(if runtime {
                "resources"
            } else {
                "unavailable-runtime"
            }),
        )
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"));
    command
}
fn ok(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn saved(root: &Path) -> (String, PathBuf) {
    let path = fs::read_dir(root.join("state"))
        .unwrap()
        .map(|p| p.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("core-context-")
        })
        .unwrap();
    let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    (value["revision"].as_str().unwrap().into(), path)
}

#[test]
fn cli_reopens_retained_findings_without_runtime_and_rejects_stale_source() {
    let temp = fixture();
    let root = temp.path();
    let opened = ok(command(root, "open", true).output().unwrap());
    let (revision, context) = saved(root);
    assert!(opened.contains(&revision));
    let held = fs::read(context).unwrap();
    let read = ok(command(root, "read", false)
        .args(["--ref", "/", "--revision", &revision])
        .output()
        .unwrap());
    assert!(read.contains("p.a"));
    assert!(read.contains(&revision));
    assert_eq!(fs::read(saved(root).1).unwrap(), held);
    fs::write(
        root.join("GROUNDING.yaml"),
        RECORD.replace("v: 12", "v: 13"),
    )
    .unwrap();
    let stale = command(root, "read", false)
        .args(["--ref", "/", "--revision", &revision])
        .output()
        .unwrap();
    assert_eq!(stale.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("changed; reopen"));
    assert_eq!(fs::read(saved(root).1).unwrap(), held);
}

#[test]
fn mcp_stdio_and_cli_return_the_same_retained_read_without_runtime() {
    let temp = fixture();
    let root = temp.path();
    ok(command(root, "open", true).output().unwrap());
    let (revision, _) = saved(root);
    let cli = ok(command(root, "read", false)
        .args(["--ref", "/", "--revision", &revision])
        .output()
        .unwrap());
    let context = ok(command(root, "context", false)
        .args([
            "--id",
            "d.keep",
            "--direction",
            "support",
            "--tokens",
            "8000",
            "--revision",
            &revision,
        ])
        .output()
        .unwrap());
    let packet: Value = serde_json::from_str(&context).unwrap();
    assert_eq!(packet["reads"].as_array().unwrap().len(), 2);
    let input = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":"list","method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":"read","method":"tools/call","params":{"name":"kpopper_read","arguments":{"ref":"/","revision":revision}}}),
        json!({"jsonrpc":"2.0","id":"context","method":"tools/call","params":{"name":"kpopper_context","arguments":{"ids":["d.keep"],"direction":"support","tokens":8000,"revision":revision}}}),
    ].into_iter().map(|v| v.to_string()+"\n").collect::<String>();
    let mut child = command(root, "serve", false)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(messages[1]["result"]["tools"].as_array().unwrap().len(), 3);
    assert_eq!(messages[2]["id"], "read");
    assert_eq!(messages[2]["result"]["isError"], false);
    assert_eq!(messages[3]["result"]["isError"], false);
    assert_eq!(
        messages[3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
        context
    );
    assert_eq!(
        messages[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .trim_end_matches('\n'),
        cli.trim_end_matches('\n')
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        RECORD
    );
}
