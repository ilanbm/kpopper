use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let cases: Value =
        serde_json::from_str(include_str!("fixtures/history-fold-candidate.json")).unwrap();
    let case = cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "0-new-value")
        .unwrap();
    for (name, raw) in case["files"].as_object().unwrap() {
        let path = temp.path().join("workspace").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw.as_str().unwrap()).unwrap();
    }
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(temp.path().join("resources/reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        temp.path()
            .join("resources/reasoning")
            .join(format!("{target}.zip")),
    )
    .unwrap();
    temp
}
fn run(temp: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args([
            "--workspace",
            temp.join("workspace").to_str().unwrap(),
            "consolidate",
        ])
        .args(args)
        .env("KPOPPER_NATIVE_RESOURCES", temp.join("resources"))
        .env("KPOPPER_NATIVE_CACHE", temp.join("cache"))
        .output()
        .unwrap()
}
fn ok(result: Output) -> String {
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
fn bytes(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut result = BTreeMap::new();
    fn visit(at: &Path, result: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, result)
            } else {
                result.insert(
                    path.clone(),
                    kpop_native::identity::sha256(&fs::read(path).unwrap()),
                );
            }
        }
    }
    visit(root, &mut result);
    // Both reference and native may create this empty coordination lock during
    // preview. Every record/evidence file must remain byte-identical.
    if let Some(lock) = result.remove(&root.join(".kpopper/project.lock")) {
        assert_eq!(lock, kpop_native::identity::sha256(b""));
    }
    result
}
#[test]
fn public_preview_fold_and_empty_result_preserve_the_active_history_contract() {
    let temp = fixture();
    let root = temp.path();
    let before = bytes(&root.join("workspace"));
    let preview = ok(run(root, &["--dry-run", "trial", "GROUNDING.yaml"]));
    assert!(preview.starts_with("prepared: trial\ncaptured history preview:\n"));
    assert_eq!(bytes(&root.join("workspace")), before);
    let duplicate = run(root, &["--dry-run", "trial", "trial"]);
    assert_eq!(duplicate.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("invalid_hypothesis_names"));
    assert_eq!(bytes(&root.join("workspace")), before);
    assert_eq!(ok(run(root, &["trial"])), "folded: trial\n");
    let after = bytes(&root.join("workspace"));
    assert_eq!(ok(run(root, &["--dry-run"])), "unchanged: \n");
    assert_eq!(bytes(&root.join("workspace")), after);
}
#[test]
fn public_refutation_json_keeps_the_reference_result_and_exit_code() {
    let temp = fixture();
    let result: Value = serde_json::from_str(&ok(run(
        temp.path(),
        &["--json", "--refute", "trial", "tested and refuted"],
    )))
    .unwrap();
    assert_eq!(
        result,
        serde_json::json!({"command":"consolidate","exit_code":0,"output":"refuted: trial\n","error":""})
    );
    assert_eq!(ok(run(temp.path(), &[])), "unchanged: \n");
}
