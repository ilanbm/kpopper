use kpop_native::{
    reasoning_context::CapturedAssessment,
    reasoning_operations::{self as O, OperationDocument, OperationWorld},
    reasoning_runtime::{OperationalBounds, Runtime, target_name},
    reasoning_snapshot::Snapshot,
    value::TypedValue as V,
};
use serde_json::Value as J;
fn data() -> J {
    serde_json::from_str(include_str!("fixtures/reasoning-operations.json")).unwrap()
}
fn normalize(v: &mut V) {
    match v {
        V::Map(m) => {
            if matches!(m.get("implementation"), Some(V::Map(_))) {
                m.insert("implementation".into(), V::Null);
            }
            if let Some(V::Map(a)) = m.get_mut("assurance") {
                a.remove("implementation");
            }
            for v in m.values_mut() {
                normalize(v)
            }
        }
        V::List(a) => {
            for v in a {
                normalize(v)
            }
        }
        _ => {}
    }
}
#[test]
fn operational_findings_and_prospective_documents_preserve_captured_inputs() {
    let data = data();
    for c in data["findings"].as_array().unwrap() {
        let context = CapturedAssessment::from_data(&c["context"]).unwrap();
        assert_eq!(
            O::findings(&context).unwrap(),
            V::from_tagged(&c["output"]).unwrap(),
            "{}",
            c["name"]
        );
    }
    for c in data["derive"].as_array().unwrap() {
        let snapshot = Snapshot::from_snapshot(&V::from_tagged(&c["snapshot"]).unwrap()).unwrap();
        let base = OperationDocument::bind(
            &if let V::Map(m) = snapshot.to_data() {
                m["document"].clone()
            } else {
                panic!()
            },
            snapshot,
        )
        .unwrap();
        let selection = c["selection"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().into())
            .collect::<Vec<_>>();
        let V::List(proposals) = V::from_tagged(&c["proposals"]).unwrap() else {
            panic!()
        };
        let result = base.derive(
            &V::from_tagged(&c["document"]).unwrap(),
            &selection,
            &proposals,
        );
        match result {
            Ok(v) => assert_eq!(
                v.snapshot().to_data(),
                V::from_tagged(&c["output"]).unwrap(),
                "{}",
                c["name"]
            ),
            Err(e) => assert!(c.get("refused").is_some(), "{} {e}", c["name"]),
        }
    }
}
#[test]
fn operational_world_uses_one_assessment_and_retains_condition_envelopes() {
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!("{}.zip", target_name().unwrap()));
    let cache = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
    for c in data()["worlds"].as_array().unwrap() {
        let snapshot = Snapshot::from_snapshot(&V::from_tagged(&c["snapshot"]).unwrap()).unwrap();
        let document = OperationDocument::from_snapshot(snapshot, false).unwrap();
        let world =
            OperationWorld::new(&document, Some(&runtime), OperationalBounds::default()).unwrap();
        for (id, truth) in c["predicates"].as_object().unwrap() {
            assert_eq!(world.predicate_for(id), truth.as_bool())
        }
        let (a, b) = world.check().unwrap();
        assert_eq!(
            V::List(vec![
                V::List(a.into_iter().map(V::Text).collect()),
                V::List(b.into_iter().map(V::Text).collect())
            ]),
            V::from_tagged(&c["check"]).unwrap()
        );
        for condition in c["conditions"].as_array().unwrap() {
            let (truth, result) = world
                .condition(&V::from_tagged(&condition["expression"]).unwrap())
                .unwrap();
            assert_eq!(truth, condition["truth"].as_bool());
            let mut actual = result.map(|v| V::from_json(&v).unwrap()).unwrap_or(V::Null);
            let mut expected = V::from_tagged(&condition["result"]).unwrap();
            normalize(&mut actual);
            normalize(&mut expected);
            assert_eq!(actual, expected);
        }
    }
}
