use kpop_native::{history_emit, value::TypedValue as V};
#[test]
fn generated_yaml_matches_pinned_python_bytes() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-emission.json")).unwrap();
    let mut differences = Vec::new();
    for case in fixtures["cases"].as_array().unwrap() {
        let value = V::from_tagged(&case["input"]).unwrap();
        match history_emit::encode_document(&value) {
            Ok(bytes) if case["bytes"].as_str().map(str::as_bytes) == Some(bytes.as_slice()) => {}
            Err(e) if case["error"].as_str() == Some(e.0.as_str()) => {}
            other => differences.push(format!(
                "{}: expected {} got {:?}",
                case["name"],
                case.get("bytes").unwrap_or(&case["error"]),
                other.map(|b| String::from_utf8(b).unwrap())
            )),
        }
    }
    assert!(
        differences.is_empty(),
        "{} mismatches:\n{}",
        differences.len(),
        differences
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
