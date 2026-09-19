use kpop_native::{
    history_authority as A, history_contract as C, history_preparation as P, history_projection,
    value::TypedValue as V,
};

#[test]
fn temporal_contract_matches_final_python() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/temporal-contract.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let value = V::from_tagged(&case["input"]).unwrap();
        let result = match case["kind"].as_str().unwrap() {
            "object" => C::validate_object(&value),
            "projection" => history_projection::validate_projection(&value),
            "capability" => {
                let V::Map(m) = &value else { panic!() };
                let V::Map(objects) = &m["objects"] else {
                    panic!()
                };
                A::validate_temporal_capability(&m["manifest"], objects)
            }
            _ => panic!(),
        };
        match result {
            Ok(()) => assert_eq!(case["ok"], true, "{}", case["name"]),
            Err(e) => assert_eq!(
                Some(e.0.as_str()),
                case["error"].as_str(),
                "{}",
                case["name"]
            ),
        }
    }
    let V::Map(m) = V::from_tagged(&fixture["make_commit"]).unwrap() else {
        panic!()
    };
    let V::Text(raw) = &m["raw"] else { panic!() };
    let V::Map(manifest) = &m["manifest"] else {
        panic!()
    };
    let actual = P::make_commit(
        &m["marker"],
        "temporal",
        &Default::default(),
        &m["baseline"],
        &[(m["claim"].clone(), raw.as_bytes().to_vec())],
        &manifest["receipt"],
        b"view",
        None,
        None,
    )
    .unwrap();
    assert_eq!(actual, m["manifest"]);
}

#[test]
fn causal_temporal_capture_matches_final_python() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/temporal-capture.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        for (name, raw) in case["files"].as_object().unwrap() {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, raw.as_str().unwrap()).unwrap();
        }
        let captured =
            kpop_native::history_capture::capture(&root.path().join("GROUNDING.yaml"), None, None)
                .unwrap_or_else(|e| panic!("{} capture: {e}", case["name"]));
        let result = kpop_native::history_adapter::from_store_capture(&captured)
            .unwrap_or_else(|e| panic!("{} adapt: {e}", case["name"]));
        assert_eq!(
            result.projection().to_tagged().unwrap(),
            case["projection"],
            "{}",
            case["name"]
        );
        assert_eq!(
            result.document().to_tagged().unwrap(),
            case["document"],
            "{}",
            case["name"]
        );
    }
}

fn capture_fixture() -> (tempfile::TempDir, kpop_native::history_capture::Capture) {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/temporal-capture.json")).unwrap();
    let root = tempfile::tempdir().unwrap();
    for (name, raw) in cases[0]["files"].as_object().unwrap() {
        let path = root.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, raw.as_str().unwrap()).unwrap();
    }
    let capture =
        kpop_native::history_capture::capture(&root.path().join("GROUNDING.yaml"), None, None)
            .unwrap();
    (root, capture)
}
fn mapping(v: &mut V) -> &mut std::collections::BTreeMap<String, V> {
    let V::Map(m) = v else { panic!() };
    m
}
#[test]
fn temporal_capture_rejects_rehashed_frontier_and_receipt_forgery() {
    use kpop_native::{
        history_adapter, history_emit as E, history_transaction as T, history_yaml as Y,
    };
    let (_root, capture) = capture_fixture();
    for mutation in ["capability", "claim", "snapshot", "body"] {
        let mut forged = capture.clone();
        let (operation, raw) = capture
            .commits
            .iter()
            .find(|(_, raw)| {
                let V::Map(m) = Y::decode_document(raw).unwrap() else {
                    panic!()
                };
                let V::Map(r) = &m["receipt"] else { panic!() };
                let V::Map(a) = &r["after"] else { panic!() };
                a.contains_key("temporal_replay")
            })
            .unwrap();
        let mut manifest = Y::decode_document(raw).unwrap();
        let m = mapping(&mut manifest);
        if mutation == "capability" {
            m.insert("requires".into(), V::List(vec![]));
        } else {
            let r = mapping(m.get_mut("receipt").unwrap());
            let a = mapping(r.get_mut("after").unwrap());
            match mutation {
                "claim" => {
                    mapping(a.get_mut("temporal_replay").unwrap())
                        .insert("claims".into(), V::Map(Default::default()));
                }
                "snapshot" => {
                    mapping(a.get_mut("temporal_replay").unwrap())
                        .insert("snapshot".into(), V::Text("{}".into()));
                }
                _ => {
                    let nodes = mapping(
                        mapping(a.get_mut("assessment").unwrap())
                            .get_mut("nodes")
                            .unwrap(),
                    );
                    let target = nodes.get_mut("p.ready").unwrap();
                    mapping(target).insert("body".into(), V::Map(Default::default()));
                }
            }
            let V::Text(profile) = &r["profile"] else {
                panic!()
            };
            let receipt =
                T::semantic_receipt(profile, &r["capabilities"], &r["before"], &r["after"])
                    .unwrap();
            m.insert("receipt".into(), receipt);
        }
        forged
            .commits
            .insert(operation.clone(), E::encode_document(&manifest).unwrap());
        if mutation == "capability" {
            let error = A::committed_objects(&forged.marker, &forged.commits, &forged.object_bytes)
                .unwrap_err();
            assert_eq!(error.0, "temporal_capability_required");
        } else {
            assert!(
                history_adapter::from_store_capture(&forged).is_err(),
                "accepted {mutation}"
            );
        }
    }
}
#[test]
fn temporal_history_bounds_produce_explicit_incomplete_coverage() {
    use kpop_native::{
        history_adapter, history_emit as E, history_transaction as T,
        history_view as W, history_yaml as Y, reasoning_history_assessment as H,
        reasoning_runtime::OperationalBounds, reasoning_snapshot::CaptureOptions,
    };
    let (_root, mut capture) = capture_fixture();
    let empty = V::Map(Default::default());
    let receipt = T::semantic_receipt("core/v1", &empty, &empty, &empty).unwrap();
    let template = W::template(&capture.commits).unwrap();
    let parents = A::commit_frontier(&capture.commits).unwrap();
    for i in 0..34 {
        let op = format!("bounded-{i:02}");
        let manifest = P::make_commit(
            &capture.marker,
            &op,
            &parents,
            &capture.baseline,
            &[],
            &receipt,
            b"view",
            Some(&template),
            None,
        )
        .unwrap();
        capture
            .commits
            .insert(op, E::encode_document(&manifest).unwrap());
    }
    let adapted = history_adapter::from_store_capture(&capture).unwrap();
    let V::Map(p) = adapted.projection() else {
        panic!()
    };
    let V::Map(temporal) = &p["temporal"] else {
        panic!()
    };
    let V::List(observations) = &temporal["observations"] else {
        panic!()
    };
    assert_eq!(observations.len(), 64);
    assert_eq!(temporal["complete"], V::Bool(false));
    let V::Map(coverage) = &p["coverage"] else {
        panic!()
    };
    assert_eq!(coverage["complete"], V::Bool(false));
    let snapshot = adapted.snapshot(CaptureOptions::default()).unwrap();
    assert_eq!(
        H::assess(
            &snapshot,
            None,
            "focused-review/v1",
            None,
            OperationalBounds::default(),
            None
        )
        .unwrap_err()
        .0,
        "temporal_assessment_unsupported"
    );
    let _ = Y::decode_document(&capture.entry_bytes).unwrap();
}
