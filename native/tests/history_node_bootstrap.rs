use kpop_native::{
    history_node_archive::Archive, history_node_bootstrap::Plan, history_node_capture::Capture,
    history_node_publication as P, history_yaml as Y, value::TypedValue as V,
};
use std::{fs, path::Path};

fn fixture(root: &Path, raw: &[u8]) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("GROUNDING.yaml"), raw).unwrap();
}
fn text<'a>(value: &'a V) -> &'a str {
    let V::Text(text) = value else {
        panic!("expected text")
    };
    text
}
fn map(value: &V) -> &std::collections::BTreeMap<String, V> {
    let V::Map(map) = value else {
        panic!("expected map")
    };
    map
}

#[test]
fn public_capture_reads_compact_semantic_objects_and_refuses_changed_view() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    fixture(&source, b"known: {p.a: {v: 1}}\n");
    let destination = tmp.path().join("compact");
    Plan::prepare(&source.join("GROUNDING.yaml"))
        .unwrap()
        .publish(&destination)
        .unwrap();
    let before = fs::read(destination.join("GROUNDING.yaml")).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(tmp.path())
            .args(["--workspace", destination.to_str().unwrap(), "history-capture", "GROUNDING.yaml", "--json"])
            .output()
            .unwrap()
    };
    let result = run();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["status"], "captured");
    assert_eq!(result["semantic_assessment"], "not_performed");
    let evidence = V::from_tagged(&result["evidence"]).unwrap();
    assert_eq!(result["digest"], evidence.digest().unwrap());
    let evidence = map(&evidence);
    assert_eq!(text(&evidence["format"]), "node-history-capture/v1");
    assert_eq!(text(&evidence["profile"]), "node-history/v1");
    let subject = &map(&map(&evidence["state"])["subjects"])["p.a"];
    let head = text(&map(subject)["head"]);
    let object = &map(&evidence["objects"])[head];
    assert_eq!(text(&map(object)["subject"]), "p.a");
    assert_eq!(map(object)["body"], map(&map(&evidence["document"])["known"])["p.a"]);
    assert_eq!(fs::read(destination.join("GROUNDING.yaml")).unwrap(), before);
    fs::write(destination.join("GROUNDING.yaml"), before.iter().copied().chain(b"\nknown: {}\n".iter().copied()).collect::<Vec<_>>()).unwrap();
    assert!(!run().status.success());
}

#[test]
fn ordinary_copy_retains_exact_source_and_unknown_historical_pins() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let raw = b"meta: {purpose: example}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {p.a: {value: 4}}\njudgments: {p.b: {rests_on: [p.a, absent], wrong_if: 'counterexample', seen: {p.a: old-observation}}}\n";
    fixture(&source, raw);
    let config =
        b"{\"version\":1,\"mode\":\"simple\",\"record\":\"GROUNDING.yaml\",\"generation\":1}";
    fs::create_dir_all(source.join(".kpopper")).unwrap();
    fs::write(source.join(".kpopper/project.json"), config).unwrap();
    let plan = Plan::prepare(&source.join("GROUNDING.yaml")).unwrap();
    let archive_raw = plan
        .files()
        .iter()
        .find(|(p, _)| p.ends_with(".zip"))
        .unwrap()
        .1;
    let archive = Archive::decode(archive_raw).unwrap();
    assert_eq!(archive.files()["GROUNDING.yaml"].as_slice(), raw);
    assert_eq!(archive.files()[".kpopper/project.json"].as_slice(), config);
    let destination = tmp.path().join("copy");
    plan.publish(&destination).unwrap();
    assert!(!destination.join(".kpopper/project.json").exists());
    assert_eq!(fs::read(source.join("GROUNDING.yaml")).unwrap(), raw);
    let capture = Capture::read(&destination).unwrap();
    assert_eq!(capture.object_count(), 2);
    let mapping_raw = plan
        .files()
        .iter()
        .find(|(p, _)| p.ends_with(".json") && p.starts_with("evidence/bootstrap/"))
        .unwrap()
        .1;
    let mapping = V::from_tagged(&serde_json::from_slice(mapping_raw).unwrap()).unwrap();
    let V::List(entries) = &map(&mapping)["entries"] else {
        panic!()
    };
    let id = entries
        .iter()
        .find(|v| map(&map(v)["source"])["id"] == V::Text("p.b".into()))
        .map(|v| text(&map(v)["semantic_id"]))
        .unwrap();
    let judgment = capture.object("p.b", id).unwrap();
    assert_eq!(
        map(&map(&judgment)["pin_gaps"])["p.a"],
        V::Text("not_recorded".into())
    );
    assert_eq!(
        map(&map(&judgment)["pin_gaps"])["absent"],
        V::Text("unavailable".into())
    );
    assert!(map(&map(&judgment)["pins"]).is_empty());
    let locator = &map(&map(&judgment)["authored"])["locator"];
    let original = &map(locator)["original"];
    for field in [
        "writer",
        "operation",
        "recorded_at",
        "source_clock",
        "condition_profile",
    ] {
        assert_eq!(map(original)[field], V::Null);
    }
    let view = Y::decode_document(&fs::read(destination.join("GROUNDING.yaml")).unwrap()).unwrap();
    assert_eq!(
        map(&map(&view)["known"])["p.a"],
        map(&map(&Y::decode_document(raw).unwrap())["known"])["p.a"]
    );
    assert!(P::export(&destination).is_ok());
}

#[test]
fn existing_destination_and_existing_history_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    fixture(&source, b"known: {p.a: {value: 1}}\n");
    let plan = Plan::prepare(&source.join("GROUNDING.yaml")).unwrap();
    let destination = tmp.path().join("copy");
    fs::create_dir(&destination).unwrap();
    assert!(plan.publish(&destination).is_err());
    assert!(fs::read_dir(&destination).unwrap().next().is_none());
    fs::create_dir(source.join(".kpopper")).unwrap();
    fs::write(source.join(".kpopper/history.yaml"), b"existing").unwrap();
    assert!(Plan::prepare(&source.join("GROUNDING.yaml")).is_err());
}

#[test]
fn core_profile_body_is_copied_without_value_conversion() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let raw = b"meta: {reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {p.x: {number: !!int '9007199254740993', date: !!timestamp '2026-09-24'}}\n";
    fixture(&source, raw);
    let plan = Plan::prepare(&source.join("GROUNDING.yaml")).unwrap();
    let destination = tmp.path().join("copy");
    plan.publish(&destination).unwrap();
    let original = Y::decode_document(raw).unwrap();
    let copied =
        Y::decode_document(&fs::read(destination.join("GROUNDING.yaml")).unwrap()).unwrap();
    assert_eq!(map(&copied)["known"], map(&original)["known"]);
    assert_eq!(
        text(&map(&map(&map(&copied)["meta"])["reasoning"])["profile"]),
        "core/v1"
    );
}
