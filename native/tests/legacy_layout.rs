use std::{fs, path::Path, process::Command};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/legacy-layout");

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .args(args)
        .output()
        .unwrap()
}

fn fixture(name: &str) -> Vec<u8> {
    fs::read(Path::new(FIXTURES).join(name)).unwrap()
}

#[test]
fn complete_scalar_container_and_separator_oracle_cases() {
    let cases: serde_json::Value =
        serde_json::from_slice(&fixture("complete-oracle.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("GROUNDING.yaml"),
            case["before"].as_str().unwrap(),
        )
        .unwrap();
        let args = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        let output = run(temp.path(), &args);
        let name = case["name"].as_str().unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            case["status"].as_i64(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            case["stdout"].as_str().unwrap(),
            "{name}: stdout"
        );
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            case["stderr"].as_str().unwrap(),
            "{name}: stderr"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("GROUNDING.yaml")).unwrap(),
            case["after"].as_str().unwrap(),
            "{name}: record bytes"
        );
    }
}

#[test]
fn legacy_layout_matches_python_shapes() {
    let cases: &[(&str, &str, &str, &[&str])] = &[
        (
            "folded-before.yaml",
            "folded-after.yaml",
            "folded.stdout",
            &[
                "set",
                "p.alpha",
                "9",
                "--as-of",
                "2026-09-20",
                "GROUNDING.yaml",
            ],
        ),
        (
            "meta4-before.yaml",
            "meta4-after.yaml",
            "",
            &[
                "set",
                "p.alpha",
                "7",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ],
        ),
        (
            "comments-before.yaml",
            "comments-after.yaml",
            "",
            &[
                "add",
                "p.aardvark",
                "v=5",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ],
        ),
        (
            "newsource-before.yaml",
            "newsource-after.yaml",
            "",
            &[
                "set",
                "p.alpha",
                "3",
                "--source",
                "s.new",
                "--at",
                "page-2",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ],
        ),
    ];
    for (before, after, stdout, args) in cases {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("GROUNDING.yaml"), fixture(before)).unwrap();
        let output = run(temp.path(), args);
        assert!(
            output.status.success(),
            "{before}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !stdout.is_empty() {
            assert_eq!(output.stdout, fixture(stdout), "{before} stdout");
        }
        assert_eq!(
            fs::read(temp.path().join("GROUNDING.yaml")).unwrap(),
            fixture(after),
            "{before}"
        );
    }
}

#[test]
fn nested_add_is_emitted_as_block_yaml() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("GROUNDING.yaml"),
        fixture("nested-before.yaml"),
    )
    .unwrap();
    let output = run(
        temp.path(),
        &[
            "add",
            "p.nested",
            "items=[[1,2],[3]]",
            "--as-of",
            "2026-09-19",
            "GROUNDING.yaml",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written = fs::read_to_string(temp.path().join("GROUNDING.yaml")).unwrap();
    assert!(
        written.contains("items:\n    - - 1\n      - 2\n    - - 3"),
        "{written}"
    );
}
