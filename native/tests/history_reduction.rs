use kpop_native::{history_reduce, value::TypedValue as V};
use serde_json::Value;

#[test]
fn acceptance_acts_and_source_clocks_match_python_without_computation() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/history-reduction.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let V::Map(input) = V::from_tagged(&case["input"]).unwrap() else {
            panic!()
        };
        let V::Map(objects) = &input["objects"] else {
            panic!()
        };
        let V::Map(rules) = &input["rules"] else {
            panic!()
        };
        let V::List(pairs) = &input["ancestry"] else {
            panic!()
        };
        let ancestry = |a: &V, b: &V| pairs.contains(&V::List(vec![a.clone(), b.clone()]));
        let raw = case["raw_objects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    (
                        row[0].as_str().unwrap().to_owned(),
                        row[1].as_str().unwrap().to_owned(),
                    ),
                    row[2].as_str().unwrap().as_bytes().to_vec(),
                )
            })
            .collect();
        for (result, expected) in [
            (
                history_reduce::reduce(objects, Some(rules), Some(&ancestry)),
                "canonical_output",
            ),
            (
                history_reduce::reduce_bytes(&raw, Some(rules), Some(&ancestry)),
                "output",
            ),
        ] {
            if case.get("error").is_some() {
                assert!(result.is_err(), "accepted {}", case["name"]);
            } else {
                let output = result.unwrap_or_else(|e| panic!("{} {expected}: {e}", case["name"]));
                assert_eq!(
                    output.to_tagged().unwrap(),
                    case[expected],
                    "{} {expected}",
                    case["name"]
                );
            }
        }
    }
}
