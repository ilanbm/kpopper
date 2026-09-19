mod support;

use base64::{Engine, engine::general_purpose::STANDARD};
use kpop_native::{
    project_modes::Project,
    public_knowledge::{self, MaterializeOptions, StatusOptions},
    public_pending,
    source_capture::ReadMode,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, process::Command};
use support::pending::{fixture, git};

fn fixtures() -> (Value, Value) {
    (
        serde_json::from_str(include_str!("fixtures/public-knowledge-oracle.json")).unwrap(),
        serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap(),
    )
}

fn ledger_case<'a>(ledgers: &'a Value, name: &str) -> &'a Value {
    ledgers
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap()
}

fn expected(entry: &Value, replacements: &[(&str, &Path)]) -> Value {
    let mut text = entry["actual"]["stdout"].as_str().unwrap().to_owned();
    for (token, path) in replacements {
        let path = if path.exists() {
            path.canonicalize().unwrap()
        } else {
            path.parent()
                .unwrap()
                .canonicalize()
                .unwrap()
                .join(path.file_name().unwrap())
        };
        text = text.replace(token, path.to_str().unwrap());
    }
    serde_json::from_str(&text).unwrap()
}

#[test]
fn knowledge_status_matches_live_frozen_and_unavailable_python_views() {
    let (oracle, ledgers) = fixtures();
    for entry in oracle["status"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        if name.starts_with("simple_") || name == "registered_other_cwd" {
            continue;
        }
        let setup = entry["setup"].as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        fixture(&root, ledger_case(&ledgers, setup));
        let mode = if entry["read_mode"] == "live" {
            ReadMode::Live
        } else {
            ReadMode::Frozen
        };
        let actual = public_knowledge::status(&root, mode, &StatusOptions::default())
            .unwrap()
            .to_json()
            .unwrap();
        let mut wanted = expected(entry, &[("$ROOT", &root)]);
        wanted["private_drafts"] = json!([]);
        assert_eq!(actual, wanted, "{name}");
    }
}

#[test]
fn simple_and_registered_status_are_routed_from_the_callers_working_directory() {
    let (oracle, _) = fixtures();
    let simple = tempfile::tempdir().unwrap();
    std::fs::write(
        simple.path().join("GROUNDING.yaml"),
        "p:\n  p.local: {v: local}\n",
    )
    .unwrap();
    for name in ["simple_live", "simple_frozen"] {
        let entry = oracle["status"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap();
        let mode = if name.ends_with("live") {
            ReadMode::Live
        } else {
            ReadMode::Frozen
        };
        assert_eq!(
            public_knowledge::status(simple.path(), mode, &StatusOptions::default())
                .unwrap()
                .to_json()
                .unwrap(),
            expected(entry, &[("$ROOT", simple.path())])
        );
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("registered");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-b", "trunk"], None);
    let record = temp.path().join("shared/facts.yaml");
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    std::fs::write(&record, "p:\n  p.shared: {v: shared}\n").unwrap();
    let project = Project::open(&root).unwrap();
    std::fs::create_dir_all(&project.state).unwrap();
    std::fs::write(
        &project.config_path,
        serde_json::to_vec(&json!({"version":1,"mode":"simple","record":record,
            "publication":null,"generation":0}))
        .unwrap(),
    )
    .unwrap();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let entry = oracle["status"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "registered_other_cwd")
        .unwrap();
    assert_eq!(
        public_knowledge::status(&nested, ReadMode::Live, &StatusOptions::default())
            .unwrap()
            .to_json()
            .unwrap(),
        expected(entry, &[("$ROOT", &root), ("$RECORD", &record)])
    );
}

#[test]
fn pending_status_matches_python_without_remote_verification() {
    let (oracle, ledgers) = fixtures();
    for entry in oracle["pending"].as_array().unwrap() {
        let setup = entry["setup"].as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        if setup != "simple" {
            fixture(&root, ledger_case(&ledgers, setup));
        }
        let actual = public_pending::status(&root, &public_pending::StatusOptions::default())
            .unwrap()
            .to_json()
            .unwrap();
        assert_eq!(actual, expected(entry, &[("$ROOT", &root)]), "{setup}");
    }
}

fn files(root: &Path) -> BTreeMap<String, String> {
    fn visit(base: &Path, path: &Path, result: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(base, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    STANDARD.encode(std::fs::read(path).unwrap()),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn materialize_matches_python_bytes_and_refuses_overwrite_or_missing_revision() {
    let (oracle, ledgers) = fixtures();
    let case = ledger_case(&ledgers, "one");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    fixture(&root, case);
    let complete = &oracle["materialize"][0];
    let revision = complete["revision"].as_str().unwrap();
    let out = temp.path().join("snapshot");
    let actual = public_knowledge::materialize(
        &root,
        &MaterializeOptions {
            revision: revision.into(),
            out: out.clone(),
            reference: None,
        },
    )
    .unwrap()
    .to_json()
    .unwrap();
    assert_eq!(actual, expected(complete, &[("$OUT", &out)]));
    assert_eq!(
        files(&out),
        serde_json::from_value(complete["files"].clone()).unwrap()
    );
    assert_eq!(
        public_knowledge::materialize(
            &root,
            &MaterializeOptions {
                revision: revision.into(),
                out: out.clone(),
                reference: None
            },
        )
        .unwrap_err()
        .0,
        "snapshot destination already exists; select a new directory"
    );
    let missing = "0".repeat(64);
    assert_eq!(
        public_knowledge::materialize(
            &root,
            &MaterializeOptions {
                revision: missing,
                out: temp.path().join("missing"),
                reference: None,
            },
        )
        .unwrap_err()
        .0,
        "contribution is unavailable at the selected ledger revision"
    );
}

#[test]
fn materialize_refuses_tampered_ledger_objects_without_publishing_a_tree() {
    let (_, ledgers) = fixtures();
    let case = ledger_case(&ledgers, "one");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    fixture(&root, case);
    let blobs = case["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|object| object["kind"] == "blob")
        .take(2)
        .map(|object| object["oid"].as_str().unwrap())
        .collect::<Vec<_>>();
    let object = |oid: &str| root.join(".git/objects").join(&oid[..2]).join(&oid[2..]);
    let replacement = std::fs::read(object(blobs[1])).unwrap();
    std::fs::remove_file(object(blobs[0])).unwrap();
    std::fs::write(object(blobs[0]), replacement).unwrap();
    let output = temp.path().join("tampered-output");
    let revision = "8887db9b513dddd4f3d7d26155713251effcfaa5e97a8edbcb1d7ced6d7297dd";
    assert!(
        public_knowledge::materialize(
            &root,
            &MaterializeOptions {
                revision: revision.into(),
                out: output.clone(),
                reference: None,
            },
        )
        .is_err()
    );
    assert!(!output.exists());
}

#[test]
fn registered_cli_surface_matches_python_json_and_exit_codes() {
    let (oracle, ledgers) = fixtures();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    fixture(&root, ledger_case(&ledgers, "one"));
    let binary = env!("CARGO_BIN_EXE_kpop-native");
    for arguments in [
        ["knowledge", "status"].as_slice(),
        ["pending", "status"].as_slice(),
    ] {
        let result = Command::new(binary)
            .args(["--workspace", root.to_str().unwrap()])
            .args(arguments)
            .env("KPOPPER_PRIVATE_HOME", temp.path().join("private"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let actual: Value = serde_json::from_slice(&result.stdout).unwrap();
        let section = if arguments[0] == "knowledge" {
            "status"
        } else {
            "pending"
        };
        let expected_entry = oracle[section]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| {
                entry["name"] == "one_live" || section == "pending" && entry["name"] == "one"
            })
            .unwrap();
        let expected = expected(expected_entry, &[("$ROOT", &root)]);
        let mut expected = expected;
        if section == "status" {
            expected["private_drafts"] = json!([]);
        }
        assert_eq!(actual, expected);
    }
    let missing = "0".repeat(64);
    let result = Command::new(binary)
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "knowledge",
            "materialize",
            &missing,
            "--out",
            temp.path().join("missing").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    let unavailable = oracle["materialize"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "unavailable")
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap(),
        expected(unavailable, &[("$OUT", &temp.path().join("missing"))])
    );
}

#[test]
fn status_does_not_require_a_strict_snapshot_for_ordinary_metadata_keys() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("GROUNDING.yaml"),
        "meta:\n  1: scalar key\np:\n  p.local: {v: local}\n",
    )
    .unwrap();
    let result = public_knowledge::status(temp.path(), ReadMode::Live, &StatusOptions::default())
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(result["mode"], "simple");
    assert_eq!(result["contributions"], json!([]));
}
