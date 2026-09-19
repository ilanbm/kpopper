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
