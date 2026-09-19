use kpop_native::{
    reasoning_assessment::assess,
    reasoning_runtime::{OperationalBounds, Runtime, target_name},
    reasoning_snapshot::Snapshot,
    value::TypedValue as V,
};
#[test]
fn historical_typed_values_require_a_version_and_evaluators_match_the_snapshot() {
    use kpop_native::{
        reasoning_assessment::{dependency_result, history},
        reasoning_evaluate::Evaluator,
    };
    let doc = V::from_json(&serde_json::json!({"r":{"a":{"v":1}}})).unwrap();
    let other = V::from_json(&serde_json::json!({"r":{"a":{"v":2}}})).unwrap();
    let a = Snapshot::from_data(&doc, Default::default()).unwrap();
    let b = Snapshot::from_data(&other, Default::default()).unwrap();
    let mut engine = Evaluator::new(&b, None, None, Default::default()).unwrap();
    assert!(dependency_result(&a, "a", &mut engine).is_err());
    let typed = serde_json::json!({"type":"number","numerator":"1","denominator":"1"});
    for old in [
        typed.clone(),
        serde_json::json!({"computed":{"value":typed}}),
    ] {
        assert!(
            history(&V::from_json(&old).unwrap(), &doc)
                .unwrap()
                .0
                .is_none()
        );
    }
}
fn normalize(value: &mut V) {
    match value {
        V::Map(m) => {
            m.remove("assessment_revision");
            if m.get("implementation")
                .is_some_and(|v| matches!(v, V::Map(_)))
            {
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
fn captured_assessment_preserves_independent_findings_attention_and_scope() {
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!("{}.zip", target_name().unwrap()));
    let cache = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/reasoning-assessment.json")).unwrap();
    let mut failures = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        let snapshot = Snapshot::from_snapshot(&V::from_tagged(&c["snapshot"]).unwrap()).unwrap();
        let selection = c["selection"].as_array().map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        });
        let result = assess(
            &snapshot,
            selection.as_deref(),
            c["policy"].as_str().unwrap(),
            Some(&runtime),
            OperationalBounds::default(),
        );
        match result {
            Ok(mut v) => {
                if let Some(output) = c.get("output") {
                    if let V::Map(m) = &v {
                        let mut preimage = m.clone();
                        preimage.remove("assessment_revision");
                        assert_eq!(
                            m["assessment_revision"],
                            V::Text(V::Map(preimage).digest().unwrap())
                        );
                    }
                    let mut expected = V::from_tagged(output).unwrap();
                    normalize(&mut v);
                    normalize(&mut expected);
                    if v != expected {
                        failures.push(format!(
                            "{}: mismatch\nactual={:?}\nexpected={:?}",
                            c["name"],
                            v.to_tagged().unwrap(),
                            expected.to_tagged().unwrap()
                        ));
                    }
                } else {
                    failures.push(format!("{} accepted", c["name"]));
                }
            }
            Err(e) => {
                if c.get("refused").is_none() {
                    failures.push(format!("{}: {e}", c["name"]));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
