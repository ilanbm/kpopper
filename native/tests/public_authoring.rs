use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    command(root).args(args).output().unwrap()
}
fn command(root: &Path) -> Command {
    let resources = root.join(".test-runtime");
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        resources.join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
    command
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"));
    command
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn empty_tmpdir_keeps_publication_receipts_outside_the_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let sessions = tempfile::tempdir().unwrap();
    let sid = format!(
        "empty-tmp-{}",
        &kpop_native::identity::sha256(root.to_string_lossy().as_bytes())[..16]
    );
    success(
        command(&root)
            .env("TMPDIR", "")
            .env("TEMP", sessions.path())
            .env_remove("TMP")
            .env("KPOPPER_AGENT_SESSION", &sid)
            .args(["add", "p.x", "v=1"])
            .output()
            .unwrap(),
    );
    assert!(!root.join(format!("kpopper-session-{sid}")).exists());
    let receipt_home = sessions.path().join(format!("kpopper-session-{sid}"));
    assert!(receipt_home.is_dir());
    assert!(fs::read_dir(receipt_home).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("writes-")
    }));
}

#[test]
fn first_add_creates_history_and_subsequent_set_retains_the_original_version() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = success(run(&root, &["add", "p.x", "v=1"]));
    assert!(first.contains("history committed:"));
    assert!(first.contains("born with its first entry"));
    let before: Value = serde_json::from_str(&success(run(&root, &["history", "status"]))).unwrap();
    let old = before["subjects"]["p.x"]["heads"][0].clone();
    let yesterday = chrono::Utc::now()
        .date_naive()
        .max(chrono::Local::now().date_naive())
        .pred_opt()
        .unwrap()
        .to_string();
    success(run(
        &root,
        &[
            "set",
            "p.x",
            "2",
            "--as-of",
            &yesterday,
            "--why",
            "new reading",
        ],
    ));
    let after: Value = serde_json::from_str(&success(run(&root, &["history", "status"]))).unwrap();
    assert_eq!(after["commits"], 2);
    assert_ne!(after["subjects"]["p.x"]["heads"][0], old);
    assert!(
        fs::read_to_string(root.join("GROUNDING.yaml"))
            .unwrap()
            .contains("v: 2")
    );
    success(run(
        &root,
        &[
            "add",
            "d.work",
            "verdict=continue",
            "rests_on=[p.x]",
            "reopened_by=new readings",
        ],
    ));
    success(run(
        &root,
        &["set", "p.x", "3", "--why", "a further observation"],
    ));
    assert!(success(run(&root, &["review", "d.work"])).contains("(review d.work)"));
}

#[test]
fn named_hypothesis_cli_writes_stay_out_of_base_and_support_add_set_review() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    success(run(&root, &["add", "p.base", "v=10"]));
    success(run(
        &root,
        &["add", "p.only", "v=1", "--hypothesis", "alpha"],
    ));
    success(run(&root, &["set", "p.only", "2", "--hypothesis", "alpha"]));
    success(run(
        &root,
        &[
            "add",
            "d.named",
            "verdict=continue",
            "rests_on=[p.only]",
            "reopened_by=new readings",
            "--hypothesis",
            "alpha",
        ],
    ));
    assert!(
        success(run(&root, &["review", "d.named", "--hypothesis", "alpha"],))
            .contains("(review d.named)")
    );
    let record = kpop_native::history_yaml::decode_source_document(
        &fs::read(root.join("GROUNDING.yaml")).unwrap(),
    )
    .unwrap();
    let known = record.get("known").unwrap();
    assert_eq!(
        known
            .get("p.base")
            .unwrap()
            .get("v")
            .unwrap()
            .typed()
            .to_tagged()
            .unwrap(),
        serde_json::json!(["int", "10"])
    );
    assert!(known.get("p.only").is_none());
    assert!(
        record
            .get("judgments")
            .and_then(|v| v.get("d.named"))
            .is_none()
    );
    let status: Value = serde_json::from_str(&success(run(&root, &["history", "status"]))).unwrap();
    assert_eq!(status["subjects"]["p.base"]["acceptance"], "accepted");
    assert_eq!(status["subjects"]["p.only"]["acceptance"], "proposed");
    assert_eq!(status["subjects"]["d.named"]["acceptance"], "proposed");
}

#[test]
fn first_named_add_bootstraps_a_proposal_without_accepting_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let output = success(run(
        &root,
        &["add", "p.first", "v=1", "--hypothesis", "opening"],
    ));
    assert!(output.contains("born with its first entry"));
    let record = kpop_native::history_yaml::decode_source_document(
        &fs::read(root.join("GROUNDING.yaml")).unwrap(),
    )
    .unwrap();
    assert!(record.get("known").unwrap().get("p.first").is_none());
    let status: Value = serde_json::from_str(&success(run(&root, &["history", "status"]))).unwrap();
    assert_eq!(status["subjects"]["p.first"]["acceptance"], "proposed");
}

#[test]
fn named_hypothesis_rejects_invalid_and_physical_authority_names() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    success(run(&root, &["add", "p.base", "v=1"]));
    let invalid = run(&root, &["add", "p.bad", "v=2", "--hypothesis", "../bad"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid_history_hypothesis"));
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/taken.yaml"),
        "known: {p.physical: {v: 3}}\n",
    )
    .unwrap();
    let collision = run(&root, &["add", "p.bad", "v=2", "--hypothesis", "taken"]);
    assert!(!collision.status.success());
    assert!(String::from_utf8_lossy(&collision.stderr).contains("hypothesis_authority_collision"));
}

#[test]
fn named_privacy_checks_use_hypothesis_only_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let private = tempfile::tempdir().unwrap();
    success(run(&root, &["add", "p.base", "v=1"]));
    success(run(
        &root,
        &["add", "p.only", "v=2", "--hypothesis", "alpha"],
    ));
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let output = command(&root)
        .env("KPOPPER_PRIVATE_HOME", private.path())
        .args([
            "add",
            "d.private",
            "verdict=stop",
            "rests_on=[p.only]",
            "private=true",
            "--hypothesis",
            "alpha",
        ])
        .output()
        .unwrap();
    let result: Value = serde_json::from_str(&success(output)).unwrap();
    assert_eq!(result["state"], "private draft");
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    assert!(!success(run(&root, &["history", "status"])).contains("d.private"));
}
#[test]
fn refused_first_add_leaves_no_partial_record_or_authority() {
    let temp = tempfile::tempdir().unwrap();
    let output = run(
        temp.path(),
        &[
            "add",
            "d.bad",
            "rests_on=[p.missing]",
            "wrong_if={expr: p.missing > 3}",
        ],
    );
    assert!(!output.status.success());
    assert!(!temp.path().join("GROUNDING.yaml").exists());
    assert!(!temp.path().join(".kpopper/history.yaml").exists());
}

#[test]
fn public_recovery_finishes_or_cancels_a_retained_first_write() {
    use kpop_native::{
        history_bootstrap as B,
        history_transaction::Layout,
        reasoning_runtime::{OperationalBounds, Runtime},
        value::TypedValue as V,
    };
    for rollback in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let entry = root.join("GROUNDING.yaml");
        let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.kpopper-runtime",
                kpop_native::reasoning_runtime::target_name().unwrap()
            ));
        let runtime = Runtime::open(
            &archive,
            &root.join(".test-cache"),
            OperationalBounds::default(),
        )
        .unwrap();
        let policy = kpop_native::project_modes::Project::open(&root)
            .unwrap()
            .config()
            .unwrap();
        let action =
            V::from_json(&serde_json::json!({"kind":"add","id":"p.x","body":{"v":1}})).unwrap();
        let mutation = B::prepare(
            &entry,
            &action,
            &policy,
            &B::BootstrapOptions {
                operation: "first-recovery".into(),
                recorded_at: "2026-09-19T12:00:00+00:00".into(),
                recording_day: "2026-09-19".into(),
                record_id: "fixture".into(),
                by: V::Null,
            },
            Some(&runtime),
        )
        .unwrap();
        let journal = root.join(Layout::for_entry("GROUNDING.yaml").unwrap().journal);
        fs::create_dir_all(journal.parent().unwrap()).unwrap();
        fs::write(&journal, mutation.to_bytes().unwrap()).unwrap();
        let result = success(run(
            &root,
            if rollback {
                &["recover", "--json", "--rollback"]
            } else {
                &["recover", "--json"]
            },
        ));
        let result: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            result["state"],
            if rollback { "restored" } else { "recovered" }
        );
        assert_eq!(entry.exists(), !rollback);
        assert!(!journal.exists());
    }
}

#[test]
fn a_private_first_or_later_add_stays_outside_the_record() {
    for existing in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let private = tempfile::tempdir().unwrap();
        // Initialize only the explicitly selected test runtime if no record exists.
        if existing {
            success(run(&root, &["add", "p.public", "v=1"]));
        } else {
            assert!(
                !run(&root, &["add", "d.bad", "rests_on=[missing]"])
                    .status
                    .success()
            );
        }
        let path = root.join("GROUNDING.yaml");
        let before = fs::read(&path).ok();
        let output = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
            .current_dir(&root)
            .env("KPOPPER_NATIVE_RESOURCES", root.join(".test-runtime"))
            .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"))
            .env("KPOPPER_PRIVATE_HOME", private.path())
            .args(["add", "p.private", "v=secret", "private=true"])
            .output()
            .unwrap();
        let result: Value = serde_json::from_str(&success(output)).unwrap();
        assert_eq!(result["state"], "private draft");
        assert!(
            Path::new(result["path"].as_str().unwrap())
                .starts_with(private.path().canonicalize().unwrap())
        );
        assert_eq!(fs::read(&path).ok(), before);
    }
}

#[test]
fn explicit_core_acts_and_proposals_need_only_the_core_program() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    success(run(&root, &["add", "p.x", "v=1"]));
    let state: Value = serde_json::from_str(&success(run(&root, &["history", "status"]))).unwrap();
    let version = state["subjects"]["p.x"]["heads"][0].as_str().unwrap();
    for action in ["retire", "accept"] {
        let result: Value = serde_json::from_str(&success(run(
            &root,
            &[
                "history",
                action,
                "--subject",
                "p.x",
                "--of",
                version,
                "--because",
                "explicit test disposition",
            ],
        )))
        .unwrap();
        assert_eq!(result["state"], "committed");
    }
    let record = root.join("GROUNDING.yaml");
    let text = fs::read_to_string(&record).unwrap();
    assert!(text.contains("v: 1"));
    fs::write(&record, text.replace("v: 1", "v: 8")).unwrap();
    let result: Value = serde_json::from_str(&success(run(
        &root,
        &[
            "history",
            "reconcile",
            "--record-proposals",
            "--proposal-subject",
            "p.x",
            "--because",
            "retain an edited reading",
        ],
    )))
    .unwrap();
    assert_eq!(result["state"], "proposed");
    assert!(!root.join(".test-runtime/ordinary").exists());
}

#[test]
fn unresolved_candidate_references_reach_the_authoring_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    success(run(&root, &["add", "p.seed", "v=1"]));
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    for (args, expected) in [
        (
            vec!["add", "d.bad", "verdict=stop", "rests_on=[p.missing]"],
            "rests on p.missing, which is not an entry",
        ),
        (
            vec!["add", "p.bad", "v={{p.missing}}"],
            "v references p.missing, which is not an entry",
        ),
    ] {
        let output = run(&root, &args);
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("missing_contribution_dependency"));
        assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    }
}

#[test]
fn session_receipts_follow_successful_publication_and_cannot_break_a_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let sessions = tempfile::tempdir().unwrap();
    let call = |args: &[&str]| {
        command(&root)
            .env("KPOPPER_AGENT_SESSION", "publication-test")
            .env("TMPDIR", sessions.path())
            .args(args)
            .output()
            .unwrap()
    };
    success(call(&["add", "p.x", "v=1"]));
    let record = root.join("GROUNDING.yaml");
    let key = kpop_native::identity::sha256(record.to_str().unwrap().as_bytes());
    let receipt = sessions
        .path()
        .join("kpopper-session-publication-test")
        .join(format!("writes-{key}.json"));
    let current =
        kpop_native::history_yaml::decode_source_document(&fs::read(&record).unwrap()).unwrap();
    let state: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    assert_eq!(
        state["p.x"],
        current
            .get("known")
            .unwrap()
            .get("p.x")
            .unwrap()
            .typed()
            .digest()
            .unwrap()
    );
    let before = fs::read(&receipt).unwrap();
    assert!(
        !call(&["add", "d.bad", "rests_on=[missing]"])
            .status
            .success()
    );
    assert_eq!(fs::read(&receipt).unwrap(), before);
    let captured = kpop_native::source_capture::capture_source(
        std::slice::from_ref(&record),
        &root,
        kpop_native::source_capture::ReadMode::Frozen,
        None,
    )
    .unwrap();
    assert_eq!(captured.origins()["known"]["p.x"], record);
    let status: Value = serde_json::from_str(&success(call(&["history", "status"]))).unwrap();
    let version = status["subjects"]["p.x"]["heads"][0].as_str().unwrap();
    success(call(&[
        "history",
        "retire",
        "--subject",
        "p.x",
        "--of",
        version,
        "--because",
        "explicitly retired",
    ]));
    let state: Value = serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
    assert!(state.get("p.x").is_none());
    // An unusable optional attribution directory never changes publication success.
    let bad = sessions.path().join("kpopper-session-unavailable");
    fs::write(&bad, b"not a directory").unwrap();
    let output = command(&root)
        .env("KPOPPER_AGENT_SESSION", "unavailable")
        .env("TMPDIR", sessions.path())
        .args(["add", "p.other", "v=2"])
        .output()
        .unwrap();
    success(output);
    assert_eq!(fs::read(bad).unwrap(), b"not a directory");
}
