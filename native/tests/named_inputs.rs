use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const NO_RECORD_HERE: &str = "missing.yaml: no record here. Run this from the directory the record sits in, or name the record file as an argument.";

fn workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/workshop/GROUNDING.yaml"),
        root.path().join("GROUNDING.yaml"),
    )
    .unwrap();
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(git.status.success());
    root
}

fn command(root: &Path) -> Command {
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .current_dir(root)
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"));
    command
}

fn run(root: &Path, args: &[&str]) -> Output {
    command(root).args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn same_and_distinct_name_a_missing_record_on_stderr() {
    let root = workspace();
    let before = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
    for args in [
        vec!["same", "stock.packages", "workshop.guests", "missing.yaml"],
        vec![
            "distinct",
            "stock.packages",
            "workshop.guests",
            "different things",
            "missing.yaml",
        ],
    ] {
        let output = run(root.path(), &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(text(&output.stdout), "", "{args:?}");
        assert_eq!(
            text(&output.stderr),
            format!("{NO_RECORD_HERE}\n"),
            "{args:?}"
        );
    }
    assert!(!root.path().join("missing.yaml").exists());
    assert_eq!(
        fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
        before
    );
}

#[test]
fn expressions_migrate_names_a_missing_record_in_its_json_error() {
    let root = workspace();
    let output = run(
        root.path(),
        &["expressions", "migrate", "--record", "missing.yaml"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(text(&output.stderr), "");
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error, serde_json::json!({"error": NO_RECORD_HERE}));
}
