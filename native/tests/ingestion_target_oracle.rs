#![cfg(unix)]
use serde_json::{Value as J, json};
use std::{fs, path::Path, process::Command};

fn oracle(record: &Path, envelope: &J) -> J {
    let python = std::env::var("KPOP_SESSION_ORACLE_PYTHON").expect("set KPOP_SESSION_ORACLE_PYTHON");
    let root = std::env::var("KPOP_SESSION_ORACLE_ROOT").expect("set KPOP_SESSION_ORACLE_ROOT");
    let code = r#"import json, pathlib, sys
sys.path.insert(0, sys.argv[1] + '/scripts')
import ingestion
try:
    print(json.dumps({'ok': ingestion._report_target(pathlib.Path(sys.argv[2]), json.loads(sys.argv[3]))}, sort_keys=True))
except BaseException as error:
    print(json.dumps({'error': str(error)}, sort_keys=True))
"#;
    let output = Command::new(python).args(["-c", code, &root, record.to_str().unwrap(), &serde_json::to_string(envelope).unwrap()]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn native(record: &Path, envelope: &J) -> J {
    let raw = fs::read(record).unwrap();
    let document = kpop_native::history_yaml::decode_document(&raw).unwrap();
    match kpop_native::ingestion_target::snapshot(&document, &raw, envelope) {
        Ok(value) => json!({"ok":value}),
        Err(error) => json!({"error":error.to_string()}),
    }
}

#[test]
#[ignore = "requires the pinned Python 1.8 oracle source and runtime"]
fn target_snapshots_match_python_for_capture_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("GROUNDING.yaml");
    let source = "schema: {deps: rests_on, snapshot: reviewed, predicate: wrong_if}\nsources:\n  s.one: {file: one.txt, read: 2026-09-01}\nother_sources:\n  s.two: {url: 'https://example.invalid', read: 2026-09-01}\nknown:\n  p.a: {v: 1, from: s.one, at: line 1}\n  p.b: {v: two, from: s.two, at: page 2}\n  p.inline: {v: 'p.a + 1'}\njudgments:\n  d.a: {rests_on: [p.a], verdict: okay, wrong_if: p.a > 9, reviewed: {p.a: 1}}\n";
    fs::write(&record, source).unwrap();
    let hash = kpop_native::identity::sha256(source.as_bytes());
    let cases = vec![
        json!({"source_quote":"q","target":"p.a","value":2,"date":"2026-09-02"}),
        json!({"source_quote":"q","target":"p.a","value":2,"date":"2026-09-02","record_sha256":"0".repeat(64)}),
        json!({"source_quote":"q","target":"p.a","value":2,"date":"2026-09-02","source":"s.one","record_sha256":hash}),
        json!({"source_quote":"q","target":"p.a","value":2,"date":"2026-09-02","source":"s.one","at":"line 3","record_sha256":hash}),
        json!({"source_quote":"q","target":"p.inline","value":"x","date":"2026-09-02"}),
        json!({"source_quote":"q","date":"2026-09-02","record_sha256":hash,"updates":[{"kind":"add","id":"p.c","body":{"v":3}}]}),
        json!({"source_quote":"q","date":"2026-09-02","record_sha256":hash,"updates":[{"kind":"set","id":"p.a","value":2},{"kind":"set","id":"p.b","value":"three"}]}),
        json!({"source_quote":"q","date":"2026-09-02","record_sha256":hash,"updates":[{"kind":"add","id":"d.c","body":{"rests_on":["p.a"],"verdict":"ok","wrong_if":"p.a > 2","reviewed":{}}}]}),
    ];
    for envelope in cases {
        assert_eq!(native(&record, &envelope), oracle(&record, &envelope), "{envelope}");
    }
}
