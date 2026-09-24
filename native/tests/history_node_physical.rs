use kpop_native::{
    history_authoring::Options, history_node_bootstrap::Plan, history_node_capture::Capture,
    history_node_publication as P, history_node_writer as W, history_paths::Scheme,
    value::TypedValue as V,
};
use serde_json::json;
use std::{collections::BTreeMap, fs};
fn v(j: serde_json::Value) -> V {
    V::from_json(&j).unwrap()
}
fn m(v: &V) -> &BTreeMap<String, V> {
    let V::Map(m) = v else { panic!() };
    m
}
#[test]
fn physical_original_becomes_named_proposal_without_being_accepted_or_lost() {
    let source = tempfile::tempdir().unwrap();
    fs::create_dir_all(source.path().join(".kpopper/hypotheses")).unwrap();
    fs::write(source.path().join("GROUNDING.yaml"),"meta: {purpose: Fixture}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {p.a: {v: 1}}\n").unwrap();
    let raw=b"hypothesis: {claim: Test the alternative, born: '2026-09-24'}\r\nknown: {p.a: {v: 2}}\r\n";
    fs::write(
        source.path().join(".kpopper/hypotheses/alternative.yaml"),
        raw,
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("copy");
    Plan::prepare(&source.path().join("GROUNDING.yaml"))
        .unwrap()
        .publish(&out)
        .unwrap();
    assert_eq!(
        fs::read(source.path().join(".kpopper/hypotheses/alternative.yaml")).unwrap(),
        raw
    );
    assert_eq!(
        fs::read(out.join(".kpopper/hypotheses/alternative.yaml")).unwrap(),
        raw
    );
    let c = Capture::read(&out).unwrap();
    assert_eq!(
        m(&m(c.document())["known"])["p.a"].to_json().unwrap()["v"],
        1
    );
    let projection = kpop_native::history_node_projection::capture(&c).unwrap();
    let (groups, _) =
        kpop_native::history_hypotheses::layers(projection.projection(), c.document()).unwrap();
    assert!(m(&groups).contains_key("alternative"));
    let copy = P::export(&out).unwrap().reconstruct().unwrap();
    drop(source);
    drop(root);
    let options = Options {
        operation: "fold".into(),
        recorded_at: "2026-09-24T12:00:00Z".into(),
        recording_day: "2026-09-24".into(),
        by: v(json!("fixture")),
        strict: true,
        paths: Scheme::Hashed,
        receipt_version: None,
    };
    let action = v(
        json!({"kind":"hypothesis","action":{"kind":"fold","names":["alternative"],"because":"tested","take":[],"drops":{}}}),
    );
    let before = P::capture_snapshot(copy.path()).unwrap().revision;
    assert_eq!(
        W::prepare(copy.path(), &action, &options, None)
            .unwrap_err()
            .0,
        "ordinary_history_authoring_unsupported"
    );
    let c = Capture::read(copy.path()).unwrap();
    assert_eq!(
        m(&m(c.document())["known"])["p.a"].to_json().unwrap()["v"],
        1
    );
    let projection = kpop_native::history_node_projection::capture(&c).unwrap();
    let (groups, _) =
        kpop_native::history_hypotheses::layers(projection.projection(), c.document()).unwrap();
    assert!(m(&groups).contains_key("alternative"));
    assert_eq!(P::capture_snapshot(copy.path()).unwrap().revision, before);
    assert_eq!(
        fs::read(copy.path().join(".kpopper/hypotheses/alternative.yaml")).unwrap(),
        raw
    );
}
