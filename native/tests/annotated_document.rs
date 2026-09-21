use kpop_native::annotated_document as document;
use serde_json::{Value, json};
use std::fs;

const NOW: &str = "2026-09-10T00:00:00+00:00";
const LATER: &str = "2026-09-11T00:00:00+00:00";

fn page(body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Report</title><style>body{{color:#243342}}</style></head><body>{body}<script>document.documentElement.dataset.authored='yes';</script></body></html>"
    )
}
fn manifest() -> Value {
    json!({
        "version": 1,
        "title": "Reading challenge",
        "language": "en",
        "sources": {"counts": {"name":"Reading counts","path":"counts.json","format":"json"}},
        "claims": [
            {"id":"registered","label":"Registered readers","kind":"value","inputs":[{"source":"counts","pointer":"/registered"}]},
            {"id":"rate","label":"Completion rate","kind":"ratio","inputs":[{"source":"counts","pointer":"/finished"},{"source":"counts","pointer":"/registered"}],"format":{"scale":100,"decimals":0,"suffix":"%"}},
            {"id":"meaning","label":"Interpretation","kind":"inference","inputs":[{"source":"counts"}],"reason":"Participation does not establish satisfaction."}
        ]
    })
}
fn authored() -> String {
    page(
        "<h1>Reading — 📚</h1><p>Of <span data-kpopper-claim=\"registered\">80</span> registered readers, <span data-kpopper-claim=\"rate\">75%</span> finished.</p><p data-kpopper-claim=\"meaning\">The challenge may help <em>build a reading habit</em>.</p>",
    )
}

#[test]
fn build_refresh_round_trip_and_saved_decision() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("counts.json"),
        r#"{"registered":80,"finished":60,"private":"do not embed"}"#,
    )
    .unwrap();
    let initial = document::build(&authored(), &manifest(), root.path(), Some(NOW)).unwrap();
    assert_eq!(initial["checks"]["registered"]["status"], "match");
    assert_eq!(initial["checks"]["rate"]["status"], "match");
    assert_eq!(initial["checks"]["meaning"]["status"], "unchecked");
    assert_eq!(
        initial["coverage"],
        json!({"anchored_claims":3,"unmarked_blocks":2,"unmarked_values":0,"excerpts":["Reading — 📚","Of registered readers, finished."]})
    );
    assert!(
        !serde_json::to_string(&initial)
            .unwrap()
            .contains("do not embed")
    );

    let rendered = document::render(&initial).unwrap();
    let loaded = document::load_artifact(&rendered).unwrap();
    assert_eq!(
        document::summary(&loaded).unwrap()["checks"]["rate"]["status"],
        "match"
    );
    assert!(rendered.contains("sandbox=\"allow-scripts\""));
    assert!(rendered.contains("connect-src 'none'"));

    fs::write(
        root.path().join("counts.json"),
        r#"{"registered":100,"finished":60}"#,
    )
    .unwrap();
    let mut changed =
        document::refresh(&loaded, &manifest()["sources"], root.path(), Some(LATER)).unwrap();
    assert_eq!(changed["groups"].as_array().unwrap().len(), 1);
    let group = &changed["groups"][0];
    assert_eq!(group["id"], "g-d6493718ed213407");
    assert_eq!(group["status"], "ready");
    assert_eq!(
        group["contexts"][0]["before"],
        "Of 80 registered readers, 75% finished."
    );
    assert_eq!(
        group["contexts"][0]["after"],
        "Of 100 registered readers, 60% finished."
    );
    changed["groups"][0]["decision"] = json!("accepted");
    changed["groups"][0]["decided_at"] = json!(LATER);
    let copy = document::render(&changed).unwrap();
    let accepted = document::load_artifact(&copy).unwrap();
    assert_eq!(
        document::summary(&accepted).unwrap()["checks"]["rate"]["status"],
        "match"
    );
}

#[test]
fn quote_arithmetic_rounding_and_unavailable_evidence() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("values.json"),
        r#"{"a":0.1,"b":0.2,"x":1234.565}"#,
    )
    .unwrap();
    fs::write(
        root.path().join("notes.txt"),
        "Heading\nExact source sentence.\n",
    )
    .unwrap();
    let manifest = json!({"version":1,"sources":{
        "values":{"name":"Values","path":"values.json","format":"json"},
        "notes":{"name":"Notes","path":"notes.txt","format":"text"},
        "missing":{"name":"Approval","unavailable":"No written approval"}},"claims":[
        {"id":"sum","label":"Sum","kind":"sum","inputs":[{"source":"values","pointer":"/a"},{"source":"values","pointer":"/b"}],"format":{"decimals":2}},
        {"id":"money","label":"Money","kind":"value","inputs":[{"source":"values","pointer":"/x"}],"format":{"decimals":2,"thousands":".","decimal":",","prefix":"€ "}},
        {"id":"quote","label":"Quote","kind":"quote","inputs":[{"source":"notes","quote":"Exact source sentence."}]},
        {"id":"approval","label":"Approval","kind":"value","inputs":[{"source":"missing","pointer":"/approved"}]}
    ]});
    let html = page(
        "<p><span data-kpopper-claim=\"sum\">0.30</span></p><p><span data-kpopper-claim=\"money\">€ 1.234,57</span></p><blockquote data-kpopper-claim=\"quote\">Exact source sentence.</blockquote><p><span data-kpopper-claim=\"approval\">yes</span></p>",
    );
    let data = document::build(&html, &manifest, root.path(), Some(NOW)).unwrap();
    assert_eq!(
        data["checks"]["sum"]["status"], "match",
        "{}",
        data["checks"]["sum"]
    );
    assert_eq!(data["checks"]["money"]["status"], "match");
    assert_eq!(data["checks"]["quote"]["status"], "match");
    assert_eq!(data["checks"]["approval"]["status"], "unavailable");
    assert_eq!(data["groups"].as_array().unwrap().len(), 1);
    assert_eq!(data["groups"][0]["status"], "blocked");
}

#[test]
fn rejects_unsafe_or_ambiguous_inputs() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("counts.json"),
        r#"{"registered":80,"finished":60}"#,
    )
    .unwrap();
    let cases = [
        page(
            "<img src=\"https://example.invalid/x.png\"><p data-kpopper-claim=\"registered\">80</p><p data-kpopper-claim=\"rate\">75%</p><p data-kpopper-claim=\"meaning\">x</p>",
        ),
        page(
            "<style>@import 'remote.css'</style><p data-kpopper-claim=\"registered\">80</p><p data-kpopper-claim=\"rate\">75%</p><p data-kpopper-claim=\"meaning\">x</p>",
        ),
        page(
            "<table><span data-kpopper-claim=\"registered\">80</span></table><p data-kpopper-claim=\"rate\">75%</p><p data-kpopper-claim=\"meaning\">x</p>",
        ),
        page(
            "<p hidden data-kpopper-claim=\"registered\">80</p><p data-kpopper-claim=\"rate\">75%</p><p data-kpopper-claim=\"meaning\">x</p>",
        ),
    ];
    for html in cases {
        assert!(document::build(&html, &manifest(), root.path(), Some(NOW)).is_err());
    }
    let mut escaped = manifest();
    escaped["sources"]["counts"]["path"] = json!("../counts.json");
    assert!(document::build(&authored(), &escaped, root.path(), Some(NOW)).is_err());
    assert!(document::read_json(r#"{"a":1,"a":2}"#).is_err());
}

#[test]
fn json_sources_preserve_literal_private_number_objects_and_large_numbers() {
    let literal = document::read_json(
        r#"{"a":{"$serde_json::private::Number":"{\"b\":5}"},"large":123456789012345678901234567890}"#,
    )
    .unwrap();
    assert_eq!(
        literal["a"],
        json!({"$serde_json::private::Number":"{\"b\":5}"})
    );
    assert_eq!(
        literal["large"].as_number().unwrap().to_string(),
        "123456789012345678901234567890"
    );
    assert!(document::read_json(r#"{"outer":{"a":1,"a":2}}"#).is_err());
}

#[test]
fn json_pointer_array_indices_must_be_canonical() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("values.json"), r#"{"a":[10,20]}"#).unwrap();
    for pointer in ["/a/01", "/a/+1"] {
        let manifest = json!({"version":1,"sources":{"values":{"name":"Values","path":"values.json","format":"json"}},"claims":[
            {"id":"value","label":"Value","kind":"value","inputs":[{"source":"values","pointer":pointer}]}
        ]});
        let data = document::build(
            &page("<p><span data-kpopper-claim=\"value\">x</span></p>"),
            &manifest,
            root.path(),
            Some(NOW),
        )
        .unwrap();
        assert_eq!(data["checks"]["value"]["status"], "unavailable");
        assert_eq!(data["groups"][0]["status"], "blocked");
    }
}

#[test]
fn refuses_tampering_overwrite_and_protected_output() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("counts.json");
    fs::write(&source, r#"{"registered":80,"finished":60}"#).unwrap();
    let mut data = document::build(&authored(), &manifest(), root.path(), Some(NOW)).unwrap();
    data["checks"]["rate"]["status"] = json!("mismatch");
    assert!(document::render(&data).is_err());
    let data = document::build(&authored(), &manifest(), root.path(), Some(NOW)).unwrap();
    assert!(document::write_output(&data, &source, true, std::slice::from_ref(&source)).is_err());
    let out = root.path().join("report.html");
    document::write_output(&data, &out, false, &[source]).unwrap();
    assert!(document::write_output(&data, &out, false, &[]).is_err());
}

#[test]
fn ordinary_record_scalar_keeps_its_recorded_citation() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), "sources:\n  source.room:\n    name: Room schedule\n    read: '2026-09-09'\nreadings:\n  reading.capacity:\n    v: 24\n    from: source.room\n    at: row 4\n    of: '2026-09-08'\n").unwrap();
    let manifest = json!({"version":1,"sources":{"record":{"name":"Current record","path":"GROUNDING.yaml","format":"record"}},"claims":[{"id":"capacity","label":"Capacity","kind":"value","inputs":[{"source":"record","pointer":"/reading.capacity/v"}]}]});
    let data = document::build(
        &page("<p>Capacity: <span data-kpopper-claim=\"capacity\">24</span></p>"),
        &manifest,
        root.path(),
        Some(NOW),
    )
    .unwrap();
    assert_eq!(data["checks"]["capacity"]["status"], "match");
    let selection =
        &data["sources"]["record"]["selections"]["{\"pointer\":\"/reading.capacity/v\"}"];
    assert_eq!(
        selection["citation"],
        json!({"source":"source.room","name":"Room schedule","at":"row 4","date":"2026-09-08"})
    );
}

#[test]
fn core_record_embeds_only_selected_findings() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), "meta:\n  reasoning:\n    version: 2\n    profile: core/v1\n    requires: [arithmetic/v1]\nsources:\n  s.report:\n    name: Count report\n    file: report.txt\n    read: '2026-09-10'\nknown:\n  reading.registered:\n    name: Registered\n    v: 80\n    from: s.report\n    at: registered line\n    of: '2026-09-10'\n  reading.finished:\n    name: Finished\n    v: 60\n    from: s.report\n    at: finished line\n  reading.private:\n    v: DO NOT EMBED THIS\n    from: s.report\n").unwrap();
    let manifest = json!({"version":1,"sources":{"record":{"name":"Core record","path":"GROUNDING.yaml","format":"record","profile":"core/v1"}},"claims":[{"id":"registered","label":"Registered","kind":"value","inputs":[{"source":"record","pointer":"/reading.registered/v"}]},{"id":"rate","label":"Rate","kind":"ratio","inputs":[{"source":"record","pointer":"/reading.finished/v"},{"source":"record","pointer":"/reading.registered/v"}],"format":{"scale":100,"decimals":0,"suffix":"%"}}]});
    let data=document::build(&page("<p><span data-kpopper-claim=\"registered\">80</span> readers; <span data-kpopper-claim=\"rate\">75%</span> finished.</p>"),&manifest,root.path(),Some(NOW)).unwrap();
    assert_eq!(data["checks"]["registered"]["status"], "match");
    assert_eq!(data["sources"]["record"]["assessment"]["schema_version"], 3);
    assert_eq!(
        data["sources"]["record"]["assessment"]["embedded_selection"],
        json!(["reading.finished", "reading.registered"])
    );
    assert!(
        !serde_json::to_string(&data)
            .unwrap()
            .contains("DO NOT EMBED THIS")
    );
    document::validate_artifact(&data).unwrap();
    let mut tampered = data.clone();
    tampered["sources"]["record"]["assessment"]["findings_revision"] = json!("0".repeat(64));
    assert!(document::validate_artifact(&tampered).is_err());
}

#[test]
fn path_wrappers_are_bounded_and_protect_every_input() {
    let root = tempfile::tempdir().unwrap();
    let html = root.path().join("draft.html");
    let manifest_path = root.path().join("manifest.json");
    let source = root.path().join("counts.json");
    let output = root.path().join("report.html");
    fs::write(&html, authored()).unwrap();
    fs::write(&source, r#"{"registered":80,"finished":60}"#).unwrap();
    fs::write(&manifest_path, serde_json::to_vec(&manifest()).unwrap()).unwrap();
    let result = document::build_files(&html, &manifest_path, None, &output, false).unwrap();
    assert_eq!(result["checks"]["match"], 2);
    assert!(document::inspect_file(&output).is_ok());
    assert!(document::build_files(&html, &manifest_path, None, &source, true).is_err());
    let source_inputs = root.path().join("sources.json");
    fs::write(
        &source_inputs,
        serde_json::to_vec(&manifest()["sources"]).unwrap(),
    )
    .unwrap();
    assert!(document::refresh_file(&output, &source_inputs, None, &output, true).is_err());
}

#[test]
fn build_files_defaults_only_an_absent_sources_member_to_empty() {
    let root = tempfile::tempdir().unwrap();
    let html = root.path().join("draft.html");
    let manifest_path = root.path().join("manifest.json");
    let output = root.path().join("report.html");
    fs::write(
        &html,
        page("<p data-kpopper-claim=\"meaning\">A supported interpretation.</p>"),
    )
    .unwrap();
    let manifest = json!({"version":1,"claims":[
        {"id":"meaning","label":"Meaning","kind":"inference","inputs":[],"reason":"The prose is explicitly an interpretation."}
    ]});
    fs::write(&manifest_path, manifest.to_string()).unwrap();
    document::build_files(&html, &manifest_path, None, &output, false).unwrap();
    assert!(document::inspect_file(&output).is_ok());

    fs::remove_file(&output).unwrap();
    let mut invalid = manifest;
    invalid["sources"] = Value::Null;
    fs::write(&manifest_path, invalid.to_string()).unwrap();
    assert!(document::build_files(&html, &manifest_path, None, &output, false).is_err());
}

#[test]
fn applied_ingestion_event_binds_current_record_reading() {
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let id = "0123456789abcdef0123456789abcdef";
    let mut state = root.path().join("state");
    for dir in ["sources", "events", "envelopes", "receipts"] {
        fs::create_dir_all(state.join(dir)).unwrap()
    }
    state = state.canonicalize().unwrap();
    let source_file = state.join("sources").join(format!("{id}.txt"));
    let envelope = br#"{"source_quote":"capacity 24"}"#;
    fs::write(&source_file, b"capacity 24").unwrap();
    fs::write(state.join("envelopes").join(format!("{id}.json")), envelope).unwrap();
    let record = root.path().join("GROUNDING.yaml");
    fs::write(&record,format!("sources:\n  s.ingest_{id}:\n    name: Captured report\n    file: '{}'\n    recorded_for: update capacity\nreadings:\n  reading.capacity:\n    v: 24\n    from: s.ingest_{id}\n    at: entire captured report\n",source_file.display())).unwrap();
    fs::write(
        state.join("record.json"),
        json!({"record":record.canonicalize().unwrap().to_string_lossy()}).to_string(),
    )
    .unwrap();
    let sha = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let event = json!({"event_id":id,"source_file":source_file,"source_sha256":sha(b"capacity 24"),"envelope_sha256":sha(envelope),"state":"processing"});
    fs::write(
        state.join("events").join(format!("{id}.json")),
        event.to_string(),
    )
    .unwrap();
    let mut receipt = event;
    receipt["state"] = json!("applied");
    receipt["source"] = json!(format!("s.ingest_{id}"));
    receipt["target"] = json!("reading.capacity");
    receipt["value"] = json!(24);
    fs::write(
        state.join("receipts").join(format!("{id}.json")),
        receipt.to_string(),
    )
    .unwrap();
    kpop_native::recording_receipt::applied_event_binding(id, &record, Some(&state)).unwrap();
    let manifest = json!({"version":1,"sources":{"record":{"name":"Record","path":"GROUNDING.yaml","format":"record","event_id":id,"state_dir":"state"}},"claims":[{"id":"capacity","label":"Capacity","kind":"value","inputs":[{"source":"record","pointer":"/reading.capacity/v"}]}]});
    let data = document::build(
        &page("<p><span data-kpopper-claim=\"capacity\">24</span></p>"),
        &manifest,
        root.path(),
        Some(NOW),
    )
    .unwrap();
    assert_eq!(data["sources"]["record"]["event"]["event_id"], id);
    assert_eq!(
        data["sources"]["record"]["event"]["target"],
        "reading.capacity"
    );
}
