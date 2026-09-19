use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop-native"))
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
                    path.strip_prefix(root).unwrap().to_str().unwrap().into(),
                    kpop_native::identity::sha256(&fs::read(path).unwrap()),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
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
    let raw = fs::read_to_string(&entry).unwrap();
    assert!(raw.contains("v: 1"));
    fs::write(&entry, raw.replace("v: 1", "v: 7")).unwrap();
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
    fs::write(
        &entry,
        fs::read_to_string(&entry).unwrap().replace("v: 1", "v: 8"),
    )
    .unwrap();
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
    assert!(fs::read_to_string(&entry).unwrap().contains("v: 1"));
}

#[test]
fn private_historical_claim_stays_a_private_draft() {
    let tmp = tempfile::tempdir().unwrap();
    let copy = imported(tmp.path(), "known: {p.x: {v: 1, private: true}}\n");
    let status = ok(run(&copy, &["history", "status"]));
    let version = status["subjects"]["p.x"]["heads"][0].as_str().unwrap();
    let entry = fs::read(copy.join("GROUNDING.yaml")).unwrap();
    let home = tmp.path().join("private");
    let output = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
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
