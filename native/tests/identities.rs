use kpop_native::{identity::identity, store::json_input};
use serde_json::{Value, json};

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

#[test]
fn literal_private_number_object_cannot_alias_a_number_token() {
    let object = json_input(br#"{"a":{"$serde_json::private::Number":"7"}}"#).unwrap();
    let number = json_input(br#"{"a":7}"#).unwrap();

    assert_eq!(object, json!({"a":{"$serde_json::private::Number":"7"}}));
    assert_eq!(
        identity(&object).unwrap(),
        "99623598af3d462d254a3fdbdfda1fb5dd96c781944b8d37d6c7b1c9560b5d1b"
    );
    assert_eq!(
        identity(&number).unwrap(),
        "6dc60cf5cb18a12b9e2de72377f1a87751115bfb2c2869882c5cc09feb4ab0ef"
    );
}
