use kpop_native::reasoning_transport as T;
use serde::Deserialize;
#[test]
fn wire_codec_matches_pinned_python_without_evaluating() {
    let mut decoder =
        serde_json::Deserializer::from_str(include_str!("fixtures/reasoning-transport.json"));
    decoder.disable_recursion_limit();
    let data = serde_json::Value::deserialize(&mut decoder).unwrap();
    let mut failures = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        let result = if c["kind"] == "encode" {
            T::encode_request(&c["input"]).map(serde_json::Value::String)
        } else {
            T::decode_response(c["input"].as_str().unwrap().as_bytes())
        };
        match result {
            Ok(v) => {
                if Some(v) != c.get("output").cloned() {
                    failures.push(format!(
                        "{} accepted/different; expected {}",
                        c["name"], c["error"]
                    ));
                }
            }
            Err(e) => {
                if Some(e.0.as_str()) != c["error"].as_str() {
                    failures.push(format!("{}: {} expected {}", c["name"], e.0, c["error"]));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
