#![cfg(unix)]
use serde_json::{Value as J, json};
use std::{collections::BTreeMap, fs, path::{Path, PathBuf}, process::{Command, Output}};

fn record(root: &Path) -> Vec<u8> {
    let bytes = b"meta: {name: Queue fixture, updated: 2026-09-07}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nsources:\n  s.old: {name: Old fixture, file: old.txt, read: 2026-09-07}\nknown:\n  facts.count: {name: Count, v: 3, from: s.old, at: line 1, of: 2026-09-07}\njudgments:\n  c.acceptable: {rests_on: [facts.count], verdict: acceptable, wrong_if: facts.count < 0, seen: {facts.count: 3}}\n".to_vec();
    fs::write(root.join("PROVENANCE.yaml"), &bytes).unwrap();
    fs::write(root.join("old.txt"), "old fixture\n").unwrap();
    bytes
}
fn run(program: &Path, prefix: &[&str], root: &Path, args: &[&str]) -> Output {
    Command::new(program).args(prefix).args(args).current_dir(root).output().unwrap()
}
fn json_output(output: &Output) -> J {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}
fn stable_capture(mut value: J) -> J {
    if let Some(object) = value.as_object_mut() { object.remove("captured_at"); }
    value
}
fn artifacts(root: &Path, event: &str) -> BTreeMap<String, Vec<u8>> {
    ["envelopes", "sources", "signals"].into_iter().flat_map(|directory| {
        let directory = root.join(directory);
        fs::read_dir(&directory).into_iter().flatten().flatten().filter_map(move |entry| {
            let path = entry.path();
            path.is_file().then(|| (format!("{}/{}", directory.file_name().unwrap().to_string_lossy(), path.file_name().unwrap().to_string_lossy().replace(event, "<event>")), fs::read(path).unwrap()))
        })
    }).collect()
}
fn without_native_provenance(mut value: J) -> J {
    match &mut value {
        J::Array(values) => for value in values { if let Some(object) = value.as_object_mut() { object.remove("mutation"); } },
        J::Object(object) => { object.remove("mutation"); },
        _ => {}
    }
    value
}

#[test]
#[ignore = "requires the pinned Python 1.8 oracle source and runtime"]
fn capture_process_status_and_retry_match_python_cli() {
    let python = PathBuf::from(std::env::var("KPOP_SESSION_ORACLE_PYTHON").expect("set KPOP_SESSION_ORACLE_PYTHON"));
    let oracle_root = PathBuf::from(std::env::var("KPOP_SESSION_ORACLE_ROOT").expect("set KPOP_SESSION_ORACLE_ROOT"));
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let original = record(&root);
    let state = root.join("state");
    let input = root.join("report.json");
    fs::write(&input, serde_json::to_vec(&json!({
        "event_id":"stable", "source_quote":"There are four packages now.",
        "target":"facts.count", "value":4, "date":"2026-09-08", "kind":"report"
    })).unwrap()).unwrap();
    let record_path = root.join("PROVENANCE.yaml");
    let common = ["--record", record_path.to_str().unwrap(), "--state-dir", state.to_str().unwrap()];

    let py_capture = run(&python, &[oracle_root.join("scripts/ingestion.py").to_str().unwrap()], &root,
        &["capture", "--file", input.to_str().unwrap(), common[0], common[1], common[2], common[3], "--no-start"]);
    let py_capture = stable_capture(json_output(&py_capture));
    let py_process = run(&python, &[oracle_root.join("scripts/ingestion.py").to_str().unwrap()], &root,
        &["process", common[0], common[1], common[2], common[3]]);
    let py_process_json = json_output(&py_process);
    let event = py_capture["event_id"].as_str().unwrap().to_owned();
    let py_status = json_output(&run(&python, &[oracle_root.join("scripts/ingestion.py").to_str().unwrap()], &root,
        &["status", "--event-id", &event, common[0], common[1], common[2], common[3]]));
    let py_record = fs::read(root.join("PROVENANCE.yaml")).unwrap();
    let py_artifacts = artifacts(&state, &event);

    fs::remove_dir_all(&state).unwrap();
    fs::write(root.join("PROVENANCE.yaml"), &original).unwrap();
    let native_capture = run(&native, &["ingest"], &root,
        &["capture", "--file", input.to_str().unwrap(), common[0], common[1], common[2], common[3], "--no-start"]);
    assert_eq!(stable_capture(json_output(&native_capture)), py_capture);
    let native_process = run(&native, &["ingest"], &root, &["process", common[0], common[1], common[2], common[3]]);
    let native_process_json = json_output(&native_process);
    let native_status = json_output(&run(&native, &["ingest"], &root,
        &["status", "--event-id", &event, common[0], common[1], common[2], common[3]]));
    assert!(native_process_json[0]["mutation"].is_object());
    assert_eq!(without_native_provenance(native_process_json), py_process_json);
    assert_eq!(without_native_provenance(native_status), py_status);
    assert_eq!(fs::read(root.join("PROVENANCE.yaml")).unwrap(), py_record);
    assert_eq!(artifacts(&state, &event), py_artifacts);
    let retry = json_output(&run(&native, &["ingest"], &root,
        &["capture", "--file", input.to_str().unwrap(), common[0], common[1], common[2], common[3], "--no-start"]));
    assert_eq!(without_native_provenance(retry), py_status);
}
