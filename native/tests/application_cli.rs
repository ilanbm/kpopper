use serde_json::Value;
use std::process::Command;

#[test]
fn catalog_boundaries_and_alias_envelopes_match_the_public_contract() {
    let cases: Value =
        serde_json::from_slice(include_bytes!("fixtures/application-cli.json")).unwrap();
    let root = tempfile::tempdir().unwrap();
    for case in cases.as_array().unwrap() {
        let args = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect::<Vec<_>>();
        let output = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
            .current_dir(root.path())
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            case["code"].as_i64(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            case["stderr"],
            "{args:?}"
        );
        if case["stdout"].is_object() {
            let packet: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(packet, case["stdout"], "{args:?}");
        } else {
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                case["stdout"],
                "{args:?}"
            );
        }
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn workspace_argument_values_do_not_become_application_commands() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("experimental");
    std::fs::create_dir(&workspace).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args(["--workspace", workspace.to_str().unwrap(), "where"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
