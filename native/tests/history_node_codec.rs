use kpop_native::{
    history_node_codec as codec,
    value::{Date, FiniteFloat, Integer, TypedValue as V},
};
use serde_json::json;
use std::collections::BTreeMap;

fn typed(value: serde_json::Value) -> V {
    V::from_json(&value).unwrap()
}
fn root(state: Option<V>) -> codec::Event {
    codec::Event::create("p.a", "create", vec![], None, state).unwrap()
}
fn child(base: &codec::Version, state: Option<V>, operation: &str) -> codec::Event {
    codec::Event::create("p.a", operation, vec![base.id().into()], Some(base), state).unwrap()
}
#[test]
fn typed_values_absence_and_field_roles_survive_deltas() {
    let original = V::Map(BTreeMap::from([
        ("null".into(), V::Null),
        ("remove".into(), typed(json!(7))),
        ("date".into(), V::Date(Date::new("2026-09-24").unwrap())),
        ("float".into(), V::Float(FiniteFloat::new(-0.0).unwrap())),
        (
            "large".into(),
            V::Integer(Integer::new("123456789012345678901234567890").unwrap()),
        ),
        ("fields".into(), typed(json!({"v":"claim"}))),
        ("long".into(), V::Text("retain ".repeat(150))),
    ]));
    let event = root(Some(original.clone()));
    let version = event.reconstruct(None).unwrap();
    let V::Map(mut changed) = original else {
        panic!()
    };
    changed.remove("remove");
    changed.insert("float".into(), V::Float(FiniteFloat::new(0.0).unwrap()));
    changed.insert("date".into(), V::Text("2026-09-24".into()));
    changed.insert("fields".into(), typed(json!({"v":"verdict"})));
    changed.insert("new_null".into(), V::Null);
    let changed = V::Map(changed);
    let next = child(&version, Some(changed.clone()), "change");
    assert!(next.encode().unwrap().len() < event.encode().unwrap().len());
    let bytes = [event.encode().unwrap(), next.encode().unwrap()].concat();
    let versions = codec::decode_stream(&bytes, "p.a").unwrap();
    assert_eq!(versions[next.id()].state(), Some(&changed));
    let deleted = child(&versions[next.id()], None, "delete");
    let deleted = deleted.reconstruct(Some(&versions[next.id()])).unwrap();
    assert!(deleted.state().is_none());
    let null = child(&deleted, Some(V::Null), "readd");
    assert_eq!(
        null.reconstruct(Some(&deleted)).unwrap().state(),
        Some(&V::Null)
    );
}
#[test]
fn sparse_saw_is_exact_and_other_lists_keep_order() {
    let ids = (0..80).map(|v| format!("{v:064x}")).collect::<Vec<_>>();
    let state = typed(json!({"saw":ids,"ordered":["b","a"],"body":{"saw":["b","a"]}}));
    let initial = root(Some(state));
    let version = initial.reconstruct(None).unwrap();
    let next_ids = (0..80)
        .filter(|n| *n != 7)
        .chain([81])
        .map(|v| format!("{v:064x}"))
        .collect::<Vec<_>>();
    let state = typed(json!({"saw":next_ids,"ordered":["a","b"],"body":{"saw":["a","b"]}}));
    let next = child(&version, Some(state.clone()), "observe");
    let raw = next.encode().unwrap();
    assert!(raw.len() < 2000, "{}", raw.len());
    assert!(String::from_utf8_lossy(&raw).contains("\"Saw\""));
    assert_eq!(
        next.reconstruct(Some(&version)).unwrap().state(),
        Some(&state)
    );
}
#[test]
fn fork_merge_pins_and_acts_do_not_use_file_order() {
    let initial = root(Some(typed(json!({"v":1}))));
    let first = initial.reconstruct(None).unwrap();
    let left = child(&first, Some(typed(json!({"v":2}))), "left");
    let right = child(&first, Some(typed(json!({"v":3}))), "right");
    let left_v = left.reconstruct(Some(&first)).unwrap();
    let merge = codec::Event::create_merge(
        "p.a",
        "merge",
        vec![left.id().into(), right.id().into()],
        &left_v,
        Some(typed(json!({"v":4}))),
    )
    .unwrap();
    assert!(
        codec::Event::create(
            "p.a",
            "unlabelled",
            vec![left.id().into(), right.id().into()],
            Some(&left_v),
            Some(V::Null)
        )
        .is_err()
    );
    let raw = [&merge, &right, &initial, &left]
        .into_iter()
        .flat_map(|e| e.encode().unwrap())
        .collect::<Vec<_>>();
    let versions = codec::decode_stream(&raw, "p.a").unwrap();
    assert_eq!(versions[initial.id()].state(), first.state());
    assert_eq!(versions[merge.id()].state(), Some(&typed(json!({"v":4}))));
    let review = child(
        &versions[merge.id()],
        versions[merge.id()].state().cloned(),
        "review",
    );
    assert_ne!(review.id(), merge.id());
    assert_eq!(
        review
            .reconstruct(Some(&versions[merge.id()]))
            .unwrap()
            .state(),
        versions[merge.id()].state()
    );
    assert!(review.reconstruct(Some(&first)).is_err());
}
#[test]
fn corruption_duplicate_truncation_wrong_subject_and_missing_parent_fail_closed() {
    let event = root(Some(typed(json!({"v":1}))));
    let raw = event.encode().unwrap();
    assert!(codec::decode_stream(&raw[..raw.len() - 1], "p.a").is_err());
    assert!(codec::decode_stream(&raw, "p.b").is_err());
    assert!(codec::decode_stream(&[raw.clone(), raw.clone()].concat(), "p.a").is_err());
    let mut bad = raw.clone();
    bad[20] ^= 1;
    assert!(codec::decode_stream(&bad, "p.a").is_err());
    let mut parsed: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    parsed["result"] = json!("0".repeat(64));
    let mut bad = serde_json::to_vec(&parsed).unwrap();
    bad.push(b'\n');
    assert!(codec::decode_stream(&bad, "p.a").is_err());
    let version = event.reconstruct(None).unwrap();
    let next = child(&version, Some(V::Null), "next");
    assert!(codec::decode_stream(&next.encode().unwrap(), "p.a").is_err());
    assert!(
        codec::Event::create(
            "p.a",
            "large",
            vec![],
            None,
            Some(V::Text("x".repeat(codec::MAX_FRAME_BYTES)))
        )
        .is_err()
    );
    assert!(
        codec::Event::create(
            "p.a",
            "duplicate",
            vec![version.id().into(), version.id().into()],
            Some(&version),
            None
        )
        .is_err()
    );
}
#[test]
fn exact_retry_and_subject_encoding_are_stable() {
    let event = root(Some(V::Null));
    let retry = root(Some(V::Null));
    assert_eq!(event.encode().unwrap(), retry.encode().unwrap());
    let path = codec::subject_path("../../שלום/é").unwrap();
    assert!(!path.contains('/'));
    assert_ne!(path, codec::subject_path("../../שלום/e\u{301}").unwrap());
}

#[test]
fn legacy_closure_objects_pins_and_reduction_survive_typed_retention() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-objects.json")).unwrap();
    let mut checked = 0;
    for case in fixture["closures"].as_array().unwrap() {
        if case.get("error").is_some() {
            continue;
        }
        let V::Map(original) = V::from_tagged(&case["value"]).unwrap() else {
            panic!()
        };
        let mut streams = BTreeMap::<String, Vec<u8>>::new();
        for (index, object) in original.values().enumerate() {
            let V::Map(fields) = object else { panic!() };
            let V::Text(subject) = &fields["subject"] else {
                panic!()
            };
            // Each legacy object retains an independent origin. No causal edges are invented.
            let event = codec::Event::create(
                subject,
                &format!("retained-{index}"),
                vec![],
                None,
                Some(object.clone()),
            )
            .unwrap();
            streams
                .entry(subject.clone())
                .or_default()
                .extend(event.encode().unwrap());
        }
        let mut restored = BTreeMap::new();
        for (subject, bytes) in streams {
            for version in codec::decode_stream(&bytes, &subject).unwrap().values() {
                let object = version.state().unwrap();
                kpop_native::history_contract::validate_object(object).unwrap();
                let V::Map(fields) = object else { panic!() };
                let V::Text(id) = &fields["id"] else { panic!() };
                restored.insert(id.clone(), object.clone());
            }
        }
        assert_eq!(restored, original);
        assert_eq!(
            kpop_native::history_reduce::reduce(&restored, None, None).map_err(|e| e.0),
            kpop_native::history_reduce::reduce(&original, None, None).map_err(|e| e.0)
        );
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn fixed_payload_event_growth_is_bounded_at_ten_thousand_changes() {
    let original = root(Some(typed(json!({"v":0,"retained":"x".repeat(1000)}))));
    let mut version = original.reconstruct(None).unwrap();
    let mut windows = Vec::new();
    let mut sum = 0;
    for i in 1..=10000 {
        let event = child(
            &version,
            Some(typed(json!({"v":i,"retained":"x".repeat(1000)}))),
            &format!("edit-{i}"),
        );
        let bytes = event.encode().unwrap().len();
        assert!(bytes < 4096);
        if (901..=1000).contains(&i) || (9901..=10000).contains(&i) {
            sum += bytes;
        }
        if i == 1000 || i == 10000 {
            windows.push(sum);
            sum = 0;
        }
        version = event.reconstruct(Some(&version)).unwrap();
    }
    assert!(windows[1] <= 2 * windows[0]);
}
