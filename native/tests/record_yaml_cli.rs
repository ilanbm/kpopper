//! A record that is not valid YAML is told the same way by every command: one diagnostic
//! naming the file, on stderr, with exit status 1. The few commands with their own error
//! conventions carry the same text in them.
use serde_json::{Value as J, json};
use std::{fs, path::Path, process::Command};

const CORPUS: &str = include_str!("fixtures/ordinary-yaml-record-diagnostics.json");
const NOT_YAML: &str = ": the record is not valid YAML.\n";
/// The cases the commands beyond check are run on: a plain one, a CRLF record and one
/// that opens with a byte order mark.
const REPRESENTATIVE: [&str; 3] = ["flow-colon", "crlf", "bom-first-line"];

fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .current_dir(root)
        .args(args)
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .env_remove("KPOPPER_RECORD")
        .env("KPOPPER_PRIVATE_HOME", root.join("private-home"))
        .env("XDG_STATE_HOME", root.join("private-state"));
    command.output().unwrap()
}

fn cases() -> Vec<(String, String, String)> {
    let corpus: J = serde_json::from_str(CORPUS).unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            (
                case["name"].as_str().unwrap().to_owned(),
                case["source"].as_str().unwrap().to_owned(),
                case["expected"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The diagnostic a command prints for the file `name`, newline included.
fn told(name: &str, expected: &str) -> String {
    format!(
        "{name}{NOT_YAML}{}\n",
        expected.replace("<unicode string>", name)
    )
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A project holding only `source` as its record.
fn project(source: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("GROUNDING.yaml"), source).unwrap();
    temp
}

fn assert_untouched(root: &Path, source: &str, context: &str) {
    assert_eq!(
        fs::read(root.join("GROUNDING.yaml")).unwrap(),
        source.as_bytes(),
        "{context}: the record changed"
    );
}

#[test]
fn check_tells_every_malformed_record_and_leaves_it_alone() {
    let cases = cases();
    assert_eq!(cases.len(), 59);
    assert!(cases.iter().any(|(_, source, _)| source.contains("\r\n")));
    for (name, source, expected) in cases {
        let temp = project(&source);
        let output = cli(temp.path(), &["check"]);
        assert_eq!(output.status.code(), Some(1), "{name}");
        assert_eq!(stdout(&output), "", "{name}");
        assert_eq!(stderr(&output), told("GROUNDING.yaml", &expected), "{name}");
        assert_untouched(temp.path(), &source, &name);
    }
}

#[test]
fn every_record_command_tells_a_malformed_record_like_check() {
    let commands: [&[&str]; 14] = [
        &["pull", "p.a"],
        &["affects", "p.a"],
        &["open"],
        &["consolidate", "--dry-run"],
        &["set", "p.a", "2"],
        &["add", "p.b", "1"],
        &["review", "p.a"],
        &["assess", "d.a"],
        &["export", "p.a"],
        &["same", "p.a", "p.b"],
        &["distinct", "p.a", "p.b", "different things"],
        &["answer", "q.a", "yes"],
        &["correct", "p.a", "2"],
        &["remeasure"],
    ];
    for (name, source, expected) in cases() {
        if !REPRESENTATIVE.contains(&name.as_str()) {
            continue;
        }
        let temp = project(&source);
        let text = told("GROUNDING.yaml", &expected);
        for args in commands {
            let context = format!("{name}: kpop {}", args.join(" "));
            let output = cli(temp.path(), args);
            assert_eq!(stderr(&output), text, "{context}");
            assert_eq!(output.status.code(), Some(1), "{context}");
            assert_eq!(stdout(&output), "", "{context}");
            assert_untouched(temp.path(), &source, &context);
        }
    }
}

#[test]
fn remeasure_with_run_tells_a_malformed_record_like_check() {
    let (_, source, expected) = cases()
        .into_iter()
        .find(|(name, _, _)| name == "flow-colon")
        .unwrap();
    let temp = project(&source);
    let output = cli(temp.path(), &["remeasure", "--run"]);
    assert_eq!(stderr(&output), told("GROUNDING.yaml", &expected));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert_untouched(temp.path(), &source, "remeasure --run");
}

#[test]
fn json_wraps_the_diagnostic_as_check_does() {
    for (name, source, expected) in cases() {
        if !REPRESENTATIVE.contains(&name.as_str()) {
            continue;
        }
        let temp = project(&source);
        let text = told("GROUNDING.yaml", &expected);
        for command in ["check", "remeasure"] {
            let context = format!("{name}: kpop --json {command}");
            let output = cli(temp.path(), &["--json", command]);
            assert_eq!(output.status.code(), Some(1), "{context}");
            let envelope: J = serde_json::from_str(&stdout(&output)).unwrap_or_else(|error| {
                panic!("{context}: {error}: {}", stdout(&output));
            });
            assert_eq!(
                envelope,
                json!({"command": command, "error": text, "exit_code": 1, "output": ""}),
                "{context}"
            );
            assert_untouched(temp.path(), &source, &context);
        }
    }
}

#[test]
fn commands_with_their_own_error_form_carry_the_same_diagnostic() {
    for (name, source, expected) in cases() {
        if !REPRESENTATIVE.contains(&name.as_str()) {
            continue;
        }
        let temp = project(&source);
        let text = told("GROUNDING.yaml", &expected);
        let bare = text.strip_suffix('\n').unwrap();

        let context = format!("{name}: kpop context");
        let output = cli(temp.path(), &["context", "p.a"]);
        assert_eq!(output.status.code(), Some(2), "{context}");
        assert_eq!(stdout(&output), "", "{context}");
        let error: J = serde_json::from_str(&stderr(&output)).unwrap();
        assert_eq!(error, json!({"error": bare}), "{context}");

        let context = format!("{name}: kpop search");
        let output = cli(temp.path(), &["search", "load"]);
        assert_eq!(output.status.code(), Some(2), "{context}");
        assert_eq!(stdout(&output), "", "{context}");
        assert_eq!(stderr(&output), text, "{context}");

        let context = format!("{name}: kpop knowledge status");
        let output = cli(temp.path(), &["knowledge", "status"]);
        assert_eq!(output.status.code(), Some(2), "{context}");
        let status: J = serde_json::from_str(&stdout(&output)).unwrap();
        assert_eq!(status["error"], json!(bare), "{context}");

        let context = format!("{name}: kpop experimental hub --verify");
        let output = cli(temp.path(), &["experimental", "hub", "--verify"]);
        assert_eq!(output.status.code(), Some(2), "{context}");
        assert_eq!(stdout(&output), "", "{context}");
        assert_eq!(
            stderr(&output),
            format!("kpop experimental hub: {text}"),
            "{context}"
        );

        assert_untouched(temp.path(), &source, &name);
    }
}

#[test]
fn a_malformed_layer_is_named_by_its_own_path() {
    let (_, source, expected) = cases()
        .into_iter()
        .find(|(name, _, _)| name == "flow-colon")
        .unwrap();
    let entry =
        "also: [sub/extra.yaml]\nknown:\n  p.a: {v: 1, from: s.note}\n  s.note: {name: source}\n";
    let temp = project(entry);
    fs::create_dir(temp.path().join("sub")).unwrap();
    fs::write(temp.path().join("sub/extra.yaml"), &source).unwrap();
    // the layer is named as the platform spells a relative path
    let shown = Path::new("sub").join("extra.yaml").display().to_string();
    let text = told(&shown, &expected);
    for args in [&["check"][..], &["pull", "p.a"], &["remeasure"]] {
        let context = format!("kpop {}", args.join(" "));
        let output = cli(temp.path(), args);
        assert_eq!(stderr(&output), text, "{context}");
        assert_eq!(output.status.code(), Some(1), "{context}");
        assert_eq!(stdout(&output), "", "{context}");
    }
    assert_untouched(temp.path(), entry, "the entry");
    assert_eq!(
        fs::read(temp.path().join("sub/extra.yaml")).unwrap(),
        source.as_bytes()
    );
}
