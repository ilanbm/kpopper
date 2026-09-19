use kpop_native::{
    reasoning_context::CapturedAssessment, reasoning_history_assessment as H,
    reasoning_snapshot::Snapshot, value::TypedValue as V,
};
fn fixtures() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/temporal-assessment.json")).unwrap()
}
fn map(v: &V) -> &std::collections::BTreeMap<String, V> {
    let V::Map(m) = v else { panic!() };
    m
}
fn map_mut(v: &mut V) -> &mut std::collections::BTreeMap<String, V> {
    let V::Map(m) = v else { panic!() };
    m
}
fn equal(actual: &V, expected: &V, label: &str) {
    fn difference(a: &V, b: &V, path: String) -> Option<String> {
        if a == b {
            return None;
        }
        match (a, b) {
            (V::Map(a), V::Map(b)) => {
                if !a.keys().eq(b.keys()) {
                    return Some(format!("{path} keys"));
                }
                a.iter()
                    .filter(|(k, _)| !k.ends_with("revision"))
                    .chain(a.iter().filter(|(k, _)| k.ends_with("revision")))
                    .find_map(|(k, v)| difference(v, &b[k], format!("{path}.{k}")))
            }
            (V::List(a), V::List(b)) => {
                if a.len() != b.len() {
                    return Some(format!("{path} length {} != {}", a.len(), b.len()));
                }
                a.iter()
                    .zip(b)
                    .enumerate()
                    .find_map(|(i, (a, b))| difference(a, b, format!("{path}[{i}]")))
            }
            _ => Some(
                format!("{path}: {:?} != {:?}", a, b)
                    .chars()
                    .take(240)
                    .collect(),
            ),
        }
    }
    assert!(
        actual == expected,
        "{label}: {}",
        difference(actual, expected, String::new()).unwrap_or_default()
    );
}
#[test]
fn frozen_temporal_assessment_matches_python_without_runtime() {
    let data = fixtures();
    for case in data["cases"].as_array().unwrap() {
        let snapshot =
            Snapshot::from_snapshot(&V::from_tagged(&case["snapshot"]).unwrap()).unwrap();
        let base = V::from_tagged(&case["base"]).unwrap();
        let evidence = V::from_tagged(&case["evidence"]).unwrap();
        let expected = V::from_tagged(&case["output"]).unwrap();
        let result = H::from_v2_temporal(&snapshot, &base, None, Some(&evidence))
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        equal(&result, &expected, case["name"].as_str().unwrap());
        equal(
            &H::from_v2(&snapshot, &base, None).unwrap(),
            &V::from_tagged(&case["unknown"]).unwrap(),
            "unknown evidence",
        );
        let context = CapturedAssessment::new(snapshot, &result).unwrap();
        equal(
            context.view(),
            &V::from_tagged(&case["view"]).unwrap(),
            "consumer view",
        );
        let restored =
            CapturedAssessment::from_json(context.to_json().unwrap().as_bytes()).unwrap();
        equal(restored.assessment(), &result, "frozen roundtrip");
    }
    for forgery in data["forgeries"].as_array().unwrap() {
        let case = &data["cases"][forgery["case"].as_u64().unwrap() as usize];
        let snapshot =
            Snapshot::from_snapshot(&V::from_tagged(&case["snapshot"]).unwrap()).unwrap();
        let result = H::from_v2_temporal(
            &snapshot,
            &V::from_tagged(&case["base"]).unwrap(),
            None,
            Some(&V::from_tagged(&forgery["evidence"]).unwrap()),
        );
        assert!(result.is_err(), "accepted forgery {}", forgery["name"]);
    }
}
#[test]
fn actual_lean_temporal_replay_obeys_applicability_and_roundtrips() {
    use kpop_native::reasoning_runtime::{OperationalBounds, Runtime, target_name};
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!("{}.zip", target_name().unwrap()));
    let cache = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
    let data = fixtures();
    for case in data["cases"].as_array().unwrap() {
        let snapshot =
            Snapshot::from_snapshot(&V::from_tagged(&case["snapshot"]).unwrap()).unwrap();
        let result = H::assess(
            &snapshot,
            None,
            "focused-review/v1",
            Some(&runtime),
            OperationalBounds::default(),
            None,
        )
        .unwrap();
        let expected = V::from_tagged(&case["output"]).unwrap();
        for (id, node) in map(&map(&expected)["history_subjects"]) {
            if let Some(temporal) = map(node).get("temporal") {
                let actual = &map(&map(&result)["history_subjects"])[id];
                let actual = &map(actual)["temporal"];
                for key in ["status", "complete", "counterexample_claim_ids"] {
                    equal(&map(actual)[key], &map(temporal)[key], key);
                }
                let V::List(episodes) = &map(actual)["episodes"] else {
                    panic!()
                };
                assert!(
                    episodes
                        .iter()
                        .all(|e| map(e)["verification"] == V::Text("verified".into())),
                    "{}",
                    case["name"]
                );
            }
        }
        let context = CapturedAssessment::new(snapshot, &result).unwrap();
        equal(
            CapturedAssessment::from_json(context.to_json().unwrap().as_bytes())
                .unwrap()
                .assessment(),
            &result,
            "actual frozen roundtrip",
        );
    }
}

#[test]
fn rehashed_temporal_context_cannot_borrow_a_boolean_or_basis() {
    let data = fixtures();
    for forgery in data["forgeries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["name"] == "boolean" || f["name"] == "basis")
    {
        let case = &data["cases"][forgery["case"].as_u64().unwrap() as usize];
        let snapshot =
            Snapshot::from_snapshot(&V::from_tagged(&case["snapshot"]).unwrap()).unwrap();
        let mut report = V::from_tagged(&case["output"]).unwrap();
        let evidence = V::from_tagged(&forgery["evidence"]).unwrap();
        for (subject, episodes) in map(&evidence) {
            for holder in ["nodes", "history_subjects"] {
                if let Some(node) =
                    map_mut(map_mut(&mut report).get_mut(holder).unwrap()).get_mut(subject)
                    && let Some(temporal) = map_mut(node).get_mut("temporal")
                {
                    map_mut(temporal).insert("episodes".into(), episodes.clone());
                }
            }
        }
        let r = map(&report);
        let mut preimage: std::collections::BTreeMap<String, V> = [
            "schema_version",
            "assessment_profile",
            "snapshot_id",
            "as_of",
            "assessment_selection",
            "history_selection",
            "scope",
            "history",
            "history_subjects",
        ]
        .iter()
        .map(|k| (k.to_string(), r[*k].clone()))
        .collect();
        preimage.insert(
            "nodes".into(),
            V::Map(
                map(&r["nodes"])
                    .iter()
                    .map(|(id, n)| {
                        (
                            id.clone(),
                            V::Map(
                                map(n)
                                    .iter()
                                    .filter(|(k, _)| *k != "attention")
                                    .map(|(k, v)| (k.clone(), v.clone()))
                                    .collect(),
                            ),
                        )
                    })
                    .collect(),
            ),
        );
        map_mut(&mut report).insert(
            "findings_revision".into(),
            V::Text(V::Map(preimage).digest().unwrap()),
        );
        map_mut(&mut report).remove("envelope_revision");
        let revision = report.digest().unwrap();
        map_mut(&mut report).insert("envelope_revision".into(), V::Text(revision));
        H::validate(&report).unwrap();
        assert!(
            CapturedAssessment::new(snapshot, &report).is_err(),
            "accepted rehashed {}",
            forgery["name"]
        );
    }
}
