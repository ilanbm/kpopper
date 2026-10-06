use std::{fs, process::Command};
const RECORD: &str = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  x.one: {v: 1, also: [x.two, x.two]}\n  x.two: {v: 2}\njudgments:\n  d.small: {rests_on: [x.one], seen: {x.one: 1}, wrong_if: x.one > 5, verdict: small}\n";
fn check(record: &str, hypothesis: Option<&str>) -> String {
    check_in(record, hypothesis, &[])
}
fn check_in(record: &str, hypothesis: Option<&str>, files: &[(&str, &str)]) -> String {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), record).unwrap();
    for (path, text) in files {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    if let Some(h) = hypothesis {
        fs::create_dir_all(root.path().join(".kpopper/hypotheses")).unwrap();
        fs::write(root.path().join(".kpopper/hypotheses/returning.yaml"), h).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(["--frozen", "check"])
        .current_dir(root.path())
        .env("KPOPPER_PRIVATE_HOME", root.path().join("private"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.path().join("GROUNDING.yaml")).unwrap(),
        record
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn a_returning_alias_is_reported_once_without_failing_the_record() {
    let output = check(RECORD, None);
    assert!(output.contains("NOTE x.one carries also: x.two, and x.two is an entry - a retirement that came back, or a sibling this record declares; the reader reads it as neither: same x.one x.two folds them, distinct x.one x.two \"why\" tells them apart\n"));
    assert_eq!(output.matches("carries also:").count(), 1);
    assert!(output.ends_with("1 judgments, 3 entries, 0 problems, 1 declared\n"));
}
#[test]
fn aliases_are_checked_against_hypotheses_and_healthy_retirements_are_quiet() {
    let record = RECORD.replace("  x.two: {v: 2}\n", "");
    assert!(!check(&record, None).contains("carries also:"));
    let hypothesis = "hypothesis: {born: '2026-09-24'}\nknown:\n  x.two: {v: 2}\n";
    assert_eq!(
        check(&record, Some(hypothesis))
            .matches("carries also:")
            .count(),
        1
    );
    let base = RECORD.replace(", also: [x.two, x.two]", "");
    let hypothesis = "hypothesis: {born: '2026-09-24'}\nknown:\n  x.one: {v: 1, also: x.two}\n";
    assert!(check(&base, Some(hypothesis)).contains("x.one carries also: x.two"));
}
#[test]
fn also_as_the_declared_dependency_field_is_not_an_identity_note() {
    let record = RECORD
        .replace(", also: [x.two, x.two]", "")
        .replace("rests_on", "also");
    assert!(!check(&record, None).contains("carries also:"));
}
#[test]
fn malformed_prefix_legend_preserves_order_and_python_scalar_spelling() {
    let record = format!(
        "meta: {{prefixes: {{on: onboarding, z: nothing, x: 7}}}}\n{}",
        RECORD.replace(", also: [x.two, x.two]", "")
    );
    let out = check(&record, None);
    let notes = out
        .lines()
        .filter(|s| s.starts_with("NOTE"))
        .collect::<Vec<_>>();
    assert_eq!(
        notes,
        vec![
            "NOTE meta.prefixes: the key True is not a word - yes, no, on, off, true and false are booleans to YAML unless quoted",
            "NOTE meta.prefixes: z is held by nothing in the record",
            "NOTE meta.prefixes: x stands for 7, which is not a word - quote it",
        ]
    );
    assert!(out.ends_with("1 judgments, 3 entries, 0 problems, 3 declared\n"));
}
#[test]
fn nonmapping_and_blank_legends_are_not_silently_ignored() {
    for (legend, expected) in [
        (
            "[x, value]",
            "meta.prefixes is not a mapping of prefix to word - nothing is printed from it",
        ),
        (
            "{x: '  '}",
            "meta.prefixes: x stands for '  ', which is not a word - quote it",
        ),
    ] {
        let record = format!(
            "meta: {{prefixes: {legend}}}\n{}",
            RECORD.replace(", also: [x.two, x.two]", "")
        );
        assert!(check(&record, None).contains(expected));
    }
}
#[test]
fn valid_and_absent_legends_remain_quiet() {
    for meta in [
        "",
        "meta: {prefixes: {x: value, d: decision}}\n",
        "meta: {prefixes: null}\n",
    ] {
        let record = format!("{meta}{}", RECORD.replace(", also: [x.two, x.two]", ""));
        assert!(!check(&record, None).contains("NOTE"));
    }
}
const FORMATTER: &str = "NOTE formatter: .prettierrc is present and .prettierignore does not list GROUNDING.yaml - a pre-commit formatter may rewrite the record; add GROUNDING.yaml and .kpopper/ to .prettierignore\n";
#[test]
fn a_formatter_that_can_rewrite_the_record_is_noted_once() {
    let record = RECORD.replace(", also: [x.two, x.two]", "");
    let quiet = check(&record, None);
    let noted = check_in(&record, None, &[(".prettierrc", "{}")]);
    let reincluded = [
        (".prettierrc", "{}"),
        (".prettierignore", "*.yaml\n!GROUNDING.yaml\n"),
    ];
    assert_eq!(check_in(&record, None, &reincluded), noted);
    assert_eq!(noted.matches("NOTE").count(), 1);
    assert_eq!(
        noted,
        format!(
            "{FORMATTER}{}",
            quiet.replace(" 0 problems\n", " 0 problems, 1 declared\n")
        )
    );
}
#[test]
fn a_formatter_configured_at_the_git_top_level_is_noted_for_a_nested_record() {
    let root = tempfile::tempdir().unwrap();
    let record = RECORD.replace(", also: [x.two, x.two]", "");
    let init = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .unwrap();
    assert!(init.success());
    fs::create_dir_all(root.path().join("notes")).unwrap();
    fs::write(root.path().join("notes/GROUNDING.yaml"), &record).unwrap();
    fs::write(
        root.path().join("package.json"),
        r#"{"lint-staged": {"*": "prettier --write"}}"#,
    )
    .unwrap();
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .args(["--frozen", "check"])
            .current_dir(root.path().join("notes"))
            .env("KPOPPER_PRIVATE_HOME", root.path().join("private"))
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(run().starts_with("NOTE formatter: the lint-staged key in package.json is present and .prettierignore does not list GROUNDING.yaml"));
    fs::write(
        root.path().join(".prettierignore"),
        "notes/GROUNDING.yaml\n",
    )
    .unwrap();
    assert!(!run().contains("NOTE"));
}
#[test]
fn a_record_the_formatter_ignores_stays_quiet() {
    let record = RECORD.replace(", also: [x.two, x.two]", "");
    let quiet = check(&record, None);
    assert!(!quiet.contains("NOTE"));
    for ignore in ["GROUNDING.yaml\n", "# local\n/GROUNDING.yaml\n", "*.yaml\n"] {
        let files = [(".prettierrc", "{}"), (".prettierignore", ignore)];
        assert_eq!(check_in(&record, None, &files), quiet, "{ignore}");
    }
    for files in [
        &[(".prettierignore", "node_modules\n")][..],
        &[("package.json", r#"{"name": "app"}"#)][..],
    ] {
        assert_eq!(check_in(&record, None, files), quiet);
    }
}
