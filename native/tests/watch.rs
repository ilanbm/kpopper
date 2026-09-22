use kpop_native::{
    watch_shared,
    watch_store::{Clock, Watch},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
const RECORD: &str = "meta:\n  name: Watch fixture\nknown:\n  facts.count:\n    name: Count\n    v: 1\n    of: 2020-01-01\njudgments:\n  choice.ok:\n    verdict: yes\n    rests_on: [facts.count]\n    seen: {facts.count: 1}\n    wrong_if: facts.count < 0\n";
fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("project");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("PROVENANCE.yaml"), RECORD).unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    git(&root, &["add", "PROVENANCE.yaml"]);
    git(
        &root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "Fixture",
        ],
    );
    (temp, root, base.join("state"))
}

#[test]
fn cli_watch_uses_python_stream_and_pretty_json_contract() {
    let (_temp, root, state) = fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(["--workspace", root.to_str().unwrap(), "watch", "status"])
        .env("XDG_STATE_HOME", &state)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert_eq!(
        output.stdout,
        br#"{
  "state": "disabled"
}
"#
    );

    let refused = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "watch",
            "setup",
            "--base-ref",
            "refs/heads/does-not-exist",
        ])
        .env("XDG_STATE_HOME", &state)
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(2));
    assert_eq!(refused.stdout, b"");
    let envelope: Value = serde_json::from_slice(&refused.stderr).unwrap();
    assert!(envelope.get("error").is_some());
    assert!(envelope.get("status").is_none());
}

#[test]
fn empty_notify_task_is_falsey_and_does_not_reserve_delivery() {
    let (_temp, root, state) = fixture();
    let setup = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(["--workspace", root.to_str().unwrap(), "watch", "setup"])
        .env("XDG_STATE_HOME", &state)
        .output()
        .unwrap();
    assert!(setup.status.success());
    let scan = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "watch",
            "scan",
            "--all",
            "--notify-task",
            "",
        ])
        .env("XDG_STATE_HOME", &state)
        .output()
        .unwrap();
    assert!(scan.status.success());
    let value: Value = serde_json::from_slice(&scan.stdout).unwrap();
    assert!(value.get("delivery_job").is_none());
}

fn watch(root: &Path, state: &Path) -> Watch {
    Watch::in_state(root, state, Clock::at(1_700_000_000.0)).unwrap()
}
#[test]
fn queued_pass_completes_and_freshness_is_rechecked() {
    let (_temp, root, state) = fixture();
    let w = watch(&root, &state);
    w.setup(None, None, false).unwrap();
    let calls = AtomicUsize::new(0);
    let launch = |_: &Watch| {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };
    assert_eq!(w.request_with(&launch).unwrap()["state"], "pending");
    w.request_with(&launch).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(w.status().unwrap()["state"], "pending");
    let result = w.process_with(&launch).unwrap();
    assert_eq!(result["state"], "clear", "{result}");
    assert_eq!(w.status().unwrap()["state"], "clear");
    fs::write(
        root.join("PROVENANCE.yaml"),
        RECORD.replace("v: 1", "v: -1"),
    )
    .unwrap();
    assert_eq!(w.status().unwrap()["state"], "pending");
    let result = w.process_with(&launch).unwrap();
    assert_eq!(result["state"], "attention", "{result}");
    assert!(
        result["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "falsified")
    );
    assert_eq!(
        fs::read_to_string(w.state.join("active.json")).unwrap(),
        "{\"until\":0}\n"
    );
}
#[test]
fn actual_native_cli_launches_processor_and_delivers_real_result() {
    let (_temp, root, state) = fixture();
    let invoke = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .arg("--workspace")
            .arg(&root)
            .arg("watch")
            .args(args)
            .env("XDG_STATE_HOME", &state)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    invoke(&["setup"]);
    assert_eq!(invoke(&["scan"])["state"], "pending");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let status = invoke(&["status"]);
        if status["state"] != "pending" {
            assert_eq!(status["state"], "clear", "{status}");
            assert!(status["checked_at"].is_number());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native detached processor did not finish"
        );
        thread::sleep(Duration::from_millis(50));
    }
}
#[test]
fn queue_race_launches_once_and_expired_lease_relaunches() {
    let (_temp, root, state) = fixture();
    watch(&root, &state).setup(None, None, false).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let calls = Arc::new(AtomicUsize::new(0));
    let workers = (0..2)
        .map(|_| {
            let (root, state, barrier, calls) =
                (root.clone(), state.clone(), barrier.clone(), calls.clone());
            thread::spawn(move || {
                let w = watch(&root, &state);
                barrier.wait();
                w.request_with(&|_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .unwrap()
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let later = Watch::in_state(&root, &state, Clock::at(1_700_000_031.0)).unwrap();
    later
        .request_with(&|_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
#[test]
fn rename_preserves_configuration_and_missing_record_is_not_recreated() {
    let (_temp, root, state) = fixture();
    let old = watch(&root, &state);
    old.setup(None, None, false).unwrap();
    fs::rename(root.join("PROVENANCE.yaml"), root.join("GROUNDING.yaml")).unwrap();
    let renamed = watch(&root, &state);
    assert_eq!(old.config_path, renamed.config_path);
    assert_eq!(old.project_state, renamed.project_state);
    assert_eq!(
        renamed.config().unwrap().unwrap()["entry"],
        "GROUNDING.yaml"
    );
    assert_eq!(renamed.process_with(&|_| Ok(())).unwrap()["state"], "clear");
    fs::remove_file(root.join("GROUNDING.yaml")).unwrap();
    let missing = watch(&root, &state);
    assert_eq!(missing.config_path, old.config_path);
    assert!(missing.setup(None, None, false).is_err());
    assert_eq!(missing.status().unwrap()["state"], "unavailable");
    assert!(!root.join("PROVENANCE.yaml").exists());
}
#[test]
fn shared_source_is_captured_applied_and_retained() {
    let (_temp, root, state) = fixture();
    let w = watch(&root, &state);
    w.setup(None, None, true).unwrap();
    let report = json!({"event_id":"source-1","id":"external.count","name":"Count","value":3,"date":"2020-01-02","scope":{"kind":"external","environment":"fixture"},"source":{"url":"https://example.invalid/report","at":"row 3"},"source_quote":"The count is 3."});
    let captured = watch_shared::capture_with(&w, report, &|_| Ok(())).unwrap();
    assert_eq!(captured["state"], "captured");
    assert_eq!(w.status().unwrap()["state"], "pending");
    let applied = watch_shared::process(&w).unwrap();
    assert!(applied, "{}", watch_shared::read(&w).unwrap());
    let data = watch_shared::read(&w).unwrap();
    assert_eq!(data["reports"][0]["state"], "applied", "{data}");
    assert_eq!(data["entries"]["external.count"]["v"], 3);
    assert_eq!(
        data["entries"]["external.count"]["from"],
        "s.shared_source_1"
    );
    assert!(fs::read_to_string(&w.record).unwrap() == RECORD);
}

#[test]
fn daily_start_queues_real_native_watch_processor() {
    let (_temp, root, state) = fixture();
    let invoke = |family: &str, args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .arg("--workspace")
            .arg(&root)
            .arg(family)
            .args(args)
            .env("XDG_STATE_HOME", &state)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    invoke("watch", &["setup"]);
    invoke("followups", &["setup", "--private", "--timezone", "UTC"]);
    let started = invoke("followups", &["daily", "start", "--owner", "daily-fixture"]);
    assert_eq!(
        started["packet"]["watch"]["worktrees"][0]["state"],
        "pending"
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let status = invoke("watch", &["status"]);
        if status["state"] != "pending" {
            assert_eq!(status["state"], "clear", "{status}");
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn cli_shared_delivery_and_resolution_use_only_retained_receipts() {
    use std::{io::Write, process::Stdio};
    let (_temp, root, state) = fixture();
    let invoke = |args: &[&str], input: Option<Value>, success: bool| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .arg("--workspace")
            .arg(&root)
            .arg("watch")
            .args(args)
            .env("XDG_STATE_HOME", &state)
            .env("CODEX_SESSION_ID", "watch-cli-fixture")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(serde_json::to_string(&input).unwrap().as_bytes())
                .unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert_eq!(
            out.status.success(),
            success,
            "args={args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        if success {
            serde_json::from_slice::<Value>(&out.stdout).unwrap()
        } else {
            Value::Null
        }
    };
    invoke(&["setup", "--shared-private"], None, true);
    let report = json!({"event_id":"cli-1","id":"external.count","name":"Count","value":3,"date":"2020-01-02","scope":{"kind":"external","environment":"fixture"},"source":{"url":"https://example.invalid/report","at":"row 3"},"source_quote":"The count is 3."});
    let capture = invoke(
        &["share", "--file", "-", "--notify-task", "watch-cli-fixture"],
        Some(report.clone()),
        true,
    );
    let id = capture["delivery_job"]["job_id"].as_str().unwrap();
    assert_eq!(
        invoke(&["wait-delivery", id, "--timeout", "10"], None, true)["state"],
        "quiet"
    );
    assert_eq!(
        invoke(&["shared"], None, true)["reports"][0]["state"],
        "applied"
    );
    fs::write(
        root.join("PROVENANCE.yaml"),
        RECORD.replace("v: 1", "v: -1"),
    )
    .unwrap();
    let scan = invoke(
        &["scan", "--all", "--notify-task", "watch-cli-fixture"],
        None,
        true,
    );
    let id = scan["delivery_job"]["job_id"].as_str().unwrap();
    let notice = invoke(&["wait-delivery", id, "--timeout", "10"], None, true);
    assert_eq!(notice["state"], "attention", "{notice}");
    assert_eq!(notice["recipient"], "watch-cli-fixture");
    let token = notice["claim_token"].as_str().unwrap();
    assert_eq!(
        invoke(
            &[
                "complete-delivery",
                id,
                "--token",
                token,
                "--outcome",
                "sent"
            ],
            None,
            true
        )["state"],
        "sent"
    );
    invoke(
        &[
            "complete-delivery",
            id,
            "--token",
            token,
            "--outcome",
            "sent",
        ],
        None,
        false,
    );
    let mut conflict = report;
    conflict["event_id"] = json!("cli-conflict");
    conflict["scope"]["environment"] = json!("other-environment");
    invoke(&["share", "--file", "-"], Some(conflict), true);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let reports = invoke(&["shared"], None, true);
        let conflict = reports["reports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "cli-conflict")
            .unwrap();
        if conflict["state"] != "captured" {
            assert_eq!(conflict["state"], "needs_review", "{reports}");
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(50));
    }
    invoke(&["pause"], None, true);
    assert_eq!(
        invoke(
            &[
                "resolve",
                "cli-conflict",
                "--evidence",
                "User reconciled the environment"
            ],
            None,
            true
        )["state"],
        "resolved"
    );
    invoke(
        &["wait-delivery", "unknown", "--timeout", "NaN"],
        None,
        false,
    );
    invoke(&["scan", "--notify-task", "another-task"], None, false);
}

#[test]
fn busy_processor_does_not_consume_queued_request() {
    use fs2::FileExt;
    let (_temp, root, state) = fixture();
    let w = watch(&root, &state);
    w.setup(None, None, false).unwrap();
    w.request_with(&|_| Ok(())).unwrap();
    let request = fs::read(w.state.join("request.json")).unwrap();
    let file = fs::OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .truncate(false)
        .open(w.state.join("processor.lock"))
        .unwrap();
    file.lock_exclusive().unwrap();
    assert_eq!(w.process_with(&|_| Ok(())).unwrap()["state"], "busy");
    assert_eq!(fs::read(w.state.join("request.json")).unwrap(), request);
    assert!(!w.state.join("result.json").exists());
    drop(file);
    assert_eq!(w.process_with(&|_| Ok(())).unwrap()["state"], "clear");
}
