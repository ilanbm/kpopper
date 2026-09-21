use kpop_native::reasoning_query as Q;
#[test]
fn canonical_query_frames_and_preflight_match_pinned_python() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/reasoning-query.json")).unwrap();
    let mut failures = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        let value = &c["input"];
        let result = match c["kind"].as_str().unwrap() {
            "request" => Q::validate_request(value),
            "canonical" => Q::canonical_json_bytes(value)
                .map(|v| serde_json::Value::String(String::from_utf8(v).unwrap())),
            "decode" => Q::decode_canonical_json(value.as_str().unwrap().as_bytes()),
            "frame" => Q::decode_frame(value.as_str().unwrap().as_bytes(), "KR4"),
            _ => Q::lower_row(value, &["a".into(), "b".into()]),
        };
        match result {
            Ok(result) => {
                if Some(result) != c.get("output").cloned() {
                    failures.push(format!("{}: accepted/different", c["name"]));
                } else if c["kind"] == "request" {
                    match Q::encode_request(value) {
                        Ok(frame)
                            if Some(frame.as_slice()) == c["frame"].as_str().map(str::as_bytes) => {
                        }
                        other => failures.push(format!("{}: frame differs {other:?}", c["name"])),
                    }
                }
            }
            Err(e) => {
                if c.get("refused").is_none() && Some(e.0.as_str()) != c["error"].as_str() {
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
#[test]
fn user_json_keys_cannot_impersonate_deserializer_internal_numbers() {
    let raw = br#"{"$serde_json::private::Number":"12"}"#;
    let value = Q::decode_canonical_json(raw).unwrap();
    assert!(value.is_object());
    assert_eq!(value["$serde_json::private::Number"], "12");
}
