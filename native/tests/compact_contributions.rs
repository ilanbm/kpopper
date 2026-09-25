//! Explicit contributions and pending readings beside compact node history.
use kpop_native::value::TypedValue as V;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    private: PathBuf,
}

fn fixture(config: Option<&str>) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("repo");
    let private = temp.path().canonicalize().unwrap().join("private");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Fixture"]);
    git(&root, &["config", "user.email", "fixture@example.test"]);
    if let Some(config) = config {
        let state = root.join(".git/kpopper/project");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("project.json"), config).unwrap();
    }
    Fixture {
        _temp: temp,
        root,
        private,
    }
}

fn run(fixture: &Fixture, args: &[&str]) -> Output {
    let root = &fixture.root;
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
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"))
        .env("KPOPPER_PRIVATE_HOME", &fixture.private)
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

fn git(root: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn pending_head(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "-q", "refs/kpopper/pending_grounding"])
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}

fn profile(fixture: &Fixture) -> Value {
    let status: Value =
        serde_json::from_str(&success(run(fixture, &["history", "status"]))).unwrap();
    status["authority"]["profile"].clone()
}

fn compact_body(root: &Path, id: &str) -> serde_json::Value {
    let capture = kpop_native::history_node_capture::Capture::read(root).unwrap();
    let V::Map(document) = capture.document() else {
        panic!("record mapping")
    };
    let V::Map(known) = &document["known"] else {
        panic!("known mapping")
    };
    known[id].to_json().unwrap()
}

/// Every file under the record root except test runtime caches.
fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = vec![];
    let mut todo = vec![root.to_owned()];
    while let Some(dir) = todo.pop() {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            let name = path.strip_prefix(root).unwrap();
            if [".git", ".test-runtime", ".test-cache"]
                .iter()
                .any(|skip| name.starts_with(skip))
            {
                continue;
            }
            if path.is_dir() {
                todo.push(path);
            } else {
                out.push((name.to_owned(), fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn compact_project_contribution_refuses_without_any_side_effect() {
    let fixture = fixture(None);
    success(run(&fixture, &["add", "p.base", "v=1"]));
    assert_eq!(profile(&fixture), "node-history/v1");
    let before = tree(&fixture.root);
    for args in [
        &[
            "add", "p.history", "v=2", "--shareability", "project", "--scope", "external",
            "--environment", "vendor", "--event-id", "history-authoring",
        ][..],
        &[
            "set", "p.base", "1", "--shareability", "project", "--scope", "project",
            "--environment", "workspace", "--event-id", "history-set",
        ][..],
    ] {
        let refused = run(&fixture, args);
        assert!(!refused.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(
            stderr.starts_with("node_history_contribution_unsupported:"),
            "{stderr}"
        );
        // No legacy store, no plain bundle, no ledger event, no record change.
        assert_eq!(tree(&fixture.root), before);
        assert_eq!(pending_head(&fixture.root), None);
    }
    assert_eq!(profile(&fixture), "node-history/v1");
}

#[test]
fn compact_private_and_local_scopes_keep_their_routes() {
    let fixture = fixture(None);
    success(run(&fixture, &["add", "p.base", "v=1"]));
    let before = tree(&fixture.root);
    let draft: Value = serde_json::from_str(&success(run(
        &fixture,
        &[
            "add", "p.private", "v=3", "--shareability", "private", "--scope", "project",
            "--environment", "workspace",
        ],
    )))
    .unwrap();
    assert_eq!(draft["state"], "private draft");
    assert!(Path::new(draft["path"].as_str().unwrap()).starts_with(&fixture.private));
    assert_eq!(tree(&fixture.root), before);

    let committed = success(run(
        &fixture,
        &[
            "add", "p.feature", "v=4", "--shareability", "project", "--scope", "feature",
            "--environment", "checkout",
        ],
    ));
    assert!(committed.contains("history committed:"), "{committed}");
    assert_eq!(
        compact_body(&fixture.root, "p.feature"),
        serde_json::json!({"v": 4, "scope": {"kind": "feature", "environment": "checkout"}})
    );
    assert_eq!(profile(&fixture), "node-history/v1");
    assert_eq!(pending_head(&fixture.root), None);
}

#[test]
fn pending_reading_captured_before_the_record_stays_an_alternative() {
    let fixture = fixture(None);
    let receipt: Value = serde_json::from_str(&success(run(
        &fixture,
        &[
            "add", "p.base", "v=9", "--shareability", "project", "--scope", "project",
            "--environment", "workspace", "--event-id", "before-record",
        ],
    )))
    .unwrap();
    assert_eq!(receipt["state"], "captured");
    assert!(!fixture.root.join("GROUNDING.yaml").exists());
    let head = pending_head(&fixture.root).unwrap();
    success(run(&fixture, &["add", "p.base", "v=1"]));
    assert_eq!(profile(&fixture), "node-history/v1");

    let pulled: Value = serde_json::from_str(&success(run(&fixture, &["pull", "p.base"]))).unwrap();
    let contention = &pulled["nodes"]["p.base"]["state"]["contention"];
    let pending = format!("pending-{}", receipt["revision"].as_str().unwrap());
    assert_eq!(contention["status"], "detected", "{contention}");
    assert_eq!(
        contention["witnesses"],
        serde_json::json!([
            ["checkout", {"v": 1}],
            [pending, {"v": 9, "scope": {"kind": "project", "environment": "workspace"}}],
        ])
    );
    // Reading neither folds the pending body nor advances the ledger.
    assert_eq!(compact_body(&fixture.root, "p.base"), serde_json::json!({"v": 1}));
    assert_eq!(pending_head(&fixture.root).unwrap(), head);
    // After birth the same request is refused, not re-captured from compact history.
    let replay = run(
        &fixture,
        &[
            "add", "p.base", "v=9", "--shareability", "project", "--scope", "project",
            "--environment", "workspace", "--event-id", "before-record",
        ],
    );
    assert!(!replay.status.success());
    assert!(
        String::from_utf8_lossy(&replay.stderr)
            .starts_with("node_history_contribution_unsupported:")
    );
    assert_eq!(pending_head(&fixture.root).unwrap(), head);
}

#[test]
fn configured_target_is_reported_unavailable_beside_compact_history() {
    let fixture = fixture(Some(
        r#"{"version":1,"mode":"advanced","record":"GROUNDING.yaml","generation":0,"publication":{"remote":"origin","repository":"test/test","target":"main","branch":"pending_grounding","standing_permission":false}}"#,
    ));
    success(run(&fixture, &["add", "p.base", "v=1"]));
    git(&fixture.root, &["add", "-A"]);
    git(
        &fixture.root,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "record"],
    );
    git(
        &fixture.root,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    success(run(&fixture, &["pull", "p.base"]));
    let entry = fixture.root.join("GROUNDING.yaml");
    let captured = kpop_native::source_capture::capture_source(
        &[entry],
        &fixture.root,
        kpop_native::source_capture::ReadMode::Live,
        None,
    )
    .unwrap();
    let data = captured.snapshot().unwrap().to_data().to_json().unwrap();
    let target = &data["context"]["target"];
    // The target reader cannot replay compact storage; it says so rather than
    // refusing the local read or inventing a comparison.
    assert_eq!(target["status"], "unavailable", "{target}");
    assert!(target["reason"].is_string(), "{target}");
}

#[test]
fn legacy_subset_transport_cannot_name_a_compact_source() {
    // Why compact records do not reuse v3: its origin binds a history/v1 authority,
    // so naming the real compact source is refused and anything else would misstate it.
    let fixture = fixture(None);
    success(run(&fixture, &["add", "p.base", "v=1"]));
    let marker = kpop_native::history_yaml::decode_document(
        &fs::read(fixture.root.join(".kpopper/history.yaml")).unwrap(),
    )
    .unwrap();
    let digest = "0".repeat(64);
    let origin = |authority: V| {
        let mut origin = V::from_json(&serde_json::json!({
            "version": 1, "kind": "selected-subject-observation",
            "operation": "contribution-subset-e", "recorded_at": "2026-09-25T00:00:00Z",
            "source_capture_digest": digest, "source_entry": "GROUNDING.yaml",
            "roots": ["p.base"], "disclosed_locators": [], "prepared_digest": digest,
            "subjects": {"p.base": {"source_state": "prepared_candidate",
                "objects_digest": digest, "reduction_digest": digest}},
        }))
        .unwrap();
        let V::Map(fields) = &mut origin else {
            panic!("origin mapping")
        };
        fields.insert("source_authority".into(), authority);
        origin
    };
    assert_eq!(
        kpop_native::history_projection::validate_subset_origin(&origin(marker))
            .unwrap_err()
            .0,
        "unsupported_authority"
    );
    // The same origin naming a history/v1 source passes the authority check.
    let legacy = V::from_json(&serde_json::json!({"version": 1, "record_id": "r",
        "authority": "history", "generation": 1, "profile": "history/v1"}))
    .unwrap();
    kpop_native::history_projection::validate_subset_origin(&origin(legacy)).unwrap();
}
