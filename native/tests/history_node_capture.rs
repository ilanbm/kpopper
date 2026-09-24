use kpop_native::{
    history_node_capture::{self as N, Capture},
    history_node_codec as C,
    history_node_observation::ObservationNode,
    history_node_publication as P, history_yaml as Y,
    identity::typed_object_identity,
    value::TypedValue as V,
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

fn text(v: &V) -> &str {
    let V::Text(s) = v else { panic!() };
    s
}
fn map(v: &V) -> &BTreeMap<String, V> {
    let V::Map(m) = v else { panic!() };
    m
}
fn map_mut(v: &mut V) -> &mut BTreeMap<String, V> {
    let V::Map(m) = v else { panic!() };
    m
}
fn claim(subject: &str, operation: &str, value: i64, saw: Vec<String>) -> V {
    let mut object = V::from_json(&json!({
        "id_scheme":"typed-history/v2", "schema_version":2, "subject":subject,
        "kind":"reading", "by":"writer", "on":"2026-09-24", "op":operation,
        "body":{"v":value}, "saw":saw, "pins":{},
        "authored":{"collection":"known", "profile":"ordinary-reader/v1",
            "fields":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}}
    }))
    .unwrap();
    let id = typed_object_identity(&object).unwrap();
    map_mut(&mut object).insert("id".into(), V::Text(id));
    object
}
fn observation(object: &V) -> ObservationNode {
    let V::List(saw) = &map(object)["saw"] else {
        panic!()
    };
    ObservationNode::root(
        text(&map(object)["id"]),
        &saw.iter().map(|v| text(v).into()).collect(),
    )
    .unwrap()
}
fn setup() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".kpopper")).unwrap();
    fs::write(root.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
    root
}
fn view(objects: &[V]) -> V {
    let mut document = V::from_json(&json!({"meta":{"purpose":"Fixture"},
        "schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}, "known":{}}))
    .unwrap();
    for object in objects {
        map_mut(map_mut(&mut document).get_mut("known").unwrap()).insert(
            text(&map(object)["subject"]).into(),
            map(object)["body"].clone(),
        );
    }
    document
}
fn publish(root: &Path, op: &str, doc: &V, frames: BTreeMap<String, Vec<u8>>) {
    let prepared = P::prepare(
        root,
        op,
        P::bind_view(&Y::encode_document(doc).unwrap(), op).unwrap(),
        frames,
    )
    .unwrap();
    P::publish(root, &prepared, |_| Ok(()), |_| Ok(())).unwrap();
}
fn lazy(root: &Path, object: &V) -> C::Event {
    let original = N::original(object, &observation(object)).unwrap();
    let mut doc = view(std::slice::from_ref(object));
    let event = original.from_document(&doc).unwrap();
    let subject = text(&map(object)["subject"]);
    map_mut(map_mut(&mut doc).get_mut("meta").unwrap()).insert(
        "node_history".into(),
        V::Map(BTreeMap::from([
            ("version".into(), V::from_json(&json!(1)).unwrap()),
            (
                "originals".into(),
                V::Map(BTreeMap::from([(
                    subject.into(),
                    original.encode().unwrap(),
                )])),
            ),
        ])),
    );
    publish(root, text(&map(object)["op"]), &doc, BTreeMap::new());
    event
}

#[test]
fn lazy_current_resolves_original_semantic_identity_without_history_file() {
    let root = setup();
    let a = claim("p.a", "create", 1, vec![]);
    let event = lazy(root.path(), &a);
    let capture = Capture::read(root.path()).unwrap();
    let id = text(&map(&a)["id"]);
    assert_eq!(capture.object("p.a", id).unwrap(), a);
    assert!(capture.object("p.a", event.id()).is_err());
    assert!(
        !root
            .path()
            .join(".kpopper/history")
            .join(C::subject_path("p.a").unwrap())
            .exists()
    );
}

#[test]
fn first_change_retains_original_pin_and_reduces_claim_and_acceptance() {
    let root = setup();
    let a = claim("p.a", "create", 1, vec![]);
    let initial = lazy(root.path(), &a);
    let base = initial.reconstruct(None).unwrap();
    let old_id = text(&map(&a)["id"]).to_owned();
    let next = claim("p.a", "change", 2, vec![old_id.clone()]);
    let next_id = text(&map(&next)["id"]).to_owned();
    let next_event = C::Event::create(
        "p.a",
        "change",
        vec![base.id().into()],
        Some(&base),
        Some(N::payload(&next, &observation(&next)).unwrap()),
    )
    .unwrap();
    let next_version = next_event.reconstruct(Some(&base)).unwrap();
    let mut act = V::from_json(&json!({"id_scheme":"typed-history/v2", "schema_version":2,
        "subject":"p.a", "kind":"act", "by":"writer", "on":"2026-09-24", "op":"change",
        "body":{"act":"accept","of":next_id,"over":[old_id],"because":"explicit set"},
        "saw":BTreeSet::from([old_id.clone(),next_id.clone()])}))
    .unwrap();
    let id = typed_object_identity(&act).unwrap();
    map_mut(&mut act).insert("id".into(), V::Text(id));
    let act_event = C::Event::create(
        "p.a",
        "change",
        vec![next_version.id().into()],
        Some(&next_version),
        Some(N::payload(&act, &observation(&act)).unwrap()),
    )
    .unwrap();
    publish(
        root.path(),
        "change",
        &view(std::slice::from_ref(&next)),
        BTreeMap::from([(
            "p.a".into(),
            [
                initial.encode().unwrap(),
                next_event.encode().unwrap(),
                act_event.encode().unwrap(),
            ]
            .concat(),
        )]),
    );
    let capture = Capture::read(root.path()).unwrap();
    assert_eq!(capture.object_count(), 3);
    assert_eq!(capture.object("p.a", &old_id).unwrap(), a);
    assert_eq!(capture.object("p.a", &next_id).unwrap(), next);
    assert_eq!(
        map(&map(capture.document())["known"])["p.a"],
        map(&next)["body"]
    );
    assert!(capture.object("other", &old_id).is_err());
    let bundle = P::export(root.path()).unwrap();
    let copy = P::Bundle::decode(&bundle.encode().unwrap())
        .unwrap()
        .reconstruct()
        .unwrap();
    drop(root);
    let restored = Capture::read(copy.path()).unwrap();
    assert_eq!(restored.object("p.a", &old_id).unwrap(), a);
    assert_eq!(restored.object("p.a", &next_id).unwrap(), next);
}

#[test]
fn storage_valid_hashes_do_not_admit_forged_semantics_or_a_false_current_body() {
    for wrong_body in [false, true] {
        let root = setup();
        let object = claim("p.a", "create", 1, vec![]);
        let mut state = N::payload(&object, &observation(&object)).unwrap();
        if !wrong_body {
            let context = map_mut(map_mut(&mut state).get_mut("context").unwrap());
            map_mut(context.get_mut("header").unwrap())
                .insert("by".into(), V::Text("forged".into()));
        }
        let event = C::Event::create("p.a", "create", vec![], None, Some(state)).unwrap();
        let shown = if wrong_body {
            claim("p.a", "create", 9, vec![])
        } else {
            object
        };
        publish(
            root.path(),
            "create",
            &view(&[shown]),
            BTreeMap::from([("p.a".into(), event.encode().unwrap())]),
        );
        assert!(P::capture(root.path()).is_ok());
        assert!(Capture::read(root.path()).is_err());
    }
}

#[test]
fn untracked_current_entries_and_misbound_publication_operation_refuse() {
    let root = setup();
    let a = claim("p.a", "claimed-op", 1, vec![]);
    let original = N::original(&a, &observation(&a)).unwrap();
    let doc = view(&[a]);
    let event = original.from_document(&doc).unwrap();
    publish(
        root.path(),
        "different-op",
        &doc,
        BTreeMap::from([("p.a".into(), event.encode().unwrap())]),
    );
    assert!(Capture::read(root.path()).is_err());

    let root = setup();
    let doc = view(&[claim("untracked", "create", 1, vec![])]);
    fs::write(
        root.path().join("GROUNDING.yaml"),
        Y::encode_document(&doc).unwrap(),
    )
    .unwrap();
    assert!(Capture::read(root.path()).is_err());
}
