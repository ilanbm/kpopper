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
            .join(format!("{target}.kpopper-runtime")),
        root.path()
            .join("resources/reasoning")
            .join(format!("{target}.zip")),
    )
    .unwrap();
    root
}
fn command(root: &Path, operation: &str, runtime: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    #[cfg(unix)]
    command.env("PATH", root.join("tools"));
    command.args(["--workspace", root.to_str().unwrap(), "--frozen"]);
    if operation == "current-context" {
        command.arg("context");
    } else {
        command.args(["session", operation]);
    }
    command
        .args([
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
fn unknown_core_session_revision_tells_the_reader_to_reopen() {
    let temp = fixture();
    let root = temp.path();
    fs::remove_file(root.join("GROUNDING.yaml")).unwrap();
    ok(Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["init", "-q"])
        .output()
        .unwrap());
    ok(Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(["add", "p.start", "v=1"])
        .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
        .env("XDG_STATE_HOME", root.join("private-state"))
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .output()
        .unwrap());
    ok(command(root, "open", true).output().unwrap());
    let (revision, retained) = saved(root);
    let before = fs::read(&retained).unwrap();
    let output = command(root, "read", false)
        .args(["--ref", "/", "--revision", &"0".repeat(64)])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap(),
        json!({"error": "unknown core session revision; reopen"})
    );
    ok(command(root, "read", false)
        .args(["--ref", "/", "--revision", &revision])
        .output()
        .unwrap());
    assert_eq!(fs::read(retained).unwrap(), before);
}

#[test]
fn public_context_captures_current_record_and_matches_legacy_route() {
    let temp = fixture();
    let root = temp.path();
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let direct = ok(command(root, "current-context", true)
        .arg("d.keep")
        .output()
        .unwrap());
    let packet: Value = serde_json::from_str(&direct).unwrap();
    assert_eq!(packet["direction"], "support");
    let revision = packet["revision"].as_str().unwrap();
    let old = ok(command(root, "context", false)
        .args([
            "--id",
            "d.keep",
            "--direction",
            "support",
            "--revision",
            revision,
        ])
        .output()
        .unwrap());
    assert_eq!(direct, old);
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    let names: Vec<_> = packet["reads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"p.a") && names.contains(&"d.keep"));
}

#[test]
fn public_context_preserves_explicit_revision_and_limits() {
    let temp = fixture();
    let root = temp.path();
    let direct = ok(command(root, "current-context", true)
        .args([
            "node:p.a",
            "--direction",
            "impact",
            "--max-nodes",
            "1",
            "--tokens",
            "1000",
        ])
        .output()
        .unwrap());
    let packet: Value = serde_json::from_str(&direct).unwrap();
    assert!(kpop_native::tokenizer::Encoding::O200kBase.count(&direct) <= 1000);
    assert_eq!(packet["candidate_limit_reached"], true);
    assert!(!packet["frontier"].as_array().unwrap().is_empty());
    let revision = packet["revision"].as_str().unwrap();
    fs::write(
        root.join("GROUNDING.yaml"),
        RECORD.replace("v: 12", "v: 13"),
    )
    .unwrap();
    let stale = command(root, "current-context", false)
        .args(["p.a", "--revision", revision])
        .output()
        .unwrap();
    assert_eq!(stale.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("changed; reopen"));
    let current = ok(command(root, "current-context", true)
        .arg("p.a")
        .output()
        .unwrap());
    let current: Value = serde_json::from_str(&current).unwrap();
    assert_ne!(current["revision"], revision);
}

#[test]
fn public_context_help_needs_no_record_or_runtime() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args([
            "--workspace",
            root.path().to_str().unwrap(),
            "context",
            "--help",
        ])
        .output()
        .unwrap();
    let text = ok(output);
    assert!(text.contains("declared dependencies") && text.contains("--direction"));
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn hook_open_uses_explicit_profile_path_and_stays_within_budget() {
    let temp = fixture();
    let root = temp.path();
    let profile_a = root.join("profile-a.json");
    let profile_b = root.join("profile with space.json");
    fs::write(&profile_a, br#"{"groups":{}}"#).unwrap();
    fs::write(&profile_b, br#"{"groups":{}}"#).unwrap();
    let budget = 1000usize;
    let profile_b = profile_b.canonicalize().unwrap();
    let output = ok(command(root, "hook-open", true)
        .args(["--profile", profile_b.to_str().unwrap(), "--tokens", "1000"])
        .output()
        .unwrap());
    assert!(kpop_native::tokenizer::Encoding::O200kBase.count(&output) <= budget);
    assert!(output.contains(&format!("--profile '{}'", profile_b.display())));
    assert!(!output.contains(&profile_a.to_string_lossy().to_string()));
    let revision = output
        .split("revision=")
        .nth(1)
        .and_then(|tail| tail.split_whitespace().next())
        .unwrap();
    let read = ok(command(root, "read", false)
        .args([
            "--profile",
            profile_b.to_str().unwrap(),
            "--ref",
            "node:p.a",
            "--revision",
            revision,
        ])
        .output()
        .unwrap());
    assert!(read.contains(revision));
}

#[test]
fn hook_open_refuses_budget_that_cannot_carry_route() {
    let temp = fixture();
    let output = command(temp.path(), "hook-open", true)
        .args(["--tokens", "32"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("hook budget cannot carry"));
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
fn configured_missing_e5_assets_fail_closed_to_complete_lexical_search() {
    let temp = fixture();
    let root = temp.path();
    ok(command(root, "open", true).output().unwrap());
    let (revision, _) = saved(root);
    let output = ok(command(root, "search", false)
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .args([
            "--revision",
            &revision,
            "--query",
            "שלום",
            "--search-mode",
            "semantic",
            "--embedding-dir",
            "missing-e5-assets",
        ])
        .output()
        .unwrap());
    let packet: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(packet["requested_mode"], "semantic");
    assert_eq!(packet["backend"], "lexical");
    assert!(
        packet["fallback"]
            .as_str()
            .unwrap()
            .contains("model assets are unavailable")
    );
    assert!(
        !packet["fallback"]
            .as_str()
            .unwrap()
            .contains(root.to_str().unwrap())
    );
    assert!(
        packet["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hit| hit["id"] == "p.a")
    );
}

#[test]
fn tilde_e5_path_degrades_only_semantic_search() {
    let temp = fixture();
    let root = temp.path();
    ok(command(root, "open", true).output().unwrap());
    let (revision, _) = saved(root);
    let output = ok(command(root, "search", false)
        .args([
            "--revision",
            &revision,
            "--query",
            "שלום",
            "--search-mode",
            "semantic",
            "--embedding-dir",
            "~",
        ])
        .output()
        .unwrap());
    let packet: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(packet["backend"], "lexical");
    assert!(packet["fallback"].as_str().unwrap().contains("unavailable"));
    assert!(
        !packet["fallback"]
            .as_str()
            .unwrap()
            .contains(root.to_str().unwrap())
    );
    assert!(
        packet["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hit| hit["id"] == "p.a")
    );
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
    let search = ok(command(root, "search", false)
        .args([
            "--query",
            "p.a",
            "--tokens",
            "8000",
            "--revision",
            &revision,
        ])
        .output()
        .unwrap());
    let packet: Value = serde_json::from_str(&search).unwrap();
    assert_eq!(packet["hits"][0]["id"], "p.a");
    let input = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":"list","method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":"read","method":"tools/call","params":{"name":"kpopper_read","arguments":{"ref":"/","revision":revision}}}),
        json!({"jsonrpc":"2.0","id":"context","method":"tools/call","params":{"name":"kpopper_context","arguments":{"ids":["d.keep"],"direction":"support","tokens":8000,"revision":revision}}}),
        json!({"jsonrpc":"2.0","id":"search","method":"tools/call","params":{"name":"kpopper_search","arguments":{"query":"p.a","tokens":8000,"revision":revision}}}),
        json!({"jsonrpc":"2.0","id":"verify","method":"tools/call","params":{"name":"kpopper_verify_claims","arguments":{"judgment":"d.keep","revision":revision,"assertions":[{"kind":"current","id":"p.a","expected":12}]}}}),
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
    assert_eq!(messages.len(), 6);
    assert_eq!(messages[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(messages[1]["result"]["tools"].as_array().unwrap().len(), 6);
    assert_eq!(messages[2]["id"], "read");
    assert_eq!(messages[2]["result"]["isError"], false);
    assert_eq!(messages[3]["result"]["isError"], false);
    assert_eq!(messages[4]["result"]["isError"], false);
    assert_eq!(messages[5]["result"]["isError"], true);
    assert_eq!(
        messages[5]["result"]["content"][0]["text"],
        "unsupported_capability: kpopper_verify_claims belongs to checked-reader/v1; read the bound core finding instead"
    );
    assert_eq!(
        messages[4]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
        search
    );
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

#[test]
fn pending_proposals_are_idempotent_readable_and_stale_after_source_changes() {
    let temp = fixture();
    let root = temp.path();
    ok(command(root, "open", true).output().unwrap());
    let (revision, _) = saved(root);
    let args = [
        "--revision",
        &revision,
        "--kind",
        "inferred",
        "--text",
        "The plan may hold",
        "--basis",
        "node:p.a",
        "--revisit",
        "Recheck when p.a changes",
    ];
    let first = ok(command(root, "propose", false).args(args).output().unwrap());
    let result: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(result["canonical_record_changed"], false);
    let id = result["id"].as_str().unwrap();
    let file = root.join("state").join(format!("proposal-{id}.json"));
    let before = fs::read(&file).unwrap();
    assert_eq!(
        ok(command(root, "propose", false).args(args).output().unwrap()),
        first
    );
    assert_eq!(fs::read(&file).unwrap(), before);
    let input = [serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"kpopper_propose","arguments":{"revision":revision,"kind":"inferred","text":"The plan may hold","basis":["node:p.a"],"revisit":"Recheck when p.a changes"}}})]
        .into_iter().map(|v|v.to_string()+"\n").collect::<String>();
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
    let result: Value = serde_json::from_str(output.lines().last().unwrap()).unwrap();
    assert_eq!(result["result"]["isError"], false);
    assert_eq!(
        result["result"]["content"][0]["text"].as_str().unwrap(),
        first.trim_end_matches('\n')
    );
    assert_eq!(fs::read(&file).unwrap(), before);
    let pending = ok(command(root, "read", false)
        .args([
            "--ref",
            "pending",
            "--revision",
            &revision,
            "--tokens",
            "8000",
        ])
        .output()
        .unwrap());
    let pending: Value = serde_json::from_str(&pending).unwrap();
    assert_eq!(pending["value"][id]["stale_base"], false);
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        RECORD
    );
    fs::write(
        root.join("GROUNDING.yaml"),
        RECORD.replace("v: 12", "v: 13"),
    )
    .unwrap();
    let opened = ok(command(root, "open", true).output().unwrap());
    assert!(opened.contains("pending=1; stale_pending=1"));
    let current = fs::read_dir(root.join("state"))
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("core-context-")
        })
        .map(|p| serde_json::from_slice::<Value>(&fs::read(p).unwrap()).unwrap())
        .find(|v| v["revision"] != revision)
        .unwrap();
    let current = current["revision"].as_str().unwrap();
    let stale = ok(command(root, "read", false)
        .args([
            "--ref",
            &format!("proposal:{id}#/stale_base"),
            "--revision",
            current,
        ])
        .output()
        .unwrap());
    assert_eq!(
        serde_json::from_str::<Value>(&stale).unwrap()["value"],
        true
    );
    assert_eq!(fs::read(&file).unwrap(), before);
}
