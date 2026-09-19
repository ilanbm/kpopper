use kpop_native::{reasoning_fields as F, value::TypedValue as V};
#[test]
fn snapshot_fields_and_declared_capabilities_match_python() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/reasoning-fields.json")).unwrap();
    let mut failures = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        let doc = V::from_tagged(&c["document"]).unwrap();
        let result = if c["kind"] == "fields" {
            F::snapshot_fields(&doc).map(V::Map)
        } else {
            F::capabilities(&doc, c["profile"].as_str())
        };
        match result {
            Ok(v) => {
                if Some(v.to_tagged().unwrap()) != c.get("output").cloned() {
                    failures.push(format!("{} accepted/different", c["name"]));
                }
            }
            Err(e) => {
                if c.get("refused").is_none() && Some(e.0.as_str()) != c["error"].as_str() {
                    failures.push(format!("{}: {} expected {}", c["name"], e.0, c["error"]));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
