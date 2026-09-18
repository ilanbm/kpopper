use kpop_native::{identity::identity, store::json_input};
use serde_json::Value;

#[test]
fn canonical_identity_matches_frozen_python_vectors() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/typed-identities.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let raw = case["json"].as_str().unwrap();
        let value = json_input(raw.as_bytes()).unwrap();
        assert_eq!(identity(&value).unwrap(), case["sha256"], "{raw}");
    }
}
