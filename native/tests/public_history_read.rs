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
    let day = if op == "set-a" { "25" } else { "24" };
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
fn write(root: &Path, op: &str, action: serde_json::Value) {
    let prepared = W::prepare(root, &v(action), &opts(op), None).unwrap();
    W::publish(root, &prepared, None, |_| Ok(())).unwrap();
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
    let before = kpop_native::identity::sha256(&fs::read(root.join("GROUNDING.yaml")).unwrap());
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
    assert_eq!(
        before,
        kpop_native::identity::sha256(&fs::read(root.join("GROUNDING.yaml")).unwrap())
    );
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
