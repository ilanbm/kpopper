use kpop_native::host_hooks::run;
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use tempfile::TempDir;
fn sid(label: &str) -> String {
    format!(
        "{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}
fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "PROVENANCE.yaml",
        "PROVENANCE.view.yaml",
        "PROVENANCE.measure.yaml",
    ] {
        fs::copy(
            repo().join("tests/fixtures/page").join(name),
            dir.path().join(name),
        )
        .unwrap();
    }
    dir
}
fn process(
    program: &Path,
    args: &[&str],
    payload: &serde_json::Value,
    cwd: &Path,
    tmp: &Path,
) -> Output {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("TMPDIR", tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(payload).unwrap().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
fn raw_process(program: &Path, args: &[&str], raw: &[u8], cwd: &Path) -> Output {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(raw).unwrap();
    child.wait_with_output().unwrap()
}
fn native_command(args: &[&str], cwd: &Path, state: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args(args)
        .current_dir(cwd)
        .env("XDG_STATE_HOME", state)
        .output()
        .unwrap()
}
fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn context(output: &Output) -> String {
    if output.stdout.is_empty() {
        return String::new();
    }
    serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_owned()
}
fn python(
    script: &str,
    args: &[&str],
    payload: &serde_json::Value,
    cwd: &Path,
    tmp: &Path,
) -> Output {
    let mut all = vec![
        repo()
            .join("scripts")
            .join(script)
            .to_string_lossy()
            .into_owned(),
    ];
    all.extend(args.iter().map(|v| (*v).to_owned()));
    let refs = all.iter().map(String::as_str).collect::<Vec<_>>();
    process(
        Path::new("/Users/ilanbm/dev/kpopper-evals/.venv/bin/python"),
        &refs,
        payload,
        cwd,
        tmp,
    )
}
fn native(args: &[&str], payload: &serde_json::Value, cwd: &Path, tmp: &Path) -> Output {
    process(
        Path::new(env!("CARGO_BIN_EXE_kpop-native")),
        args,
        payload,
        cwd,
        tmp,
    )
}

#[test]
fn malformed_or_subagent_payloads_are_nonblocking() {
    let sub = run(
        "ground",
        Some("codex"),
        Some("prompt"),
        &json!({"agent_id":"child"}),
        0.0,
    )
    .unwrap();
    assert_eq!(
        (sub.stdout, sub.stderr, sub.code),
        (String::new(), String::new(), 0)
    );
    let missing = run(
        "followups",
        None,
        None,
        &json!({"cwd":"/definitely/missing","session_id":"s"}),
        0.0,
    );
    assert!(missing.is_ok());
}

#[test]
fn ground_is_silent_without_a_record_and_edit_ignores_unknown_tools() {
    let payload = json!({"cwd":"/tmp","session_id":"host-hook-test","prompt":"unrelated words"});
    assert!(
        run("ground", Some("codex"), Some("prompt"), &payload, 0.0)
            .unwrap()
            .stdout
            .is_empty()
    );
    let edit = json!({"cwd":"/tmp","session_id":"host-hook-test","tool_name":"Bash","tool_input":{"file_path":"x"}});
    assert!(
        run("edit", Some("claude"), None, &edit, 0.0)
            .unwrap()
            .stdout
            .is_empty()
    );
}

#[test]
fn watch_unknown_kind_is_reported_without_host_side_effects() {
    let error = run("no-such-hook", None, None, &json!({}), 0.0)
        .unwrap_err()
        .to_string();
    assert!(error.contains("unknown host hook kind"));
}

#[test]
fn native_process_matches_python_ground_state_transitions_and_preserves_record() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    let before = fs::read(root.join("PROVENANCE.yaml")).unwrap();
    let py = json!({"cwd":root,"session_id":sid("ground-python"),"hook_event_name":"UserPromptSubmit","prompt":"the heat loss on a -5 night"});
    let native_payload = json!({"cwd":root,"session_id":sid("ground-native"),"hook_event_name":"UserPromptSubmit","prompt":"the heat loss on a -5 night"});
    let p = python("ground_hook.py", &["claude", "prompt"], &py, &root, &root);
    let n = native(
        &["_hook", "ground", "claude", "prompt"],
        &native_payload,
        &root,
        &root,
    );
    assert_eq!(p.status.code(), Some(0));
    assert_eq!(n.status.code(), Some(0));
    assert_eq!(context(&n), context(&p));
    assert!(context(&n).contains("heat.loss_kw"));
    assert_eq!(
        context(&native(
            &["_hook", "ground", "claude", "prompt"],
            &native_payload,
            &root,
            &root
        )),
        ""
    );
    let read = json!({"cwd":root,"session_id":native_payload["session_id"],"tool_response":{"stdout":"heat.loss_kw"}});
    assert_eq!(
        native(&["_hook", "ground", "claude", "read"], &read, &root, &root)
            .status
            .code(),
        Some(0)
    );
    for _ in 0..11 {
        let quiet = json!({"cwd":root,"session_id":native_payload["session_id"],"prompt":"something unrelated entirely"});
        native(
            &["_hook", "ground", "claude", "prompt"],
            &quiet,
            &root,
            &root,
        );
    }
    assert!(
        !context(&native(
            &["_hook", "ground", "claude", "prompt"],
            &native_payload,
            &root,
            &root
        ))
        .contains("heat.loss_kw")
    );
    let compact = json!({"cwd":root,"session_id":native_payload["session_id"],"source":"compact"});
    native(
        &["_hook", "ground", "claude", "start"],
        &compact,
        &root,
        &root,
    );
    assert!(
        context(&native(
            &["_hook", "ground", "claude", "prompt"],
            &native_payload,
            &root,
            &root
        ))
        .contains("heat.loss_kw")
    );
    assert_eq!(fs::read(root.join("PROVENANCE.yaml")).unwrap(), before);
    let cross = json!({"cwd":root,"session_id":sid("ground-cross"),"hook_event_name":"UserPromptSubmit","prompt":"the heat loss on a -5 night"});
    assert!(
        context(&python(
            "ground_hook.py",
            &["claude", "prompt"],
            &cross,
            &root,
            &root
        ))
        .contains("heat.loss_kw")
    );
    let cross_read =
        json!({"cwd":root,"session_id":cross["session_id"],"tool_response":"heat.loss_kw"});
    native(
        &["_hook", "ground", "claude", "read"],
        &cross_read,
        &root,
        &root,
    );
    assert!(
        !context(&native(
            &["_hook", "ground", "claude", "prompt"],
            &cross,
            &root,
            &root
        ))
        .contains("heat.loss_kw")
    );
}

#[test]
fn native_process_matches_python_citation_selection_and_once_only_state() {
    let dir = fixture();
    let root = dir.path().canonicalize().unwrap();
    let before = fs::read(root.join("PROVENANCE.yaml")).unwrap();
    let py = json!({"cwd":root,"session_id":sid("edit-python"),"hook_event_name":"PreToolUse","tool_name":"Edit","tool_input":{"file_path":"boiler/service-2025.pdf"}});
    let nv = json!({"cwd":root,"session_id":sid("edit-native"),"hook_event_name":"PreToolUse","tool_name":"Edit","tool_input":{"file_path":"boiler/service-2025.pdf"}});
    let p = python("edit_hook.py", &["claude"], &py, &root, &root);
    let n = native(&["_hook", "edit", "claude"], &nv, &root, &root);
    assert_eq!(context(&n), context(&p));
    assert!(context(&n).contains("doc.boiler_sheet"));
    assert_eq!(
        context(&native(&["_hook", "edit", "claude"], &nv, &root, &root)),
        ""
    );
    let mut child = nv.clone();
    child["session_id"] = json!(sid("edit-child"));
    child["agent_id"] = json!("child");
    assert_eq!(
        context(&native(&["_hook", "edit", "claude"], &child, &root, &root)),
        ""
    );
    child["agent_id"] = json!("");
    assert!(
        context(&native(&["_hook", "edit", "claude"], &child, &root, &root))
            .contains("doc.boiler_sheet")
    );
    assert_eq!(fs::read(root.join("PROVENANCE.yaml")).unwrap(), before);
}

#[test]
fn malformed_cli_input_is_nonblocking_for_every_hook() {
    for kind in ["followups", "watch", "ground", "edit"] {
        let output = raw_process(
            Path::new(env!("CARGO_BIN_EXE_kpop-native")),
            &["_hook", kind, "claude", "prompt"],
            b"{not-json",
            Path::new("/tmp"),
        );
        assert_eq!(output.status.code(), Some(0), "{kind}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unavailable"),
            "{kind}"
        );
    }
}

#[test]
fn followup_process_matches_python_summary_and_keeps_product_backup_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let work = root.join("work");
    let state = root.join("state");
    fs::create_dir(&work).unwrap();
    fs::write(work.join("PROVENANCE.yaml"),"known:\n  facts.count: {v: 1}\njudgments:\n  c.ok:\n    rests_on: [facts.count]\n    seen: {facts.count: 1}\n    wrong_if: facts.count > 3\n    verdict: OK\n").unwrap();
    let setup = native_command(
        &[
            "--workspace",
            work.to_str().unwrap(),
            "followups",
            "setup",
            "--private",
            "--timezone",
            "Asia/Jerusalem",
        ],
        &work,
        &state,
    );
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let spec = root.join("spec.json");
    fs::write(&spec,serde_json::to_vec(&json!({"id":"read","title":"Read the new report","why":"Inform a decision","how":"Read the source","related":["facts.count"],"when":{"at":"2020-01-01"},"scope":"Read and report"})).unwrap()).unwrap();
    let add = native_command(
        &[
            "--workspace",
            work.to_str().unwrap(),
            "followups",
            "add",
            "--file",
            spec.to_str().unwrap(),
        ],
        &work,
        &state,
    );
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let before = fs::read(work.join("PROVENANCE.yaml")).unwrap();
    let store_root = fs::read_dir(state.join("kpopper/followups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let backup = fs::read(store_root.join("followups.previous.yaml")).unwrap();
    let payload_py = json!({"cwd":work,"session_id":sid("follow-python")});
    let payload_nv = json!({"cwd":work,"session_id":sid("follow-native")});
    let mut py = Command::new("/Users/ilanbm/dev/kpopper-evals/.venv/bin/python")
        .arg(repo().join("scripts/followups_hook.py"))
        .current_dir(&work)
        .env("XDG_STATE_HOME", &state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    py.stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&payload_py).unwrap().as_bytes())
        .unwrap();
    let py = py.wait_with_output().unwrap();
    let mut nv = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args(["_hook", "followups"])
        .current_dir(&work)
        .env("XDG_STATE_HOME", &state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    nv.stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&payload_nv).unwrap().as_bytes())
        .unwrap();
    let nv = nv.wait_with_output().unwrap();
    assert_eq!(
        nv.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&nv.stderr)
    );
    assert!(
        nv.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&nv.stderr)
    );
    let native_text = context(&nv);
    let python_text = context(&py);
    let native_json = serde_json::from_str::<serde_json::Value>(
        native_text
            .strip_prefix("KPOPPER_FOLLOWUPS ")
            .unwrap()
            .split_once('\n')
            .unwrap()
            .0,
    )
    .unwrap();
    let python_json = serde_json::from_str::<serde_json::Value>(
        python_text
            .strip_prefix("KPOPPER_FOLLOWUPS ")
            .unwrap()
            .split_once('\n')
            .unwrap()
            .0,
    )
    .unwrap();
    assert_eq!(native_json, python_json);
    assert_eq!(
        native_text.split_once('\n').unwrap().1,
        python_text.split_once('\n').unwrap().1
    );
    assert_eq!(
        fs::read(store_root.join("followups.previous.yaml")).unwrap(),
        backup
    );
    assert_eq!(fs::read(work.join("PROVENANCE.yaml")).unwrap(), before);
}

#[test]
fn watch_process_matches_python_delivery_and_consumes_each_session_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("main");
    let work = root.join("work");
    let state = root.join("state");
    fs::create_dir(&main).unwrap();
    git(&main, &["init", "-b", "main"]);
    git(&main, &["config", "user.email", "fixture@example.test"]);
    git(&main, &["config", "user.name", "Fixture"]);
    let initial = json!({"known":{"api.timeout":{"v":5},"other.value":{"v":1}},"judgments":{"c.positive":{"rests_on":["api.timeout"],"seen":{"api.timeout":5},"verdict":"Positive","wrong_if":"api.timeout < 0"}}});
    fs::write(
        main.join("PROVENANCE.yaml"),
        serde_json::to_vec_pretty(&initial).unwrap(),
    )
    .unwrap();
    git(&main, &["add", "PROVENANCE.yaml"]);
    git(&main, &["commit", "-m", "initial"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-b",
            "experiment",
            work.to_str().unwrap(),
        ],
    );
    let updated = json!({"known":{"api.timeout":{"v":5},"checkout.budget":{"v":10},"other.value":{"v":2}},"judgments":{"c.positive":{"rests_on":["api.timeout"],"seen":{"api.timeout":5},"verdict":"Positive","wrong_if":"api.timeout < 0"},"c.checkout":{"rests_on":["api.timeout","checkout.budget"],"seen":{"api.timeout":5,"checkout.budget":10},"verdict":"Fits","wrong_if":"api.timeout > checkout.budget"}}});
    fs::write(
        main.join("PROVENANCE.yaml"),
        serde_json::to_vec_pretty(&updated).unwrap(),
    )
    .unwrap();
    git(&main, &["add", "PROVENANCE.yaml"]);
    git(&main, &["commit", "-m", "base change"]);
    let changed = json!({"known":{"api.timeout":{"v":30},"other.value":{"v":1}},"judgments":{"c.positive":{"rests_on":["api.timeout"],"seen":{"api.timeout":5},"verdict":"Positive","wrong_if":"api.timeout < 0"}}});
    fs::write(
        work.join("PROVENANCE.yaml"),
        serde_json::to_vec_pretty(&changed).unwrap(),
    )
    .unwrap();
    let setup = native_command(
        &[
            "--workspace",
            work.to_str().unwrap(),
            "watch",
            "setup",
            "--base-ref",
            "main",
        ],
        &work,
        &state,
    );
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let processed = native_command(
        &["--workspace", work.to_str().unwrap(), "watch", "_process"],
        &work,
        &state,
    );
    assert!(
        processed.status.success(),
        "{}",
        String::from_utf8_lossy(&processed.stderr)
    );
    let before = [
        fs::read(main.join("PROVENANCE.yaml")).unwrap(),
        fs::read(work.join("PROVENANCE.yaml")).unwrap(),
    ];
    let py_payload =
        json!({"cwd":work,"session_id":sid("watch-python"),"hook_event_name":"PostToolUse"});
    let mut py = Command::new("/Users/ilanbm/dev/kpopper-evals/.venv/bin/python")
        .args([
            repo().join("scripts/watch_hook.py").to_str().unwrap(),
            "codex",
            "wait",
        ])
        .current_dir(&work)
        .env("XDG_STATE_HOME", &state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    py.stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&py_payload).unwrap().as_bytes())
        .unwrap();
    let py = py.wait_with_output().unwrap();
    let nv_payload =
        json!({"cwd":work,"session_id":sid("watch-native"),"hook_event_name":"PostToolUse"});
    let mut nv = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args(["_hook", "watch", "codex", "wait", "--wait-seconds", "0"])
        .current_dir(&work)
        .env("XDG_STATE_HOME", &state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    nv.stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&nv_payload).unwrap().as_bytes())
        .unwrap();
    let nv = nv.wait_with_output().unwrap();
    assert_eq!(py.status.code(), Some(0));
    assert_eq!(nv.status.code(), Some(0));
    let p = context(&py);
    let n = context(&nv);
    assert!(p.starts_with("KPOPPER_WATCH "));
    assert!(n.starts_with("KPOPPER_WATCH "));
    assert!(p.contains("c.checkout"));
    assert!(n.contains("c.checkout"));
    let mut again = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args(["_hook", "watch", "codex", "wait", "--wait-seconds", "0"])
        .current_dir(&work)
        .env("XDG_STATE_HOME", &state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    again
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(&nv_payload).unwrap().as_bytes())
        .unwrap();
    let again = again.wait_with_output().unwrap();
    assert!(again.stdout.is_empty());
    assert_eq!(
        [
            fs::read(main.join("PROVENANCE.yaml")).unwrap(),
            fs::read(work.join("PROVENANCE.yaml")).unwrap()
        ],
        before
    );
}
