use kpop_native::{
    history_authoring::Options, history_node_publication as P, history_node_writer as W,
    history_paths::Scheme, value::TypedValue as V,
};
use serde_json::json;
use std::{fs, path::Path, process::Command};

fn v(x: serde_json::Value) -> V {
    V::from_json(&x).unwrap()
}
fn opts(op: &str) -> Options {
    let day = match op {
        "set-a" => "25",
        "proposal-b" => "26",
        "accept-b" => "27",
        "proposal-a" => "28",
        "accept-a" => "29",
        _ => "24",
    };
    Options {
        operation: op.into(),
        recorded_at: format!("2026-09-{day}T12:00:00+00:00"),
        recording_day: format!("2026-09-{day}"),
        by: v(json!("writer")),
        strict: false,
        paths: Scheme::Hashed,
        receipt_version: None,
    }
}
fn map(value: &V) -> &std::collections::BTreeMap<String, V> {
    let V::Map(value) = value else {
        panic!("expected map: {value:?}")
    };
    value
}
fn typed_text(value: &V) -> &str {
    let V::Text(value) = value else {
        panic!("expected text: {value:?}")
    };
    value
}
fn core_runtime() -> (tempfile::TempDir, kpop_native::reasoning_runtime::Runtime) {
    use kpop_native::reasoning_runtime::{OperationalBounds, Runtime};
    let cache = tempfile::tempdir().unwrap();
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!(
            "{}.kpopper-runtime",
            kpop_native::reasoning_runtime::target_name().unwrap()
        ));
    let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
    (cache, runtime)
}
fn tree(root: &Path) -> std::collections::BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    kpop_native::identity::sha256(&fs::read(path).unwrap()),
                );
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, root, &mut out);
    out
}
fn write(root: &Path, op: &str, action: serde_json::Value) {
    let prepared = W::prepare(root, &v(action), &opts(op), None).unwrap();
    W::publish(root, &prepared, None, |_| Ok(())).unwrap();
}

/// Public migration creates compact history. Existing history/v1 and bootstrap
/// copies are produced here by their retained library emitters.
fn legacy_copy(source: &Path, destination: &Path) {
    kpop_native::history_migration::Plan::prepare(
        Path::new("GROUNDING.yaml"),
        source,
        kpop_native::history_migration::Options {
            operation: "import-fixture".into(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
            record_id: Some("record-fixture".into()),
            read_mode: kpop_native::source_capture::ReadMode::Frozen,
            route: false,
            as_of: None,
        },
        None,
    )
    .unwrap()
    .publish(destination)
    .unwrap();
}
fn bootstrap_copy(source: &Path, destination: &Path) {
    kpop_native::history_node_bootstrap::Plan::prepare(&source.join("GROUNDING.yaml"))
        .unwrap()
        .publish(destination)
        .unwrap();
}
fn cli_with_runtime(root: &Path, args: &[&str]) -> std::process::Output {
    let resources = if let Some(resources) = std::env::var_os("KPOPPER_NATIVE_RESOURCES") {
        std::path::PathBuf::from(resources)
    } else {
        let resources = root.parent().unwrap_or(root).join(format!(
            ".history-test-runtime-{}",
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace")
        ));
        let target = kpop_native::reasoning_runtime::target_name().unwrap();
        fs::create_dir_all(resources.join("reasoning")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!("{target}.kpopper-runtime")),
            resources.join("reasoning").join(format!("{target}.zip")),
        )
        .unwrap();
        resources
    };
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(args)
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env(
            "KPOPPER_NATIVE_CACHE",
            root.parent().unwrap_or(root).join(format!(
                ".history-test-cache-{}",
                root.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("workspace")
            )),
        )
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap()
}

#[test]
fn pull_history_reads_retained_node_bodies_and_clips_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(root.join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
    fs::write(root.join("GROUNDING.yaml"), "meta: {purpose: Fixture}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {}\n").unwrap();
    write(
        root,
        "add-a",
        json!({"kind":"add","id":"p.a","body":{"v":1,"reason":"first"}}),
    );
    write(root, "set-b", json!({"kind":"set","id":"p.a","value":2}));
    write(root, "set-a", json!({"kind":"set","id":"p.a","value":1}));
    let before = tree(root);
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(["--frozen", "pull", "p.a", "--history"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("HISTORICAL SECTION"), "{text}");
    assert!(text.contains("first"), "{text}");
    assert!(text.contains("\"v\":2"), "{text}");
    assert!(
        !text.contains("\"saw\":"),
        "cumulative ancestry leaked into display: {text}"
    );
    assert_eq!(before, tree(root));
    let bounded = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(["--frozen", "pull", "p.a", "--history", "--chars", "1"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        bounded.status.success(),
        "{}",
        String::from_utf8_lossy(&bounded.stderr)
    );
    assert!(String::from_utf8_lossy(&bounded.stdout).contains("omitted"));
    let _ = P::capture_snapshot(root).unwrap();
    fn first_file(dir: &Path) -> Option<std::path::PathBuf> {
        let mut entries = fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if let Some(found) = first_file(&path) {
                    return Some(found);
                }
            } else if path.is_file() {
                return Some(path);
            }
        }
        None
    }
    let object_file = first_file(&root.join(".kpopper/history")).unwrap();
    fs::write(object_file, b"corrupt\n").unwrap();
    let refused = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(["--frozen", "pull", "p.a", "--history"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        !refused.status.success(),
        "corrupt committed history was displayed"
    );
}

#[test]
fn default_history_clip_keeps_the_newest_causal_versions() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(root.join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
    let note = "retained body ".repeat(32);
    fs::write(root.join("GROUNDING.yaml"), "meta: {purpose: Fixture}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {}\n").unwrap();
    write(
        root,
        "add-a",
        json!({"kind":"add","id":"p.a","body":{"v":0,"note":note}}),
    );
    for value in 1..=24 {
        let day = value + 1;
        let op = format!("set-{value}");
        let mut options = opts(&op);
        options.recording_day = format!("2026-09-{day:02}");
        options.recorded_at = format!("2026-09-{day:02}T12:00:00+00:00");
        let action = v(json!({"kind":"set","id":"p.a","value":value}));
        let prepared = W::prepare(root, &action, &options, None).unwrap();
        W::publish(root, &prepared, None, |_| Ok(())).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(["--frozen", "pull", "p.a", "--history"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let history = text.split_once("HISTORICAL SECTION").unwrap().1;
    assert!(
        history.contains("\"v\":24"),
        "newest version omitted: {history}"
    );
    assert!(
        history.contains("PARTIAL:"),
        "expected honest older omissions: {history}"
    );
}

#[test]
fn default_clip_keeps_the_current_judgment_after_many_later_reviews() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(root.join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
    fs::write(root.join("GROUNDING.yaml"), "meta: {purpose: Fixture, reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nreadings: {}\njudgments: {}\n").unwrap();
    let (_cache, runtime) = core_runtime();
    let mut initial = opts("review-base");
    initial.recording_day = "2026-01-01".into();
    initial.recorded_at = "2026-01-01T12:00:00+00:00".into();
    let action = v(json!({"kind":"batch","actions":[
        {"kind":"add","id":"p.a","body":{"v":1},"into":"readings"},
        {"kind":"add","id":"d.j","body":{"verdict":"ready","rests_on":["p.a"],"wrong_if":{"expr":"p.a > 100"}},"into":"judgments"}
    ]}));
    let prepared = W::prepare(root, &action, &initial, Some(&runtime)).unwrap();
    W::publish(root, &prepared, Some(&runtime), |_| Ok(())).unwrap();
    let base = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    for n in 1..=55 {
        let date = base + chrono::Days::new(n);
        let mut options = opts(&format!("review-{n}"));
        options.strict = true;
        options.recording_day = date.to_string();
        options.recorded_at = format!("{date}T12:00:00+00:00");
        let action = v(json!({"kind":"review","id":"d.j","why":format!("review-{n}")}));
        let prepared = W::prepare(root, &action, &options, Some(&runtime)).unwrap();
        W::publish(root, &prepared, Some(&runtime), |_| Ok(())).unwrap();
    }
    let capture = kpop_native::history_node_capture::Capture::read(root).unwrap();
    let subject = map(map(capture.state()).get("subjects").unwrap())
        .get("d.j")
        .unwrap();
    let head = typed_text(map(subject).get("head").unwrap());
    capture.object("d.j", head).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(["--frozen", "pull", "d.j", "--history"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let history = text.split_once("HISTORICAL SECTION").unwrap().1;
    assert!(
        history.contains(&format!("version {head} (storage event")),
        "current judgment head clipped: {history}"
    );
    assert!(
        history.contains("PARTIAL:"),
        "expected later review history omissions: {history}"
    );
    assert!(
        history.contains("review-55"),
        "latest review acts should remain visible: {history}"
    );
}

#[test]
fn node_bootstrap_history_displays_selected_legacy_archive_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let node = tmp.path().join("node");
    fs::create_dir_all(source.join(".kpopper")).unwrap();
    fs::write(source.join("GROUNDING.yaml"), "meta: {purpose: Fixture, reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {p.runs: {v: 0, of: 2026-09-20}}\njudgments:\n  d.done: {verdict: done, rests_on: [p.runs], seen: {p.runs: 0}, wrong_if: {expr: 'p.runs > 0'}}\n").unwrap();
    fs::write(source.join(".kpopper/replaced.yaml"), "d.done:\n- verdict: not demonstrated\n  because: no brief\n  rests_on: [p.runs]\n  seen: {p.runs: 0}\n  wrong_if: 'p.runs > 0'\n  ended: its wrong_if holds (p.runs > 0)\n  day: '2026-09-25'\n  dropped: {p.old: retired after review}\nd.hidden:\n- verdict: private historical reason\n  because: do not include unrelated subject\n  day: '2026-09-24'\n").unwrap();
    bootstrap_copy(&source, &node);
    let before = tree(&node);
    let output = cli_with_runtime(&node, &["--frozen", "pull", "d.done", "--history"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("LEGACY ARCHIVE EVIDENCE"), "{text}");
    for expected in [
        "not demonstrated",
        "no brief",
        "2026-09-25",
        "its wrong_if holds",
        "p.old",
        "retired after review",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(
        !text.contains("do not include unrelated subject"),
        "unselected archive data leaked: {text}"
    );
    let structured = cli_with_runtime(
        &node,
        &["--json", "--frozen", "pull", "d.done", "--history"],
    );
    assert!(
        structured.status.success(),
        "{}",
        String::from_utf8_lossy(&structured.stderr)
    );
    let wrapper: serde_json::Value = serde_json::from_slice(&structured.stdout).unwrap();
    let payload: serde_json::Value =
        serde_json::from_str(wrapper["output"].as_str().unwrap()).unwrap();
    let archived = &payload["historical_section"]["legacy_archive_evidence"][0];
    assert_eq!(archived["source"], "verified_bootstrap_archive");
    assert_eq!(archived["archive_member"], ".kpopper/replaced.yaml");
    assert_eq!(archived["member_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(
        archived["entry"]["ended"],
        "its wrong_if holds (p.runs > 0)"
    );
    assert_eq!(
        archived["entry"]["dropped"]["p.old"],
        "retired after review"
    );
    assert!(
        archived.get("id").is_none(),
        "archive evidence gained a semantic id: {archived}"
    );
    assert_eq!(
        payload["historical_section"]["legacy_archive_ordering"],
        "archive source sequence only; no causal order inferred"
    );
    let clipped = cli_with_runtime(
        &node,
        &[
            "--json",
            "--frozen",
            "pull",
            "d.done",
            "--history",
            "--chars",
            "1",
        ],
    );
    assert!(
        clipped.status.success(),
        "{}",
        String::from_utf8_lossy(&clipped.stderr)
    );
    let clipped_wrapper: serde_json::Value = serde_json::from_slice(&clipped.stdout).unwrap();
    let clipped_payload: serde_json::Value =
        serde_json::from_str(clipped_wrapper["output"].as_str().unwrap()).unwrap();
    assert_eq!(
        clipped_payload["historical_section"]["omitted_legacy_archive_entries"],
        1
    );
    assert_eq!(
        clipped_payload["historical_section"]["legacy_archive_evidence"],
        serde_json::json!([])
    );
    assert_eq!(before, tree(&node));
}

#[test]
fn legacy_migration_history_displays_bound_replaced_member() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let legacy = tmp.path().join("legacy");
    fs::create_dir_all(source.join(".kpopper")).unwrap();
    fs::write(source.join("GROUNDING.yaml"), "meta: {purpose: Fixture, reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {p.runs: {v: 0, of: 2026-09-20}}\njudgments:\n  d.done: {verdict: done, rests_on: [p.runs], seen: {p.runs: 0}, wrong_if: {expr: 'p.runs > 0'}}\n").unwrap();
    fs::write(source.join(".kpopper/replaced.yaml"), "d.done:\n- verdict: not demonstrated\n  because: legacy import reason\n  rests_on: [p.runs]\n  seen: {p.runs: 0}\n  wrong_if: 'p.runs > 0'\n  ended: the old predicate fired\n  day: '2026-09-24'\n  dropped: {p.previous: superseded in legacy source}\nd.unselected:\n- verdict: omit me\n  ended: unselected archive value\n").unwrap();
    legacy_copy(&source, &legacy);
    let before = tree(&legacy);
    let pulled = cli_with_runtime(&legacy, &["--frozen", "pull", "d.done", "--history"]);
    assert!(
        pulled.status.success(),
        "{}",
        String::from_utf8_lossy(&pulled.stderr)
    );
    let text = String::from_utf8_lossy(&pulled.stdout);
    assert!(text.contains("LEGACY ARCHIVE EVIDENCE"), "{text}");
    for expected in [
        "legacy import reason",
        "the old predicate fired",
        "2026-09-24",
        "superseded in legacy source",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(
        !text.contains("unselected archive value"),
        "unselected member leaked: {text}"
    );
    let structured = cli_with_runtime(
        &legacy,
        &["--json", "--frozen", "pull", "d.done", "--history"],
    );
    assert!(
        structured.status.success(),
        "{}",
        String::from_utf8_lossy(&structured.stderr)
    );
    let wrapper: serde_json::Value = serde_json::from_slice(&structured.stdout).unwrap();
    let payload: serde_json::Value =
        serde_json::from_str(wrapper["output"].as_str().unwrap()).unwrap();
    let archived = &payload["historical_section"]["legacy_archive_evidence"][0];
    assert_eq!(archived["source"], "verified_history_import_member");
    assert_eq!(archived["archive_member"], ".kpopper/replaced.yaml");
    assert_eq!(
        archived["entry"]["dropped"]["p.previous"],
        "superseded in legacy source"
    );
    assert_eq!(archived["member_sha256"].as_str().unwrap().len(), 64);
    assert!(
        archived.get("id").is_none(),
        "archive evidence gained semantic id: {archived}"
    );
    assert_eq!(before, tree(&legacy));
}

#[test]
fn core_pull_reads_active_history_v1_and_json_budget_reports_omissions() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let copy = tmp.path().join("copy");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("GROUNDING.yaml"), "meta: {purpose: Fixture, reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.a: {v: 1, note: 'historic body with details'}\n").unwrap();
    legacy_copy(&source, &copy);
    let authority = fs::read_to_string(copy.join(".kpopper/history.yaml")).unwrap();
    assert!(authority.contains("history/v1"), "{authority}");
    let before = tree(&copy);
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&copy)
        .args(["--frozen", "pull", "p.a", "--history"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("HISTORICAL SECTION"), "{text}");
    assert!(text.contains("historic body"), "{text}");
    let structured = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&copy)
        .args([
            "--json",
            "--frozen",
            "pull",
            "p.a",
            "--history",
            "--budget",
            "1",
        ])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        structured.status.success(),
        "{}",
        String::from_utf8_lossy(&structured.stderr)
    );
    let wrapped: serde_json::Value = serde_json::from_slice(&structured.stdout).unwrap();
    assert!(
        wrapped["output"].as_str().unwrap_or("").contains("omitted"),
        "{wrapped}"
    );
    let missing = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&copy)
        .args(["--json", "--frozen", "pull", "p.missing", "--history"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    let refused: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert!(
        !refused["output"]
            .as_str()
            .unwrap_or("")
            .contains("HISTORICAL SECTION"),
        "{refused}"
    );
    assert_eq!(before, tree(&copy));
}

#[test]
fn ordinary_pull_reads_active_history_v1() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let copy = tmp.path().join("copy");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("GROUNDING.yaml"),
        "meta: {purpose: Fixture}\nknown:\n  p.a: {v: 1, note: 'ordinary retained body'}\n",
    )
    .unwrap();
    legacy_copy(&source, &copy);
    let before = tree(&copy);
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&copy)
        .args(["--frozen", "pull", "p.a", "--history"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("HISTORICAL SECTION"), "{text}");
    assert!(text.contains("ordinary retained body"), "{text}");
    let structured = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&copy)
        .args(["--json", "--frozen", "pull", "p.a", "--history"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        structured.status.success(),
        "{}",
        String::from_utf8_lossy(&structured.stderr)
    );
    let wrapper: serde_json::Value = serde_json::from_slice(&structured.stdout).unwrap();
    let payload: serde_json::Value =
        serde_json::from_str(wrapper["output"].as_str().unwrap()).unwrap();
    assert!(payload["output"].is_string(), "{payload}");
    assert_eq!(
        payload["historical_section"]["source"],
        "captured_committed_history"
    );
    assert!(
        !payload["output"]
            .as_str()
            .unwrap()
            .contains("HISTORICAL SECTION"),
        "{payload}"
    );
    assert_eq!(before, tree(&copy));
}

#[test]
fn judgment_history_keeps_reversal_request_reason_and_dependency_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(root.join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
    fs::write(root.join("GROUNDING.yaml"), "meta: {purpose: Fixture, reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nreadings: {}\njudgments: {}\n").unwrap();
    let (_cache, runtime) = core_runtime();
    let mut initial = opts("core-batch");
    let action = v(json!({"kind":"batch","actions":[
        {"kind":"add","id":"p.a","body":{"v":1},"into":"readings"},
        {"kind":"add","id":"d.b","body":{"verdict":"ready","rests_on":["p.a"],"wrong_if":{"expr":"p.a > 3"}},"into":"judgments"}
    ]}));
    let prepared = W::prepare(root, &action, &initial, Some(&runtime)).unwrap();
    W::publish(root, &prepared, Some(&runtime), |_| Ok(())).unwrap();
    for (op, verdict, deps, because) in [
        (
            "proposal-b",
            "hold",
            json!([]),
            "request: remove obsolete p.a basis and hold",
        ),
        (
            "proposal-a",
            "ready",
            json!(["p.a"]),
            "request: restore p.a after retest",
        ),
    ] {
        let proposal = v(
            json!({"kind":"proposal","id":"d.b","into":"judgments","body":{"verdict":verdict,"rests_on":deps,"wrong_if":{"expr":"p.a > 3"}},"because":because}),
        );
        let mut step_options = opts(op);
        step_options.strict = true;
        let prepared = W::prepare(root, &proposal, &step_options, Some(&runtime)).unwrap();
        W::publish(root, &prepared, Some(&runtime), |_| Ok(())).unwrap();
        let capture = kpop_native::history_node_capture::Capture::read(root).unwrap();
        let state = map(map(capture.state()).get("subjects").unwrap());
        let judgment = map(state.get("d.b").unwrap());
        let V::List(proposals) = judgment.get("proposals").unwrap() else {
            panic!()
        };
        let proposal_id = typed_text(proposals.last().unwrap());
        let V::Text(head) = judgment.get("head").unwrap() else {
            panic!()
        };
        let correction = v(
            json!({"kind":"correct","id":"d.b","of":proposal_id,"over":[head],"because":format!("accepted request: {because}")}),
        );
        let accept_op = if op == "proposal-b" {
            "accept-b"
        } else {
            "accept-a"
        };
        let mut accept_options = opts(accept_op);
        accept_options.strict = true;
        let prepared = W::prepare(root, &correction, &accept_options, Some(&runtime)).unwrap();
        W::publish(root, &prepared, Some(&runtime), |_| Ok(())).unwrap();
        initial.operation = accept_op.into();
    }
    let before = tree(root);
    let read = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(["--frozen", "pull", "d.b", "--history"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    let text = String::from_utf8_lossy(&read.stdout);
    let capture = kpop_native::history_node_capture::Capture::read(root).unwrap();
    let judgment = map(map(capture.state()).get("subjects").unwrap())
        .get("d.b")
        .unwrap();
    let current_head = typed_text(map(judgment).get("head").unwrap());
    capture.object("d.b", current_head).unwrap();
    assert!(text.contains(&format!("version {current_head}")), "{text}");
    for expected in [
        "\"verdict\":\"ready\"",
        "\"verdict\":\"hold\"",
        "request: remove obsolete p.a basis and hold",
        "accepted request:",
        "\"rests_on\":[]",
        "\"rests_on\":[\"p.a\"]",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    let structured = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(["--json", "--frozen", "pull", "d.b", "--history"])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(structured.status.success());
    let wrapper: serde_json::Value = serde_json::from_slice(&structured.stdout).unwrap();
    let payload: serde_json::Value =
        serde_json::from_str(wrapper["output"].as_str().unwrap()).unwrap();
    let versions = payload["historical_section"]["versions"]
        .as_array()
        .unwrap();
    let semantic = versions
        .iter()
        .find(|row| row["id"] == current_head)
        .unwrap();
    assert_ne!(semantic["id"], semantic["storage_event_id"]);
    assert_eq!(semantic["id"], current_head);
    for row in versions.iter().filter(|row| row["subject"] == "d.b") {
        let id = row["id"].as_str().unwrap();
        capture.object("d.b", id).unwrap();
    }
    assert_eq!(before, tree(root));
}

#[test]
fn public_drop_reason_survives_active_legacy_and_node_history() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("GROUNDING.yaml"), "meta: {purpose: Fixture, reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {p.x: {v: 1, of: 2026-09-20}, p.y: {v: 2, of: 2026-09-20}}\njudgments:\n  d.j: {verdict: ready, rests_on: [p.x], wrong_if: {expr: 'p.x < 3'}}\n").unwrap();
    for name in ["legacy", "node"] {
        let target = tmp.path().join(name);
        if name == "legacy" {
            legacy_copy(&source, &target);
        } else {
            let migrated = cli_with_runtime(
                &source,
                &["history", "migrate", "--to", target.to_str().unwrap()],
            );
            assert!(
                migrated.status.success(),
                "{}",
                String::from_utf8_lossy(&migrated.stderr)
            );
        }
        let authority = fs::read_to_string(target.join(".kpopper/history.yaml")).unwrap();
        assert!(
            authority.contains(if name == "node" {
                "node-history/v1"
            } else {
                "history/v1"
            }),
            "{authority}"
        );
        let drop = "distinctive original --drop explanation";
        let today = chrono::Local::now().date_naive().to_string();
        let added = cli_with_runtime(
            &target,
            &[
                "add",
                "d.j",
                "verdict=hold",
                "rests_on=[p.y]",
                "wrong_if={expr: 'p.y > 3'}",
                "--as-of",
                &today,
                "--drop",
                &format!("p.x: {drop}"),
            ],
        );
        assert!(
            added.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&added.stdout),
            String::from_utf8_lossy(&added.stderr)
        );
        let pulled = cli_with_runtime(&target, &["--frozen", "pull", "d.j", "--history"]);
        assert!(
            pulled.status.success(),
            "{}",
            String::from_utf8_lossy(&pulled.stderr)
        );
        let text = String::from_utf8_lossy(&pulled.stdout);
        assert!(text.contains(drop), "{name}: {text}");
    }
}
