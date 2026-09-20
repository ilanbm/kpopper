use kpop_native::public_remeasure::{self, Options};
use std::{fs, path::PathBuf};

#[test]
fn plan_and_run_use_only_the_allowlisted_recipe_and_leave_record_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".kpopper")).unwrap();
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.a:\n    v: 1\n    measure: echo\n").unwrap();
    let recipe = root.join("recipe");
    fs::write(&recipe, b"#!/bin/sh\nprintf '1\\n'\n").unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&recipe, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(root.join(".kpopper/measure.yaml"), "echo: [./recipe]\n").unwrap();
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let record = PathBuf::from(root.join("GROUNDING.yaml"));
    let plan = public_remeasure::run(&Options { run: false, record: Some(record.clone()) }, root, true).unwrap();
    assert!(plan.contains("nothing ran - add --run"));
    let measured = public_remeasure::run(&Options { run: true, record: Some(record) }, root, true).unwrap();
    assert!(measured.contains("p.a: 1 - as recorded (echo)"));
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
}

fn fixture(output: &str, status: i32, stderr: &str, value: &str, recipe_yaml: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".kpopper")).unwrap();
    fs::write(root.join("GROUNDING.yaml"), format!("known:\n  p.a:\n    v: {value}\n    measure: echo\n")).unwrap();
    let recipe = root.join("recipe");
    fs::write(&recipe, format!("#!/bin/sh\nprintf '%s\\n' '{output}' >&1\nprintf '%s' '{stderr}' >&2\nexit {status}\n")).unwrap();
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; fs::set_permissions(&recipe, fs::Permissions::from_mode(0o700)).unwrap(); }
    fs::write(root.join(".kpopper/measure.yaml"), recipe_yaml).unwrap();
    let record = root.join("GROUNDING.yaml");
    (temp, record)
}

#[test]
fn changed_reading_is_reported_and_never_claimed_clean() {
    let (temp, record) = fixture("2", 0, "", "1", "echo: [./recipe]\n");
    let out = public_remeasure::run(&Options { run: true, record: Some(record) }, temp.path(), true).unwrap();
    assert!(out.contains("p.a: 1 -> 2 measured by echo"));
    assert!(out.contains("tree reads entries differently"));
    assert!(!out.contains("record holds what this tree measures"));
}

#[test]
fn nonzero_recipe_with_stderr_is_a_failure() {
    let (temp, record) = fixture("1", 7, "bad recipe", "1", "echo: [./recipe]\n");
    let out = public_remeasure::run(&Options { run: true, record: Some(record) }, temp.path(), true).unwrap();
    assert!(out.contains("FAIL echo (p.a): exited 7 - stderr: bad recipe"));
    assert!(out.contains("not clean: a hole"));
}

#[test]
fn invalid_allowlist_recipe_name_is_refused() {
    let (temp, record) = fixture("1", 0, "", "1", "bad.name: [./recipe]\n");
    let err = public_remeasure::run(&Options { run: false, record: Some(record) }, temp.path(), true).unwrap_err();
    assert!(err.to_string().contains("not a recipe name"));
}

#[test]
fn no_measures_is_a_clean_noop() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("GROUNDING.yaml"), "known:\n  p.a:\n    v: 1\n").unwrap();
    let out = public_remeasure::run(&Options { run: false, record: Some(temp.path().join("GROUNDING.yaml")) }, temp.path(), true).unwrap();
    assert_eq!(out, "no measures beside the record - nothing to re-measure\n");
}

#[test]
fn invalid_scalar_for_numeric_record_is_a_hole() {
    let (temp, record) = fixture("hello", 0, "", "1", "echo: [./recipe]\n");
    let out = public_remeasure::run(&Options { run: true, record: Some(record) }, temp.path(), true).unwrap();
    assert!(out.contains("printed \"hello\" where the record holds a number"));
    assert!(out.contains("not clean: a hole"));
}
