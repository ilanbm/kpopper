use kpop_native::{
    session_gate::{self, GateOptions},
    source_capture::ReadMode,
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const SCHEMA: &str = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\n";

fn oracle() -> (PathBuf, PathBuf) {
    (
        PathBuf::from(
            std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("set explicit oracle Python"),
        ),
        PathBuf::from(
            std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("set immutable oracle root"),
        )
        .join("scripts/provenance.py"),
    )
}

fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(args)
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .env("XDG_STATE_HOME", root.join("private-state"))
        .output()
        .unwrap()
}

fn options<'a>(state: &'a Path, record: &'a PathBuf, root: &'a Path) -> GateOptions<'a> {
    GateOptions {
        state_path: state,
        paths: std::slice::from_ref(record),
        workspace: root,
        read_mode: ReadMode::Frozen,
        runtime: None,
        turns: 0,
        host: None,
        nudged_at: None,
        session_id: None,
        private_tmp: None,
    }
}

fn native_mark(text: &str) -> Value {
    let tmp = tempfile::tempdir().unwrap();
    let record = tmp.path().join("GROUNDING.yaml");
    let state = tmp.path().join("private/mark.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&record, text).unwrap();
    session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
    serde_json::from_slice(&fs::read(state).unwrap()).unwrap()
}

#[test]
fn ordinary_commands_keep_nontext_body_keys_and_nested_values() {
    let cases = [
        format!("{SCHEMA}known:\n  p.a: {{v: 1, on: note}}\n"),
        format!(
            "{SCHEMA}known:\n  p.a: {{v: 1}}\njudgments:\n  d.go: {{verdict: go, rests_on: [p.a], seen: {{p.a: 1}}, wrong_if: 'p.a > 2', on: note}}\n"
        ),
        format!(
            "{SCHEMA}known:\n  p.map: {{v: {{on: x, 'true': y}}}}\njudgments:\n  d.go: {{verdict: go, rests_on: [p.map], seen: {{p.map: {{on: x, 'true': y}}}}, wrong_if: 'p.map == never'}}\n"
        ),
    ];
    for text in cases {
        let tmp = tempfile::tempdir().unwrap();
        let record = tmp.path().join("GROUNDING.yaml");
        fs::write(&record, &text).unwrap();
        for arguments in [
            ["--frozen", "check"].as_slice(),
            ["--frozen", "pull"].as_slice(),
        ] {
            let output = cli(tmp.path(), arguments);
            assert!(
                output.status.success(),
                "{}: {}",
                arguments[1],
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let id = if text.contains("p.a:") {
            "p.a"
        } else {
            "p.map"
        };
        let assess = cli(tmp.path(), &["--frozen", "assess", id, "--json"]);
        assert!(
            assess.status.success(),
            "assess: {}",
            String::from_utf8_lossy(&assess.stderr)
        );
        assert!(!String::from_utf8_lossy(&assess.stdout).contains("kpopper:ordinary-key"));
        let state = tmp.path().join("private/mark.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
        let mark: Value = serde_json::from_slice(&fs::read(state).unwrap()).unwrap();
        assert_eq!(mark["fails"], 0);
    }
}

#[test]
fn gate_shape_distinguishes_typed_keys_from_quoted_text_and_keeps_collisions() {
    let body = |keys: &str| {
        format!(
            "{SCHEMA}known:\n  p.a: {{v: 1}}\njudgments:\n  d.go:\n    verdict: go\n    rests_on: [p.a]\n    seen: {{p.a: 1}}\n    wrong_if: p.a > 2\n{keys}"
        )
    };
    let collision = native_mark(&body(
        "    on: first\n    true: second\n    1: third\n    1.0: last\n",
    ));
    let typed_and_text = native_mark(&body("    on: typed\n    'true': text\n"));
    let quoted = native_mark(&body("    'true': text\n"));
    let shape = |mark: &Value| {
        mark["judgments"]["d.go"]["shape"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_ne!(shape(&collision), shape(&typed_and_text));
    assert_ne!(shape(&typed_and_text), shape(&quoted));
}

#[test]
fn nontext_unknown_expression_field_is_not_erased() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("GROUNDING.yaml"),
        format!("{SCHEMA}known:\n  p.a: {{v: 1}}\n  p.bad: {{rule: {{op: add, args: [{{ref: p.a}}, {{num: '1'}}], on: hidden}}}}\n"),
    )
    .unwrap();
    let output = cli(tmp.path(), &["--frozen", "check"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("p.bad: rule: invalid expression fields")
    );
}

#[test]
fn collection_only_physical_hypothesis_inherits_base_roles() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(
        tmp.path().join("GROUNDING.yaml"),
        format!("{SCHEMA}known:\n  p.a: {{v: 1}}\n"),
    )
    .unwrap();
    fs::create_dir_all(tmp.path().join(".kpopper/hypotheses")).unwrap();
    fs::write(
        tmp.path().join(".kpopper/hypotheses/decision.yaml"),
        "hypothesis: {born: '2026-09-19'}\njudgments:\n  d.next: {verdict: wait, rests_on: [p.a], seen: {p.a: 1}, wrong_if: 'p.a > 2'}\n",
    )
    .unwrap();
    let output = cli(tmp.path(), &["--frozen", "check"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("ordinary_hypothesis_unreadable"));
}

#[test]
#[ignore = "uses the supplied immutable Python 1.8 oracle"]
fn collection_only_physical_hypothesis_matches_python() {
    let (python, oracle) = oracle();
    let tmp = tempfile::tempdir().unwrap();
    let record = tmp.path().join("GROUNDING.yaml");
    fs::write(&record, format!("{SCHEMA}known:\n  p.a: {{v: 1}}\n")).unwrap();
    fs::create_dir_all(tmp.path().join(".kpopper/hypotheses")).unwrap();
    fs::write(
        tmp.path().join(".kpopper/hypotheses/decision.yaml"),
        "hypothesis: {born: '2026-09-19'}\njudgments:\n  d.next: {verdict: wait, rests_on: [p.a], seen: {p.a: 1}, wrong_if: 'p.a > 2'}\n",
    )
    .unwrap();
    let native = cli(tmp.path(), &["--frozen", "check"]);
    let legacy = Command::new(&python)
        .arg(&oracle)
        .arg("--frozen")
        .arg("check")
        .arg(&record)
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_eq!(native.status.code(), legacy.status.code());
    assert_eq!(native.stdout, legacy.stdout);
}

#[test]
#[ignore = "uses the supplied immutable Python 1.8 oracle"]
fn ordinary_key_matrix_matches_python_check_pull_and_mark() {
    let (python, oracle) = oracle();
    let cases = [
        format!("{SCHEMA}known:\n  p.a: {{v: 1, on: note}}\n"),
        format!(
            "{SCHEMA}known:\n  p.a: {{v: 1}}\njudgments:\n  d.go: {{verdict: go, rests_on: [p.a], seen: {{p.a: 1}}, wrong_if: 'p.a > 2', on: note}}\n"
        ),
        format!(
            "{SCHEMA}known:\n  p.map: {{v: {{on: x, 'true': y}}}}\njudgments:\n  d.go: {{verdict: go, rests_on: [p.map], seen: {{p.map: {{1: one, '1': text}}}}, wrong_if: 'p.map == never'}}\n"
        ),
        format!(
            "{SCHEMA}known:\n  p.a: {{v: 1}}\njudgments:\n  d.go: {{verdict: go, rests_on: [p.a], seen: {{p.a: 1}}, wrong_if: 'p.a > 2', on: first, true: second, 1: third, 1.0: last, 'true': text}}\n"
        ),
        format!(
            "{SCHEMA}known:\n  p.a: {{v: 1}}\n  p.bad: {{rule: {{op: add, args: [{{ref: p.a}}, {{num: '1'}}], on: hidden}}}}\n"
        ),
    ];
    for text in cases {
        let tmp = tempfile::tempdir().unwrap();
        let record = tmp.path().join("GROUNDING.yaml");
        fs::write(&record, text).unwrap();
        for command in ["check", "pull"] {
            let native = cli(tmp.path(), &["--frozen", command]);
            let legacy = Command::new(&python)
                .arg(&oracle)
                .arg("--frozen")
                .arg(command)
                .arg(&record)
                .current_dir(tmp.path())
                .output()
                .unwrap();
            assert_eq!(native.status.code(), legacy.status.code(), "{command}");
            assert_eq!(native.stdout, legacy.stdout, "{command}");
        }
        let native_state = tmp.path().join("native-mark.json");
        let python_state = tmp.path().join("python-mark.json");
        session_gate::mark(&options(&native_state, &record, tmp.path())).unwrap();
        let legacy = Command::new(&python)
            .arg(&oracle)
            .arg("mark")
            .arg(&python_state)
            .arg(&record)
            .current_dir(tmp.path())
            .output()
            .unwrap();
        assert!(
            legacy.status.success(),
            "{}",
            String::from_utf8_lossy(&legacy.stderr)
        );
        let native: Value = serde_json::from_slice(&fs::read(native_state).unwrap()).unwrap();
        let legacy: Value = serde_json::from_slice(&fs::read(python_state).unwrap()).unwrap();
        for key in ["fails", "ids", "judgments"] {
            assert_eq!(native[key], legacy[key], "mark field {key}");
        }
    }
}
