//! Complete immutable Python CLI oracles: output streams, exit status, and every
//! resulting source/copy file. Only root and generated backup names are normalized.
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/public-expressions-oracle.json")).unwrap()
}
fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}
fn normalize(text: &str, root: &Path) -> String {
    let text = text.replace(root.to_str().unwrap(), "$ROOT");
    regex::Regex::new(r"\.GROUNDING\.yaml\.pre-core-[A-Za-z0-9_-]+\.bak")
        .unwrap()
        .replace_all(&text, ".GROUNDING.yaml.pre-core-$$BACKUP.bak")
        .into_owned()
}
fn images(root: &Path) -> Value {
    fn walk(root: &Path, path: &Path, result: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, result);
            } else {
                let key = normalize(path.strip_prefix(root).unwrap().to_str().unwrap(), root);
                let raw = fs::read(&path).unwrap();
                result.insert(key, raw.iter().map(|v| format!("{v:02x}")).collect());
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result);
    json!(result)
}
fn check_case(case: &Value) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for (path, raw) in case["input"].as_object().unwrap() {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes(raw.as_str().unwrap())).unwrap();
    }
    let mut expected = case["expected"].clone();
    if !case["config_template"].is_null() {
        let config = serde_json::to_string(&case["config_template"])
            .unwrap()
            .replace("$ROOT", root.to_str().unwrap());
        let config: Value = serde_json::from_str(&config).unwrap();
        let digest = kpop_native::value::TypedValue::from_json(&config)
            .unwrap()
            .digest()
            .unwrap();
        expected["stdout"] = json!(
            expected["stdout"]
                .as_str()
                .unwrap()
                .replace("$CONFIG_SHA256", &digest)
        );
        for raw in expected["files"].as_object_mut().unwrap().values_mut() {
            let data = bytes(raw.as_str().unwrap());
            if let Ok(text) = std::str::from_utf8(&data) {
                let data = text.replace("$CONFIG_SHA256", &digest);
                *raw = json!(
                    data.as_bytes()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                );
            }
        }
    }
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(
            case["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap()),
        )
        .current_dir(&root)
        .output()
        .unwrap();
    let actual = json!({
        "exit": output.status.code(),
        "stdout": normalize(std::str::from_utf8(&output.stdout).unwrap(), &root),
        "stderr": normalize(std::str::from_utf8(&output.stderr).unwrap(), &root),
        "files": images(&root),
    });
    if actual != expected {
        let keys = ["exit", "stdout", "stderr", "files"]
            .into_iter()
            .filter(|key| actual[key] != expected[key])
            .collect::<Vec<_>>();
        let stdout = actual["stdout"].as_str().unwrap();
        let expected_stdout = expected["stdout"].as_str().unwrap();
        let at = stdout
            .bytes()
            .zip(expected_stdout.bytes())
            .position(|(a, b)| a != b)
            .unwrap_or(stdout.len().min(expected_stdout.len()));
        let snippet = |s: &str| {
            s.chars()
                .skip(at.saturating_sub(50))
                .take(180)
                .collect::<String>()
        };
        panic!(
            "{} differs in {:?} at byte {}\nactual: {}\nexpected: {}\nstderr: {}",
            case["name"],
            keys,
            at,
            snippet(stdout),
            snippet(expected_stdout),
            actual["stderr"]
        );
    }
}
#[test]
fn public_conversion_and_parser_errors_match_complete_python_cli() {
    for case in corpus()["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if name.starts_with("convert-")
            || ["invalid-profile", "missing-record-value", "invalid-action"].contains(&name)
        {
            check_case(case);
        }
    }
}
#[test]
fn public_migrations_match_reports_and_all_file_images() {
    if std::env::var_os("KPOPPER_NATIVE_RESOURCES").is_none() {
        eprintln!("set KPOPPER_NATIVE_RESOURCES to exercise the complete migration oracles");
        return;
    }
    for case in corpus()["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if name != "core-frozen-global"
            && !name.starts_with("convert-")
            && !["invalid-profile", "missing-record-value", "invalid-action"].contains(&name)
        {
            check_case(case);
        }
    }
}

#[test]
fn public_global_frozen_preserves_explicit_migration_capture() {
    if std::env::var_os("KPOPPER_NATIVE_RESOURCES").is_none() {
        return;
    }
    let corpus = corpus();
    let case = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "core-frozen-global")
        .unwrap();
    check_case(case);
}
