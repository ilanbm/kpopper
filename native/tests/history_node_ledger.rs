//! Public writer regression for compact subject-local action records.
use kpop_native::{
    history_authoring::Options, history_node_capture::Capture, history_node_codec as C,
    history_node_publication as P, history_node_writer as W, history_paths::Scheme,
    value::TypedValue as V,
};
use serde_json::json;
use std::{collections::BTreeMap, fs, path::Path};
fn value(v: serde_json::Value) -> V {
    V::from_json(&v).unwrap()
}
fn setup() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".kpopper")).unwrap();
    fs::write(root.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), "meta:\n  purpose: Fixture\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown: {}\n").unwrap();
    root
}
fn write(root: &Path, operation: &str, day: u32, action: serde_json::Value) -> P::Prepared {
    let options = Options {
        operation: operation.into(),
        recorded_at: format!("2000-01-{day:02}T12:00:00+00:00"),
        recording_day: format!("2000-01-{day:02}"),
        by: value(json!("writer")),
        strict: false,
        paths: Scheme::Hashed,
        receipt_version: None,
    };
    let p = W::prepare(root, &value(action), &options, None).unwrap();
    W::publish(root, &p, None, |_| Ok(())).unwrap();
    p
}
fn map(value: &V) -> &BTreeMap<String, V> {
    let V::Map(map) = value else { panic!() };
    map
}
fn stream(root: &Path) -> Vec<u8> {
    fs::read(
        root.join(".kpopper/history")
            .join(C::subject_path("p.a").unwrap()),
    )
    .unwrap()
}
#[test]
fn small_set_appends_one_frame_preserves_ids_and_exports_without_receipt_worlds() {
    let root = setup();
    write(
        root.path(),
        "seed",
        1,
        json!({"kind":"add","id":"p.a","body":{"v":1}}),
    );
    let before = Capture::read(root.path()).unwrap();
    let original_id = map(&map(before.state())["subjects"])["p.a"].clone();
    let original_id = map(&original_id)["head"].clone();
    write(
        root.path(),
        "set-1",
        2,
        json!({"kind":"set","id":"p.a","value":2}),
    );
    let prior = stream(root.path());
    let prepared = write(
        root.path(),
        "set-2",
        3,
        json!({"kind":"set","id":"p.a","value":3}),
    );
    let after = stream(root.path());
    assert!(after.starts_with(&prior), "append-only node history");
    let added = &after[prior.len()..];
    assert_eq!(
        added.split_inclusive(|b| *b == b'\n').count(),
        1,
        "one physical action record, not claim + accept + evidence frames"
    );
    let frame: serde_json::Value = serde_json::from_slice(added).unwrap();
    assert_eq!(frame["operation"], "set-2");
    let m: serde_json::Value = serde_json::from_slice(
        &fs::read(root.path().join(".kpopper/history-commits/set-2.json")).unwrap(),
    )
    .unwrap();
    let context = m["context"].to_string();
    assert!(
        !context.contains("selection_from_nodes"),
        "fresh manifest must not retain a packed receipt side"
    );
    assert!(
        !context.contains("node-receipt-side"),
        "fresh manifest must not retain a packed receipt side"
    );
    W::publish(root.path(), &prepared, None, |_| {
        panic!("exact retry wrote bytes")
    })
    .unwrap();
    let copy = P::export(root.path()).unwrap().reconstruct().unwrap();
    let captured = Capture::read(copy.path()).unwrap();
    let V::Text(original_id) = original_id else {
        panic!()
    };
    assert_eq!(
        map(&captured.object("p.a", &original_id).unwrap())["body"],
        value(json!({"v":1}))
    );
    assert_eq!(
        captured.object_count(),
        5,
        "claim and acceptance still exist semantically"
    );
}

#[test]
fn core_reading_update_does_not_append_receipt_events_to_fifty_dependents() {
    use kpop_native::{
        history_yaml as Y,
        reasoning_runtime::{OperationalBounds, Runtime},
    };
    use std::collections::BTreeSet;
    let root = setup();
    fs::write(root.path().join("GROUNDING.yaml"),Y::encode_document(&value(json!({
        "meta":{"purpose":"Fixture","reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},
        "schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"readings":{},"judgments":{}
    }))).unwrap()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!(
            "{}.kpopper-runtime",
            kpop_native::reasoning_runtime::target_name().unwrap()
        ));
    let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
    let mut options = Options {
        operation: "core-seed".into(),
        recorded_at: "2000-01-01T12:00:00+00:00".into(),
        recording_day: "2000-01-01".into(),
        by: value(json!("writer")),
        strict: false,
        paths: Scheme::Hashed,
        receipt_version: None,
    };
    let mut actions = vec![json!({"kind":"add","id":"p.a","body":{"v":1},"into":"readings"})];
    for index in 0..50 {
        actions.push(json!({"kind":"add","id":format!("d.{index:02}"),"body":{"verdict":"ready","rests_on":["p.a"],"wrong_if":{"expr":"p.a > 3"}},"into":"judgments"}));
    }
    let p = W::prepare(
        root.path(),
        &value(json!({"kind":"batch","actions":actions})),
        &options,
        Some(&runtime),
    )
    .unwrap();
    W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    let before = Capture::read(root.path()).unwrap();
    let prior = map(&map(before.state())["subjects"]).clone();
    options.operation = "core-set".into();
    options.recorded_at = "2000-01-02T12:00:00+00:00".into();
    options.recording_day = "2000-01-02".into();
    let p = W::prepare(
        root.path(),
        &value(json!({"kind":"set","id":"p.a","value":2})),
        &options,
        Some(&runtime),
    )
    .unwrap();
    W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(root.path().join(".kpopper/history-commits/core-set.json")).unwrap(),
    )
    .unwrap();
    let touched = manifest["touched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|touch| touch["subject"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        touched,
        BTreeSet::from(["p.a"]),
        "changed assessments must not create stored receipt events for unchanged dependents"
    );
    let after = Capture::read(root.path()).unwrap();
    let current = map(&map(after.state())["subjects"]);
    for index in 0..50 {
        let id = format!("d.{index:02}");
        let head = &map(&prior[&id])["head"];
        assert_eq!(head, &map(&current[&id])["head"]);
        let V::Text(head) = head else { panic!() };
        assert_eq!(
            before.object(&id, head).unwrap(),
            after.object(&id, head).unwrap()
        );
    }
}

#[test]
fn capture_checks_identity_of_a_closed_acceptance_act() {
    use kpop_native::{
        history_node_observation::ObservationNode, history_node_semantics::History,
        identity::typed_object_identity,
    };
    use std::collections::BTreeSet;
    fn identify(object: &mut V) -> String {
        let id = typed_object_identity(object).unwrap();
        let V::Map(m) = object else { panic!() };
        m.insert("id".into(), V::Text(id.clone()));
        id
    }
    let mut claim = value(
        json!({"schema_version":2,"id_scheme":"typed-history/v2","subject":"p.a","kind":"reading","by":"writer","on":"2000-01-01","op":"seed","body":{"v":1},"saw":[],"pins":{},"authored":{"collection":"known","profile":"ordinary-reader/v1","fields":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}}}),
    );
    let claim_id = identify(&mut claim);
    let claim_obs = ObservationNode::root(&claim_id, &BTreeSet::new()).unwrap();
    let mut act = value(
        json!({"schema_version":2,"id_scheme":"typed-history/v2","subject":"p.a","kind":"act","by":"writer","on":"2000-01-01","op":"accept","body":{"act":"accept","of":claim_id,"over":[],"because":"original"},"saw":[claim_id]}),
    );
    let act_id = identify(&mut act);
    let act_obs = ObservationNode::root(&act_id, &BTreeSet::from([claim_id])).unwrap();
    let V::Map(claim) = &mut claim else { panic!() };
    claim.remove("saw");
    let V::Map(act_map) = &mut act else { panic!() };
    act_map.remove("saw");
    assert!(
        History::from_objects(vec![
            (V::Map(claim.clone()), claim_obs.clone()),
            (V::Map(act_map.clone()), act_obs.clone()),
        ])
        .is_ok(),
        "control fixture is valid before tampering"
    );
    // The acceptance is closed, so a reader may never explicitly expand this act.
    // Its identity must nevertheless be validated during complete capture.
    let V::Map(body) = act_map.get_mut("body").unwrap() else {
        panic!()
    };
    body.insert(
        "because".into(),
        V::Text("tampered without changing semantic id".into()),
    );
    let result = History::from_objects(vec![(V::Map(claim.clone()), claim_obs), (act, act_obs)]);
    assert!(
        result.is_err(),
        "a valid observation id string is not proof of the object's identity"
    );
}
