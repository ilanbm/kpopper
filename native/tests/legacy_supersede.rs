use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn resources() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(root.path().join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        root.path().join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean")
                .join(&target)
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    let destination = root.path().join("ordinary").join(&target);
    fs::create_dir_all(&destination).unwrap();
    for name in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(program.join(name), destination.join(name)).unwrap();
    }
    root
}

#[test]
fn replacements_and_returning_versions_match_complete_python_images() {
    let cases: Value =
        serde_json::from_str(include_str!("fixtures/legacy-supersede/oracle.json")).unwrap();
    let runtime = resources();
    let mut failures = Vec::new();
    for case in cases.as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join(case["record"].as_str().unwrap());
        let sidecar = temp.path().join(case["sidecar"].as_str().unwrap());
        fs::write(&record, case["before"].as_str().unwrap()).unwrap();
        if let Some(before) = case["kept_before"].as_str() {
            fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
            fs::write(&sidecar, before).unwrap();
        }
        let args = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect::<Vec<_>>();
        let result = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(temp.path())
            .args(args)
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env("KPOPPER_NATIVE_RESOURCES", runtime.path())
            .env("KPOPPER_NATIVE_CACHE", runtime.path().join("cache"))
            .output()
            .unwrap();
        let name = case["name"].as_str().unwrap();
        if result.status.code().map(i64::from) != case["status"].as_i64() {
            failures.push(format!(
                "{name}: exit {:?}, stderr {}",
                result.status.code(),
                String::from_utf8_lossy(&result.stderr)
            ));
        }
        for (kind, actual, expected) in [
            (
                "record",
                fs::read_to_string(&record).unwrap(),
                case["after"].as_str().unwrap().to_owned(),
            ),
            (
                "sidecar",
                fs::read_to_string(&sidecar).unwrap_or_default(),
                case["kept_after"].as_str().unwrap_or("").to_owned(),
            ),
            (
                "stdout",
                String::from_utf8(result.stdout).unwrap(),
                case["stdout"].as_str().unwrap().to_owned(),
            ),
            (
                "stderr",
                String::from_utf8(result.stderr).unwrap(),
                case["stderr"].as_str().unwrap().to_owned(),
            ),
        ] {
            if actual != expected {
                failures.push(format!(
                    "{name}: {kind}\nactual: {actual}\nexpected: {expected}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
