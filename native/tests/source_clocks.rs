use kpop_native::{history_yaml::SourceValue, source_clock, value::TypedValue};
use serde_json::Value;
use std::cmp::Ordering;

#[test]
fn source_clock_order_and_invalid_clocks_match_python() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/source-clocks.json")).unwrap();
    for (i, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let a = SourceValue::from_typed(&TypedValue::from_tagged(&case["a"]).unwrap());
        let b = SourceValue::from_typed(&TypedValue::from_tagged(&case["b"]).unwrap());
        let result = source_clock::order(&a, &b, None);
        if case.get("error").is_some() {
            assert!(result.is_err(), "accepted {i}: {case}");
        } else {
            let result = result.unwrap_or_else(|e| panic!("{i}: {case}: {e}"));
            let actual = result.map(|v| match v {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            });
            assert_eq!(serde_json::json!(actual), case["output"], "{i}: {case}");
        }
    }
}
