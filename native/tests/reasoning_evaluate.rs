use kpop_native::{
    reasoning_evaluate::Evaluator,
    reasoning_runtime::{OperationalBounds, Runtime, target_name},
    reasoning_snapshot::Snapshot,
    value::TypedValue as V,
};
use serde_json::Value as J;
#[test]
fn operational_refusals_never_become_computed_values_or_partial_batches() {
    let doc = V::from_json(&serde_json::json!({"r":{"a":{"v":2}}})).unwrap();
    let snapshot = Snapshot::from_data(&doc, Default::default()).unwrap();
    let expression = V::from_json(&serde_json::json!({"num":"2"})).unwrap();
    let mut absent = Evaluator::new(&snapshot, None, None, OperationalBounds::default()).unwrap();
    let result = absent.evaluate(expression.clone(), vec![]).unwrap();
    assert_eq!(result["status"], "operational_error");
    assert!(
        result["value"].is_null()
            && result["basis"].is_null()
            && result["computation_id"].is_null()
    );
    assert_eq!(result["diagnostics"][0]["code"], "runtime_unavailable");
    let mut one = Evaluator::new(
        &snapshot,
        None,
        None,
        OperationalBounds {
            batch_requests: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        one.evaluate_many(&[(expression.clone(), vec![]), (expression.clone(), vec![])])
            .unwrap_err()
            .0,
        "batch_request_limit"
    );
    let mut tiny = Evaluator::new(
        &snapshot,
        None,
        None,
        OperationalBounds {
            output_bytes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        tiny.evaluate(expression, vec![]).unwrap_err().0,
        "output_limit"
    );
}
fn without_adapter(mut value: J) -> J {
    if value["implementation"].is_object() {
        value["implementation"] = J::Null;
        value["assurance"]
            .as_object_mut()
            .unwrap()
            .remove("implementation");
    }
    value
}
#[test]
fn actual_lean_evaluator_preserves_complete_envelopes_across_all_protocols() {
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!("{}.kpopper-runtime", target_name().unwrap()));
    let cache = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
    let empty = Snapshot::from_data(
        &V::from_json(&serde_json::json!({})).unwrap(),
        Default::default(),
    )
    .unwrap();
    // Hex wire strings exceed their compact authored JSON charge. The runtime
    // limit remains a whole-batch refusal after preparation has succeeded.
    let mut wire_bound = Evaluator::new(
        &empty,
        Some(&runtime),
        None,
        OperationalBounds {
            input_bytes: 1500,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        wire_bound
            .evaluate(
                V::from_json(&serde_json::json!({"text":"x".repeat(1000)})).unwrap(),
                vec![]
            )
            .unwrap_err()
            .0,
        "batch_input_limit"
    );
    let mut output_bound = Evaluator::new(
        &empty,
        Some(&runtime),
        None,
        OperationalBounds {
            output_bytes: 12000,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        output_bound
            .evaluate(
                V::from_json(&serde_json::json!({"text":"x".repeat(10000)})).unwrap(),
                vec![]
            )
            .unwrap_err()
            .0,
        "output_limit"
    );
    let corpus: J = serde_json::from_str(include_str!("fixtures/reasoning-evaluate.json")).unwrap();
    let mut failures = Vec::new();
    for c in corpus["cases"].as_array().unwrap() {
        let snapshot = Snapshot::from_snapshot(&V::from_tagged(&c["snapshot"]).unwrap()).unwrap();
        let requests = c["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    V::from_tagged(&r["expression"]).unwrap(),
                    r["declared"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().into())
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        let mut evaluator = Evaluator::new(
            &snapshot,
            Some(&runtime),
            if c["limits"].is_null() {
                None
            } else {
                Some(c["limits"].clone())
            },
            OperationalBounds::default(),
        )
        .unwrap();
        let actual = evaluator.evaluate_many(&requests);
        match actual {
            Ok(results) => {
                if let Some(expected) = c["output"].as_array() {
                    assert_eq!(results.len(), expected.len());
                    for (i, (a, e)) in results.into_iter().zip(expected).enumerate() {
                        if a["implementation"].is_object() {
                            assert_eq!(
                                a["implementation"]["adapter_source_sha256"],
                                runtime.implementation["adapter_source_sha256"]
                            );
                            assert_eq!(
                                a["implementation"]["source_sha256"],
                                runtime.implementation["source_sha256"]
                            );
                        }
                        let a = without_adapter(a);
                        let e = without_adapter(e.clone());
                        if a != e {
                            let keys = a
                                .as_object()
                                .unwrap()
                                .keys()
                                .filter(|k| a[*k] != e[*k])
                                .cloned()
                                .collect::<Vec<_>>();
                            failures.push(format!(
                                "{}[{i}]: differing {keys:?}\nactual={}\nexpected={}",
                                c["name"], a, e
                            ));
                        }
                    }
                } else {
                    failures.push(format!("{} accepted", c["name"]));
                }
            }
            Err(e) => {
                if c.get("error").is_none() {
                    failures.push(format!("{}: {e}", c["name"]));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
