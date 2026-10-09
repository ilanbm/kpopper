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

/// A Simple project whose one record sits in `notes/`, with no file at the root.
fn simple_project() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = workspace();
    let base = root.path().canonicalize().unwrap();
    fs::create_dir_all(base.join("notes")).unwrap();
    fs::create_dir_all(base.join(".git/kpopper/project")).unwrap();
    fs::rename(base.join("GROUNDING.yaml"), base.join("notes/rec.yaml")).unwrap();
    fs::write(
        base.join(".git/kpopper/project/project.json"),
        r#"{"version": 1, "mode": "simple", "record": "notes/rec.yaml", "publication": null, "generation": 0}"#,
    )
    .unwrap();
    let record = base.join("notes/rec.yaml");
    (root, record)
}

#[test]
fn a_named_root_entry_reaches_the_configured_record() {
    // The root entry name stands for the configured record, so it is not refused as missing.
    let (root, record) = simple_project();
    let before = fs::read(&record).unwrap();
    for args in [
        vec![
            "same",
            "stock.packages",
            "workshop.guests",
            "GROUNDING.yaml",
        ],
        vec![
            "distinct",
            "stock.packages",
            "workshop.guests",
            "different things",
            "GROUNDING.yaml",
        ],
        vec!["expressions", "migrate", "--record", "GROUNDING.yaml"],
    ] {
        let output = run(root.path(), &args);
        let told = format!("{}{}", text(&output.stdout), text(&output.stderr));
        assert!(!told.contains("no record here"), "{args:?}: {told}");
        if output.status.success() && args[0] == "expressions" {
            let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(plan["record"], record.to_str().unwrap());
        }
    }
    assert!(!root.path().join("GROUNDING.yaml").exists());
    assert_eq!(fs::read(&record).unwrap(), before);
}

#[test]
fn a_missing_configured_record_is_named_in_full() {
    let (root, record) = simple_project();
    fs::remove_file(&record).unwrap();
    let refusal = format!(
        "{}: no record here. Run this from the directory the record sits in, or name the record file as an argument.",
        record.display()
    );
    let same = run(
        root.path(),
        &[
            "same",
            "stock.packages",
            "workshop.guests",
            "GROUNDING.yaml",
        ],
    );
    assert_eq!(same.status.code(), Some(1));
    assert_eq!(text(&same.stdout), "");
    assert_eq!(text(&same.stderr), format!("{refusal}\n"));
    let migrate = run(
        root.path(),
        &["expressions", "migrate", "--record", "GROUNDING.yaml"],
    );
    assert_eq!(migrate.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&migrate.stdout).unwrap();
    assert_eq!(error, serde_json::json!({"error": refusal}));
}
