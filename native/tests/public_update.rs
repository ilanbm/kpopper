use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn record(path: &Path) {
    fs::write(
        path.join("GROUNDING.yaml"),
        r#"meta:
  updated: 2026-09-01
sources:
  s.old:
    url: https://example.test/old
    read: 2026-09-01
known:
  p.price:
    v: 10
    from: s.old
    of: 2026-09-01
judgments:
  d.price:
    rests_on: [p.price]
    verdict: price is acceptable
    wrong_if: p.price > 11
    seen: {p.price: 10}
"#,
    )
    .unwrap();
}

fn run(root: &Path, state: &Path, report: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "update",
            "--file",
            "-",
            "--state-dir",
            state.to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(report).unwrap().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn hash(path: &Path) -> String {
    kpop_native::identity::sha256(&fs::read(path).unwrap())
}

fn captured_id(entry: &Path, event_id: &str) -> String {
    kpop_native::identity::sha256(
        format!("{}\0{event_id}", entry.canonicalize().unwrap().display()).as_bytes(),
    )[..32]
        .into()
}

#[test]
fn recursive_alias_refusal_preserves_the_entire_record() {
    let temp = tempfile::tempdir().unwrap();
    record(temp.path());
    let entry = temp.path().join("GROUNDING.yaml");
    let original = format!(
        "{}custom: &loop {{p.cycle: *loop}}\n",
        fs::read_to_string(&entry).unwrap()
    );
    fs::write(&entry, &original).unwrap();
    let output = run(
        temp.path(),
        &temp.path().join("state"),
        &json!({
            "event_id":"cyclic-record", "date":"2026-09-21", "source_quote":"price 12",
            "source":"s.old", "at":"entire captured report", "record_sha256":hash(&entry),
            "updates":[{"kind":"add","id":"p.new","body":{"v":12}}],
        }),
    );
    assert!(!output.status.success());
    let diagnostic = [output.stdout, output.stderr].concat();
    assert!(
        String::from_utf8_lossy(&diagnostic).contains("recursive_yaml_alias"),
        "{}",
        String::from_utf8_lossy(&diagnostic)
    );
    assert_eq!(fs::read(&entry).unwrap(), original.as_bytes());
}

#[test]
fn source_only_custom_collection_accepts_its_first_reading() {
    let temp = tempfile::tempdir().unwrap();
    let entry = temp.path().join("GROUNDING.yaml");
    fs::write(
        &entry,
        "evidence:\n  s.old: {file: old.txt, read: 2026-09-01}\n",
    )
    .unwrap();
    let output = run(
        temp.path(),
        &temp.path().join("state"),
        &json!({
            "event_id":"source-only", "date":"2026-09-20", "source_quote":"price 12",
            "source":"s.old", "at":"entire captured report", "record_sha256":hash(&entry),
            "updates":[{"kind":"add","id":"p.price","body":{"v":12}}],
        }),
    );

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["state"], "applied");
    let saved = fs::read_to_string(&entry).unwrap();
    assert!(saved.contains("evidence:\n"));
    assert!(saved.contains("p.price:\n"));
    assert!(!saved.contains("judgments:"));
}

#[test]
fn empty_schema_collections_accept_the_first_batch_update() {
    let temp = tempfile::tempdir().unwrap();
    let entry = temp.path().join("GROUNDING.yaml");
    fs::write(
        &entry,
        "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nsources: {}\nknown: {}\njudgments: {}\n",
    )
    .unwrap();
    let output = run(
        temp.path(),
        &temp.path().join("state"),
        &json!({
            "event_id":"empty-schema", "date":"2026-09-20", "source_quote":"price 12",
            "record_sha256":hash(&entry),
            "updates":[{"kind":"add","id":"p.price","body":{"v":12}}],
        }),
    );

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["state"], "applied");
    let saved = fs::read_to_string(&entry).unwrap();
    assert!(saved.contains("sources:\n  s.ingest_"));
    assert!(saved.contains("known:\n  p.price:\n"));
    assert!(saved.contains("judgments: {}\n"));
}

#[test]
fn no_schema_custom_value_collection_stays_unreadable() {
    let temp = tempfile::tempdir().unwrap();
    let entry = temp.path().join("GROUNDING.yaml");
    fs::write(
        &entry,
        "evidence:\n  s.old: {file: old.txt, read: 2026-09-01}\nparameters:\n  p.existing: {v: 1}\n",
    )
    .unwrap();
    let before = fs::read(&entry).unwrap();
    let output = run(
        temp.path(),
        &temp.path().join("state"),
        &json!({
            "event_id":"ambiguous-custom", "date":"2026-09-20", "source_quote":"price 12",
            "source":"s.old", "at":"entire captured report", "record_sha256":hash(&entry),
            "updates":[{"kind":"add","id":"p.price","body":{"v":12}}],
        }),
    );

    assert_eq!(output.status.code(), Some(1));
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["state"], "needs_primary");
    assert!(receipt["reason"].as_str().unwrap().contains("ordinary_fields_unreadable"));
    assert_eq!(fs::read(&entry).unwrap(), before);
}

#[test]
fn explicit_question_keeps_its_target_and_never_applies_the_value() {
    let temp = tempfile::tempdir().unwrap();
    record(temp.path());
    let entry = temp.path().join("GROUNDING.yaml");
    let before = fs::read(&entry).unwrap();
    let state = temp.path().join("state");
    let output = run(
        temp.path(),
        &state,
        &json!({
            "event_id":"question-only", "date":"2026-09-20", "source_quote":"Please verify this reading",
            "kind":"question", "question":"Where was the price observed?", "target":"p.price", "value":12,
        }),
    );
    assert_eq!(output.status.code(), Some(1));
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["state"], "needs_primary");
    let signals = receipt["signal_ids"].as_array().unwrap();
    assert_eq!(signals.len(), 1);
    let signal: Value = serde_json::from_slice(
        &fs::read(
            state
                .join("signals")
                .join(format!("{}.json", signals[0].as_str().unwrap())),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(signal["target"], "p.price");
    assert_eq!(signal["question"], "Where was the price observed?");
    assert_eq!(signal["source_quote"], "Please verify this reading");
    assert_eq!(fs::read(entry).unwrap(), before);
}

#[test]
fn ordinary_report_persists_declared_scope_in_python_order() {
    let temp = tempfile::tempdir().unwrap();
    record(temp.path());
    let entry = temp.path().join("GROUNDING.yaml");
    let state = temp.path().join("state");
    let output = run(
        temp.path(),
        &state,
        &json!({
            "event_id":"scoped-ordinary", "date":"2026-09-20", "source_quote":"price 12",
            "target":"p.price", "value":12,
            "shareability":"project",
            "scope":{"kind":"feature","environment":"example"},
        }),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let event = captured_id(&entry, "scoped-ordinary");
    let source_file = state
        .join("sources")
        .join(format!("{event}.txt"))
        .canonicalize()
        .unwrap();
    assert_eq!(
        fs::read_to_string(entry).unwrap(),
        format!(
            "meta:\n  updated: 2026-09-20\nsources:\n  s.ingest_{event}:\n    name: \"Captured report\"\n    file: \"{}\"\n    read: \"2026-09-20\"\n    recorded_for: \"Update p.price from this captured report.\"\n  s.old:\n    url: https://example.test/old\n    read: 2026-09-01\nknown:\n  p.price:\n    v: 12\n    from: s.ingest_{event}\n    of: \"2026-09-20\"\n    at: \"entire captured report\"\n    scope: {{environment: example, kind: feature}}\njudgments:\n  d.price:\n    rests_on: [p.price]\n    verdict: price is acceptable\n    wrong_if: p.price > 11\n    seen: {{p.price: 10}}\n",
            source_file.display().to_string().replace('\\', "\\\\")
        )
    );
}

#[test]
fn captured_source_at_requires_an_explicit_cited_source() {
    for (event_id, source, source_metadata, target_source) in [
        ("at-without-source", None, "", None),
        (
            "at-with-cited-source",
            Some("s.old"),
            "    from: s.old\n    at: \"page 7\"\n",
            Some("s.old"),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        record(temp.path());
        let entry = temp.path().join("GROUNDING.yaml");
        let state = temp.path().join("state");
        let mut report = json!({
            "event_id":event_id, "date":"2026-09-20", "source_quote":"price 12",
            "target":"p.price", "value":12, "at":"page 7",
        });
        if let Some(source) = source {
            report["source"] = json!(source);
            report["record_sha256"] = json!(hash(&entry));
        }
        let output = run(temp.path(), &state, &report);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let event = captured_id(&entry, event_id);
        let source_file = state
            .join("sources")
            .join(format!("{event}.txt"))
            .canonicalize()
            .unwrap();
        let target_source = target_source
            .map(str::to_owned)
            .unwrap_or_else(|| format!("s.ingest_{event}"));
        assert_eq!(
            fs::read_to_string(&entry).unwrap(),
            format!(
                "meta:\n  updated: 2026-09-20\nsources:\n  s.ingest_{event}:\n    name: \"Captured report\"\n    file: \"{}\"\n    read: \"2026-09-20\"\n    recorded_for: \"Update p.price from this captured report.\"\n{source_metadata}  s.old:\n    url: https://example.test/old\n    read: 2026-09-01\nknown:\n  p.price:\n    v: 12\n    from: {target_source}\n    of: 2026-09-20\n    at: \"page 7\"\njudgments:\n  d.price:\n    rests_on: [p.price]\n    verdict: price is acceptable\n    wrong_if: p.price > 11\n    seen: {{p.price: 10}}\n",
                source_file.display().to_string().replace('\\', "\\\\")
            ),
            "{event_id}"
        );
    }
}

#[test]
fn advanced_cli_routes_project_private_and_local_reports_and_replays_success() {
    for (kind, private, expected) in [
        ("project", false, "project_captured"),
        ("project", true, "needs_primary"),
        ("feature", false, "applied"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        record(&root);
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "GROUNDING.yaml"]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
        ]);
        let entry = root.join("GROUNDING.yaml");
        let before = fs::read(&entry).unwrap();
        let state = temp.path().join("state");
        let mut report = json!({"event_id":"advanced-cli", "date":"2026-09-20", "source_quote":"price 12", "target":"p.price", "value":12,
            "shareability":"project", "scope":{"kind":kind,"environment":"example"}});
        if private {
            report["privacy"] = json!(true);
        }
        let first = run(&root, &state, &report);
        let result: Value = serde_json::from_slice(&first.stdout).unwrap_or_else(|e| {
            panic!(
                "{kind}/{private}: {e}; {}",
                String::from_utf8_lossy(&first.stderr)
            )
        });
        assert_eq!(result["state"], expected, "{result}");
        assert_eq!(first.status.code(), Some(if private { 1 } else { 0 }));
        if expected == "applied" {
            let after = fs::read_to_string(&entry).unwrap();
            let event = captured_id(&entry, "advanced-cli");
            let source_file = state
                .join("sources")
                .join(format!("{event}.txt"))
                .canonicalize()
                .unwrap();
            assert_eq!(
                after,
                format!(
                    "meta:\n  updated: 2026-09-20\nsources:\n  s.ingest_{event}:\n    name: \"Captured report\"\n    file: \"{}\"\n    read: \"2026-09-20\"\n    recorded_for: \"Update p.price from this captured report.\"\n  s.old:\n    url: https://example.test/old\n    read: 2026-09-01\nknown:\n  p.price:\n    v: 12\n    from: s.ingest_{event}\n    of: \"2026-09-20\"\n    at: \"entire captured report\"\n    scope: {{environment: example, kind: feature}}\njudgments:\n  d.price:\n    rests_on: [p.price]\n    verdict: price is acceptable\n    wrong_if: p.price > 11\n    seen: {{p.price: 10}}\n",
                    source_file.display().to_string().replace('\\', "\\\\")
                )
            );
        } else {
            assert_eq!(fs::read(&entry).unwrap(), before);
        }
        let replay = run(&root, &state, &report);
        assert_eq!(replay.status.code(), first.status.code());
        assert_eq!(replay.stdout, first.stdout);
        if private {
            assert!(
                !Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args([
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        "refs/kpopper/pending_grounding"
                    ])
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
}

#[test]
fn source_and_two_dependent_writes_publish_once_and_retry_exactly() {
    let temp = tempfile::tempdir().unwrap();
    record(temp.path());
    let entry = temp.path().join("GROUNDING.yaml");
    let state = temp.path().join("state");
    let report = json!({
        "event_id":"batch-one", "record_sha256":hash(&entry), "date":"2026-09-20", "source_quote":"price 12, fee 3",
        "shareability":"project", "scope":{"kind":"feature","environment":"example"},
        "updates":[
            {"kind":"set","id":"p.price","value":12},
            {"kind":"add","id":"p.fee","body":{"v":3}},
            {"kind":"add","id":"p.total","body":{"rule":"p.price + p.fee"}}
        ]
    });
    let first = run(temp.path(), &state, &report);
    assert!(
        first.status.success(),
        "{}{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let receipt: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(receipt["state"], "applied");
    assert_eq!(receipt["newly_fired_judgments"], json!(["d.price"]));
    assert_eq!(receipt["reach"]["judgments"], json!(["d.price"]));
    let after = fs::read_to_string(&entry).unwrap();
    assert!(
        after.contains("scope: {environment: example, kind: feature}"),
        "{after}"
    );
    assert_eq!(after.matches("scope: {environment: example, kind: feature}").count(), 3);
    assert!(
        receipt["diagnostics"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
    );
    assert_eq!(receipt["target_after_sha256"].as_str().unwrap().len(), 64);
    assert!(
        state
            .join("signals")
            .join(format!(
                "{}.json",
                receipt["signal_ids"][0].as_str().unwrap()
            ))
            .is_file()
    );
    assert!(receipt["mutation"]["receipt"]["before"]["batch"].is_object());
    let after = fs::read(&entry).unwrap();
    let text = String::from_utf8(after.clone()).unwrap();
    assert!(text.contains("p.fee:"));
    assert!(text.contains("p.total:"));
    assert!(text.contains("v: 12"));
    let again = run(temp.path(), &state, &report);
    assert!(again.status.success());
    assert_eq!(again.stdout, first.stdout);
    assert_eq!(fs::read(entry).unwrap(), after);
}

#[test]
fn late_failure_and_private_report_retain_source_without_shared_changes() {
    for (name, extra, last) in [
        (
            "late",
            json!({}),
            json!({"kind":"add","id":"c.bad","body":{"rests_on":["p.missing"],"verdict":"bad","wrong_if":"p.missing > 0"}}),
        ),
        (
            "private",
            json!({"privacy":"private"}),
            json!({"kind":"add","id":"p.ok","body":{"v":2}}),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        record(temp.path());
        let entry = temp.path().join("GROUNDING.yaml");
        let before = fs::read(&entry).unwrap();
        let state = temp.path().join("state");
        let mut report = json!({"event_id":name,"record_sha256":hash(&entry),"date":"2026-09-20","source_quote":"retained","updates":[{"kind":"set","id":"p.price","value":12},last]});
        report
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let output = run(temp.path(), &state, &report);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt["state"], "needs_primary");
        assert_eq!(fs::read(&entry).unwrap(), before);
        assert_eq!(
            fs::read(receipt["source_file"].as_str().unwrap()).unwrap(),
            b"retained"
        );
    }
}

#[test]
fn stale_primary_hash_retains_report_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    record(temp.path());
    let entry = temp.path().join("GROUNDING.yaml");
    let before = fs::read(&entry).unwrap();
    let output = run(
        temp.path(),
        &temp.path().join("state"),
        &json!({
            "event_id":"stale", "record_sha256":"0000000000000000000000000000000000000000000000000000000000000000",
            "date":"2026-09-20", "source_quote":"new fact", "updates":[{"kind":"add","id":"p.new","body":{"v":2}}]
        }),
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read(entry).unwrap(), before);
}

#[test]
fn batch_arrangement_judgments_require_primary_review_and_emit_a_question() {
    let temp = tempfile::tempdir().unwrap();
    let cases: Value =
        serde_json::from_slice(include_bytes!("fixtures/ordinary-page-facts.json")).unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "page-arrangement-take-dry")
        .unwrap();
    for (relative, raw) in case["files"].as_object().unwrap() {
        if relative.contains("hypotheses/") {
            continue;
        }
        let path = temp.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw.as_str().unwrap()).unwrap();
    }
    let entry = temp.path().join("GROUNDING.yaml");
    let arranged = fs::read_to_string(&entry).unwrap()
        .replace("page.unserved], seen: {s.now: read 2026-09-19, page.unserved: 0}, wrong_if: page.unserved > 0",
            "graph.judgments], seen: {s.now: read 2026-09-19, graph.judgments: 1}, wrong_if: graph.judgments > 1");
    fs::write(&entry, &arranged).unwrap();
    let output = run(
        temp.path(),
        &temp.path().join("state"),
        &json!({
            "event_id":"batch-arrangement", "record_sha256":hash(&entry), "date":"2026-09-20",
            "source_quote":"A second request arrived and the layout was re-decided.",
            "updates":[
                {"kind":"add","id":"c.extra","body":{"rests_on":["p.value"],"verdict":"extra","wrong_if":"p.value > 100"}},
                {"kind":"add","id":"v.layout","body":{"rests_on":["s.now","graph.judgments"],"verdict":"re-decided","wrong_if":"graph.judgments > 1"}}
            ]
        }),
    );
    assert_eq!(output.status.code(), Some(1));
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["state"], "needs_primary");
    assert_eq!(
        receipt["reason"],
        "batch judgments about reader/page counts require primary review; use domain entries as premises"
    );
    let signals = receipt["signal_ids"].as_array().unwrap();
    assert_eq!(signals.len(), 1);
    let signal: Value = serde_json::from_slice(
        &fs::read(
            temp.path()
                .join("state/signals")
                .join(format!("{}.json", signals[0].as_str().unwrap())),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(signal["category"], "question");
    assert_eq!(signal["reason"], receipt["reason"]);
    assert_eq!(signal["question"], receipt["reason"]);
    assert_eq!(fs::read_to_string(entry).unwrap(), arranged);
}

#[test]
fn private_dependency_closure_is_retained_without_a_shared_write() {
    let temp = tempfile::tempdir().unwrap();
    record(temp.path());
    let entry = temp.path().join("GROUNDING.yaml");
    let private = fs::read_to_string(&entry).unwrap().replace(
        "    read: 2026-09-01\nknown:",
        "    read: 2026-09-01\n    private: true\nknown:",
    );
    fs::write(&entry, private).unwrap();
    let before = fs::read(&entry).unwrap();
    let output = run(
        temp.path(),
        &temp.path().join("state"),
        &json!({
            "event_id":"private-closure", "date":"2026-09-20", "source_quote":"price 12",
            "updates":[{"kind":"set","id":"p.price","value":12}]
        }),
    );
    assert_eq!(output.status.code(), Some(1));
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(receipt["reason"].as_str().unwrap().contains("private"));
    assert_eq!(fs::read(entry).unwrap(), before);
}

#[test]
fn private_shareability_and_nested_permissions_never_enter_shared_record() {
    for marker in [
        json!({"shareability":"private"}),
        json!({"shareability":"unclear"}),
        json!({"shareability":null}),
        json!({"scope":{"kind":"project","environment":"fixture","permissions":{"privacy":"private"}}}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        record(temp.path());
        let entry = temp.path().join("GROUNDING.yaml");
        let before = fs::read(&entry).unwrap();
        let state = temp.path().join("state");
        let mut report = json!({"event_id":"private-envelope", "date":"2026-09-20", "source_quote":"private fixture evidence", "target":"p.price", "value":12});
        report
            .as_object_mut()
            .unwrap()
            .extend(marker.as_object().unwrap().clone());
        let output = run(temp.path(), &state, &report);
        assert_eq!(output.status.code(), Some(1));
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt["state"], "needs_primary");
        assert_eq!(fs::read(&entry).unwrap(), before);
        assert!(
            fs::read_dir(state.join("journals"))
                .unwrap()
                .next()
                .is_none()
        );
        assert_eq!(
            fs::read(receipt["source_file"].as_str().unwrap()).unwrap(),
            b"private fixture evidence"
        );
    }
}
