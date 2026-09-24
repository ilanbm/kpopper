use kpop_native::{history_node_evidence::Evidence, value::TypedValue as V};
use serde_json::json;
fn value(snapshot: &str) -> V {
    let basis = json!({"expression":{"ref":"a"},"profile":"core/v1","modules":["arithmetic/v1"],"as_of":null});
    let resources = json!({"depth":128,"steps":1000000,"digits":256,"version":"resources/v2"});
    let id=V::from_json(&json!({"snapshot_id":snapshot,"expression":basis["expression"],"profile":"core/v1","modules":["arithmetic/v1"],"declared":["a"],"as_of":null,"resources":resources})).unwrap().digest().unwrap();
    V::from_json(&json!({"computation":{"snapshot_id":snapshot,"computation_id":id,"basis":basis,"resource_profile":resources,"modules":["arithmetic/v1"],"potential_ids":["a"],"implementation":{"identity":"retained-runtime"},"value":3},"authored":{"snapshot_id":"unrelated-original-observation"}})).unwrap()
}
#[test]
fn context_variation_reconstructs_exact_ids_without_changing_node_payload() {
    let a = "a".repeat(64);
    let b = "b".repeat(64);
    let first = value(&a);
    let next = value(&b);
    let packed = Evidence::pack(&first, &a, &[]).unwrap();
    let second = Evidence::pack(&next, &b, &[]).unwrap();
    assert_eq!(packed.encode().unwrap(), second.encode().unwrap());
    let decoded = Evidence::decode(&packed.encode().unwrap()).unwrap();
    assert_eq!(decoded.restore(&a).unwrap(), first);
    assert_eq!(decoded.restore(&b).unwrap(), next);
}
#[test]
fn unknown_recipes_and_authored_placeholder_shaped_maps_stay_literal() {
    let snapshot = "a".repeat(64);
    let value=V::from_json(&json!({"computation_id":"unknown","snapshot_id":"unrelated","basis":null,"resource_profile":{},"data":{"$receipt_spike_id_ref":"user-authored","kind":"Snapshot","path":[]}})).unwrap();
    let encoded = Evidence::pack(&value, &snapshot, &[]).unwrap();
    assert_eq!(encoded.restore(&"b".repeat(64)).unwrap(), value);
}
#[test]
fn invalid_versions_and_duplicate_recipe_targets_refuse() {
    let snapshot = "a".repeat(64);
    let packed = Evidence::pack(&value(&snapshot), &snapshot, &[]).unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&packed.encode().unwrap()).unwrap();
    let duplicate = json["recipes"][0].clone();
    json["recipes"].as_array_mut().unwrap().push(duplicate);
    // Use the canonical struct serialization via serde transport; restore still validates it.
    let invalid: Evidence = serde_json::from_value(json).unwrap();
    assert!(invalid.restore(&snapshot).is_err());
    assert!(packed.restore("invalid").is_err());
}
