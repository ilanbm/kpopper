use std::{fs, process::Command};
const RECORD: &str = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  x.one: {v: 1, also: [x.two, x.two]}\n  x.two: {v: 2}\njudgments:\n  d.small: {rests_on: [x.one], seen: {x.one: 1}, wrong_if: x.one > 5, verdict: small}\n";
fn check(record: &str, hypothesis: Option<&str>) -> String {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), record).unwrap();
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
