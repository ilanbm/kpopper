use kpop_native::{reasoning_history_support as H, value::TypedValue as V};
use serde_json::Value as J;
#[test]
fn captured_pin_evidence_and_support_reduction_match_python() {
    let data: J =
        serde_json::from_str(include_str!("fixtures/reasoning-history-support.json")).unwrap();
    for c in data["support"].as_array().unwrap() {
        let roots = c["roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().into())
            .collect::<Vec<_>>();
        let result = H::reduce_support_graph(
            &roots,
            &V::from_json(&c["graph"]).unwrap(),
            &V::from_json(&c["outcomes"]).unwrap(),
            H::MAX_VISITS,
            &mut H::SupportBudget::default(),
        );
        match result {
            Ok(v) => assert_eq!(v.to_json().unwrap(), c["output"], "{}", c["name"]),
            Err(e) => assert!(c.get("refused").is_some(), "{}: {e}", c["name"]),
        }
    }
    for c in data["pins"].as_array().unwrap() {
        let result = H::pin_review_evidence(
            &V::from_tagged(&c["projection"]).unwrap(),
            c["version"].as_str().unwrap(),
            c["subject"].as_str(),
        );
        match result {
            Ok(v) => assert_eq!(v, V::from_tagged(&c["output"]).unwrap(), "{}", c["name"]),
            Err(e) => assert!(c.get("refused").is_some(), "{}: {e}", c["name"]),
        }
    }
}
#[test]
fn support_work_and_path_limits_fail_without_partial_findings() {
    let graph=V::from_json(&serde_json::json!({"s@v":{"subject":"s","version":"v","state":"accepted","dependencies":[{"subject":"s","version":"v"}]}})).unwrap();
    let outcomes = V::from_json(&serde_json::json!({})).unwrap();
    for mut budget in [
        H::SupportBudget {
            remaining: 0,
            path_items: H::MAX_PATH_ITEMS,
        },
        H::SupportBudget {
            remaining: H::MAX_VISITS,
            path_items: 1,
        },
    ] {
        assert!(
            H::reduce_support_graph(
                &["s@v".into()],
                &graph,
                &outcomes,
                H::MAX_VISITS,
                &mut budget
            )
            .is_err()
        )
    }
}
