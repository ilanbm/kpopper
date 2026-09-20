use kpop_native::public_remeasure::{self, Options};
use std::{fs, path::{Path, PathBuf}, process::Command};

const ORACLE_PYTHON: &str = "/Users/ilanbm/.local/share/kpopper/runtimes/bf4942511207a39e/bin/python";
const ORACLE_ROOT: &str = "/Users/ilanbm/docs/kpopper/rust-runtime-spike/baseline-f480ea6";

#[derive(Debug, PartialEq, Eq)]
struct ProcessOutput {
    code: i32,
    stdout: String,
    stderr: String,
}

fn oracle(record: &Path, run: bool) -> ProcessOutput {
    let program = r#"import pathlib, sys
sys.path.insert(0, sys.argv[1])
import remeasure
try:
    lines, code = remeasure.measure([sys.argv[2]], run=(sys.argv[3] == "1"))
    sys.stdout.write("\n".join(lines) + "\n")
except Exception as error:
    sys.stderr.write(str(error) + "\n")
    code = 2
sys.exit(code)
"#;
    let output = Command::new(ORACLE_PYTHON)
        .args(["-c", program, &format!("{ORACLE_ROOT}/scripts"), record.to_str().unwrap(), if run { "1" } else { "0" }])
        .current_dir(record.parent().unwrap())
        .output()
        .unwrap();
    ProcessOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

fn native(record: &Path, run: bool) -> ProcessOutput {
    match public_remeasure::run(
        &Options { run, record: Some(record.to_path_buf()) },
        record.parent().unwrap(),
        true,
    ) {
        Ok(output) => ProcessOutput { code: output.code, stdout: output.text, stderr: output.stderr },
        Err(error) => ProcessOutput { code: 2, stdout: String::new(), stderr: format!("{error}\n") },
    }
}

fn assert_oracle(record: &Path, run: bool) {
    let before = fs::read(record).unwrap();
    let expected = oracle(record, run);
    assert_eq!(fs::read(record).unwrap(), before, "Python oracle changed the record");
    let actual = native(record, run);
    assert_eq!(fs::read(record).unwrap(), before, "native remeasure changed the record");
    assert_eq!(actual, expected);
}

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
    assert!(plan.text.contains("nothing ran - add --run"));
    let measured = public_remeasure::run(&Options { run: true, record: Some(record) }, root, true).unwrap();
    assert!(measured.text.contains("p.a: 1 - as recorded (echo)"));
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
    let record = root.canonicalize().unwrap().join("GROUNDING.yaml");
    (temp, record)
}

#[test]
fn changed_reading_is_reported_and_never_claimed_clean() {
    let (temp, record) = fixture("2", 0, "", "1", "echo: [./recipe]\n");
    let out = public_remeasure::run(&Options { run: true, record: Some(record) }, temp.path(), true).unwrap();
    assert!(out.text.contains("p.a: 1 -> 2 measured by echo"));
    assert!(out.text.contains("tree reads entries differently"));
    assert!(!out.text.contains("record holds what this tree measures"));
}

#[test]
fn nonzero_recipe_with_stderr_is_a_failure() {
    let (temp, record) = fixture("1", 7, "bad recipe", "1", "echo: [./recipe]\n");
    let out = public_remeasure::run(&Options { run: true, record: Some(record) }, temp.path(), true).unwrap();
    assert!(out.text.contains("FAIL echo (p.a): exited 7 - stderr: bad recipe"));
    assert!(out.text.contains("not clean: a hole"));
}

#[test]
fn invalid_allowlist_recipe_name_is_refused() {
    let (temp, record) = fixture("1", 0, "", "1", "bad.name: [./recipe]\n");
    let output = public_remeasure::run(&Options { run: false, record: Some(record) }, temp.path(), true).unwrap();
    assert_eq!(output.code, 1);
    assert!(output.stderr.contains("not a recipe name"));
}

#[test]
fn no_measures_is_a_clean_noop() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("GROUNDING.yaml"), "known:\n  p.a:\n    v: 1\n").unwrap();
    let out = public_remeasure::run(&Options { run: false, record: Some(temp.path().join("GROUNDING.yaml")) }, temp.path(), true).unwrap();
    assert_eq!(out.text, "no measures beside the record - nothing to re-measure\n");
}

#[test]
fn invalid_scalar_for_numeric_record_is_a_hole() {
    let (temp, record) = fixture("hello", 0, "", "1", "echo: [./recipe]\n");
    let out = public_remeasure::run(&Options { run: true, record: Some(record) }, temp.path(), true).unwrap();
    assert!(out.text.contains("printed 'hello' where the record holds a number"));
    assert!(out.text.contains("not clean: a hole"));
}

#[test]
fn python_18_oracle_matches_complete_plan_output() {
    let (_temp, record) = fixture("1", 0, "", "1", "echo: [./recipe]\nspare: [./recipe]\n");
    assert_oracle(&record, false);
}

#[test]
fn python_18_oracle_matches_complete_no_measures_output() {
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().canonicalize().unwrap().join("GROUNDING.yaml");
    fs::write(&record, "known:\n  p.a:\n    v: 1\n").unwrap();
    assert_oracle(&record, true);
}

#[test]
fn python_18_oracle_matches_complete_unchanged_output() {
    let (_temp, record) = fixture("1", 0, "", "1", "echo: [./recipe]\n");
    assert_oracle(&record, true);
}

#[test]
fn python_18_oracle_matches_complete_recipe_failure_output() {
    let (_temp, record) = fixture("1", 7, "bad recipe", "1", "echo: [./recipe]\n");
    assert_oracle(&record, true);
}

#[test]
fn python_18_oracle_matches_complete_scalar_failure_output() {
    let (_temp, record) = fixture("hello", 0, "", "1", "echo: [./recipe]\n");
    assert_oracle(&record, true);
}

#[test]
fn python_18_oracle_matches_complete_invalid_allowlist_output() {
    let (_temp, record) = fixture("1", 0, "", "1", "bad.name: [./recipe]\n");
    assert_oracle(&record, false);
}
