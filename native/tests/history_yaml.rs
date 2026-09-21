use kpop_native::{history_yaml, identity, value::TypedValue};
use serde_json::Value;

fn raw(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn strict_history_yaml_matches_python() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/history-yaml.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let result = history_yaml::decode_document(&raw(case["raw_hex"].as_str().unwrap()));
        if case.get("error").is_some() {
            assert!(result.is_err(), "accepted {}: {result:?}", case["name"]);
            continue;
        }
        let value = result.unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        assert_eq!(
            value.to_tagged().unwrap(),
            case["typed"],
            "{}",
            case["name"]
        );
        assert_eq!(value.digest().unwrap(), case["sha256"], "{}", case["name"]);
        let encoded = history_yaml::encode_document(&value)
            .unwrap_or_else(|e| panic!("encode {}: {e}", case["name"]));
        assert_eq!(
            history_yaml::decode_document(&encoded).unwrap(),
            value,
            "{}",
            case["name"]
        );
    }
}

#[test]
fn legacy_preimages_and_ids_match_python() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/history-yaml.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        if case.get("error").is_some() {
            continue;
        }
        let value = TypedValue::from_tagged(&case["typed"]).unwrap();
        assert_eq!(
            String::from_utf8(identity::legacy_preimage(&value).unwrap()).unwrap(),
            case["legacy_canonical"],
            "{}",
            case["name"]
        );
        assert_eq!(
            identity::typed_object_identity(&value).unwrap(),
            case["legacy_id"],
            "{}",
            case["name"]
        );
    }
}

#[test]
fn legacy_and_declared_typed_objects_retain_their_distinct_preimages() {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/legacy-identities.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let value = TypedValue::from_tagged(&case["legacy"]).unwrap();
        assert_eq!(
            String::from_utf8(identity::legacy_preimage(&value).unwrap()).unwrap(),
            case["legacy_preimage"],
            "{}",
            case["name"]
        );
        assert_eq!(
            identity::typed_object_identity(&value).unwrap(),
            case["legacy_id"],
            "{}",
            case["name"]
        );
        let value = TypedValue::from_tagged(&case["typed"]).unwrap();
        assert_eq!(
            identity::typed_object_identity(&value).unwrap(),
            case["typed_id"],
            "{}",
            case["name"]
        );
    }
    for declaration in [
        serde_json::json!(null),
        serde_json::json!(false),
        serde_json::json!("prototype/v1"),
        serde_json::json!("unknown"),
    ] {
        let value = TypedValue::from_json(&serde_json::json!({"id_scheme":declaration})).unwrap();
        assert!(identity::typed_object_identity(&value).is_err());
    }
    assert!(identity::typed_object_identity(&TypedValue::Null).is_err());
}

#[test]
fn history_source_and_serialization_limits_fail_closed() {
    use kpop_native::value::{MAX_DEPTH, MAX_VALUES};
    use std::collections::BTreeMap;
    assert!(
        history_yaml::decode_document(&vec![b' '; history_yaml::MAX_DOCUMENT_BYTES + 1]).is_err()
    );
    assert!(history_yaml::decode_document(format!("v: {}", "9".repeat(4301)).as_bytes()).is_err());
    assert!(
        history_yaml::decode_document(format!("v: 0x{}", "f".repeat(4000)).as_bytes()).is_err()
    );
    assert!(
        history_yaml::decode_document(
            format!("v: {}0{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH)).as_bytes()
        )
        .is_err()
    );
    let mut deep = TypedValue::Null;
    for _ in 0..MAX_DEPTH - 1 {
        deep = TypedValue::List(vec![deep]);
    }
    let value = TypedValue::Map(BTreeMap::from([("v".into(), deep)]));
    let bytes = history_yaml::encode_document(&value).unwrap();
    assert_eq!(history_yaml::decode_document(&bytes).unwrap(), value);
    let wide = TypedValue::Map(BTreeMap::from([(
        "v".into(),
        TypedValue::List(vec![TypedValue::Null; MAX_VALUES]),
    )]));
    assert!(history_yaml::encode_document(&wide).is_err());
    let expanded = TypedValue::Map(BTreeMap::from([(
        "v".into(),
        TypedValue::Text("🦀".repeat(history_yaml::MAX_DOCUMENT_BYTES / 12)),
    )]));
    assert!(history_yaml::encode_document(&expanded).is_err());
    let seconds = TypedValue::from_tagged(&serde_json::json!([
        "map",
        [["v", ["datetime", "2026-09-19T01:02:03+00:00:01"]]]
    ]))
    .unwrap();
    assert!(history_yaml::encode_document(&seconds).is_err());
    assert!(history_yaml::encode_document(&TypedValue::Null).is_err());
}
