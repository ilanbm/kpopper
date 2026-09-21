use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn case(prefix: &str, kind: &str) -> Value {
    kpop_native::json_ingress::parse_slice(
        include_bytes!("fixtures/history-identity.json"),
        kpop_native::json_ingress::DuplicateKeys::Reject,
    )
    .unwrap()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"].as_str().unwrap().starts_with(prefix) && case["kind"] == kind)
        .unwrap()
        .clone()
}

fn materialize(case: &Value) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, raw) in case["files"].as_object().unwrap() {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, STANDARD.decode(raw.as_str().unwrap()).unwrap()).unwrap();
    }
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
    command
        .current_dir(root)
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"));
    command
}

fn run(root: &Path, args: &[&str]) -> Output {
    command(root).args(args).output().unwrap()
}

#[test]
fn same_and_distinct_are_complete_public_commands() {
    for (prefix, kind, extra) in [
        ("same_rewrites_current", "same", Vec::<&str>::new()),
        (
            "distinct_keeps_original",
            "distinct",
            vec!["separate observations"],
        ),
    ] {
        let root = materialize(&case(prefix, kind));
        let mut args = vec![
            "--workspace",
            root.path().to_str().unwrap(),
            kind,
            "p.input",
            "p.other",
        ];
        args.extend(extra);
        args.extend(["--as-of", "2026-09-19", "GROUNDING.yaml"]);
        let output = run(root.path(), &args);
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.starts_with(&format!("history committed: history-{kind}-")));
        assert!(stdout.ends_with(&format!(" ({kind} p.input, p.other)\n")));
        let status = run(
            root.path(),
            &[
                "--workspace",
                root.path().to_str().unwrap(),
                "history",
                "status",
            ],
        );
        assert!(status.status.success());
        let state: Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(
            state["commits"].as_u64().unwrap(),
            if kind == "same" { 4 } else { 3 }
        );
        let add = run(
            root.path(),
            &[
                "--workspace",
                root.path().to_str().unwrap(),
                "add",
                "p.after",
                "v=3",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ],
        );
        assert!(
            add.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&add.stdout),
            String::from_utf8_lossy(&add.stderr)
        );
    }
}

#[test]
fn distinct_requires_a_single_line_reason_and_refusals_preserve_the_record() {
    let root = materialize(&case("distinct_keeps_original", "distinct"));
    let entry = root.path().join("GROUNDING.yaml");
    let before = fs::read(&entry).unwrap();
    let missing = run(
        root.path(),
        &[
            "--workspace",
            root.path().to_str().unwrap(),
            "distinct",
            "p.input",
            "p.other",
        ],
    );
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("<WHY>"));
    let multiline = run(
        root.path(),
        &[
            "--workspace",
            root.path().to_str().unwrap(),
            "distinct",
            "p.input",
            "p.other",
            "first line\nsecond line",
            "GROUNDING.yaml",
        ],
    );
    assert_eq!(multiline.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(multiline.stdout).unwrap(),
        "refused: the why is one line: a second line would be a line of the record\n"
    );
    assert_eq!(fs::read(entry).unwrap(), before);
}

#[test]
fn json_refusal_wraps_the_ordinary_output_and_exit_code() {
    let root = materialize(&case("distinct_keeps_original", "distinct"));
    let record = root.path().join("GROUNDING.yaml");
    let before = fs::read(&record).unwrap();
    let output = run(
        root.path(),
        &["distinct", "p.input", "p.other", "first\nsecond", "--json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let packet: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        packet,
        serde_json::json!({"command":"distinct","exit_code":1,
        "output":"refused: the why is one line: a second line would be a line of the record\n","error":""})
    );
    assert_eq!(fs::read(record).unwrap(), before);
}
