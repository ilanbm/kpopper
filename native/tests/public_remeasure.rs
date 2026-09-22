use kpop_native::public_remeasure::{self, Options};
use std::fs;
#[cfg(unix)]
use std::{
    collections::BTreeMap,
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
        .env("KPOPPER_READ_MODE", "frozen")
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command.args(["--frozen", "remeasure"]);
    if let Some(resources) = std::env::var_os("KPOP_SESSION_NATIVE_RESOURCES") {
        command.env("KPOPPER_NATIVE_RESOURCES", resources);
    }
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

#[cfg(unix)]
fn assert_core_oracle(record: &Path, run: bool) {
    fn files(root: &Path, at: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let root = record.parent().unwrap();
    let mut before = BTreeMap::new();
    files(root, root, &mut before);
    let expected = oracle(record, run);
    let actual = native(record, run);
    let mut after = BTreeMap::new();
    files(root, root, &mut after);
    assert_eq!(after, before, "remeasure changed the captured tree");
    assert_eq!(
        actual.code, expected.code,
        "actual={actual:?}\nexpected={expected:?}"
    );
    assert_eq!(actual.stderr, expected.stderr);
    let revision = regex::Regex::new(
        r"(?m)^(core/v1 prospective snapshot [0-9a-f]{64}; findings )[0-9a-f]{64}$",
    )
    .unwrap();
    let basis = regex::Regex::new(
        r"(?m)^(  MOVED [^\n]+ value/rule/basis changed )\[[0-9a-f]{16}\]$",
    )
    .unwrap();
    let actual = revision.replace_all(&actual.stdout, "$1<runtime-provenance>");
    let expected = revision.replace_all(&expected.stdout, "$1<runtime-provenance>");
    assert_eq!(
        basis.replace_all(&actual, "$1[runtime-provenance]"),
        basis.replace_all(&expected, "$1[runtime-provenance]")
    );
}

#[cfg(unix)]
fn review_case(name: &str) -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let (record_text, hypothesis, view, measured) = match name {
        "flatbody" => ("known:\n  heat.loss_kw: {v: 28, of: 2026-09-01, measure: echo}\n", Some("hypothesis: {claim: the loss is a bare number, born: 2026-09-19}\nknown:\n  heat.loss_kw: 28\n"), None, "31"),
        "helpval" => ("known:\n  note.label: {v: \"old\", name: the label, of: 2026-09-01, measure: echo}\n", None, None, "--help"),
        "retyped" => ("known:\n  p.a: {v: \"one\", of: 2026-09-01, measure: echo}\n", None, None, "42"),
        "negvalue" => ("known:\n  p.a: {v: 1, of: 2026-09-01, measure: echo}\n", None, None, "-5"),
        "hyponly" => ("known:\n  p.base: {v: 1, of: 2026-09-01}\n", Some("hypothesis: {claim: a reading only the hypothesis holds, born: 2026-09-19}\nknown:\n  p.only: {v: 1, of: 2026-09-19, measure: echo}\n"), None, "7"),
        "reversal" => ("known:\n  p.a: {v: 1, of: 2026-09-01, measure: echo}\njudgments:\n  d.go: {rests_on: [p.a], verdict: go, wrong_if: p.a > 100, seen: {p.a: 1}, because: the base decided to go}\n", Some("hypothesis: {claim: the decision should be reversed, born: 2026-09-19}\njudgments:\n  d.go: {rests_on: [p.a], verdict: stop, wrong_if: p.a > 100, seen: {p.a: 1}, because: a person should take this by name}\n"), None, "2"),
        "twoids" => ("known:\n  p.a: {v: 1, of: 2026-09-01, measure: echo}\n  p.b: {v: 1, of: 2026-09-01, measure: echo}\n", Some("hypothesis: {claim: the hypothesis disagrees about p.b only, born: 2026-09-19}\nknown:\n  p.b: {v: 5, of: 2026-09-19, measure: echo}\n"), None, "9"),
        "movedonly" => ("known:\n  p.a: {v: 1, of: 2026-09-01, measure: echo}\njudgments:\n  d.plain: {rests_on: [p.a], verdict: go, wrong_if: p.a > 100, seen: {p.a: 1}, because: a plain judgment with a stale snapshot}\n", None, None, "2"),
        "pagebound" => ("known:\n  p.a: {v: 1, of: 2026-09-01, measure: echo}\nsources:\n  s.q: {asked: Inspect this record}\njudgments:\n  d.page: {rests_on: [p.a, s.q, page.unserved], verdict: go, wrong_if: page.unserved > 0 or p.a > 100, seen: {p.a: 1, s.q: Inspect this record, page.unserved: 0}, because: the page decides this sign}\n", None, Some("tabs:\n- title: Decision\n  serves: [s.q]\n  sections:\n  - {title: d.page, pick: judgments, as: cards}\n"), "2"),
        _ => panic!("unknown review case"),
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(root.join("GROUNDING.yaml"), record_text).unwrap();
    fs::write(root.join(".kpopper/measure.yaml"), "echo: [./recipe]\n").unwrap();
    if let Some(hypothesis) = hypothesis { fs::write(root.join(".kpopper/hypotheses/proposal.yaml"), hypothesis).unwrap(); }
    if let Some(view) = view { fs::write(root.join(".kpopper/view.yaml"), view).unwrap(); }
    fs::write(root.join("recipe"), format!("#!/bin/sh\nprintf %s\\\\n \"{measured}\"\n")).unwrap();
    fs::set_permissions(root.join("recipe"), fs::Permissions::from_mode(0o700)).unwrap();
    (temp, root.join("GROUNDING.yaml"))
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
fn python_18_oracle_matches_changed_reading_beside_nonfinite_value() {
    let (_temp, record) = fixture("2", 0, "", "1", "echo: [./recipe]\n");
    fs::write(
        &record,
        "known:\n  p.a: {v: 1, of: 2026-09-01, measure: echo}\n  p.infinity: {v: .inf}\n",
    )
    .unwrap();
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
    fs::write(&record, "known:\n  p.a:\n    v: 1\n    measure: echo\n").unwrap();
    fs::write(root.join("PROVENANCE.measure.yaml"), "echo: [./recipe]\n").unwrap();
    fs::write(root.join("recipe"), "#!/bin/sh\nprintf '1\\n'\n").unwrap();
    fs::set_permissions(root.join("recipe"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_oracle(&record, false);
    assert_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_core_changed_falsifier() {
    let (temp, record) = fixture("30", 0, "", "10", "echo: [./recipe]\n");
    fs::write(&record, "meta:\n  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  p.a: {v: 10, of: 2026-09-01, measure: echo}\njudgments:\n  d.limit:\n    rests_on: [p.a]\n    seen: {p.a: 10}\n    verdict: okay\n    wrong_if: {op: gt, args: [{ref: p.a}, {num: '20'}]}\n").unwrap();
    assert_core_oracle(&record, true);
    drop(temp);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_core_changed_without_crossing() {
    let (temp, record) = fixture("15", 0, "", "10", "echo: [./recipe]\n");
    fs::write(&record, "meta:\n  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  p.a: {v: 10, of: 2026-09-01, measure: echo}\njudgments:\n  d.limit:\n    rests_on: [p.a]\n    seen: {p.a: 10}\n    verdict: okay\n    wrong_if: {op: gt, args: [{ref: p.a}, {num: '20'}]}\n").unwrap();
    assert_core_oracle(&record, true);
    drop(temp);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime and source root"]
fn python_18_oracle_matches_core_unchanged_hole() {
    let (temp, record) = fixture("10", 0, "", "10", "echo: [./recipe]\n");
    fs::write(&record, "meta:\n  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  p.a: {v: 10, measure: echo}\njudgments:\n  d.limit:\n    rests_on: [p.a]\n    seen: {p.a: 10}\n    verdict: okay\n    wrong_if: {op: gt, args: [{ref: missing.x}, {num: '20'}]}\n").unwrap();
    assert_core_oracle(&record, true);
    drop(temp);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime/source and native runtime resources"]
fn python_18_oracle_matches_active_history_changed_falsifier() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().canonicalize().unwrap().join("history");
    let python = std::env::var("KPOP_SESSION_ORACLE_PYTHON").unwrap();
    let root = std::env::var("KPOP_SESSION_ORACLE_ROOT").unwrap();
    let setup = r#"import pathlib, shutil, sys
sys.path.insert(0, sys.argv[1])
from tests.test_history_remeasure import HistoryRemeasure
case = HistoryRemeasure('test_changed_value_is_final_prospective_state_and_trips_falsifier')
case.setUp()
case.fixture()
shutil.copytree(case.root, pathlib.Path(sys.argv[2]))
"#;
    let output = Command::new(python)
        .args(["-c", setup, &root, target.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_core_oracle(&target.join("GROUNDING.yaml"), true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime/source and native runtime resources"]
fn python_18_oracle_matches_active_history_named_head() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().canonicalize().unwrap().join("history");
    let python = std::env::var("KPOP_SESSION_ORACLE_PYTHON").unwrap();
    let root = std::env::var("KPOP_SESSION_ORACLE_ROOT").unwrap();
    let setup = r#"import datetime, pathlib, shutil, sys
sys.path.insert(0, sys.argv[1])
from tests.test_history_remeasure import HistoryRemeasure
from scripts import history_hypotheses as HH
case = HistoryRemeasure('test_named_fold_and_measurement_share_final_scope_and_recheck_head')
case.setUp()
case.fixture(limit=100)
day = (datetime.datetime.now(datetime.timezone.utc).date() - datetime.timedelta(days=1)).isoformat()
proposal = HH.prepare(case.entry, 'candidate', {'kind':'set','id':'p.input','value':15,'as_of':day},
                      head={'claim':'candidate','wrong_if':{'expr':'p.input > 25'}},
                      by='writer', operation='candidate-proposal')
HH.commit(case.entry, proposal, verify=lambda _data: None)
shutil.copytree(case.root, pathlib.Path(sys.argv[2]))
"#;
    let output = Command::new(python)
        .args(["-c", setup, &root, target.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_core_oracle(&target.join("GROUNDING.yaml"), true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime/source and native runtime resources"]
fn python_18_oracle_matches_active_history_unchanged_and_recipe_failure() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().canonicalize().unwrap().join("history");
    let python = std::env::var("KPOP_SESSION_ORACLE_PYTHON").unwrap();
    let root = std::env::var("KPOP_SESSION_ORACLE_ROOT").unwrap();
    let setup = r#"import pathlib, shutil, sys
sys.path.insert(0, sys.argv[1])
from tests.test_history_remeasure import HistoryRemeasure
case = HistoryRemeasure('test_unchanged_and_scalar_values_are_clean_and_use_real_utc_day')
case.setUp()
case.fixture(value=10, recipe='10', judgment=False)
shutil.copytree(case.root, pathlib.Path(sys.argv[2]))
"#;
    let output = Command::new(python)
        .args(["-c", setup, &root, target.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record = target.join("GROUNDING.yaml");
    assert_core_oracle(&record, true);
    fs::write(
        target.join(".kpopper/measure.yaml"),
        "base_value: [./recipe]\n",
    )
    .unwrap();
    fs::write(
        target.join("recipe"),
        "#!/bin/sh\nprintf 'bad recipe' >&2\nexit 7\n",
    )
    .unwrap();
    fs::set_permissions(target.join("recipe"), fs::Permissions::from_mode(0o700)).unwrap();
    assert_core_oracle(&record, true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime/source and native runtime resources"]
fn python_18_oracle_matches_active_history_same_day_refusal() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().canonicalize().unwrap().join("history");
    let python = std::env::var("KPOP_SESSION_ORACLE_PYTHON").unwrap();
    let root = std::env::var("KPOP_SESSION_ORACLE_ROOT").unwrap();
    let setup = r#"import datetime, pathlib, shutil, sys
sys.path.insert(0, sys.argv[1])
from tests.test_history_remeasure import HistoryRemeasure
from scripts import history_authoring
case = HistoryRemeasure('test_same_day_measurement_refuses_instead_of_forcing_acceptance')
case.setUp()
case.fixture(limit=100)
today = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
mutation = history_authoring.prepare_batch(case.entry, [{'kind':'set','id':'p.input','value':15,'as_of':today}], by='writer', operation='same-day-base')
history_authoring.commit(case.entry, mutation, verify=lambda _data: None)
shutil.copytree(case.root, pathlib.Path(sys.argv[2]))
"#;
    let output = Command::new(python)
        .args(["-c", setup, &root, target.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_core_oracle(&target.join("GROUNDING.yaml"), true);
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime/source and review cases"]
fn python_18_oracle_matches_scalar_replacement_and_safe_refresh_cases() {
    for name in ["flatbody", "helpval", "retyped", "negvalue", "hyponly"] {
        let (_temp, record) = review_case(name);
        assert_oracle(&record, true);
    }
}

#[test]
#[cfg(unix)]
#[ignore = "requires explicit Python 1.8 oracle runtime/source and review cases"]
fn python_18_oracle_matches_structured_remeasure_decisions() {
    for name in ["reversal", "twoids", "movedonly", "pagebound"] {
        let (_temp, record) = review_case(name);
        assert_oracle(&record, true);
    }
}
