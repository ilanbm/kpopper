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
    // The ordinary program ships only when the run provides one.
    if let Some(program) = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM") {
        let ordinary = resources.join("ordinary").join(&target);
        fs::create_dir_all(&ordinary).unwrap();
        for name in ["build.json", "epistemic-core", "epistemic-core.exe"] {
            let source = Path::new(&program).join(name);
            if source.exists() {
                fs::copy(source, ordinary.join(name)).unwrap();
            }
        }
    }
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
    for (named, refusal) in [
        ("missing.yaml", NO_RECORD_HERE.to_owned()),
        (
            "no-such-dir/missing.yaml",
            format!("no-such-dir/{NO_RECORD_HERE}"),
        ),
    ] {
        for args in [
            vec!["same", "stock.packages", "workshop.guests", named],
            vec![
                "distinct",
                "stock.packages",
                "workshop.guests",
                "different things",
                named,
            ],
        ] {
            let output = run(root.path(), &args);
            assert_eq!(output.status.code(), Some(1), "{args:?}");
            assert_eq!(text(&output.stdout), "", "{args:?}");
            assert_eq!(text(&output.stderr), format!("{refusal}\n"), "{args:?}");
        }
    }
    assert!(!root.path().join("no-such-dir").exists());
    assert!(!root.path().join("missing.yaml").exists());
    assert_eq!(
        fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
        before
    );
}

#[test]
fn expressions_migrate_names_a_missing_record_in_its_json_error() {
    let root = workspace();
    for (named, refusal) in [
        ("missing.yaml", NO_RECORD_HERE.to_owned()),
        (
            "no-such-dir/missing.yaml",
            format!("no-such-dir/{NO_RECORD_HERE}"),
        ),
    ] {
        let output = run(root.path(), &["expressions", "migrate", "--record", named]);
        assert_eq!(output.status.code(), Some(2), "{named}");
        assert_eq!(text(&output.stderr), "", "{named}");
        let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error, serde_json::json!({"error": refusal}), "{named}");
    }
}

#[test]
fn a_named_path_that_is_no_file_is_refused_as_unreadable() {
    let root = workspace();
    let base = root.path().canonicalize().unwrap();
    fs::create_dir(base.join("adir")).unwrap();
    let refusal = format!(
        "The record path exists but is not an accessible file. {}",
        base.join("adir").display()
    );
    for args in [
        vec!["same", "stock.packages", "workshop.guests", "adir"],
        vec![
            "distinct",
            "stock.packages",
            "workshop.guests",
            "why",
            "adir",
        ],
    ] {
        let output = run(root.path(), &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(text(&output.stdout), "", "{args:?}");
        assert_eq!(text(&output.stderr), format!("{refusal}\n"), "{args:?}");
    }
    let output = run(root.path(), &["expressions", "migrate", "--record", "adir"]);
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error, serde_json::json!({"error": refusal}));
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
    // Past the name, the configured record's own reading decides, or the absence of the
    // ordinary program when the run ships none.
    const UNAVAILABLE: &str = "native ordinary program is unavailable";
    let migrate = run(
        root.path(),
        &["expressions", "migrate", "--record", "GROUNDING.yaml"],
    );
    let plan: serde_json::Value = serde_json::from_slice(&migrate.stdout).unwrap();
    if migrate.status.code() == Some(2) {
        assert_eq!(plan, serde_json::json!({"error": UNAVAILABLE}));
    } else {
        assert_eq!(migrate.status.code(), Some(0), "{plan}");
        assert_eq!(plan["record"], record.to_str().unwrap());
    }
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
    let told = text(&same.stderr);
    assert!(
        told == format!("{UNAVAILABLE}\n")
            || told.starts_with("refused - stock.packages holds 24 as of 2026-09-09"),
        "{told}"
    );
    let distinct = run(
        root.path(),
        &[
            "distinct",
            "stock.packages",
            "workshop.guests",
            "why",
            "GROUNDING.yaml",
        ],
    );
    if distinct.status.success() {
        assert!(
            text(&distinct.stdout).starts_with("distinct stock.packages from workshop.guests"),
            "{}",
            text(&distinct.stdout)
        );
        assert_ne!(fs::read(&record).unwrap(), before);
    } else {
        assert_eq!(distinct.status.code(), Some(1));
        assert_eq!(text(&distinct.stderr), format!("{UNAVAILABLE}\n"));
        assert_eq!(fs::read(&record).unwrap(), before);
    }
    assert!(!root.path().join("GROUNDING.yaml").exists());
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
