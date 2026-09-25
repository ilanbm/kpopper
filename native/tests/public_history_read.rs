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

fn cli_with_runtime(root: &Path, args: &[&str]) -> std::process::Output {
    let resources = root.join(".history-test-runtime");
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
        .args(args)
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env("KPOPPER_NATIVE_CACHE", root.join(".history-test-cache"))
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
fn core_pull_reads_active_history_v1_and_json_budget_reports_omissions() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let copy = tmp.path().join("copy");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("GROUNDING.yaml"), "meta: {purpose: Fixture, reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.a: {v: 1, note: 'historic body with details'}\n").unwrap();
    let migrated = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&source)
        .args(["history", "migrate", "--to", copy.to_str().unwrap()])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
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
    let migrated = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&source)
        .args(["history", "migrate", "--to", copy.to_str().unwrap()])
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .output()
        .unwrap();
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
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
    for (name, extra) in [("legacy", vec![]), ("node", vec!["--node-history"])] {
        let target = tmp.path().join(name);
        let mut args = vec!["history", "migrate"];
        args.extend(extra.iter().copied());
        args.extend(["--to", target.to_str().unwrap()]);
        let migrated = cli_with_runtime(&source, &args);
        assert!(
            migrated.status.success(),
            "{}",
            String::from_utf8_lossy(&migrated.stderr)
        );
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
