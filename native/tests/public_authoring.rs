use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    let resources = root.join(".test-runtime");
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.zip")),
        resources.join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .current_dir(root)
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"))
        .args(args)
        .output()
        .unwrap()
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
                "{}.zip",
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
