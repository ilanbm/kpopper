use std::{fs, path::Path, process::{Command, Output}};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
    command.current_dir(root).env_remove("KPOPPER_NATIVE_RESOURCES");
    command
}

fn run(root: &Path, args: &[&str]) -> Output { command(root).args(args).output().unwrap() }

fn fixture_named(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let cases: serde_json::Value = serde_json::from_slice(include_bytes!("fixtures/ordinary-page-facts.json")).unwrap();
    let case = cases.as_array().unwrap().iter()
        .find(|case| case["name"] == name).unwrap();
    for (relative, raw) in case["files"].as_object().unwrap() {
        let path = temp.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw.as_str().unwrap()).unwrap();
    }
    let record = temp.path().join("GROUNDING.yaml");
    (temp, record)
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    fixture_named("page-arrangement-fired-dry")
}

fn assert_ok(output: Output) -> String {
    assert!(output.status.success(), "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn fired_arrangement_is_renewed_with_born_stood_and_replaced() {
    let (temp, record) = fixture();
    let output = assert_ok(run(temp.path(), &["add", "v.layout",
        "rests_on=[s.now,page.unserved]", "verdict=re-decided",
        "wrong_if=page.unserved > 0", "because=the sign appeared", "--as-of", "2026-09-20",
        record.to_str().unwrap()]));
    assert!(output.contains("with its tab intact"), "{output}");
    let raw = fs::read_to_string(record).unwrap();
    assert!(raw.contains("born: \"2026-09-20\""));
    assert!(raw.contains("stood 1 session"));
    assert_eq!(raw.matches("v.layout:").count(), 1);
}

#[test]
fn cut_and_same_day_arrangements_are_refused_without_writes() {
    for (case, day, message) in [
        ("page-arrangement-cut-dry", "2026-09-20", "brief no longer carries"),
        ("page-arrangement-fired-dry", "2026-09-18", "second decision on the same day"),
    ] {
        let (temp, record) = fixture_named(case);
        let before = fs::read(&record).unwrap();
        let output = run(temp.path(), &["add", "v.layout", "rests_on=[s.now,page.unserved]",
            "verdict=re-decided", "wrong_if=page.unserved > 0", "--as-of", day,
            record.to_str().unwrap()]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(message));
        assert_eq!(fs::read(&record).unwrap(), before);
    }
}

#[test]
fn first_arrangement_gets_born_and_cannot_be_born_broken() {
    for (predicate, success) in [("page.covered > 9", true), ("page.covered > 0", false)] {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("GROUNDING.yaml");
        fs::write(&record, "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nsources:\n  s.now: {asked: test the page, read: 2026-09-19}\nknown:\n  p.value: {v: 1, from: s.now, of: 2026-09-19}\njudgments:\n").unwrap();
        fs::create_dir_all(temp.path().join(".kpopper")).unwrap();
        fs::write(temp.path().join(".kpopper/view.yaml"),
            "tabs:\n- title: Now\n  serves: [s.now]\n  sections:\n  - {title: Facts, pick: p.value, as: rows}\n").unwrap();
        let before = fs::read(&record).unwrap();
        let output = run(temp.path(), &["add", "v.layout", "rests_on=[s.now,page.covered]",
            "verdict=one page", &format!("wrong_if={predicate}"), "--as-of", "2026-09-20",
            record.to_str().unwrap()]);
        assert_eq!(output.status.success(), success, "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        if success {
            let raw = fs::read_to_string(&record).unwrap();
            assert!(raw.contains("born: \"2026-09-20\""));
            assert!(raw.contains("page.covered: 1"));
        } else {
            assert!(String::from_utf8_lossy(&output.stderr).contains("born broken"));
            assert_eq!(fs::read(&record).unwrap(), before);
        }
    }
}
