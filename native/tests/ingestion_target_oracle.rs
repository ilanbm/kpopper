#![cfg(unix)]
use serde_json::{Value as J, json};
use std::{fs, path::Path, process::Command};

fn oracle(record: &Path, envelope: &J) -> J {
    let python =
        std::env::var("KPOP_SESSION_ORACLE_PYTHON").expect("set KPOP_SESSION_ORACLE_PYTHON");
    let root = std::env::var("KPOP_SESSION_ORACLE_ROOT").expect("set KPOP_SESSION_ORACLE_ROOT");
    let code = r#"import json, pathlib, sys
sys.path.insert(0, sys.argv[1] + '/scripts')
import ingestion
try:
    print(json.dumps({'ok': ingestion._report_target(pathlib.Path(sys.argv[2]), json.loads(sys.argv[3]))}, sort_keys=True))
except BaseException as error:
    print(json.dumps({'error': str(error)}, sort_keys=True))
"#;
    let output = Command::new(python)
        .args([
            "-c",
            code,
            &root,
            record.to_str().unwrap(),
            &serde_json::to_string(envelope).unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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

fn native_public(record: &Path, envelope: &J) -> J {
    match kpop_native::public_update::capture_target(envelope, record, record.parent().unwrap()) {
        Ok(value) => json!({"ok":value}),
        Err(error) => json!({"error":error.to_string()}),
    }
}

fn parity_case(name: &str, record: &Path, envelope: &J) {
    let expected = oracle(record, envelope);
    let decoded = native(record, envelope);
    let public = native_public(record, envelope);
    if let Ok(root) = std::env::var("KPOP_SESSION_EVIDENCE_DIR") {
        let slug = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>();
        let directory = Path::new(&root).join(slug);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("record.yaml"), fs::read(record).unwrap()).unwrap();
        fs::write(
            directory.join("envelope.json"),
            serde_json::to_vec_pretty(envelope).unwrap(),
        )
        .unwrap();
        for (file, value) in [
            ("python.json", &expected),
            ("native-direct.json", &decoded),
            ("native-public.json", &public),
        ] {
            fs::write(directory.join(file), serde_json::to_vec_pretty(value).unwrap()).unwrap();
        }
    }
    println!(
        "{}\t{}\t{}",
        name,
        serde_json::to_string(&expected).unwrap(),
        kpop_native::identity::sha256(&fs::read(record).unwrap())
    );
    assert_eq!(decoded, expected, "decoded target: {name}: {envelope}");
    assert_eq!(public, expected, "public target: {name}: {envelope}");
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
        assert_eq!(
            native(&record, &envelope),
            oracle(&record, &envelope),
            "{envelope}"
        );
    }
}

#[test]
#[ignore = "requires the pinned Python 1.8 oracle source and runtime"]
fn hostile_target_shapes_match_python_for_successes_and_refusals() {
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("GROUNDING.yaml");
    let schema = "schema: {deps: rests_on, snapshot: reviewed, predicate: wrong_if}\n";
    let plain = format!(
        "{schema}sources:\n  s.one: {{file: one.txt, read: 2026-09-01}}\nknown:\n  p.a: {{v: 1, from: s.one, at: line 1}}\n  p.b: {{v: two, from: s.one, at: line 2}}\njudgments:\n  d.base: {{rests_on: [p.a], verdict: okay, wrong_if: 'p.a > 9', reviewed: {{p.a: 1}}}}\n"
    );
    let run = |name: &str, source: &str, mut envelope: J| {
        fs::write(&record, source).unwrap();
        if envelope["record_sha256"] == "@RECORD_SHA@" {
            envelope["record_sha256"] = json!(kpop_native::identity::sha256(source.as_bytes()));
        }
        parity_case(name, &record, &envelope);
    };

    for (operator, expression) in [
        ("or", "p.a or p.b"),
        ("and", "p.a and p.b"),
        ("not", "not p.a"),
    ] {
        run(
            &format!("word operator {operator} is derived"),
            &format!(
                "{schema}sources:\n  s.one: {{file: one.txt, read: 2026-09-01}}\nknown:\n  p.a: {{v: 1}}\n  p.b: {{v: 2}}\n  p.expr: {{v: '{expression}'}}\n"
            ),
            json!({"source_quote":"q","date":"2026-09-02","target":"p.expr","value":"x"}),
        );
    }
    run(
        "null rule remains a stored reading",
        &format!(
            "{schema}sources:\n  s.one: {{file: one.txt, read: 2026-09-01}}\nknown:\n  p.a: {{v: 1, rule: null}}\n"
        ),
        json!({"source_quote":"q","date":"2026-09-02","target":"p.a","value":2}),
    );
    for (name, source) in [
        (
            "loose cited home with another source collection",
            format!(
                "{schema}sources:\n  s.one: {{file: one.txt, read: 2026-09-01}}\nnotes:\n  n.x: {{note: hello}}\nknown:\n  p.a: {{v: 1, from: n.x}}\njudgments:\n  d.base: {{rests_on: [p.a], verdict: ok, wrong_if: 'p.a > 9', reviewed: {{p.a: 1}}}}\n"
            ),
        ),
        (
            "loose cited home without a source collection",
            format!(
                "{schema}notes:\n  n.x: {{note: hello}}\nknown:\n  p.a: {{v: 1, from: n.x}}\njudgments:\n  d.base: {{rests_on: [p.a], verdict: ok, wrong_if: 'p.a > 9', reviewed: {{p.a: 1}}}}\n"
            ),
        ),
    ] {
        run(
            name,
            &source,
            json!({"source_quote":"q","date":"2026-09-02","target":"p.a","value":2}),
        );
    }
    run(
        "duplicate cited source homes are ambiguous",
        &format!(
            "{schema}sources:\n  s.one: {{file: one.txt, read: 2026-09-01}}\nother_sources:\n  s.one: {{url: 'https://example.invalid', read: 2026-09-01}}\nknown:\n  p.a: {{v: 1, from: s.one}}\njudgments:\n  d.base: {{rests_on: [p.a], verdict: ok, wrong_if: 'p.a > 9', reviewed: {{p.a: 1}}}}\n"
        ),
        json!({"source_quote":"q","date":"2026-09-02","target":"p.a","value":2}),
    );
    run(
        "duplicate explicit source homes are refused",
        &format!(
            "{schema}sources:\n  s.one: {{file: one.txt, read: 2026-09-01}}\nother_sources:\n  s.one: {{url: 'https://example.invalid', read: 2026-09-01}}\nknown:\n  p.a: {{v: 1}}\njudgments:\n  d.base: {{rests_on: [p.a], verdict: ok, wrong_if: 'p.a > 9', reviewed: {{p.a: 1}}}}\n"
        ),
        json!({"source_quote":"q","date":"2026-09-02","source":"s.one","at":"line 1","record_sha256":"@RECORD_SHA@","target":"p.a","value":2}),
    );
    for id in ["graph.one", "page.one"] {
        run(
            &format!("non-builtin prefixed source {id}"),
            &format!(
                "{schema}sources:\n  {id}: {{file: one.txt, read: 2026-09-01}}\nknown:\n  p.a: {{v: 1}}\njudgments:\n  d.base: {{rests_on: [p.a], verdict: ok, wrong_if: 'p.a > 9', reviewed: {{p.a: 1}}}}\n"
            ),
            json!({"source_quote":"q","date":"2026-09-02","source":id,"at":"line 1","record_sha256":"@RECORD_SHA@","target":"p.a","value":2}),
        );
    }
    run(
        "exact builtin identity cannot be a source",
        &format!(
            "{schema}sources:\n  graph.entries: {{file: one.txt, read: 2026-09-01}}\nknown:\n  p.a: {{v: 1}}\njudgments:\n  d.base: {{rests_on: [p.a], verdict: ok, wrong_if: 'p.a > 9', reviewed: {{p.a: 1}}}}\n"
        ),
        json!({"source_quote":"q","date":"2026-09-02","source":"graph.entries","at":"line 1","record_sha256":"@RECORD_SHA@","target":"p.a","value":2}),
    );
    run(
        "mixed batch requires at only for readings",
        &plain,
        json!({"source_quote":"q","date":"2026-09-02","source":"s.one","record_sha256":"@RECORD_SHA@","updates":[{"kind":"set","id":"p.a","value":2,"at":"line 9"},{"kind":"add","id":"p.c","body":{"rule":"p.a + 1"}},{"kind":"add","id":"d.c","body":{"rests_on":["p.a"],"verdict":"ok","wrong_if":"p.a > 9"}}]}),
    );
    run(
        "mixed batch refuses a reading without at",
        &plain,
        json!({"source_quote":"q","date":"2026-09-02","source":"s.one","record_sha256":"@RECORD_SHA@","updates":[{"kind":"add","id":"p.c","body":{"rule":"p.a + 1"}},{"kind":"set","id":"p.a","value":2}]}),
    );
    run(
        "shared at covers every batch reading",
        &plain,
        json!({"source_quote":"q","date":"2026-09-02","source":"s.one","at":"shared location","record_sha256":"@RECORD_SHA@","updates":[{"kind":"set","id":"p.a","value":2},{"kind":"add","id":"p.c","body":{"v":3}}]}),
    );
    for (name, value) in [
        ("map stored reading is refused", json!({"nested":1})),
        ("list stored reading is refused", json!([1, 2])),
    ] {
        run(
            name,
            &plain,
            json!({"source_quote":"q","date":"2026-09-02","record_sha256":"@RECORD_SHA@","updates":[{"kind":"add","id":"p.new","body":{"v":value}}]}),
        );
    }
    for builtin in ["page.unserved", "graph.entries"] {
        run(
            &format!("judgment on builtin {builtin} is refused"),
            &plain,
            json!({"source_quote":"q","date":"2026-09-02","record_sha256":"@RECORD_SHA@","updates":[{"kind":"add","id":"d.new","body":{"rests_on":[builtin],"verdict":"ok","wrong_if":format!("{builtin} > 0")}}]}),
        );
    }
    run(
        "replacement add does not choose the existing entry home",
        &plain,
        json!({"source_quote":"q","date":"2026-09-02","record_sha256":"@RECORD_SHA@","updates":[{"kind":"add","id":"p.a","body":{"v":5}}]}),
    );
    run(
        "replacement adds across entry homes still choose the source home",
        &format!(
            "{schema}sources:\n  s.one: {{file: one.txt, read: 2026-09-01}}\nknown:\n  p.a: {{v: 1}}\nother:\n  q.a: {{v: 5}}\njudgments:\n  d.base: {{rests_on: [p.a], verdict: ok, wrong_if: 'p.a > 9', reviewed: {{p.a: 1}}}}\n"
        ),
        json!({"source_quote":"q","date":"2026-09-02","record_sha256":"@RECORD_SHA@","updates":[{"kind":"add","id":"p.a","body":{"v":5}},{"kind":"add","id":"q.a","body":{"v":6}}]}),
    );
}
