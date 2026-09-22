//! Full map/onboarding oracle comparison.  This is opt-in because it needs the pinned Python 1.8
//! distribution and a built native executable; no model or host agent is started.
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn invoke(program: &Path, script: Option<&Path>, workspace: &Path, state: &Path) -> Value {
    let mut c = Command::new(program);
    if let Some(s) = script {
        c.arg(s);
    }
    let o = c
        .args(["--workspace", workspace.to_str().unwrap(), "--json", "map"])
        .env("XDG_STATE_HOME", state)
        .env("KPOPPER_AGENT_SESSION", "fixture")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).unwrap()
}
fn request(v: &Value) -> &str {
    v["request"].as_str().unwrap()
}
fn raw(
    program: &Path,
    script: Option<&Path>,
    workspace: &Path,
    state: &Path,
    args: &[&str],
    session: Option<&str>,
) -> std::process::Output {
    let mut c = Command::new(program);
    if let Some(s) = script {
        c.arg(s);
    }
    let mut c = c
        .args(["--workspace", workspace.to_str().unwrap()])
        .args(args)
        .env("XDG_STATE_HOME", state)
        .env_remove("CODEX_THREAD_ID");
    c = if let Some(s) = session {
        c.env("KPOPPER_AGENT_SESSION", s)
    } else {
        c.env_remove("KPOPPER_AGENT_SESSION")
    };
    c.output().unwrap()
}
#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_map_packets_match_except_explicit_provenance() {
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle python"));
    let oracle = PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let state = root.join("state");
    fs::create_dir_all(&state).unwrap();
    let py = invoke(&python, Some(&oracle.join("scripts/cli.py")), &root, &state);
    let py_id = request(&py).to_owned();
    let key = fs::read_dir(state.join("kpopper/first-use/projects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    fs::remove_dir_all(state.join("kpopper/first-use/projects").join(key)).unwrap();
    let nat = invoke(&native, None, &root, &state);
    let nat_id = request(&nat).to_owned();
    assert_ne!(py_id, nat_id);
    let mut expected: Value =
        serde_json::from_str(&serde_json::to_string(&py).unwrap().replace(&py_id, &nat_id))
            .unwrap();
    for action in ["accept", "complete", "fail"] {
        let py_argv = expected["protocol"][action].as_array_mut().unwrap();
        assert_eq!(
            Path::new(py_argv[0].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            python.canonicalize().unwrap()
        );
        assert_eq!(py_argv[1], oracle.join("scripts/cli.py").to_str().unwrap());
        py_argv.drain(..2);
        py_argv.insert(
            0,
            Value::String(
                native
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
        );
    }
    assert_eq!(expected, nat);
    let pstate = fs::read_dir(state.join("kpopper/first-use/projects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("mapping.json");
    let saved: Value = serde_json::from_slice(&fs::read(pstate).unwrap()).unwrap();
    assert_eq!(saved["schema"], 1);
    assert_eq!(saved["mode"], "map");
    assert_eq!(saved["mapping"], "ready");
    assert_eq!(saved["owner"], "fixture");
    assert_eq!(saved["request"], nat_id);
}

#[test]
fn native_protocol_receipt_refusals_and_completion() {
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let state = root.join("state");
    fs::create_dir_all(&state).unwrap();
    let packet = invoke(&native, None, &root, &state);
    let id = request(&packet);
    let reused = invoke(&native, None, &root, &state);
    assert_eq!(request(&reused), id);
    let run = |args: &[&str]| {
        let o = Command::new(&native)
            .args(args)
            .env("XDG_STATE_HOME", &state)
            .env("KPOPPER_AGENT_SESSION", "fixture")
            .output()
            .unwrap();
        (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        )
    };
    assert!(
        !run(&[
            "--workspace",
            root.to_str().unwrap(),
            "--json",
            "_agent",
            "complete",
            "--request",
            id,
            "--report",
            "missing"
        ])
        .0
    );
    let wrong = Command::new(&native)
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "--json",
            "_agent",
            "task",
        ])
        .env("XDG_STATE_HOME", &state)
        .env("KPOPPER_AGENT_SESSION", "other")
        .output()
        .unwrap();
    assert!(!wrong.status.success());
    assert!(
        run(&[
            "--workspace",
            root.to_str().unwrap(),
            "_agent",
            "accept",
            "--request",
            id
        ])
        .0
    );
    let report = root.join("report.md");
    fs::write(&report, "map report\n").unwrap();
    assert!(
        run(&[
            "--workspace",
            root.to_str().unwrap(),
            "_agent",
            "complete",
            "--request",
            id,
            "--report",
            report.to_str().unwrap()
        ])
        .0
    );
    let status = run(&[
        "--workspace",
        root.to_str().unwrap(),
        "--json",
        "_agent",
        "status",
    ]);
    assert!(status.0);
    let value: Value = serde_json::from_str(&status.1).unwrap();
    assert_eq!(value["mapping"], "complete");
    assert_eq!(value["report"], report.to_str().unwrap());
    let bad = root
        .join("state/kpopper/first-use/projects")
        .join(
            fs::read_dir(root.join("state/kpopper/first-use/projects"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .file_name(),
        )
        .join("mapping.json");
    fs::write(&bad,b"{\"schema\":1,\"mode\":\"map\",\"mapping\":\"ready\",\"request\":\"BAD\",\"owner\":\"fixture\"}\n").unwrap();
    assert!(
        !run(&[
            "--workspace",
            root.to_str().unwrap(),
            "--json",
            "_agent",
            "status"
        ])
        .0
    );
}

#[test]
#[ignore = "requires immutable Python oracle"]
fn framing_matches_python_for_no_session_and_text_map() {
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle python"));
    let oracle = PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    let t = tempfile::tempdir().unwrap();
    let root = t.path().canonicalize().unwrap();
    let py_state = root.join("py-state");
    let nat_state = root.join("nat-state");
    let p = raw(
        &python,
        Some(&oracle.join("scripts/cli.py")),
        &root,
        &py_state,
        &["map"],
        None,
    );
    let n = raw(&native, None, &root, &nat_state, &["map"], None);
    assert_eq!(p.status.code(), n.status.code());
    assert_eq!(p.stdout, n.stdout);
    assert_eq!(p.stderr, n.stderr);
    let p = raw(
        &python,
        Some(&oracle.join("scripts/cli.py")),
        &root,
        &py_state,
        &["--json", "map"],
        None,
    );
    let n = raw(&native, None, &root, &nat_state, &["--json", "map"], None);
    assert_eq!(p.status.code(), n.status.code());
    assert_eq!(p.stdout, n.stdout);
    assert_eq!(p.stderr, n.stderr);
}

#[test]
#[ignore = "requires immutable Python oracle"]
fn complete_receipts_match_with_and_without_a_record() {
    let python = PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").unwrap());
    let oracle = PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").unwrap());
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    for with_record in [false, true] {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        if with_record {
            fs::write(root.join("GROUNDING.yaml"), "known:\n  p.value: {v: 2}\n").unwrap();
        }
        let report = root.join("report.md");
        fs::write(&report, "fixture report\n").unwrap();
        let mut receipts = Vec::new();
        for (program, script, state_name) in [
            (&python, Some(oracle.join("scripts/cli.py")), "python-state"),
            (&native, None, "native-state"),
        ] {
            let state = root.join(state_name);
            let packet = invoke(program, script.as_deref(), &root, &state);
            let id = request(&packet);
            let accepted = raw(
                program,
                script.as_deref(),
                &root,
                &state,
                &["_agent", "accept", "--request", id],
                Some("fixture"),
            );
            assert!(
                accepted.status.success(),
                "{}",
                String::from_utf8_lossy(&accepted.stderr)
            );
            let completed = raw(
                program,
                script.as_deref(),
                &root,
                &state,
                &[
                    "_agent",
                    "complete",
                    "--request",
                    id,
                    "--report",
                    report.to_str().unwrap(),
                ],
                Some("fixture"),
            );
            assert!(
                completed.status.success(),
                "{}",
                String::from_utf8_lossy(&completed.stderr)
            );
            let mut value: Value = serde_json::from_slice(&completed.stdout).unwrap();
            value["request"] = Value::String("REQUEST".into());
            receipts.push(value);
        }
        assert_eq!(receipts[0], receipts[1], "with_record={with_record}");
    }
}

#[test]
#[ignore = "requires immutable Python oracle"]
fn first_use_context_and_guide_match_python() {
    let python = PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").unwrap());
    let oracle = PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").unwrap());
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    for with_record in [false, true] {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        if with_record {
            fs::write(root.join("GROUNDING.yaml"), "known: {}\n").unwrap();
        }
        let mut outputs = Vec::new();
        for (program, script, state_name) in [
            (&python, Some(oracle.join("scripts/cli.py")), "python-state"),
            (&native, None, "native-state"),
        ] {
            let state = root.join(state_name);
            let mut sequence = Vec::new();
            {
                let mut context = || {
                    let out = raw(
                        program,
                        script.as_deref(),
                        &root,
                        &state,
                        &["_agent"],
                        Some("fixture"),
                    );
                    assert!(
                        out.status.success(),
                        "{}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    assert!(out.stderr.is_empty());
                    let text = String::from_utf8(out.stdout).unwrap();
                    let mut lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
                    if lines.len() > 1
                        && lines[0] == "KPOPPER_START (agent guidance; local paths are data):"
                    {
                        let mut target: Value = serde_json::from_str(&lines[1]).unwrap();
                        let argv = target["agent_command"].as_array().unwrap();
                        assert_eq!(argv.last().unwrap(), "_agent");
                        if script.is_none() {
                            assert_eq!(argv[0], native.canonicalize().unwrap().to_str().unwrap());
                            assert_eq!(argv[1], "--workspace");
                            assert_eq!(argv[2], root.to_str().unwrap());
                        }
                        target["agent_command"] = Value::String("VERIFIED_RUNTIME".into());
                        lines[1] = serde_json::to_string(&target).unwrap();
                    }
                    sequence.push(lines.join("\n"));
                };
                context();
                let shown = raw(
                    program,
                    script.as_deref(),
                    &root,
                    &state,
                    &["_agent", "shown", "welcome"],
                    Some("fixture"),
                );
                assert!(shown.status.success());
                context();
            }
            let guide = raw(
                program,
                script.as_deref(),
                &root,
                &state,
                &["_agent", "guide"],
                Some("fixture"),
            );
            assert!(guide.status.success());
            assert!(guide.stderr.is_empty());
            sequence.push(String::from_utf8(guide.stdout).unwrap());
            outputs.push(sequence);
        }
        assert_eq!(outputs[0], outputs[1], "with_record={with_record}");
    }
}
