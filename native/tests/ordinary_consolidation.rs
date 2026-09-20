use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path, process::Command};
fn visit(root: &Path, at: &Path, files: &mut BTreeMap<String, String>, drafts: &mut Vec<String>) {
    let mut children = fs::read_dir(at)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect::<Vec<_>>();
    children.sort();
    for path in children {
        if path.is_dir() {
            if path.file_name().and_then(|s| s.to_str()) != Some(".git") {
                visit(root, &path, files, drafts);
            }
        } else {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("yaml" | "yml")
            ) {
                files.insert(name.clone(), fs::read_to_string(&path).unwrap());
            }
            if name.contains(".history-local/") && name.ends_with(".json") {
                drafts.push(name);
            }
        }
    }
}
fn json_files(at: &Path) -> Vec<String> {
    let mut found = vec![];
    if !at.exists() {
        return found;
    }
    for entry in fs::read_dir(at).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            found.extend(json_files(&p));
        } else if p.extension().and_then(|s| s.to_str()) == Some("json") {
            found.push(fs::read_to_string(p).unwrap());
        }
    }
    found.sort();
    found
}
fn normalize(text: &str, root: &Path) -> String {
    let text = text.replace(root.to_str().unwrap(), "$ROOT");
    if let Some(rest) = text.strip_prefix("private draft retained at ")
        && let Some((_, suffix)) = rest.split_once(".json")
    {
        format!("private draft retained at $PRIVATE_DRAFT{suffix}")
    } else {
        text
    }
}
#[test]
fn actual_ordinary_consolidation_matches_python_packets_and_complete_record_images() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/ordinary-consolidation.json")).unwrap();
    let mut failures = vec![];
    for case in fixture["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let root = directory.join("project");
        fs::create_dir(&root).unwrap();
        if case["git"] == true {
            assert!(
                Command::new("git")
                    .args(["init", "-q"])
                    .arg(&root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        for (path, raw) in case["files"].as_object().unwrap() {
            let p = root.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, raw.as_str().unwrap()).unwrap();
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
        command
            .current_dir(&root)
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap()),
            )
            .env("KPOPPER_PRIVATE_HOME", directory.join("private-data"))
            .env("XDG_STATE_HOME", directory.join("state"))
            .env_remove("KPOPPER_READ_MODE")
            .env_remove("KPOPPER_NATIVE_RESOURCES");
        if let Some(resources) = std::env::var_os("KPOP_CONSOLIDATION_RESOURCES") {
            command.env("KPOPPER_NATIVE_RESOURCES", resources);
        }
        let output = command.output().unwrap();
        let stdout = normalize(std::str::from_utf8(&output.stdout).unwrap(), &root);
        let stderr = normalize(std::str::from_utf8(&output.stderr).unwrap(), &root);
        let expected_stdout = normalize(case["stdout"].as_str().unwrap(), &root);
        let expected_stderr = normalize(case["stderr"].as_str().unwrap(), &root);
        let mut actual = BTreeMap::new();
        let mut journals = vec![];
        visit(&root, &root, &mut actual, &mut journals);
        let expected = case["after"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect::<BTreeMap<_, _>>();
        let mut expected_private = case["private"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        expected_private.sort();
        let fields = [
            (
                "exit",
                output.status.code() == case["exit"].as_i64().map(|v| v as i32),
            ),
            ("stdout", stdout == expected_stdout),
            ("stderr", stderr == expected_stderr),
            ("record/hypothesis images", actual == expected),
            (
                "private closure",
                json_files(&directory.join("private-data")) == expected_private,
            ),
            ("transaction cleanup", journals.is_empty()),
        ];
        let mismatched = fields
            .into_iter()
            .filter(|(_, ok)| !*ok)
            .map(|(k, _)| k)
            .collect::<Vec<_>>();
        if !mismatched.is_empty() {
            failures.push(format!(
                "{}: {}\nstdout: {stdout}\nstderr: {stderr}",
                case["name"].as_str().unwrap(),
                mismatched.join(", ")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
