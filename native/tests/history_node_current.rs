use kpop_native::{
    history_node_codec as C,
    history_node_current::Original,
    history_yaml as Y,
    value::{Date, TypedValue as V},
};
use serde_json::json;
use std::collections::BTreeMap;
fn typed(v: serde_json::Value) -> V {
    V::from_json(&v).unwrap()
}
#[test]
fn first_change_retains_the_original_identity_without_copying_body_into_metadata() {
    let body = typed(json!({"v":"long authored passage ".repeat(1000)}));
    let context = BTreeMap::from([
        ("on".into(), V::Date(Date::new("2026-09-24").unwrap())),
        ("pin".into(), V::Text("a".repeat(64))),
    ]);
    let original = Original::create("p.a", "known", "birth", body.clone(), context).unwrap();
    let binding = original.encode().unwrap();
    assert!(Y::encode_document(&binding).unwrap().len() < 2000);
    let retained =
        Original::decode(&Y::decode_document(&Y::encode_document(&binding).unwrap()).unwrap())
            .unwrap();
    let doc = V::Map(BTreeMap::from([(
        "known".into(),
        V::Map(BTreeMap::from([("p.a".into(), body.clone())])),
    )]));
    let event = retained.from_document(&doc).unwrap();
    assert_eq!(event.id(), original.restore(body.clone()).unwrap().id());
    let first = event.reconstruct(None).unwrap();
    let old_id = first.id().to_owned();
    let next = C::Event::create(
        "p.a",
        "change",
        vec![first.id().into()],
        Some(&first),
        Some(original.state(typed(json!({"v":2})))),
    )
    .unwrap();
    let deleted = C::Event::create(
        "p.a",
        "delete",
        vec![next.id().into()],
        Some(&next.reconstruct(Some(&first)).unwrap()),
        None,
    )
    .unwrap();
    let bytes = [
        event.encode().unwrap(),
        next.encode().unwrap(),
        deleted.encode().unwrap(),
    ]
    .concat();
    let versions = C::decode_stream(&bytes, "p.a").unwrap();
    assert_eq!(versions[&old_id].state(), first.state());
    assert!(versions[deleted.id()].state().is_none());
    assert!(retained.restore(typed(json!({"v":"hand edit"}))).is_err());
}
#[test]
fn bindings_reject_subject_collection_and_context_rebinding() {
    let original = Original::create("p.a", "known", "birth", V::Null, BTreeMap::new()).unwrap();
    for (key, value) in [
        ("subject", V::Text("p.b".into())),
        ("collection", V::Text("judgments".into())),
        ("context", typed(json!(["map", [["extra", ["null"]]]]))),
    ] {
        let V::Map(mut fields) = original.encode().unwrap() else {
            panic!()
        };
        fields.insert(key.into(), value);
        let binding = Original::decode(&V::Map(fields)).unwrap();
        assert!(binding.restore(V::Null).is_err());
    }
}
