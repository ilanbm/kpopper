use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
fn ok(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn tree(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().iter()
                        .map(|part| part.to_str().unwrap()).collect::<Vec<_>>().join("/"),
                    kpop_native::identity::sha256(&fs::read(path).unwrap()),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Compact views are canonical tagged YAML; edit the decoded reading, as a person's editor would.
fn edit_reading(entry: &Path, subject: &str, value: i64) -> i64 {
    use kpop_native::value::TypedValue as V;
    let mut document =
        kpop_native::history_yaml::decode_document(&fs::read(entry).unwrap()).unwrap();
    let V::Map(fields) = &mut document else { panic!("record mapping") };
    let V::Map(known) = fields.get_mut("known").unwrap() else { panic!("known mapping") };
    let V::Map(body) = known.get_mut(subject).unwrap() else { panic!("reading body") };
    let old = body["v"].to_json().unwrap().as_i64().unwrap();
    body.insert("v".into(), V::from_json(&serde_json::json!(value)).unwrap());
    fs::write(entry, kpop_native::history_yaml::encode_document(&document).unwrap()).unwrap();
    old
}

#[test]
fn public_copy_preview_publish_reconcile_and_rebuild_preserve_source_and_history() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("GROUNDING.yaml"),
        "meta: {purpose: Fixture}\nknown: {p.x: {v: 1, of: 2026-09-19}}\n",
    )
    .unwrap();
    let before = tree(&source);
    let preview = ok(run(&source, &["history", "migrate"]));
    assert_eq!(preview["state"], "preview");
    assert_eq!(preview["source_read_mode"], "frozen");
    assert_eq!(tree(&source), before);
    let copy = tmp.path().join("copy");
    ok(run(
        &source,
        &["history", "migrate", "--to", copy.to_str().unwrap()],
    ));
    assert_eq!(tree(&source), before);
    let status = ok(run(&copy, &["history", "status"]));
    assert_eq!(status["commits"], 1);
    assert_eq!(status["subjects"]["p.x"]["acceptance"], "accepted");
    let captured = tree(&copy);
    let reconcile = ok(run(&copy, &["history", "reconcile"]));
    assert_eq!(reconcile["rebuild_safe"], true);
    assert_eq!(tree(&copy), captured);
    assert_eq!(ok(run(&copy, &["history", "rebuild"]))["state"], "rebuilt");
    for (name, bytes) in captured {
        if name.contains("history/") || name.contains("history-commits/") {
            assert_eq!(
                kpop_native::identity::sha256(&fs::read(copy.join(name)).unwrap()),
                bytes
            );
        }
    }
    let prior = tree(&copy);
    let refused = run(
        &source,
        &["history", "migrate", "--to", copy.to_str().unwrap()],
    );
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&refused.stdout).unwrap()["state"],
        "refused"
    );
    assert_eq!(tree(&copy), prior);
}

#[test]
fn public_history_rejects_authored_view_edits_without_losing_them() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("GROUNDING.yaml"), "known: {p.x: {v: 1}}\n").unwrap();
    let copy = tmp.path().join("copy");
    ok(run(
        &source,
        &["history", "migrate", "--to", copy.to_str().unwrap()],
    ));
    let entry = copy.join("GROUNDING.yaml");
    assert_eq!(edit_reading(&entry, "p.x", 7), 1);
    let before = tree(&copy);
    assert_eq!(
        ok(run(&copy, &["history", "reconcile"]))["rebuild_safe"],
        false
    );
    let output = run(&copy, &["history", "rebuild"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["code"],
        "unresolved_view_edit"
    );
    let mut after = tree(&copy);
    // Mutation attempts create the policy lock even when the authored bytes refuse.
    assert_eq!(
        after.remove(".kpopper/project.lock"),
        Some(kpop_native::identity::sha256(b""))
    );
    assert_eq!(after, before);
}

#[test]
fn explicit_record_and_copy_only_option_validation() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("custom.yml"), "known: {p.x: {v: 2}}\n").unwrap();
    let before = tree(tmp.path());
    let preview = ok(run(
        tmp.path(),
        &[
            "history",
            "migrate",
            "--record",
            "custom.yml",
            "--read-mode",
            "live",
            "--json",
        ],
    ));
    assert_eq!(preview["source_read_mode"], "live");
    assert!(preview["record"].as_str().unwrap().ends_with("custom.yml"));
    assert_eq!(tree(tmp.path()), before);
    let output = run(tmp.path(), &["history", "rebuild", "--to", "unused"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["detail"],
        "--to belongs to history migrate"
    );
    assert_eq!(tree(tmp.path()), before);
}

fn imported(root: &Path, record: &str) -> std::path::PathBuf {
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("GROUNDING.yaml"), record).unwrap();
    let copy = root.join("copy");
    ok(run(
        &source,
        &["history", "migrate", "--to", copy.to_str().unwrap()],
    ));
    copy
}

#[test]
fn explicit_acts_and_edited_view_proposals_retain_old_claims() {
    let tmp = tempfile::tempdir().unwrap();
    let copy = imported(tmp.path(), "known: {p.x: {v: 1}}\n");
    let status = ok(run(&copy, &["history", "status"]));
    let version = status["subjects"]["p.x"]["heads"][0].as_str().unwrap();
    let retired = ok(run(
        &copy,
        &[
            "history",
            "retire",
            "--subject",
            "p.x",
            "--of",
            version,
            "--because",
            "no longer used",
        ],
    ));
    assert_eq!(retired["state"], "committed");
    assert_eq!(
        ok(run(&copy, &["history", "status"]))["subjects"]["p.x"]["acceptance"],
        "retired"
    );
    ok(run(
        &copy,
        &[
            "history",
            "accept",
            "--subject",
            "p.x",
            "--of",
            version,
            "--because",
            "explicitly restore",
        ],
    ));
    assert_eq!(
        ok(run(&copy, &["history", "status"]))["subjects"]["p.x"]["acceptance"],
        "accepted"
    );
    let entry = copy.join("GROUNDING.yaml");
    assert_eq!(edit_reading(&entry, "p.x", 8), 1);
    let result = ok(run(
        &copy,
        &[
            "history",
            "reconcile",
            "--record-proposals",
            "--proposal-subject",
            "p.x",
            "--because",
            "observed changed reading",
            "--by",
            "fixture",
        ],
    ));
    assert_eq!(result["state"], "proposed");
    let status = ok(run(&copy, &["history", "status"]));
    assert_eq!(status["subjects"]["p.x"]["heads"][0], version);
    // The accepted reading is unchanged and the view verifies again.
    assert_eq!(edit_reading(&entry, "p.x", 1), 1);
    assert_eq!(ok(run(&copy, &["history", "reconcile"]))["rebuild_safe"], true);
}

#[test]
fn private_historical_claim_stays_a_private_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let copy = imported(tmp.path(), "known: {p.x: {v: 1, private: true}}\n");
    let status = ok(run(&copy, &["history", "status"]));
    let version = status["subjects"]["p.x"]["heads"][0].as_str().unwrap();
    let entry = fs::read(copy.join("GROUNDING.yaml")).unwrap();
    let home = tmp.path().join("private");
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&copy)
        .env("KPOPPER_PRIVATE_HOME", &home)
        .args([
            "history",
            "retire",
            "--subject",
            "p.x",
            "--of",
            version,
            "--because",
            "private decision",
        ])
        .output()
        .unwrap();
    let result = ok(output);
    assert_eq!(result["state"], "private draft");
    let path = Path::new(result["path"].as_str().unwrap());
    assert!(path.starts_with(home.canonicalize().unwrap()));
    let retained: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let retained = kpop_native::value::TypedValue::from_tagged(&retained)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(retained["action"]["of"], version);
    assert_eq!(fs::read(copy.join("GROUNDING.yaml")).unwrap(), entry);
    assert_eq!(ok(run(&copy, &["history", "status"]))["commits"], 1);
}

#[test]
fn migration_read_mode_is_explicit_or_derived_from_project_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&root)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
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
    fs::write(root.join("GROUNDING.yaml"), "known: {p.x: {v: 1}}\n").unwrap();
    git(&["add", "GROUNDING.yaml"]);
    git(&["commit", "-qm", "fixture"]);
    let project = kpop_native::project_modes::Project::open(&root).unwrap();
    fs::create_dir_all(project.config_path.parent().unwrap()).unwrap();
    fs::write(
        project.config_path,
        r#"{"version":1,"mode":"advanced","generation":1,"record":"GROUNDING.yaml"}"#,
    )
    .unwrap();
    let implicit = ok(run(&root, &["--frozen", "history", "migrate"]));
    assert_eq!(implicit["source_read_mode"], "live");
    let explicit = ok(run(&root, &["history", "migrate", "--read-mode", "frozen"]));
    assert_eq!(explicit["source_read_mode"], "frozen");
}
