use kpop_native::public_remeasure::{self, Options};
use std::fs;
#[cfg(unix)]
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
struct ProcessOutput {
    code: i32,
    stdout: String,
    stderr: String,
}

#[cfg(unix)]
fn oracle(record: &Path, run: bool) -> ProcessOutput {
    let python = std::env::var("KPOP_SESSION_ORACLE_PYTHON")
        .expect("set KPOP_SESSION_ORACLE_PYTHON to the Python 1.8 runtime");
    let root = PathBuf::from(
        std::env::var("KPOP_SESSION_ORACLE_ROOT")
            .expect("set KPOP_SESSION_ORACLE_ROOT to the Python 1.8 source root"),
    );
    let mut command = Command::new(python);
    command.arg(root.join("scripts/remeasure.py"));
    if run {
        command.arg("--run");
    }
    let output = command
        .arg(record)
        .current_dir(record.parent().unwrap())
        .output()
        .unwrap();
    ProcessOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

#[cfg(unix)]
fn native(record: &Path, run: bool) -> ProcessOutput {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
    command.args(["--frozen", "remeasure"]);
    if run {
        command.arg("--run");
    }
    let output = command
        .arg(record)
        .current_dir(record.parent().unwrap())
        .output()
        .unwrap();
    ProcessOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

#[cfg(unix)]
fn assert_oracle(record: &Path, run: bool) {
    let before = fs::read(record).unwrap();
    let expected = oracle(record, run);
    assert_eq!(
        fs::read(record).unwrap(),
        before,
        "Python oracle changed the record"
    );
    let actual = native(record, run);
    assert_eq!(
        fs::read(record).unwrap(),
        before,
        "native remeasure changed the record"
    );
    assert_eq!(actual, expected);
}

#[test]
#[cfg(unix)]
fn plan_and_run_use_only_the_allowlisted_recipe_and_leave_record_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".kpopper")).unwrap();
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.a:\n    v: 1\n    measure: echo\n",
    )
    .unwrap();
    let recipe = root.join("recipe");
    fs::write(&recipe, b"#!/bin/sh\nprintf '1\\n'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&recipe, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(root.join(".kpopper/measure.yaml"), "echo: [./recipe]\n").unwrap();
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let record = root.join("GROUNDING.yaml");
    let plan = public_remeasure::run(
        &Options {
            run: false,
            record: Some(record.clone()),
        },
        root,
        true,
    )
    .unwrap();
    assert!(plan.text.contains("nothing ran - add --run"));
    let measured = public_remeasure::run(
        &Options {
            run: true,
            record: Some(record),
        },
        root,
        true,
    )
    .unwrap();
    assert!(measured.text.contains("p.a: 1 - as recorded (echo)"));
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
}

#[cfg(unix)]
fn fixture(
    output: &str,
    status: i32,
    stderr: &str,
    value: &str,
    recipe_yaml: &str,
) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".kpopper")).unwrap();
    fs::write(
        root.join("GROUNDING.yaml"),
        format!("known:\n  p.a:\n    v: {value}\n    measure: echo\n"),
    )
    .unwrap();
    let recipe = root.join("recipe");
    fs::write(
        &recipe,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{output}' >&1\nprintf '%s' '{stderr}' >&2\nexit {status}\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&recipe, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(root.join(".kpopper/measure.yaml"), recipe_yaml).unwrap();
    let record = root.canonicalize().unwrap().join("GROUNDING.yaml");
    (temp, record)
}

#[test]
#[cfg(unix)]
fn changed_reading_is_reported_and_never_claimed_clean() {
    let (temp, record) = fixture("2", 0, "", "1", "echo: [./recipe]\n");
    let out = public_remeasure::run(
        &Options {
            run: true,
            record: Some(record),
        },
        temp.path(),
        true,
    )
    .unwrap();
    assert!(
        out.text
            .contains("p.a: 1 recorded (undated) -> 2 measured by echo")
    );
    assert_eq!(out.code, 0);
    assert!(out.text.contains("tree reads 1 entry differently"));
    assert!(!out.text.contains("record holds what this tree measures"));
}

#[test]
#[cfg(unix)]
fn nonzero_recipe_with_stderr_is_a_failure() {
    let (temp, record) = fixture("1", 7, "bad recipe", "1", "echo: [./recipe]\n");
    let out = public_remeasure::run(
        &Options {
            run: true,
            record: Some(record),
        },
        temp.path(),
        true,
    )
    .unwrap();
    assert!(
        out.text
            .contains("FAIL echo (p.a): exited 7 - stderr: bad recipe")
    );
    assert!(out.text.contains("not clean: a hole"));
}

#[test]
#[cfg(unix)]
fn invalid_allowlist_recipe_name_is_refused() {
    let (temp, record) = fixture("1", 0, "", "1", "bad.name: [./recipe]\n");
    let output = public_remeasure::run(
        &Options {
            run: false,
            record: Some(record),
        },
        temp.path(),
        true,
    )
    .unwrap();
    assert_eq!(output.code, 1);
    assert!(output.stderr.contains("not a recipe name"));
}

#[test]
fn no_measures_is_a_clean_noop() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("GROUNDING.yaml"),
        "known:\n  p.a:\n    v: 1\n",
    )
    .unwrap();
    let out = public_remeasure::run(
        &Options {
            run: false,
            record: Some(temp.path().join("GROUNDING.yaml")),
        },
        temp.path(),
        true,
    )
    .unwrap();
    assert_eq!(
        out.text,
        "no measures beside the record - nothing to re-measure\n"
    );
}

#[test]
#[cfg(unix)]
fn invalid_scalar_for_numeric_record_is_a_hole() {
    let (temp, record) = fixture("hello", 0, "", "1", "echo: [./recipe]\n");
    let out = public_remeasure::run(
        &Options {
            run: true,
            record: Some(record),
        },
        temp.path(),
        true,
    )
    .unwrap();
    assert!(
        out.text
            .contains("printed 'hello' where the record holds a number")
    );
    assert!(out.text.contains("not clean: a hole"));
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_complete_plan_output() {
    let (_temp, record) = fixture("1", 0, "", "1", "echo: [./recipe]\nspare: [./recipe]\n");
    assert_oracle(&record, false);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_complete_no_measures_output() {
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().canonicalize().unwrap().join("GROUNDING.yaml");
    fs::write(&record, "known:\n  p.a:\n    v: 1\n").unwrap();
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_complete_unchanged_output() {
    let (_temp, record) = fixture("1", 0, "", "1", "echo: [./recipe]\n");
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_complete_recipe_failure_output() {
    let (_temp, record) = fixture("1", 7, "bad recipe", "1", "echo: [./recipe]\n");
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_complete_scalar_failure_output() {
    let (_temp, record) = fixture("hello", 0, "", "1", "echo: [./recipe]\n");
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_complete_invalid_allowlist_output() {
    let (_temp, record) = fixture("1", 0, "", "1", "bad.name: [./recipe]\n");
    assert_oracle(&record, false);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_hypothesis_owned_reading() {
    let (temp, record) = fixture("2", 0, "", "1", "echo: [./recipe]\n");
    fs::create_dir_all(temp.path().join(".kpopper/hypotheses")).unwrap();
    fs::write(temp.path().join(".kpopper/hypotheses/proposal.yaml"), "hypothesis:\n  claim: later reading\n  born: 2026-09-19\nknown:\n  p.a:\n    v: 2\n    measure: echo\n").unwrap();
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_hypothesis_recipe_drop_refusal() {
    let (temp, record) = fixture("2", 0, "", "1", "echo: [./recipe]\n");
    fs::create_dir_all(temp.path().join(".kpopper/hypotheses")).unwrap();
    fs::write(
        temp.path().join(".kpopper/hypotheses/proposal.yaml"),
        "hypothesis:\n  claim: later reading\n  born: 2026-09-19\nknown:\n  p.a:\n    v: 2\n",
    )
    .unwrap();
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_changed_reading_evaluation() {
    let (_temp, record) = fixture("2", 0, "", "1", "echo: [./recipe]\n");
    fs::write(
        &record,
        "known:\n  p.a:\n    v: 1\n    of: 2026-09-01\n    measure: echo\n",
    )
    .unwrap();
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_changed_reading_falsifier() {
    let (_temp, record) = fixture("2", 0, "", "1", "echo: [./recipe]\n");
    fs::write(&record, "known:\n  p.a:\n    v: 1\n    of: 2026-09-01\n    measure: echo\njudgments:\n  c.a:\n    rests_on: [p.a]\n    verdict: one\n    wrong_if: p.a > 1\n    seen: {p.a: 1}\n").unwrap();
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_changed_reading_against_hypothesis() {
    let (temp, record) = fixture("3", 0, "", "1", "echo: [./recipe]\n");
    fs::write(
        &record,
        "known:\n  p.a:\n    v: 1\n    of: 2026-09-01\n    measure: echo\n",
    )
    .unwrap();
    fs::create_dir_all(temp.path().join(".kpopper/hypotheses")).unwrap();
    fs::write(temp.path().join(".kpopper/hypotheses/proposal.yaml"), "hypothesis:\n  claim: later reading\n  born: 2026-09-19\nknown:\n  p.a:\n    v: 2\n    of: 2026-09-19\n    measure: echo\n").unwrap();
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_arbitrary_integer_and_decimal_readings() {
    let huge = "12345678901234567890123456789012345678901234567890";
    let (_temp, record) = fixture(huge, 0, "", huge, "echo: [./recipe]\n");
    assert_oracle(&record, true);

    fs::write(&record, "known:\n  p.a:\n    v: 1\n    measure: echo\n").unwrap();
    fs::write(
        record.parent().unwrap().join("recipe"),
        "#!/bin/sh\nprintf '1.0\\n'\n",
    )
    .unwrap();
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_nonfinite_record_plan_and_failure() {
    let (_temp, record) = fixture("inf", 0, "", ".inf", "echo: [./recipe]\n");
    assert_oracle(&record, false);
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_missing_allowlist_and_recipe() {
    let (temp, record) = fixture("1", 0, "", "1", "spare: [./recipe]\n");
    assert_oracle(&record, false);
    fs::remove_file(temp.path().join(".kpopper/measure.yaml")).unwrap();
    assert_oracle(&record, false);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_nested_record_checkout_root() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    let nested = root.join("facts");
    fs::create_dir_all(nested.join(".kpopper")).unwrap();
    let record = nested.join("GROUNDING.yaml");
    fs::write(&record, "known:\n  p.a:\n    v: 1\n    measure: echo\n").unwrap();
    fs::write(nested.join(".kpopper/measure.yaml"), "echo: [./recipe]\n").unwrap();
    fs::write(root.join("recipe"), "#!/bin/sh\nprintf '1\\n'\n").unwrap();
    fs::set_permissions(root.join("recipe"), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        Command::new("git")
            .args(["add", "."])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-c",
                "user.name=Oracle",
                "-c",
                "user.email=oracle@example.invalid",
                "commit",
                "-qm",
                "fixture"
            ])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    assert_oracle(&record, false);
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_legacy_allowlist_layout() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let record = root.join("PROVENANCE.yaml");
    fs::write(
        &record,
        "known:\n  p.a:\n    v: 1\n    measure: echo\n",
    )
    .unwrap();
    fs::write(root.join("PROVENANCE.measure.yaml"), "echo: [./recipe]\n").unwrap();
    fs::write(root.join("recipe"), "#!/bin/sh\nprintf '1\\n'\n").unwrap();
    fs::set_permissions(root.join("recipe"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_oracle(&record, false);
    assert_oracle(&record, true);
}
