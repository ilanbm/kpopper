use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
fn run(root: &Path, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command.args(["--workspace", root.to_str().unwrap(), "where"]);
    if json {
        command.arg("--json");
    }
    command.output().unwrap()
}
#[test]
fn where_locates_without_reading_and_missing_is_quiet() {
    let root = tempfile::tempdir().unwrap();
    let missing = run(root.path(), false);
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty() && missing.stderr.is_empty());
    let wrapped = run(root.path(), true);
    assert_eq!(wrapped.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&wrapped.stdout).unwrap(),
        json!({"command":"where","exit_code":1,"output":"","error":""})
    );
    fs::write(root.path().join("GROUNDING.yaml"), "not: [valid YAML").unwrap();
    let nested = root.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let found = run(&nested, false);
    assert!(
        found.status.success(),
        "{}",
        String::from_utf8_lossy(&found.stderr)
    );
    assert_eq!(
        String::from_utf8(found.stdout).unwrap(),
        format!(
            "{}\n",
            root.path()
                .canonicalize()
                .unwrap()
                .join("GROUNDING.yaml")
                .display()
        )
    );
    assert_eq!(
        fs::read_to_string(root.path().join("GROUNDING.yaml")).unwrap(),
        "not: [valid YAML"
    );
}
